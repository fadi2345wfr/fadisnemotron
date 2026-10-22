//! Remote access for the Android app: a small HTTP/JSON server on the Mac
//! (protocol in docs/remote-api.md).
//!
//! The server runs on its own threads. It never touches the UI state itself:
//! commands go to the app through a channel and are answered there, and the
//! app publishes a JSON snapshot of what it shows, which long-poll requests
//! wait on.
//!
//! The HTTP part is written for this one job instead of using a library, so
//! that anyone on the network can be turned away cheaply: the request head is
//! size- and time-limited, the key is checked before any body is read, bodies
//! must announce their length, and the memory they may use is capped.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::agent::Decision;
use crate::attachments::{self, Attachment, AttachmentKind};
use crate::tools::ApprovalMode;

pub const DEFAULT_PORT: u16 = 7878;
pub const PROTOCOL: u32 = 2;
/// Format of the QR code; independent of the protocol version.
const PAIRING_FORMAT: u32 = 1;
const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
/// Photos arrive already scaled down by the phone.
const MAX_IMAGE_UPLOAD_BYTES: usize = 30 * 1024 * 1024;
/// Bodies being received at the same moment may hold this much memory.
const MAX_BODY_BYTES_IN_FLIGHT: usize = 300 * 1024 * 1024;
const MAX_WAIT_SECS: u64 = 30;
const MAX_CONCURRENT: usize = 32;
/// The request line and headers together.
const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
/// Time a client gets to send the request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Time without a single byte while a body comes in.
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a request waits for the app to handle a command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

/// Something the phone asks the app to do.
#[derive(Debug)]
pub enum Command {
    Send {
        text: String,
        /// Photos sent inline (protocol version 1), already prepared.
        attachments: Vec<Attachment>,
        /// Ids from earlier uploads.
        uploads: Vec<String>,
    },
    /// A photo or video that was received and prepared; the app keeps it
    /// until a `Send` names it.
    Upload {
        attachment: Attachment,
    },
    Stop,
    Decide {
        call_id: String,
        decision: Decision,
    },
    NewChat,
    OpenChat {
        id: String,
    },
    DeleteChat {
        id: String,
    },
    Retry,
    Settings {
        approval: Option<ApprovalMode>,
        thinking: Option<bool>,
    },
    Undo {
        path: String,
    },
    Diff {
        path: String,
    },
}

/// The app's answer: a JSON body, or an HTTP status with a message for the user.
pub type Reply = Result<Value, (u16, String)>;

pub struct Request {
    pub command: Command,
    reply: Sender<Reply>,
    /// After this the phone has been told "no answer", so the command must not run.
    deadline: Instant,
}

impl Request {
    pub fn answer(self, reply: Reply) {
        let _ = self.reply.send(reply);
    }
}

struct Snapshot {
    version: u64,
    body: Value,
}

struct Shared {
    token: String,
    name: String,
    snapshot: Mutex<Snapshot>,
    changed: Condvar,
    stopping: AtomicBool,
    in_flight: AtomicUsize,
    body_bytes: AtomicUsize,
    /// Photos are scaled to this size when they arrive.
    max_image_dim: AtomicU32,
    last_seen: Mutex<Option<(Instant, String)>>,
}

/// A place among the requests being served; freed even if the thread panics.
struct Slot(Arc<Shared>);

