//! Remote access for the Android app: a small HTTP/JSON server on the Mac
//! (protocol in docs/remote-api.md).
//!
//! The server runs on its own threads. It never touches the UI state itself:
//! commands go to the app through a channel and are answered there, and the
//! app publishes a JSON snapshot of what it shows, which long-poll requests
//! wait on.

use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::agent::Decision;
use crate::tools::ApprovalMode;

pub const DEFAULT_PORT: u16 = 7878;
pub const PROTOCOL: u32 = 2;
/// Format of the QR code; independent of the protocol version.
const PAIRING_FORMAT: u32 = 1;
const MAX_BODY_BYTES: usize = 20 * 1024 * 1024;
/// Photos and videos come as raw bytes; the phone shrinks videos first.
const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;
const MAX_WAIT_SECS: u64 = 30;
const MAX_CONCURRENT: usize = 32;
/// How long a request waits for the app to handle a command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

/// Something the phone asks the app to do.
#[derive(Debug)]
pub enum Command {
    Send {
        text: String,
        images: Vec<(String, Vec<u8>)>,
        /// Ids from earlier uploads.
        uploads: Vec<String>,
    },
    /// A photo or video, as raw bytes; the app answers with an id for `Send`.
    Upload {
        name: String,
        content_type: String,
        bytes: Vec<u8>,
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
    last_seen: Mutex<Option<(Instant, String)>>,
}

pub struct RemoteServer {
    shared: Arc<Shared>,
    server: Arc<tiny_http::Server>,
    commands: Mutex<Receiver<Request>>,
    port: u16,
}

impl RemoteServer {
    /// Listens on all interfaces. `wake` is called whenever a command waits
    /// for the app (e.g. to request a repaint of a hidden window).
    pub fn start(
        port: u16,
        token: String,
        name: String,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let server = tiny_http::Server::http((Ipv4Addr::UNSPECIFIED, port)).map_err(|e| {
            format!("Port {port} lässt sich nicht öffnen ({e}). Läuft die App schon zweimal, oder nutzt ein anderes Programm den Port?")
        })?;
        let port = server.server_addr().to_ip().map_or(port, |a| a.port());
        let server = Arc::new(server);
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
            last_seen: Mutex::new(None),
        });
        let (tx, commands) = mpsc::channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        {
            let server = server.clone();
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("fernzugriff".into())
                .spawn(move || accept_loop(&server, &shared, &tx, &wake))
                .map_err(|e| format!("Fernzugriff konnte nicht starten: {e}"))?;
        }
        Ok(Self {
            shared,
            server,
            commands: Mutex::new(commands),
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The next command from the phone, if any.
    pub fn next_command(&self) -> Option<Request> {
        self.commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv()
            .ok()
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
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::Relaxed);
        self.shared.changed.notify_all();
        self.server.unblock();
    }
}

fn accept_loop(
    server: &tiny_http::Server,
    shared: &Arc<Shared>,
    commands: &Sender<Request>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) {
    for request in server.incoming_requests() {
        if shared.stopping.load(Ordering::Relaxed) {
            break;
        }
        if shared.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
            shared.in_flight.fetch_sub(1, Ordering::SeqCst);
            let _ = request.respond(json_response(
                503,
                &json!({"error": "Zu viele gleichzeitige Anfragen."}),
            ));
            continue;
        }
        let worker_shared = shared.clone();
        let commands = commands.clone();
        let wake = wake.clone();
        // One thread per request: long-polls must not hold up commands.
        let spawned = std::thread::Builder::new()
            .name("fernzugriff-anfrage".into())
            .spawn(move || {
                let mut request = request;
                let (status, body) = route(&mut request, &worker_shared, &commands, &*wake);
                let _ = request.respond(json_response(status, &body));
                worker_shared.in_flight.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            // The request was dropped with the closure, which closes the connection.
            shared.in_flight.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn route(
    request: &mut tiny_http::Request,
    shared: &Shared,
    commands: &Sender<Request>,
    wake: &(dyn Fn() + Send + Sync),
) -> (u16, Value) {
    if !authorized(request, &shared.token) {
        return error(
            401,
            "Falscher oder fehlender Schlüssel. Koppel das Handy neu (QR-Code in der Mac-App).",
        );
    }
    if let Some(peer) = request.remote_addr() {
        *shared.last_seen.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((Instant::now(), peer.ip().to_string()));
    }
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let method = request.method().as_str().to_ascii_uppercase();

    match (method.as_str(), path) {
        ("GET", "/api/v1/ping") => (
            200,
            json!({"ok": true, "app": "Fadis Nemotron", "version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL, "name": shared.name}),
        ),
        ("GET", "/api/v1/state") => (200, wait_for_state(shared, query)),
        ("GET", "/api/v1/diff") => match query_param(query, "path") {
            Some(path) => ask_app(commands, wake, Command::Diff { path }),
            None => error(400, "path fehlt"),
        },
        ("POST", "/api/v1/upload") => {
            let name = query_param(query, "name").unwrap_or_else(|| "anhang".to_owned());
            let content_type = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Type"))
                .map(|h| h.value.as_str().to_owned())
                .unwrap_or_default();
            match read_body(request, MAX_UPLOAD_BYTES) {
                Ok(bytes) if bytes.is_empty() => error(400, "Leerer Upload."),
                Ok(bytes) => ask_app(
                    commands,
                    wake,
                    Command::Upload {
                        name,
                        content_type,
                        bytes,
                    },
                ),
                Err(e) => e,
            }
        }
        ("POST", endpoint) => {
            let body = match read_json(request) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let command = match parse_command(endpoint, &body) {
                Ok(c) => c,
                Err(e) => return e,
            };
            ask_app(commands, wake, command)
        }
        _ => error(404, "Unbekannter Endpunkt."),
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
    if commands.send(Request { command, reply }).is_err() {
        return error(503, "Die Mac-App beendet gerade den Fernzugriff.");
    }
    wake();
    match answer.recv_timeout(COMMAND_TIMEOUT) {
        Ok(Ok(body)) => (200, body),
        Ok(Err((status, message))) => error(status, &message),
        Err(_) => error(503, "Die Mac-App antwortet nicht. Ist sie geöffnet?"),
    }
}

fn parse_command(endpoint: &str, body: &Value) -> Result<Command, (u16, Value)> {
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
                images.push((name, bytes));
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
                images,
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

fn read_body(request: &mut tiny_http::Request, max: usize) -> Result<Vec<u8>, (u16, Value)> {
    let too_big = || {
        error(
            413,
            &format!(
                "Die Anfrage ist zu groß (höchstens {} MB).",
                max / (1024 * 1024)
            ),
        )
    };
    if request.body_length().is_some_and(|n| n > max) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    request
        .as_reader()
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| error(400, &format!("Anfrage konnte nicht gelesen werden: {e}")))?;
    if bytes.len() > max {
        return Err(too_big());
    }
    Ok(bytes)
}

fn read_json(request: &mut tiny_http::Request) -> Result<Value, (u16, Value)> {
    let bytes = read_body(request, MAX_BODY_BYTES)?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|e| error(400, &format!("Ungültiges JSON: {e}")))
}

fn authorized(request: &tiny_http::Request, token: &str) -> bool {
    let presented = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str())
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

fn percent_decode(s: &str) -> String {
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

fn json_response(status: u16, body: &Value) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let header = tiny_http::Header::from_bytes("Content-Type", "application/json; charset=utf-8")
        .expect("valid header");
    tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header)
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
                            images,
                            uploads,
                        } => {
                            seen.lock().unwrap().push(format!(
                                "send:{text}:{}:{}",
                                images.len(),
                                uploads.join(",")
                            ));
                            if text == "besetzt" {
                                Err((409, "Nemotron arbeitet noch.".into()))
                            } else {
                                Ok(json!({"ok": true}))
                            }
                        }
                        Command::Upload {
                            name,
                            content_type,
                            bytes,
                        } => {
                            seen.lock()
                                .unwrap()
                                .push(format!("upload:{name}:{content_type}:{}", bytes.len()));
                            Ok(json!({"id": "up-1", "kind": "video", "size": bytes.len()}))
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

        let image = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
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
            format!("upload:Mein Clip.mp4:video/mp4:{}", video.len())
        );
        assert_eq!(seen[6], "send:Was passiert?:0:up-1");
        assert!(seen[7].contains("OpenChat") && seen[7].contains("c-1"));
        assert!(seen[8].contains("DeleteChat") && seen[8].contains("c-2"));
        assert_eq!(seen.len(), 9, "bad requests must not reach the app");
        assert!(server.last_seen().is_some_and(|(_, ip)| ip == "127.0.0.1"));
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