impl Slot {
    fn take(shared: &Arc<Shared>) -> Option<Self> {
        if shared.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            shared.in_flight.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(shared.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Memory promised to a body that is about to be read.
struct Reservation<'a> {
    counter: &'a AtomicUsize,
    bytes: usize,
}

impl<'a> Reservation<'a> {
    fn take(counter: &'a AtomicUsize, bytes: usize) -> Option<Self> {
        if counter.fetch_add(bytes, Ordering::SeqCst) + bytes > MAX_BODY_BYTES_IN_FLIGHT {
            counter.fetch_sub(bytes, Ordering::SeqCst);
            return None;
        }
        Some(Self { counter, bytes })
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

pub struct RemoteServer {
    shared: Arc<Shared>,
    commands: Mutex<Receiver<Request>>,
    accept: Option<JoinHandle<()>>,
    port: u16,
}

impl RemoteServer {
    /// Listens on all interfaces. `wake` is called whenever the phone asks
    /// for something (e.g. to request a repaint of a hidden window).
    pub fn start(
        port: u16,
        token: String,
        name: String,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let bind = || TcpListener::bind((Ipv4Addr::UNSPECIFIED, port));
        // A server that was just stopped may still hold the port for a moment.
        let mut listener = bind();
        for _ in 0..50 {
            if listener
                .as_ref()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::AddrInUse)
            {
                std::thread::sleep(Duration::from_millis(20));
                listener = bind();
            }
        }
        let listener = listener.map_err(|e| {
            format!("Port {port} lässt sich nicht öffnen ({e}). Läuft die App schon zweimal, oder nutzt ein anderes Programm den Port?")
        })?;
        let port = listener.local_addr().map_or(port, |a| a.port());
        let shared = Arc::new(Shared {
            token,
            name,
            snapshot: Mutex::new(Snapshot {
                version: 1,
                body: json!({}),
            }),
            changed: Condvar::new(),
            stopping: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            body_bytes: AtomicUsize::new(0),
            max_image_dim: AtomicU32::new(crate::config::DEFAULT_MAX_IMAGE_DIM),
            last_seen: Mutex::new(None),
        });
        let (tx, commands) = mpsc::channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let accept = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("fernzugriff".into())
                .spawn(move || accept_loop(&listener, &shared, &tx, &wake))
                .map_err(|e| format!("Fernzugriff konnte nicht starten: {e}"))?
        };
        Ok(Self {
            shared,
            commands: Mutex::new(commands),
            accept: Some(accept),
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The next command from the phone, if any. Commands the phone was
    /// already told "no answer" for are dropped.
    pub fn next_command(&self) -> Option<Request> {
        let commands = self.commands.lock().unwrap_or_else(|e| e.into_inner());
        while let Ok(request) = commands.try_recv() {
            if request.deadline > Instant::now() {
                return Some(request);
            }
        }
        None
    }

    /// Replaces the state the phone sees and wakes waiting long-polls.
    /// Unchanged snapshots are ignored.
    pub fn publish(&self, body: Value) {
        let mut snap = self
            .shared
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if snap.body != body {
            snap.version += 1;
            snap.body = body;
            self.shared.changed.notify_all();
        }
    }

    /// When and from where the phone last asked something.
    pub fn last_seen(&self) -> Option<(Instant, String)> {
        self.shared
            .last_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// True if the phone asked something within the last `within`.
    pub fn recently_seen(&self, within: Duration) -> bool {
        self.last_seen()
            .is_some_and(|(when, _)| when.elapsed() < within)
    }

    /// Photos from the phone are scaled to this size.
    pub fn set_max_image_dim(&self, dim: u32) {
        self.shared.max_image_dim.store(dim, Ordering::Relaxed);
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        // Wake the accept loop and wait for it, so the port is free again when
        // this returns: a new key or port restarts the server right away.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            Duration::from_millis(500),
        );
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

fn accept_loop(
    listener: &TcpListener,
    shared: &Arc<Shared>,
    commands: &Sender<Request>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) {
    loop {
        let (stream, peer) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(_) if shared.stopping.load(Ordering::SeqCst) => return,
            Err(_) => {
                // Out of file descriptors, a reset connection: try again soon.
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        if shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        let Some(slot) = Slot::take(shared) else {
            let (status, body) = error(503, "Zu viele gleichzeitige Anfragen.");
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            write_json(&stream, status, &body);
            continue;
        };
        let commands = commands.clone();
        let wake = wake.clone();
        // One thread per request: long-polls must not hold up commands. If the
        // thread cannot start, the closure (and with it the slot) is dropped.
        let _ = std::thread::Builder::new()
            .name("fernzugriff-anfrage".into())
            .spawn(move || {
                serve(stream, peer.ip(), &slot.0, &commands, &*wake);
                drop(slot);
            });
    }
}

/// Reads with a limit on the whole head, then with a limit on silence.
struct TimedReader {
    stream: TcpStream,
    /// `Some`: everything must be done by then. `None`: only silence counts.
    deadline: Option<Instant>,
}

impl Read for TimedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let wait = match self.deadline {
            Some(deadline) => deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))?,
            None => BODY_IDLE_TIMEOUT,
        };
        self.stream.set_read_timeout(Some(wait))?;
        self.stream.read(buf)
    }
}

type Reader = BufReader<TimedReader>;

struct Head {
    method: String,
    target: String,
    /// Names in lower case.
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The announced body length. A POST must announce one.
    fn content_length(&self) -> Result<usize, (u16, Value)> {
        if self.header("transfer-encoding").is_some() {
            return Err(error(
                411,
                "Die Länge der Anfrage muss angegeben werden (kein Transfer-Encoding).",
            ));
        }
        match self.header("content-length") {
            None if self.method == "POST" => Err(error(411, "Content-Length fehlt.")),
            None => Ok(0),
            Some(v) => v
                .trim()
                .parse()
                .map_err(|_| error(400, "Content-Length ist keine Zahl.")),
        }
    }
}

fn read_head(reader: &mut Reader) -> Result<Head, (u16, Value)> {
    let bad = || error(400, "Ungültige Anfrage.");
    let mut total = 0;
    let mut lines: Vec<String> = Vec::new();
    loop {
        let mut line = Vec::new();
        let left = (MAX_HEAD_BYTES + 1 - total) as u64;
        match reader.by_ref().take(left).read_until(b'\n', &mut line) {
            Ok(0) => return Err(bad()),
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                return Err(error(408, "Die Anfrage kam zu langsam."));
            }
            Err(_) => return Err(bad()),
        }
        if !line.ends_with(b"\n") {
            return Err(error(431, "Die Kopfzeilen sind zu groß."));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if lines.is_empty() {
                continue; // a blank line before the request line is allowed
            }
            break;
        }
        lines.push(text.to_owned());
        if lines.len() > MAX_HEADERS + 1 {
            return Err(error(431, "Zu viele Kopfzeilen."));
        }
    }
    let mut parts = lines[0].split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad());
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(bad());
    }
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Ok(Head {
        method: method.to_ascii_uppercase(),
        target: target.to_owned(),
        headers,
    })
}

/// Answers one connection; every connection carries one request.
fn serve(
    stream: TcpStream,
    peer: IpAddr,
    shared: &Arc<Shared>,
    commands: &Sender<Request>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::with_capacity(
        16 * 1024,
        TimedReader {
            stream: read_half,
            deadline: Some(Instant::now() + HEAD_TIMEOUT),
        },
    );
    let (status, body) = match read_head(&mut reader) {
        Ok(head) => route(&head, &mut reader, &stream, peer, shared, commands, wake),
        Err(answer) => answer,
    };
    write_json(&stream, status, &body);
    // Tell the client the answer is complete; it then closes its side too.
    let _ = stream.shutdown(Shutdown::Write);
    // An answer sent before the body was read (401, 413) must not be lost to
    // a connection reset: take in a little of what is still coming, bounded.
    reader.get_mut().deadline = Some(Instant::now() + Duration::from_secs(1));
    let _ = std::io::copy(&mut reader.by_ref().take(64 * 1024), &mut std::io::sink());
    let _ = stream.shutdown(Shutdown::Both);
}

fn write_json(mut stream: &TcpStream, status: u16, body: &Value) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let body = body.to_string();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .and_then(|()| stream.flush());
}

fn route(
    head: &Head,
    reader: &mut Reader,
    stream: &TcpStream,
    peer: IpAddr,
    shared: &Arc<Shared>,
    commands: &Sender<Request>,
    wake: &(dyn Fn() + Send + Sync),
) -> (u16, Value) {
    // The key comes first: nobody without it gets a body read, let alone parsed.
    if !authorized(head, &shared.token) {
        return error(
            401,
            "Falscher oder fehlender Schlüssel. Koppel das Handy neu (QR-Code in der Mac-App).",
        );
    }
    *shared.last_seen.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), peer.to_string()));
    let (path, query) = head
        .target
        .split_once('?')
        .unwrap_or((head.target.as_str(), ""));

    match (head.method.as_str(), path) {
        ("GET", "/api/v1/ping") => (
            200,
            json!({"ok": true, "app": "Fadis Nemotron", "version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL, "name": shared.name}),
        ),
        ("GET", "/api/v1/state") => {
            // Lets a window that was idle build a fresh snapshot.
            wake();
            (200, wait_for_state(shared, query))
        }
        ("GET", "/api/v1/diff") => match query_param(query, "path") {
            Some(path) => ask_app(commands, wake, Command::Diff { path }),
            None => error(400, "path fehlt"),
        },
        ("POST", "/api/v1/upload") => {
            let name: String = query_param(query, "name")
                .unwrap_or_else(|| "anhang".to_owned())
                .chars()
                .take(120)
                .collect();
            let content_type = head.header("content-type").unwrap_or("").to_owned();
            let len = match head.content_length() {
                Ok(n) => n,
                Err(e) => return e,
            };
            let Some(kind) = upload_kind(&name, &content_type) else {
                return error(
                    400,
                    &format!(
                        "{name}: nur Fotos und Videos (MP4, MOV, WebM) können angehängt werden."
                    ),
                );
            };
            let max = match kind {
                AttachmentKind::Image => MAX_IMAGE_UPLOAD_BYTES,
                AttachmentKind::Video => attachments::MAX_VIDEO_BYTES,
            };
            if len == 0 {
                return error(400, "Leerer Upload.");
            }
            let bytes = match read_body(head, reader, stream, len, max, &shared.body_bytes) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let max_dim = shared.max_image_dim.load(Ordering::Relaxed);
            // Scaling a photo takes a moment: done here, not on the app's thread.
            match prepare_upload(&name, &content_type, kind, bytes, max_dim) {
                Ok(attachment) => ask_app(commands, wake, Command::Upload { attachment }),
                Err(e) => e,
            }
        }
        ("POST", endpoint) => {
            let len = match head.content_length() {
                Ok(n) => n,
                Err(e) => return e,
            };
            let bytes = match read_body(
                head,
                reader,
                stream,
                len,
                MAX_BODY_BYTES,
                &shared.body_bytes,
            ) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let body = if bytes.iter().all(u8::is_ascii_whitespace) {
                json!({})
            } else {
                match serde_json::from_slice(&bytes) {
                    Ok(v) => v,
                    Err(e) => return error(400, &format!("Ungültiges JSON: {e}")),
                }
            };
            let max_dim = shared.max_image_dim.load(Ordering::Relaxed);
            let command = match parse_command(endpoint, &body, max_dim) {
                Ok(c) => c,
                Err(e) => return e,
            };
            ask_app(commands, wake, command)
        }
        _ => error(404, "Unbekannter Endpunkt."),
    }
}

/// Reads exactly `len` bytes, if that is within `max` and the memory budget.
fn read_body(
    head: &Head,
    reader: &mut Reader,
    stream: &TcpStream,
    len: usize,
    max: usize,
    budget: &AtomicUsize,
) -> Result<Vec<u8>, (u16, Value)> {
    if len > max {
        return Err(error(
            413,
            &format!(
                "Die Anfrage ist zu groß (höchstens {} MB).",
                max / (1024 * 1024)
            ),
        ));
    }
    let Some(_reserved) = Reservation::take(budget, len) else {
        return Err(error(
            503,
            "Der Mac nimmt gerade viele Daten an. Bitte kurz warten und noch einmal senden.",
        ));
    };
    // curl and some libraries wait for this before they send a large body.
    if head
        .header("expect")
        .is_some_and(|v| v.eq_ignore_ascii_case("100-continue"))
    {
        let _ = (&*stream).write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    reader.get_mut().deadline = None;
    let mut bytes = Vec::with_capacity(len);
    reader
        .by_ref()
        .take(len as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| error(400, &format!("Anfrage konnte nicht gelesen werden: {e}")))?;
    if bytes.len() != len {
        return Err(error(400, "Die Anfrage ist unvollständig angekommen."));
    }
    Ok(bytes)
}

/// Photo or video, judged by the file name and then the content type.
fn upload_kind(name: &str, content_type: &str) -> Option<AttachmentKind> {
    let ext = std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    attachments::kind_for_extension(&ext).or_else(|| {
        let content_type = content_type.to_ascii_lowercase();
        if content_type.starts_with("image/") {
            Some(AttachmentKind::Image)
        } else if content_type.starts_with("video/") {
            Some(AttachmentKind::Video)
        } else {
            None
        }
    })
}

fn prepare_upload(
    name: &str,
    content_type: &str,
    kind: AttachmentKind,
    bytes: Vec<u8>,
    max_image_dim: u32,
) -> Result<Attachment, (u16, Value)> {
    match kind {
        AttachmentKind::Image => {
            attachments::image_attachment(name, &bytes, max_image_dim).map_err(|e| error(400, &e))
        }
        AttachmentKind::Video => {
            let ext = std::path::Path::new(name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let ext = match (ext.as_str(), content_type.to_ascii_lowercase().as_str()) {
                ("mov" | "webm" | "mp4" | "m4v", _) => ext,
                (_, "video/quicktime") => "mov".to_owned(),
                (_, "video/webm") => "webm".to_owned(),
                _ => "mp4".to_owned(),
            };
            attachments::video_attachment(name, &ext, bytes).map_err(|e| error(400, &e))
        }
    }
}

fn wait_for_state(shared: &Shared, query: &str) -> Value {
    let after = query_param(query, "after").and_then(|v| v.parse::<u64>().ok());
    let wait = query_param(query, "wait")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(MAX_WAIT_SECS);
    let deadline = Instant::now() + Duration::from_secs(wait);
    let mut snap = shared.snapshot.lock().unwrap_or_else(|e| e.into_inner());
    while after == Some(snap.version) && !shared.stopping.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        snap = shared
            .changed
            .wait_timeout(snap, deadline - now)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    let mut body = snap.body.clone();
    if !body.is_object() {
        body = json!({});
    }
    body["version"] = json!(snap.version);
    body
}

fn ask_app(
    commands: &Sender<Request>,
    wake: &(dyn Fn() + Send + Sync),
    command: Command,
) -> (u16, Value) {
    let (reply, answer) = mpsc::channel();
    let request = Request {
        command,
        reply,
        deadline: Instant::now() + COMMAND_TIMEOUT,
    };
    if commands.send(request).is_err() {
        return error(503, "Die Mac-App beendet gerade den Fernzugriff.");
    }
    wake();
    match answer.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(body)) => (200, body),
        Ok(Err((status, message))) => error(status, &message),
        Err(_) => error(503, "Die Mac-App antwortet nicht. Ist sie geöffnet?"),
    }
}

fn parse_command(
    endpoint: &str,
    body: &Value,
    max_image_dim: u32,
) -> Result<Command, (u16, Value)> {
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
    match endpoint {
        "/api/v1/send" => {
            let mut images = Vec::new();
            for (i, image) in body
                .get("images")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let name = image
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("foto.jpg")
                    .to_owned();
                let data = image.get("data").and_then(Value::as_str).unwrap_or("");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .map_err(|_| {
                        error(400, &format!("Bild {} ist kein gültiges Base64.", i + 1))
                    })?;
                images.push(
                    attachments::image_attachment(&name, &bytes, max_image_dim)
                        .map_err(|e| error(400, &e))?,
                );
            }
            let uploads: Vec<String> = body
                .get("uploads")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let text = text("text").unwrap_or_default();
            if text.trim().is_empty() && images.is_empty() && uploads.is_empty() {
                return Err(error(400, "Leere Nachricht."));
            }
            Ok(Command::Send {
                text,
                attachments: images,
                uploads,
            })
        }
        "/api/v1/stop" => Ok(Command::Stop),
        "/api/v1/new_chat" => Ok(Command::NewChat),
        "/api/v1/open_chat" => Ok(Command::OpenChat {
            id: text("id").ok_or_else(|| error(400, "id fehlt"))?,
        }),
        "/api/v1/delete_chat" => Ok(Command::DeleteChat {
            id: text("id").ok_or_else(|| error(400, "id fehlt"))?,
        }),
        "/api/v1/retry" => Ok(Command::Retry),
        "/api/v1/decide" => {
            let call_id = text("call_id").ok_or_else(|| error(400, "call_id fehlt"))?;
            let decision = match text("decision").as_deref() {
                Some("allow") => Decision::Allow,
                Some("allow_for_run") => Decision::AllowForRun,
                Some("deny") => Decision::Deny,
                _ => {
                    return Err(error(
                        400,
                        "decision muss allow, allow_for_run oder deny sein",
                    ));
                }
            };
            Ok(Command::Decide { call_id, decision })
        }
        "/api/v1/settings" => {
            let approval = match text("approval").as_deref() {
                None => None,
                Some("AskAll") => Some(ApprovalMode::AskAll),
                Some("AutoEdits") => Some(ApprovalMode::AutoEdits),
                Some("AutoAll") => Some(ApprovalMode::AutoAll),
                Some(other) => {
                    return Err(error(400, &format!("Unbekannter Freigabe-Modus: {other}")));
                }
            };
            let thinking = body.get("thinking").and_then(Value::as_bool);
            Ok(Command::Settings { approval, thinking })
        }
        "/api/v1/undo" => Ok(Command::Undo {
            path: text("path").ok_or_else(|| error(400, "path fehlt"))?,
        }),
        _ => Err(error(404, "Unbekannter Endpunkt.")),
    }
}

fn authorized(head: &Head, token: &str) -> bool {
    let presented = head
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    !token.is_empty() && constant_time_eq(presented.trim().as_bytes(), token.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| percent_decode(v))
    })
}

pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn error(status: u16, message: &str) -> (u16, Value) {
    (status, json!({ "error": message }))
}

/// A new random key for pairing: 32 URL-safe characters.
pub fn new_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("the operating system provides random numbers");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The Mac's IPv4 addresses the phone can try, Tailscale first, then the
/// home network.
pub fn local_addresses() -> Vec<Ipv4Addr> {
    let mut addrs: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && !i.is_link_local())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .collect();
    addrs.sort_by_key(|a| (address_rank(*a), *a));
    addrs.dedup();
    addrs
}

fn address_rank(addr: Ipv4Addr) -> u8 {
    if is_tailscale(addr) {
        0
    } else if addr.is_private() {
        1
    } else {
        2
    }
}

/// Tailscale hands out addresses from the carrier-grade NAT range 100.64.0.0/10.
pub fn is_tailscale(addr: Ipv4Addr) -> bool {
    let [a, b, ..] = addr.octets();
    a == 100 && (64..128).contains(&b)
}

/// The QR code content the Android app scans.
pub fn pairing_payload(name: &str, port: u16, token: &str, addrs: &[Ipv4Addr]) -> String {
    let urls: Vec<String> = addrs.iter().map(|a| format!("http://{a}:{port}")).collect();
    json!({"fadis": PAIRING_FORMAT, "name": name, "urls": urls, "token": token}).to_string()
}

/// A short name for this computer, shown on the phone.
pub fn device_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its whole length; gethostname writes a C string into it.
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
        if ok {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]);
            let name = name.trim_end_matches(".local").trim();
            if !name.is_empty() {
                return name.to_owned();
            }
        }
    }
    "Mac".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Client {
        base: String,
        agent: ureq::Agent,
    }

    impl Client {
        fn new(port: u16) -> Self {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .proxy(None)
                .timeout_global(Some(Duration::from_secs(10)))
                .build()
                .into();
            Self {
                base: format!("http://127.0.0.1:{port}"),
                agent,
            }
        }

        fn get(&self, path: &str, token: Option<&str>) -> (u16, Value) {
            let mut req = self.agent.get(format!("{}{path}", self.base));
            if let Some(t) = token {
                req = req.header("Authorization", format!("Bearer {t}"));
            }
            let mut res = req.call().unwrap();
            (
                res.status().as_u16(),
                serde_json::from_str(&res.body_mut().read_to_string().unwrap_or_default())
                    .unwrap_or(Value::Null),
            )
        }

        fn post(&self, path: &str, token: &str, body: &str) -> (u16, Value) {
            let mut res = self
                .agent
                .post(format!("{}{path}", self.base))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .send(body.to_owned())
                .unwrap();
            (
                res.status().as_u16(),
                serde_json::from_str(&res.body_mut().read_to_string().unwrap_or_default())
                    .unwrap_or(Value::Null),
            )
        }
    }

    impl Client {
        fn upload(&self, path: &str, content_type: &str, bytes: &[u8]) -> (u16, Value) {
            let mut res = self
                .agent
                .post(format!("{}{path}", self.base))
                .header("Authorization", "Bearer geheim123")
                .header("Content-Type", content_type)
                .send(bytes)
                .unwrap();
            (
                res.status().as_u16(),
                serde_json::from_str(&res.body_mut().read_to_string().unwrap_or_default())
                    .unwrap_or(Value::Null),
            )
        }
    }

    /// A real (tiny) picture: photos are decoded when they arrive.
    fn tiny_png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(4, 4)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// Sends raw bytes and reads whatever comes back, like a hostile client.
    fn raw(port: u16, request: &[u8]) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = stream.write_all(request);
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn start() -> (RemoteServer, Client) {
        let server = RemoteServer::start(0, "geheim123".into(), "Testmac".into(), || {}).unwrap();
        let client = Client::new(server.port());
        (server, client)
    }

    /// Answers commands like the app would, on a helper thread.
    fn answer_commands(
        server: Arc<RemoteServer>,
        seen: Arc<Mutex<Vec<String>>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until {
                if let Some(req) = server.next_command() {
                    let reply = match &req.command {
                        Command::Send {
                            text,
                            attachments,
                            uploads,
                        } => {
                            seen.lock().unwrap().push(format!(
                                "send:{text}:{}:{}",
                                attachments.len(),
                                uploads.join(",")
                            ));
                            if text == "besetzt" {
                                Err((409, "Nemotron arbeitet noch.".into()))
                            } else {
                                Ok(json!({"ok": true}))
                            }
                        }
                        Command::Upload { attachment } => {
                            seen.lock().unwrap().push(format!(
                                "upload:{}:{}",
                                attachment.name,
                                attachment.size_label()
                            ));
                            Ok(json!({"id": "up-1", "kind": "video", "size": 0}))
                        }
                        other => {
                            seen.lock().unwrap().push(format!("{other:?}"));
                            Ok(json!({"ok": true}))
                        }
                    };
                    req.answer(reply);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn requires_the_token() {
        let (_server, client) = start();
        assert_eq!(client.get("/api/v1/ping", None).0, 401);
        assert_eq!(client.get("/api/v1/ping", Some("falsch")).0, 401);
        let (status, body) = client.get("/api/v1/ping", Some("geheim123"));
        assert_eq!(status, 200);
        assert_eq!(body["name"], "Testmac");
        assert_eq!(body["protocol"], 2);
        assert_eq!(client.get("/api/v1/gibtsnicht", Some("geheim123")).0, 404);
    }

    #[test]
    fn long_poll_returns_as_soon_as_the_state_changes() {
        let (server, client) = start();
        server.publish(json!({"busy": false}));
        let (_, first) = client.get("/api/v1/state", Some("geheim123"));
        let version = first["version"].as_u64().unwrap();
        assert_eq!(first["busy"], false);

        let server = Arc::new(server);
        let publisher = {
            let server = server.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                server.publish(json!({"busy": true}));
            })
        };
        let started = Instant::now();
        let (_, next) = client.get(
            &format!("/api/v1/state?after={version}&wait=10"),
            Some("geheim123"),
        );
        publisher.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(next["busy"], true);
        assert_eq!(next["version"].as_u64().unwrap(), version + 1);

        // Nothing changes: the request comes back after `wait` with the same version.
        let started = Instant::now();
        let (_, same) = client.get(
            &format!("/api/v1/state?after={}&wait=1", version + 1),
            Some("geheim123"),
        );
        assert!(started.elapsed() >= Duration::from_millis(900));
        assert_eq!(same["version"].as_u64().unwrap(), version + 1);
    }

    #[test]
    fn commands_reach_the_app_and_errors_come_back() {
        let (server, client) = start();
        let server = Arc::new(server);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let helper = answer_commands(server.clone(), seen.clone());

        let image = base64::engine::general_purpose::STANDARD.encode(tiny_png());
        let (status, _) = client.post(
            "/api/v1/send",
            "geheim123",
            &json!({"text": "Hallo", "images": [{"name": "a.jpg", "data": image}]}).to_string(),
        );
        assert_eq!(status, 200);
        let (status, body) = client.post("/api/v1/send", "geheim123", r#"{"text":"besetzt"}"#);
        assert_eq!(
            (status, body["error"].as_str()),
            (409, Some("Nemotron arbeitet noch."))
        );
        assert_eq!(
            client
                .post(
                    "/api/v1/decide",
                    "geheim123",
                    r#"{"call_id":"c1","decision":"allow_for_run"}"#
                )
                .0,
            200
        );
        assert_eq!(
            client
                .post(
                    "/api/v1/settings",
                    "geheim123",
                    r#"{"approval":"AskAll","thinking":false}"#
                )
                .0,
            200
        );
        assert_eq!(client.post("/api/v1/stop", "geheim123", "").0, 200);

        assert_eq!(
            client
                .post("/api/v1/send", "geheim123", r#"{"text":"  "}"#)
                .0,
            400
        );
        assert_eq!(
            client
                .post(
                    "/api/v1/decide",
                    "geheim123",
                    r#"{"call_id":"c1","decision":"vielleicht"}"#
                )
                .0,
            400
        );
        assert_eq!(
            client
                .post("/api/v1/settings", "geheim123", r#"{"approval":"Egal"}"#)
                .0,
            400
        );
        assert_eq!(client.post("/api/v1/send", "geheim123", "{kaputt").0, 400);
        assert_eq!(
            client.post("/api/v1/send", "falsch", r#"{"text":"x"}"#).0,
            401
        );

        // Videos come as raw bytes, then the send names the upload.
        let video = vec![7u8; 3 * 1024 * 1024];
        let (status, body) =
            client.upload("/api/v1/upload?name=Mein%20Clip.mp4", "video/mp4", &video);
        assert_eq!((status, body["id"].as_str()), (200, Some("up-1")));
        assert_eq!(
            client
                .upload("/api/v1/upload?name=leer.mp4", "video/mp4", &[])
                .0,
            400
        );
        let (status, _) = client.post(
            "/api/v1/send",
            "geheim123",
            r#"{"text":"Was passiert?","uploads":["up-1"]}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(
            client
                .post("/api/v1/open_chat", "geheim123", r#"{"id":"c-1"}"#)
                .0,
            200
        );
        assert_eq!(
            client
                .post("/api/v1/delete_chat", "geheim123", r#"{"id":"c-2"}"#)
                .0,
            200
        );
        assert_eq!(client.post("/api/v1/open_chat", "geheim123", "{}").0, 400);

        helper.join().unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0], "send:Hallo:1:");
        assert_eq!(seen[1], "send:besetzt:0:");
        assert!(seen[2].contains("AllowForRun"), "{seen:?}");
        assert!(seen[3].contains("AskAll") && seen[3].contains("Some(false)"));
        assert_eq!(seen[4], "Stop");
        assert_eq!(
            seen[5],
            format!(
                "upload:Mein Clip.mp4:{}",
                attachments::format_size(video.len())
            )
        );
        assert_eq!(seen[6], "send:Was passiert?:0:up-1");
        assert!(seen[7].contains("OpenChat") && seen[7].contains("c-1"));
        assert!(seen[8].contains("DeleteChat") && seen[8].contains("c-2"));
        assert_eq!(seen.len(), 9, "bad requests must not reach the app");
        assert!(server.last_seen().is_some_and(|(_, ip)| ip == "127.0.0.1"));
    }

    fn ping_works(client: &Client) -> bool {
        client.get("/api/v1/ping", Some("geheim123")).0 == 200
    }

    #[test]
    fn strangers_cannot_crash_or_clog_the_server() {
        let (server, client) = start();
        let port = server.port();
        // A body of absurd size announced without the key: answered, not read.
        for length in ["1125899906842624", "18446744073709551615", "1000000000"] {
            let answer = raw(
                port,
                format!("POST /api/v1/send HTTP/1.1\r\nContent-Length: {length}\r\n\r\n")
                    .as_bytes(),
            );
            assert!(answer.starts_with("HTTP/1.1 401"), "{length}: {answer}");
        }
        assert!(ping_works(&client));

        // The same with the key: refused as too big, before anything is read.
        let answer = raw(
            port,
            b"POST /api/v1/send HTTP/1.1\r\nAuthorization: Bearer geheim123\r\nContent-Length: 1125899906842624\r\n\r\n",
        );
        assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");

        // Idle connections that announced a huge body do not hold slots for long.
        let held: Vec<TcpStream> = (0..40)
            .map(|_| {
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                s.write_all(b"POST /api/v1/send HTTP/1.1\r\nContent-Length: 1000000000\r\n\r\n")
                    .unwrap();
                s
            })
            .collect();
        std::thread::sleep(Duration::from_millis(2_500));
        assert!(ping_works(&client), "the server stayed blocked");
        drop(held);

        // A header line without end, and too many headers.
        let mut flood = b"GET /api/v1/ping HTTP/1.1\r\nX-A: ".to_vec();
        flood.extend(std::iter::repeat_n(b'A', 200_000));
        assert!(raw(port, &flood).starts_with("HTTP/1.1 431"));
        let mut many = b"GET /api/v1/ping HTTP/1.1\r\n".to_vec();
        for i in 0..100 {
            many.extend(format!("X-{i}: 1\r\n").as_bytes());
        }
        many.extend(b"\r\n");
        assert!(raw(port, &many).starts_with("HTTP/1.1 431"));
        // Not HTTP at all.
        assert!(raw(port, b"\x16\x03\x01\x02\x00\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(ping_works(&client));
    }

    #[test]
    fn bodies_must_announce_their_length_and_stay_in_bounds() {
        let (server, client) = start();
        let port = server.port();
        let auth = "Authorization: Bearer geheim123\r\n";
        let no_length = format!("POST /api/v1/stop HTTP/1.1\r\n{auth}\r\n");
        assert!(raw(port, no_length.as_bytes()).starts_with("HTTP/1.1 411"));
        let chunked = format!(
            "POST /api/v1/stop HTTP/1.1\r\n{auth}Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
        );
        assert!(raw(port, chunked.as_bytes()).starts_with("HTTP/1.1 411"));
        let not_a_number =
            format!("POST /api/v1/stop HTTP/1.1\r\n{auth}Content-Length: zehn\r\n\r\n");
        assert!(raw(port, not_a_number.as_bytes()).starts_with("HTTP/1.1 400"));

        // Uploads: kind and size are judged from the head, before the body.
        let big_video = format!(
            "POST /api/v1/upload?name=film.mp4 HTTP/1.1\r\n{auth}Content-Length: {}\r\n\r\n",
            attachments::MAX_VIDEO_BYTES + 1
        );
        assert!(raw(port, big_video.as_bytes()).starts_with("HTTP/1.1 413"));
        let big_photo = format!(
            "POST /api/v1/upload?name=foto.jpg HTTP/1.1\r\n{auth}Content-Length: {}\r\n\r\n",
            MAX_IMAGE_UPLOAD_BYTES + 1
        );
        assert!(raw(port, big_photo.as_bytes()).starts_with("HTTP/1.1 413"));
        let text_file = format!(
            "POST /api/v1/upload?name=notiz.txt HTTP/1.1\r\n{auth}Content-Type: text/plain\r\nContent-Length: 5\r\n\r\nhallo"
        );
        assert!(raw(port, text_file.as_bytes()).starts_with("HTTP/1.1 400"));

        // Bodies that are being received share a memory budget.
        let announce = format!(
            "POST /api/v1/upload?name=film.mp4 HTTP/1.1\r\n{auth}Content-Length: {}\r\n\r\n",
            attachments::MAX_VIDEO_BYTES
        );
        let mut waiting = Vec::new();
        for _ in 0..3 {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(announce.as_bytes()).unwrap();
            waiting.push(s);
        }
        std::thread::sleep(Duration::from_millis(500));
        let answer = raw(port, announce.as_bytes());
        assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
        drop(waiting);
        assert!(ping_works(&client));
    }

    #[test]
    fn a_stopped_server_gives_its_port_back_at_once() {
        let (server, _) = start();
        let mut port = server.port();
        drop(server);
        for _ in 0..20 {
            let again = RemoteServer::start(port, "geheim123".into(), "Testmac".into(), || {})
                .expect("the port is free right after the server stopped");
            port = again.port();
            assert_eq!(port, again.port());
        }
    }

    #[test]
    fn photos_are_prepared_where_they_arrive() {
        let (server, client) = start();
        let server = Arc::new(server);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let helper = answer_commands(server.clone(), seen.clone());
        let mut res = client
            .agent
            .post(format!("{}/api/v1/upload?name=foto.png", client.base))
            .header("Authorization", "Bearer geheim123")
            .header("Content-Type", "image/png")
            .send(tiny_png())
            .unwrap();
        assert_eq!(res.status().as_u16(), 200);
        let _ = res.body_mut().read_to_string();
        // Not a picture: refused here, never handed to the app.
        let res = client
            .agent
            .post(format!("{}/api/v1/upload?name=kaputt.jpg", client.base))
            .header("Authorization", "Bearer geheim123")
            .header("Content-Type", "image/jpeg")
            .send(b"das ist kein Bild".as_slice())
            .unwrap();
        assert_eq!(res.status().as_u16(), 400);
        helper.join().unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].starts_with("upload:foto.png:"));
    }

    #[test]
    fn helpers() {
        assert_eq!(new_token().len(), 32);
        assert_ne!(new_token(), new_token());
        assert!(is_tailscale(Ipv4Addr::new(100, 101, 2, 3)));
        assert!(!is_tailscale(Ipv4Addr::new(100, 1, 2, 3)));
        assert!(!is_tailscale(Ipv4Addr::new(192, 168, 1, 2)));
        assert_eq!(
            query_param("after=3&path=src%2Fmain.rs", "path").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            query_param("path=a+b%C3%BC", "path").as_deref(),
            Some("a bü")
        );
        assert_eq!(query_param("path=100%", "path").as_deref(), Some("100%"));
        assert_eq!(query_param("path=%ü%zz", "path").as_deref(), Some("%ü%zz"));
        assert_eq!(query_param("x=1", "path"), None);
        let payload: Value = serde_json::from_str(&pairing_payload(
            "Mac",
            7878,
            "t",
            &[Ipv4Addr::new(100, 64, 0, 1), Ipv4Addr::new(192, 168, 0, 5)],
        ))
        .unwrap();
        assert_eq!(
            payload["urls"],
            json!(["http://100.64.0.1:7878", "http://192.168.0.5:7878"])
        );
        assert_eq!(payload["fadis"], 1);
        assert!(!device_name().is_empty());
        assert!(
            constant_time_eq(b"abc", b"abc")
                && !constant_time_eq(b"abc", b"abd")
                && !constant_time_eq(b"ab", b"abc")
        );
    }
}
