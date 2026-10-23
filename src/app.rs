//! The main window: Nemotron as an agent that reads, edits and runs things
//! on the computer (in the home folder, or in an open project folder), with
//! saved chats that the phone sees too.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use egui_commonmark::CommonMarkCache;
use serde_json::{Value, json};

use crate::agent::{self, AgentConfig, AgentEvent, Decision};
use crate::api::{self, ChatMessage, Role, TextDelta, ToolCall};
use crate::attachments::{self, Attachment, AttachmentKind};
use crate::awake;
use crate::chats::{self, Chat, ChatStore, ChatSummary};
use crate::config::Settings;
use crate::history::InputHistory;
use crate::markers;
use crate::remote::{self, Command, RemoteServer, Reply};
use crate::tools::{ApprovalMode, FileChange, SharedApproval, Workspace};
use crate::widgets::{self, UiAction};

/// What "Weitermachen" sends after an error stopped a run.
const CONTINUE_TEXT: &str = "Weiter: Mach genau dort weiter, wo du wegen des Fehlers aufgehört hast. Schau kurz nach, was schon erledigt ist, und mach nichts doppelt.";
/// After this long without any sign of life from the server the window says
/// that one can stop, or type on to go on at once.
const STALLED_AFTER: Duration = Duration::from_secs(20);
/// A request may move on to a new chat this many times by itself; after that
/// old tool output is cut instead.
const MAX_HANDOFFS: usize = 3;
const APK_RELEASE_URL: &str =
    "https://github.com/fadi2345wfr/fadisnemotron/releases/download/neueste/FadisNemotron.apk";
const NOT_RUN: &str = "Not run: the request was stopped before this tool ran.";
const INTERRUPTED: &str = "Stopped by the user while running; it may have partly run. \
     Check its effects before running it again.";
/// Tool outputs of earlier answers are cut to this many characters in the
/// history, so long sessions still fit the context.
const OLD_TOOL_OUTPUT_CHARS: usize = 2_000;
/// Uploads from the phone wait this long for the message that uses them.
const UPLOAD_TTL: Duration = Duration::from_secs(30 * 60);
/// Uploads waiting for their message may hold this much memory together.
const MAX_STORED_UPLOAD_BYTES: usize = 400 * 1024 * 1024;
/// A phone that asked something within this time counts as connected.
const PHONE_RECENTLY: Duration = Duration::from_secs(90);
/// The phone gets at most this many chats in its list.
const MAX_LISTED_CHATS: usize = 200;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Status {
    Streaming,
    Done,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToolState {
    Planned,
    AwaitingApproval,
    Running,
    Done,
    Declined,
    NotRun,
    /// Was running when the user pressed Stop.
    Interrupted,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ToolView {
    pub call: ToolCall,
    pub state: ToolState,
    pub output: String,
    pub ok: bool,
    /// What the tool changed on disk. Not saved: undo has its own list.
    #[serde(skip)]
    pub change: Option<FileChange>,
    /// What it would change, shown while it waits for approval.
    #[serde(skip)]
    pub preview: Option<FileChange>,
    /// Why this command always needs a yes (e.g. `sudo`).
    pub warning: Option<String>,
    /// Unified diff of `change` or `preview`, computed once.
    pub diff: String,
    /// Added and removed lines of `change` or `preview`.
    pub stats: Option<(usize, usize)>,
}

impl ToolView {
    fn planned(call: ToolCall) -> Self {
        Self {
            call,
            state: ToolState::Planned,
            output: String::new(),
            ok: false,
            change: None,
            preview: None,
            warning: None,
            diff: String::new(),
            stats: None,
        }
    }

    fn set_file_change(&mut self, change: Option<&FileChange>) {
        self.diff = change.map(FileChange::diff).unwrap_or_default();
        self.stats = change.map(FileChange::line_stats);
    }
}

/// One model request within an answer: its thinking, text and tool calls.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Step {
    pub reasoning: String,
    pub content: String,
    pub tools: Vec<ToolView>,
    /// Set once so the thinking folds away when the answer or tools arrive.
    #[serde(skip)]
    pub collapse_reasoning: bool,
    #[serde(skip)]
    reasoning_collapsed: bool,
    /// Thinking was requested; only then can `<think>` in the text be thinking.
    thinking: bool,
}

impl Step {
    /// `(thinking, answer)`, also when the server sent the thinking inline.
    pub fn parts(&self) -> (Option<&str>, &str) {
        if self.reasoning.trim().is_empty() {
            if !self.thinking {
                return (None, &self.content);
            }
            api::split_inline_thinking(&self.content)
        } else {
            (Some(self.reasoning.trim()), self.content.trim_start())
        }
    }

    fn collapse_once(&mut self) {
        if !self.reasoning_collapsed && self.parts().0.is_some() {
            self.collapse_reasoning = true;
            self.reasoning_collapsed = true;
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Message {
    /// Identifies the message while its worker may still report, also after
    /// other messages were added or removed.
    #[serde(skip, default = "next_message_id")]
    id: u64,
    pub role: Role,
    /// The user's text.
    pub text: String,
    /// Saved by the chat store next to the chat.
    #[serde(skip)]
    pub attachments: Vec<Attachment>,
    /// The assistant's steps.
    pub steps: Vec<Step>,
    pub status: Status,
    pub finish_reason: Option<String>,
    pub notices: Vec<String>,
    /// The work went on in this other chat because this one got too long:
    /// the message shows a button that opens it.
    #[serde(default)]
    pub continued_in: Option<String>,
    /// Thinking was on for this answer.
    thinking: bool,
}

impl Message {
    fn user(text: String, attachments: Vec<Attachment>) -> Self {
        Self {
            id: next_message_id(),
            role: Role::User,
            text,
            attachments,
            steps: Vec::new(),
            status: Status::Done,
            finish_reason: None,
            notices: Vec::new(),
            continued_in: None,
            thinking: false,
        }
    }

    fn assistant(thinking: bool) -> Self {
        Self {
            role: Role::Assistant,
            status: Status::Streaming,
            thinking,
            ..Self::user(String::new(), Vec::new())
        }
    }

    fn new_step(&self) -> Step {
        Step {
            thinking: self.thinking,
            ..Step::default()
        }
    }

    /// The final answer: the text of the last step that has some.
    pub fn answer(&self) -> &str {
        self.steps
            .iter()
            .rev()
            .map(|s| s.parts().1.trim())
            .find(|a| !a.is_empty())
            .unwrap_or("")
    }

    fn tool_mut(&mut self, call_id: &str) -> Option<&mut ToolView> {
        self.steps
            .last_mut()?
            .tools
            .iter_mut()
            .find(|t| t.call.id == call_id)
    }

    /// Marks tools that never got to run, or were cut short, after a stop,
    /// an error or the end of the answer.
    fn settle_tools(&mut self) {
        for tool in self.steps.iter_mut().flat_map(|s| &mut s.tools) {
            let state = match tool.state {
                ToolState::Planned | ToolState::AwaitingApproval => ToolState::NotRun,
                ToolState::Running => ToolState::Interrupted,
                _ => continue,
            };
            tool.state = state;
            tool.preview = None;
            tool.set_file_change(None);
        }
    }
}

fn next_message_id() -> u64 {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A file the agent changed during this session, with its original content.
pub struct TrackedChange {
    pub path: String,
    pub abs: PathBuf,
    /// `None` if the agent created the file.
    pub original: Option<String>,
    pub current: String,
    /// Added and removed lines against `original`, kept up to date.
    pub stats: (usize, usize),
    /// Someone else changed the file between two of the agent's edits;
    /// undo would discard that too, so it asks first.
    pub outside_edits: bool,
}

impl TrackedChange {
    fn update_stats(&mut self) {
        self.stats =
            crate::tools::line_stats(self.original.as_deref().unwrap_or(""), &self.current);
    }

    pub fn diff(&self) -> String {
        crate::tools::unified_diff(
            self.original.as_deref().unwrap_or(""),
            &self.current,
            &self.path,
        )
    }
}

struct ActiveRequest {
    rx: Receiver<AgentEvent>,
    cancel: Arc<AtomicBool>,
    decisions: Sender<(String, Decision)>,
    /// What Nemotron may do right now; the switch in the input bar changes it.
    approval: SharedApproval,
    /// When the worker last reported anything (text, a tool, a notice).
    last_event: Instant,
}

struct SettingsDraft {
    settings: Settings,
    show_key: bool,
    check: Option<Receiver<Result<String, String>>>,
    check_result: Option<Result<String, String>>,
}

struct UrlDraft {
    url: String,
    kind: AttachmentKind,
    kind_chosen: bool,
    error: Option<String>,
}

struct Notice {
    text: String,
    error: bool,
}

/// A comment line the app wrote into a file when an error stopped Nemotron
/// there; "Weitermachen" removes it again.
struct ErrorMark {
    abs: PathBuf,
    /// As shown to the user.
    path: String,
}

/// The window that shows one file's change line by line (red: what went
/// away, green: what came in) and undoes it from the same place, so that the
/// result is seen right there.
struct DiffWindow {
    title: String,
    /// The tracked change it belongs to; `None` for the diff of one tool call.
    abs: Option<PathBuf>,
    diff: String,
    /// The file as Nemotron left it, to notice when the change moves on.
    current: Option<String>,
    state: DiffState,
    error: Option<String>,
}

enum DiffState {
    Showing,
    /// Undo was asked for, but the file was changed again outside Nemotron:
    /// the window asks before it throws that away.
    Confirming,
    /// Undone: `diff` now shows what the undo did. `change` lets one take it back.
    Undone {
        change: Option<TrackedChange>,
        deleted: bool,
    },
}

/// A button in the change window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffAction {
    Undo,
    /// "Trotzdem zurücksetzen" after the file was changed outside Nemotron.
    ForceUndo,
    Cancel,
    Redo,
}

/// What an attempt to undo a change came to.
enum UndoResult {
    Done(TrackedChange),
    NeedsConfirm,
    Failed(String),
}

impl DiffWindow {
    fn for_change(tracked: &TrackedChange, state: DiffState) -> Self {
        Self {
            title: tracked.path.clone(),
            abs: Some(tracked.abs.clone()),
            diff: tracked.diff(),
            current: Some(tracked.current.clone()),
            state,
            error: None,
        }
    }

    /// The window after an undo: it shows what the undo changed, from
    /// Nemotron's version back to the original.
    fn undone(tracked: TrackedChange) -> Self {
        let original = tracked.original.clone().unwrap_or_default();
        let diff = crate::tools::unified_diff(&tracked.current, &original, &tracked.path);
        Self {
            title: tracked.path.clone(),
            abs: Some(tracked.abs.clone()),
            diff,
            current: None,
            state: DiffState::Undone {
                deleted: tracked.original.is_none(),
                change: Some(tracked),
            },
            error: None,
        }
    }
}

/// A few words for the mark in the code: the part of the error that says what went wrong.
fn error_reason(error: &str) -> String {
    let first = error
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(error);
    // "Verbindung zu https://… fehlgeschlagen: timeout: receive response"
    let reason = first
        .split("fehlgeschlagen: ")
        .nth(1)
        .map_or(first, |rest| rest);
    if first.starts_with("Verbindung zu") {
        format!("Server nicht erreichbar ({reason})")
    } else {
        reason.to_owned()
    }
}

/// Added and removed lines of a unified diff.
fn diff_stats(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with("+++") || line.starts_with("---") {
            (added, removed)
        } else if line.starts_with('+') {
            (added + 1, removed)
        } else if line.starts_with('-') {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

pub struct NemotronApp {
    settings: Settings,
    settings_path: Option<PathBuf>,
    settings_draft: Option<SettingsDraft>,
    /// Where the agent works: the open project, or the home folder.
    workspace: Workspace,
    changes: Vec<TrackedChange>,
    /// The open chat.
    messages: Vec<Message>,
    chat_id: String,
    chat_created: u64,
    /// Saved chats, newest first; `None` disables saving (tests).
    chat_store: Option<ChatStore>,
    chats: Vec<ChatSummary>,
    delete_chat_confirm: Option<String>,
    /// Photos and videos the phone uploaded for its next message.
    uploads: Vec<(String, Instant, Attachment)>,
    input: String,
    pending: Vec<Attachment>,
    loading_attachments: usize,
    attach_tx: Sender<Result<Attachment, String>>,
    attach_rx: Receiver<Result<Attachment, String>>,
    url_draft: Option<UrlDraft>,
    active: Option<ActiveRequest>,
    /// Workers that were stopped but may still report a file they wrote
    /// just before noticing; kept so the change can still be undone.
    /// With the id of the message they belong to.
    stopped: Vec<(u64, Receiver<AgentEvent>)>,
    notice: Option<Notice>,
    diff_window: Option<DiffWindow>,
    remote: Option<RemoteServer>,
    remote_error: Option<String>,
    remote_window: bool,
    /// The pairing payload and its QR code texture.
    remote_qr: Option<(String, egui::TextureHandle)>,
    /// QR code of the page the phone opens to load the app.
    install_qr: Option<(String, egui::TextureHandle)>,
    /// The Android app came with this Mac app and is offered to phones.
    apk_offered: bool,
    remote_show_token: bool,
    keep_awake: awake::KeepAwake,
    /// What was sent before, for ↑ and ↓ in the input field.
    history: InputHistory,
    /// The files Nemotron changed during the request that runs (or ran last).
    run_edits: Vec<FileChange>,
    /// Error marks in the code that have not been removed yet.
    error_marks: Vec<ErrorMark>,
    /// A run ended because the chat got too long: Nemotron's note for the new
    /// chat and the approval mode of that run.
    pending_handoff: Option<(String, ApprovalMode)>,
    /// How often a request moved on to a new chat without the user writing.
    handoff_chain: usize,
    /// Approval mode carried over to the continued run.
    run_approval: Option<ApprovalMode>,
    /// Answer of the password dialog for "run with the lid closed".
    lid_task: Option<mpsc::Receiver<Result<(), String>>>,
    lid_disabled: Option<bool>,
    lid_checked: Option<Instant>,
    lid_message: Option<String>,
    remote_published: Option<Instant>,
    device_name: String,
    markdown: CommonMarkCache,
    http: ureq::Agent,
    focus_input: bool,
    scroll_to_bottom: bool,
}

impl NemotronApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        widgets::setup_style(&cc.egui_ctx);
        let settings_path = Settings::default_path();
        let settings = settings_path
            .as_deref()
            .map(Settings::load_from)
            .unwrap_or_default();
        let mut app = Self::with_settings(settings, settings_path);
        app.update_title(&cc.egui_ctx);
        // First start: the phone comes first (app, then pairing); the key
        // window follows when that one is closed.
        if app.first_phone_setup() {
            app.remote_window = true;
        } else if app.needs_api_key() {
            app.open_settings();
        }
        // Command line arguments: a folder opens as project, files are attached
        // (`open -a "Fadis Nemotron" --args ~/Projekt bild.png`).
        let mut files = Vec::new();
        for arg in std::env::args_os().skip(1).map(PathBuf::from) {
            if arg.is_dir() {
                app.open_workspace(&arg, &cc.egui_ctx);
            } else if arg.is_file() {
                files.push(arg);
            }
        }
        app.load_paths(files, &cc.egui_ctx);
        if app.settings.remote_enabled {
            app.start_remote(&cc.egui_ctx);
        }
        app
    }

    fn with_settings(settings: Settings, settings_path: Option<PathBuf>) -> Self {
        let (attach_tx, attach_rx) = mpsc::channel();
        let workspace = settings
            .workspace
            .as_deref()
            .and_then(|p| Workspace::open(p).ok())
            .unwrap_or_else(Workspace::home);
        // Saved chats live next to the settings; without a settings file
        // (tests) nothing is written.
        let chat_store = settings_path
            .as_ref()
            .and_then(|_| ChatStore::default_dir())
            .map(ChatStore::new);
        let chats = chat_store.as_ref().map(ChatStore::list).unwrap_or_default();
        let history = InputHistory::load(
            settings_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|dir| dir.join("history.json")),
        );
        Self {
            settings,
            settings_path,
            settings_draft: None,
            workspace,
            changes: Vec::new(),
            messages: Vec::new(),
            chat_id: ChatStore::new_id(),
            chat_created: chats::now(),
            chat_store,
            chats,
            delete_chat_confirm: None,
            uploads: Vec::new(),
            input: String::new(),
            pending: Vec::new(),
            loading_attachments: 0,
            attach_tx,
            attach_rx,
            url_draft: None,
            active: None,
            stopped: Vec::new(),
            notice: None,
            diff_window: None,
            remote: None,
            remote_error: None,
            remote_window: false,
            remote_qr: None,
            install_qr: None,
            apk_offered: false,
            remote_show_token: false,
            keep_awake: awake::KeepAwake::new(),
            history,
            run_edits: Vec::new(),
            error_marks: Vec::new(),
            pending_handoff: None,
            handoff_chain: 0,
            run_approval: None,
            lid_task: None,
            lid_disabled: None,
            lid_checked: None,
            lid_message: None,
            remote_published: None,
            device_name: remote::device_name(),
            markdown: CommonMarkCache::default(),
            http: api::agent(),
            focus_input: true,
            scroll_to_bottom: false,
        }
    }

    /// True until the phone setup window was shown once (and for nobody who
    /// already paired a phone).
    fn first_phone_setup(&self) -> bool {
        !self.settings.phone_setup_seen && self.settings.remote_token.trim().is_empty()
    }

    /// NVIDIA's cloud needs a key; a local vLLM/NIM server usually does not.
    fn needs_api_key(&self) -> bool {
        self.settings.effective_api_key().is_empty()
            && self.settings.base_url.contains("api.nvidia.com")
    }

    fn open_settings(&mut self) {
        if self.settings_draft.is_none() {
            self.settings_draft = Some(SettingsDraft {
                settings: self.settings.clone(),
                show_key: false,
                check: None,
                check_result: None,
            });
        }
    }

    fn save_settings(&mut self) {
        let Some(path) = &self.settings_path else {
            return;
        };
        if let Err(e) = self.settings.save_to(path) {
            self.notice = Some(Notice {
                text: format!("Einstellungen konnten nicht gespeichert werden: {e}"),
                error: true,
            });
        }
    }

    fn update_title(&self, ctx: &egui::Context) {
        let title = if self.workspace.is_home() {
            "Fadis Nemotron".to_owned()
        } else {
            format!("Fadis Nemotron – {}", self.workspace.name())
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    // ------------------------------------------------------------- project

    fn pick_workspace(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Projektordner öffnen");
        dialog = dialog.set_directory(self.workspace.root());
        if let Some(dir) = dialog.pick_folder() {
            self.open_workspace(&dir, ctx);
        }
    }

    fn open_workspace(&mut self, dir: &Path, ctx: &egui::Context) {
        if self.active.is_some() {
            self.notice = Some(Notice {
                text: "Erst die laufende Antwort stoppen, dann den Ordner wechseln.".into(),
                error: true,
            });
            return;
        }
        match Workspace::open(dir) {
            Ok(ws) => {
                self.notice = Some(Notice {
                    text: format!(
                        "Projekt „{}“ geöffnet. Nemotron arbeitet jetzt direkt in diesem Ordner.",
                        ws.name()
                    ),
                    error: false,
                });
                self.settings.workspace = (!ws.is_home()).then(|| ws.root().to_path_buf());
                self.workspace = ws;
                self.changes.clear();
                self.diff_window = None;
                self.save_settings();
                self.update_title(ctx);
                self.focus_input = true;
            }
            Err(e) => {
                self.notice = Some(Notice {
                    text: e,
                    error: true,
                })
            }
        }
    }

    fn close_workspace(&mut self, ctx: &egui::Context) {
        if self.active.is_some() {
            return;
        }
        self.workspace = Workspace::home();
        self.changes.clear();
        self.diff_window = None;
        self.settings.workspace = None;
        self.save_settings();
        self.update_title(ctx);
    }

    fn record_change(&mut self, change: &FileChange) {
        if let Some(tracked) = self.changes.iter_mut().find(|t| t.abs == change.abs) {
            tracked.outside_edits |= change.before.as_deref() != Some(tracked.current.as_str());
            tracked.current = change.after.clone();
            tracked.update_stats();
        } else {
            let mut tracked = TrackedChange {
                path: change.path.clone(),
                abs: change.abs.clone(),
                original: change.before.clone(),
                current: change.after.clone(),
                stats: (0, 0),
                outside_edits: false,
            };
            tracked.update_stats();
            self.changes.push(tracked);
        }
    }

    /// Restores a file to how it was before the agent first touched it.
    /// Puts the file back as it was before Nemotron first touched it. Without
    /// `force` a file that was changed again outside Nemotron is left alone.
    fn try_undo(&mut self, index: usize, force: bool) -> UndoResult {
        let Some(tracked) = self.changes.get(index) else {
            return UndoResult::Failed("Diese Änderung gibt es nicht mehr.".into());
        };
        if !force && self.needs_undo_confirm(tracked) {
            return UndoResult::NeedsConfirm;
        }
        let tracked = self.changes.remove(index);
        let result = match &tracked.original {
            Some(text) => std::fs::write(&tracked.abs, text),
            None => match std::fs::remove_file(&tracked.abs) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        match result {
            Ok(()) => UndoResult::Done(tracked),
            Err(e) => {
                let text = format!("{} konnte nicht zurückgesetzt werden: {e}", tracked.path);
                self.changes.insert(index.min(self.changes.len()), tracked);
                UndoResult::Failed(text)
            }
        }
    }

    /// Undoes a change. With `show` the diff window opens with what the undo
    /// did (or asks first, inside itself, if the file was changed since).
    fn undo_change(&mut self, index: usize, force: bool, show: bool) {
        match self.try_undo(index, force) {
            UndoResult::Done(tracked) => {
                let deleted = tracked.original.is_none();
                self.notice = Some(Notice {
                    text: if deleted {
                        format!("{} gelöscht (war neu).", tracked.path)
                    } else {
                        format!("{} wiederhergestellt.", tracked.path)
                    },
                    error: false,
                });
                if show {
                    self.diff_window = Some(DiffWindow::undone(tracked));
                }
            }
            UndoResult::NeedsConfirm => {
                if let Some(tracked) = self.changes.get(index) {
                    self.diff_window = Some(DiffWindow::for_change(tracked, DiffState::Confirming));
                }
            }
            UndoResult::Failed(text) => {
                self.notice = Some(Notice { text, error: true });
            }
        }
    }

    /// Takes an undo back: the file gets Nemotron's version again.
    fn redo_change(&mut self, tracked: TrackedChange) -> Result<(), String> {
        std::fs::write(&tracked.abs, &tracked.current).map_err(|e| {
            format!(
                "{} konnte nicht wiederhergestellt werden: {e}",
                tracked.path
            )
        })?;
        self.changes.push(tracked);
        Ok(())
    }

    /// True if undo would also discard changes not made by the agent.
    fn needs_undo_confirm(&self, tracked: &TrackedChange) -> bool {
        tracked.outside_edits
            || std::fs::read_to_string(&tracked.abs).ok().as_deref()
                != Some(tracked.current.as_str())
    }

    fn reveal_in_file_manager(path: &Path) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(path).spawn();
    }

    /// Starts an empty chat. If the open one cannot be saved it stays open.
    fn new_chat(&mut self, ctx: &egui::Context) -> Result<(), String> {
        self.stop();
        self.try_save_chat()?;
        for att in &self.pending {
            ctx.forget_image(&att.preview_uri());
        }
        self.pending.clear();
        self.input.clear();
        self.history.stop_browsing();
        self.show_chat(Vec::new(), ctx);
        self.chat_id = ChatStore::new_id();
        self.chat_created = chats::now();
        Ok(())
    }

    // --------------------------------------------------------------- chats

    /// Replaces the messages on screen, e.g. with another chat's.
    fn show_chat(&mut self, messages: Vec<Message>, ctx: &egui::Context) {
        for att in self.messages.iter().flat_map(|m| &m.attachments) {
            ctx.forget_image(&att.preview_uri());
        }
        self.messages = messages;
        self.notice = None;
        self.markdown = CommonMarkCache::default();
        self.focus_input = true;
        self.scroll_to_bottom = true;
    }

    fn chat_title(&self) -> String {
        let first = self.messages.iter().find(|m| m.role == Role::User);
        chats::title_from(
            first.map_or("", |m| m.text.as_str()),
            first
                .and_then(|m| m.attachments.first())
                .map(|a| a.name.as_str()),
        )
    }

    /// Writes the open chat to disk as well as possible; the notice says if it fails.
    fn save_chat(&mut self) {
        let _ = self.try_save_chat();
    }

    /// Writes the open chat to disk; a chat without messages is not saved.
    /// On failure the notice says why, and the chat must not be thrown away.
    fn try_save_chat(&mut self) -> Result<(), String> {
        let Some(store) = &self.chat_store else {
            return Ok(());
        };
        if self.messages.is_empty() {
            return Ok(());
        }
        let chat = Chat {
            id: self.chat_id.clone(),
            title: self.chat_title(),
            created: self.chat_created,
            messages: self
                .messages
                .iter()
                .map(|m| (m, m.attachments.clone()))
                .collect(),
        };
        match store.save(&chat) {
            Ok(summary) => {
                self.chats.retain(|c| c.id != summary.id);
                self.chats.push(summary);
                // An unchanged chat keeps its old time, so it must not jump to the top.
                self.chats
                    .sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
                Ok(())
            }
            Err(e) => {
                let text = format!("Chat konnte nicht gespeichert werden: {e}");
                self.notice = Some(Notice {
                    text: text.clone(),
                    error: true,
                });
                Err(text)
            }
        }
    }

    fn open_chat(&mut self, id: &str, ctx: &egui::Context) -> Result<(), (u16, String)> {
        if self.active.is_some() {
            return Err(busy_error());
        }
        if id == self.chat_id {
            return Ok(());
        }
        let not_found = || (404, "Chat nicht gefunden.".to_owned());
        let store = self.chat_store.as_ref().ok_or_else(not_found)?;
        if !store.exists(id) {
            return Err(not_found());
        }
        let chat: Chat<Message> = store.load(id).map_err(|e| (500, e))?;
        // The open chat is only left once it is safe.
        self.try_save_chat().map_err(|e| (500, e))?;
        let messages = chat
            .messages
            .into_iter()
            .map(|(mut message, attachments)| {
                message.attachments = attachments;
                // Saved while it was still answering, e.g. the app was quit.
                if message.status == Status::Streaming {
                    message.status = Status::Cancelled;
                    message.settle_tools();
                }
                message
            })
            .collect();
        self.show_chat(messages, ctx);
        self.chat_id = chat.id;
        self.chat_created = chat.created;
        Ok(())
    }

    fn delete_chat(&mut self, id: &str, ctx: &egui::Context) -> Result<(), (u16, String)> {
        let current = id == self.chat_id;
        if current && self.active.is_some() {
            return Err(busy_error());
        }
        let saved = self.chat_store.as_ref().is_some_and(|s| s.exists(id));
        if !saved && !current {
            return Err((404, "Chat nicht gefunden.".to_owned()));
        }
        if saved && let Some(store) = &self.chat_store {
            store
                .delete(id)
                .map_err(|e| (500, format!("Chat konnte nicht gelöscht werden: {e}")))?;
        }
        self.chats.retain(|c| c.id != id);
        if current {
            // Nothing left to save: then open the newest other chat, or a new one.
            self.show_chat(Vec::new(), ctx);
            match self.chats.first().map(|c| c.id.clone()) {
                Some(next) if self.open_chat(&next, ctx).is_ok() => {}
                _ => self.new_chat(ctx).map_err(|e| (500, e))?,
            }
        }
        Ok(())
    }

    /// The open chat and the saved ones, for the phone.
    fn chat_list_json(&self) -> Value {
        let mut list: Vec<Value> = Vec::new();
        if !self.chats.iter().any(|c| c.id == self.chat_id) {
            list.push(json!({
                "id": self.chat_id,
                "title": self.chat_title(),
                "updated": self.chat_created,
                "messages": self.messages.len(),
            }));
        }
        list.extend(self.chats.iter().map(
            |c| json!({"id": c.id, "title": c.title, "updated": c.updated, "messages": c.messages}),
        ));
        list.truncate(MAX_LISTED_CHATS);
        Value::Array(list)
    }

    /// Keeps a photo or video from the phone until a message uses it. The
    /// server already checked and prepared it.
    fn take_upload(&mut self, attachment: Attachment) -> Reply {
        let size = match &attachment.source {
            attachments::Source::Data { bytes, .. } => bytes.len(),
            attachments::Source::Url(_) => 0,
        };
        let kind = match attachment.kind {
            AttachmentKind::Image => "image",
            AttachmentKind::Video => "video",
        };
        self.prune_uploads();
        // Oldest go first when the phone sends more than fits.
        let stored = |uploads: &[(String, Instant, Attachment)]| -> usize {
            uploads.iter().map(|(_, _, a)| attachment_bytes(a)).sum()
        };
        while !self.uploads.is_empty() && stored(&self.uploads) + size > MAX_STORED_UPLOAD_BYTES {
            self.uploads.remove(0);
        }
        let mut random = [0u8; 4];
        let _ = getrandom::fill(&mut random);
        let id = format!(
            "up-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        self.uploads.push((id.clone(), Instant::now(), attachment));
        Ok(json!({"id": id, "kind": kind, "size": size}))
    }

    /// Drops uploads no message used within [`UPLOAD_TTL`].
    fn prune_uploads(&mut self) {
        self.uploads.retain(|(_, at, _)| at.elapsed() < UPLOAD_TTL);
    }

    // ------------------------------------------------------------- sending

    /// What Nemotron may do: the running request's mode, else the setting.
    fn current_mode(&self) -> ApprovalMode {
        self.active
            .as_ref()
            .map_or(self.settings.approval, |a| a.approval.get())
    }

    /// Switches the mode for good and for the request that is running now.
    fn set_mode(&mut self, mode: ApprovalMode) {
        if let Some(active) = &self.active {
            active.approval.set(mode);
        }
        if self.settings.approval != mode {
            self.settings.approval = mode;
            if let Some(draft) = &mut self.settings_draft {
                draft.settings.approval = mode;
            }
            self.save_settings();
        }
    }

    /// The error that ended the last answer, if the chat stands there: the
    /// window then offers to go on with one click.
    fn interrupted_by(&self) -> Option<&str> {
        if self.active.is_some() {
            return None;
        }
        match self.messages.last() {
            Some(Message {
                role: Role::Assistant,
                status: Status::Failed(e),
                ..
            }) => Some(e.as_str()),
            _ => None,
        }
    }

    /// An error ended the answer after Nemotron changed files: one comment
    /// line, behind the last line it changed in the last file, says so. The
    /// developer sees it in the diff; "Weitermachen" removes it. Only if the
    /// file is still exactly as Nemotron left it and its type has comments.
    fn mark_error_in_code(&mut self, error: &str) {
        if !self.settings.mark_errors {
            return;
        }
        let Some(last) = self.run_edits.last().cloned() else {
            return;
        };
        if self.error_marks.iter().any(|m| m.abs == last.abs) {
            return;
        }
        let Ok(current) = std::fs::read_to_string(&last.abs) else {
            return;
        };
        if current != last.after {
            return;
        }
        let Some(mark) = markers::marker_line(&last.abs, &error_reason(error)) else {
            return;
        };
        let above = markers::lines_above_mark(last.before.as_deref(), &last.after);
        let marked = markers::insert_mark(&current, above, &mark);
        if std::fs::write(&last.abs, &marked).is_err() {
            return;
        }
        // The mark is part of the change now, so Undo and the diff see it.
        if let Some(tracked) = self.changes.iter_mut().find(|t| t.abs == last.abs) {
            tracked.current = marked;
            tracked.update_stats();
        }
        self.error_marks.push(ErrorMark {
            abs: last.abs,
            path: last.path,
        });
    }

    /// Takes the marks out of the code again; returns how many lines went.
    /// Only lines the app wrote are touched.
    fn remove_error_marks(&mut self) -> usize {
        let mut removed_total = 0;
        for mark in std::mem::take(&mut self.error_marks) {
            let Ok(content) = std::fs::read_to_string(&mark.abs) else {
                continue;
            };
            let (cleaned, removed) = markers::remove_marks(&content);
            if removed == 0 {
                continue;
            }
            if std::fs::write(&mark.abs, &cleaned).is_err() {
                self.error_marks.push(mark);
                continue;
            }
            removed_total += removed;
            if let Some(tracked) = self.changes.iter_mut().find(|t| t.abs == mark.abs) {
                tracked.current = markers::remove_marks(&tracked.current).0;
                tracked.update_stats();
            }
        }
        removed_total
    }

    /// "Weitermachen": the marks leave the code, and a short message asks
    /// Nemotron to go on where the error stopped it. Typed text and
    /// attachments stay in the input field.
    fn continue_after_error(&mut self, ctx: &egui::Context) {
        if self.interrupted_by().is_none() {
            return;
        }
        if self.needs_api_key() {
            self.open_settings();
            return;
        }
        let removed = self.remove_error_marks();
        self.messages
            .push(Message::user(CONTINUE_TEXT.to_owned(), Vec::new()));
        self.notice = (removed > 0).then(|| Notice {
            text: format!(
                "{removed} Fehler-{} aus dem Code entfernt.",
                if removed == 1 {
                    "Markierung"
                } else {
                    "Markierungen"
                }
            ),
            error: false,
        });
        self.save_chat();
        self.handoff_chain = 0;
        self.start_request(ctx);
    }

    /// Sending is possible while Nemotron works too: that run is stopped and
    /// the new message goes out at once (the server may be hanging).
    fn can_send(&self) -> bool {
        self.loading_attachments == 0 && (!self.input.trim().is_empty() || !self.pending.is_empty())
    }

    /// How long the running request has been silent, if that is long enough
    /// to be worth a hint: no text, no tool, no notice, and the app is not
    /// just waiting for the user's answer or for a command.
    fn stalled_for(&self) -> Option<Duration> {
        let active = self.active.as_ref()?;
        let silent = active.last_event.elapsed();
        if silent < STALLED_AFTER {
            return None;
        }
        let busy_with_tool = self.messages.last().is_some_and(|m| {
            m.steps
                .iter()
                .flat_map(|s| &s.tools)
                .any(|t| matches!(t.state, ToolState::AwaitingApproval | ToolState::Running))
        });
        (!busy_with_tool).then_some(silent)
    }

    fn send(&mut self, ctx: &egui::Context) {
        if !self.can_send() {
            return;
        }
        if self.needs_api_key() {
            self.notice = Some(Notice {
                text: "Bitte zuerst einen NVIDIA-API-Schlüssel in den Einstellungen eintragen."
                    .into(),
                error: true,
            });
            self.open_settings();
            return;
        }
        let text = self.input.trim().to_owned();
        // A hanging request makes way for the new message.
        self.stop();
        self.history.push(&text);
        self.messages
            .push(Message::user(text, std::mem::take(&mut self.pending)));
        self.input.clear();
        self.notice = None;
        self.save_chat();
        self.handoff_chain = 0;
        self.start_request(ctx);
    }

    /// The conversation so far in API form, including every tool call and
    /// its result, so follow-up messages keep the context.
    fn api_history(&self) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        let last_answer = self
            .messages
            .iter()
            .rposition(|m| m.role == Role::Assistant && m.status != Status::Streaming);
        for (index, msg) in self.messages.iter().enumerate() {
            match msg.role {
                Role::User => out.push(ChatMessage::User {
                    text: msg.text.clone(),
                    attachments: msg.attachments.clone(),
                }),
                Role::Assistant if msg.status == Status::Streaming => {}
                Role::Assistant => {
                    for step in &msg.steps {
                        let answer = step.parts().1.trim().to_owned();
                        if step.tools.is_empty() {
                            if !answer.is_empty() {
                                out.push(ChatMessage::Assistant {
                                    content: answer,
                                    reasoning: None,
                                    tool_calls: Vec::new(),
                                });
                            }
                            continue;
                        }
                        out.push(ChatMessage::Assistant {
                            content: answer,
                            reasoning: None,
                            tool_calls: step.tools.iter().map(|t| t.call.clone()).collect(),
                        });
                        for tool in &step.tools {
                            let content = match tool.state {
                                ToolState::Done | ToolState::Declined
                                    if Some(index) == last_answer =>
                                {
                                    tool.output.clone()
                                }
                                ToolState::Done | ToolState::Declined => {
                                    shorten_old_output(&tool.output)
                                }
                                ToolState::Interrupted => INTERRUPTED.to_owned(),
                                _ => NOT_RUN.to_owned(),
                            };
                            out.push(ChatMessage::Tool {
                                call_id: tool.call.id.clone(),
                                content,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    fn start_request(&mut self, ctx: &egui::Context) {
        let history = self.api_history();
        self.run_edits.clear();
        self.messages
            .push(Message::assistant(self.settings.thinking));
        self.scroll_to_bottom = true;

        let (tx, rx) = mpsc::channel();
        let (decision_tx, decision_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let system_prompt = agent::system_prompt(&self.settings.system_prompt, &self.workspace);
        let approval =
            SharedApproval::new(self.run_approval.take().unwrap_or(self.settings.approval));
        let config = AgentConfig {
            base_url: self.settings.base_url.clone(),
            api_key: self.settings.effective_api_key(),
            system_prompt,
            options: self.settings.generation_options(),
            workspace: Some(self.workspace.clone()),
            approval: approval.clone(),
            max_steps: self.settings.max_steps.max(1),
            handoff: self.handoff_chain < MAX_HANDOFFS,
        };
        let http = self.http.clone();
        let worker_cancel = cancel.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            agent::run(
                &http,
                config,
                history,
                &worker_cancel,
                &decision_rx,
                |event| {
                    let delivered = tx.send(event).is_ok();
                    ctx.request_repaint();
                    delivered
                },
            );
        });
        self.active = Some(ActiveRequest {
            rx,
            cancel,
            decisions: decision_tx,
            approval,
            last_event: Instant::now(),
        });
    }

    fn stop(&mut self) {
        // Take in whatever already arrived, e.g. a finished edit.
        self.poll_agent();
        if let Some(active) = self.active.take() {
            active.cancel.store(true, Ordering::Relaxed);
            if let Some(msg) = self.messages.last_mut()
                && msg.status == Status::Streaming
            {
                msg.status = Status::Cancelled;
                msg.settle_tools();
            }
            let id = self.messages.last().map_or(0, |m| m.id);
            self.stopped.push((id, active.rx));
            self.save_chat();
        }
    }

    /// Takes in what stopped workers still report: a tool that was running
    /// at Stop finishes, maybe after writing a file.
    fn poll_stopped(&mut self) {
        let mut changes = Vec::new();
        let mut finished = Vec::new();
        self.stopped.retain(|(id, rx)| {
            loop {
                match rx.try_recv() {
                    Ok(AgentEvent::ToolFinished {
                        call_id,
                        output,
                        ok,
                        change,
                    }) => {
                        if let Some(change) = &change {
                            changes.push(change.clone());
                        }
                        finished.push((*id, call_id, output, ok, change));
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break true,
                    Err(TryRecvError::Disconnected) => break false,
                }
            }
        });
        for (id, call_id, output, ok, change) in finished {
            if let Some(tool) = self
                .messages
                .iter_mut()
                .find(|m| m.id == id)
                .and_then(|m| m.tool_mut(&call_id))
                .filter(|t| t.state == ToolState::Interrupted)
            {
                tool.state = ToolState::Done;
                tool.output = output;
                tool.ok = ok;
                tool.set_file_change(change.as_ref());
                tool.change = change;
            }
        }
        for change in &changes {
            self.record_change(change);
        }
        if !changes.is_empty() {
            self.notice = Some(Notice {
                text: "Nemotron hat vor dem Stopp noch eine Datei geändert – sie steht rechts in den Änderungen.".into(),
                error: false,
            });
        }
    }

    /// Drops the last answer and asks again. File changes stay in place.
    fn retry(&mut self, ctx: &egui::Context) {
        if self.active.is_some() {
            return;
        }
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Assistant)
        {
            self.messages.pop();
        }
        if self.messages.last().is_some_and(|m| m.role == Role::User) {
            self.remove_error_marks();
            self.handoff_chain = 0;
            self.start_request(ctx);
        }
    }

    fn decide(&mut self, call_id: &str, decision: Decision) {
        let Some(active) = &self.active else { return };
        let _ = active.decisions.send((call_id.to_owned(), decision));
        if let Some(tool) = self.messages.last_mut().and_then(|m| m.tool_mut(call_id)) {
            tool.state = if decision == Decision::Deny {
                ToolState::Declined
            } else {
                ToolState::Running
            };
            tool.preview = None;
            tool.set_file_change(None);
        }
    }

    fn poll_agent(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        let mut events = Vec::new();
        let mut disconnected = false;
        loop {
            match active.rx.try_recv() {
                Ok(e) => events.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if !events.is_empty() {
            active.last_event = Instant::now();
        }
        let mut finished = false;
        let mut changes = Vec::new();
        let mut handoff = None;
        if let Some(msg) = self.messages.last_mut() {
            for event in events {
                if let AgentEvent::Handoff { note, approval } = &event {
                    handoff = Some((note.clone(), *approval));
                }
                finished |= apply_event(msg, event, &mut changes);
            }
            if disconnected && !finished {
                msg.status = Status::Failed("Die Verbindung wurde unerwartet beendet.".into());
                msg.settle_tools();
                finished = true;
            }
        }
        for change in &changes {
            self.record_change(change);
            self.run_edits.push(change.clone());
        }
        if finished {
            self.active = None;
            // An error after changes: say so in the code where it stopped.
            if let Some(Message {
                role: Role::Assistant,
                status: Status::Failed(error),
                ..
            }) = self.messages.last()
            {
                let error = error.clone();
                self.mark_error_in_code(&error);
            }
            self.save_chat();
            if handoff.is_some() {
                self.pending_handoff = handoff;
            }
        }
    }

    /// The chat got too long: opens a new chat that holds the user's request
    /// and Nemotron's note, and lets Nemotron go on there.
    fn continue_in_new_chat(&mut self, ctx: &egui::Context) {
        let Some((note, approval)) = self.pending_handoff.take() else {
            return;
        };
        let request = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| (m.text.clone(), m.attachments.clone()));
        let old_title = self.chat_title();
        // The old chat points to the new one; it is saved with that link.
        let new_id = ChatStore::new_id();
        if let Some(last) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant)
        {
            last.continued_in = Some(new_id.clone());
        }
        if let Err(e) = self.new_chat(ctx) {
            self.notice = Some(Notice {
                text: format!(
                    "Der Chat wurde nicht gespeichert, deshalb geht es nicht in einem neuen Chat weiter: {e}"
                ),
                error: true,
            });
            return;
        }
        self.chat_id = new_id;
        let (request, attachments) = request.unwrap_or_default();
        // The first line becomes the title in the chat list.
        let text = format!(
            "↪ {}\n\nAuftrag aus dem vorigen Chat, der zu lang geworden war:\n{request}\n\nNemotrons Notiz zum Stand:\n{note}\n\nMach genau dort weiter, wo die Notiz endet. Lies Dateien nur wieder, wo du sie wirklich brauchst.",
            old_title.trim_start_matches("↪ ")
        );
        self.messages
            .push(Message::user(text.trim().to_owned(), attachments));
        self.handoff_chain += 1;
        self.run_approval = Some(approval);
        self.save_chat();
        self.start_request(ctx);
    }

    // --------------------------------------------------------- attachments

    fn load_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if paths.is_empty() {
            return;
        }
        self.loading_attachments += paths.len();
        let max_dim = self.settings.max_image_dim;
        let tx = self.attach_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in paths {
                let result = attachments::load_file(&path, max_dim);
                if tx.send(result).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
    }

    fn poll_attachments(&mut self) {
        while let Ok(result) = self.attach_rx.try_recv() {
            self.loading_attachments = self.loading_attachments.saturating_sub(1);
            match result {
                Ok(att) => self.pending.push(att),
                Err(e) => {
                    let text = match self.notice.take() {
                        Some(n) if n.error => format!("{}\n{e}", n.text),
                        _ => e,
                    };
                    self.notice = Some(Notice { text, error: true });
                }
            }
        }
    }

    fn pick_files(&mut self, ctx: &egui::Context) {
        let files = rfd::FileDialog::new()
            .set_title("Bilder oder Videos anhängen")
            .add_filter("Bilder und Videos", &attachments::picker_extensions())
            .pick_files();
        if let Some(paths) = files {
            self.load_paths(paths, ctx);
        }
    }

    fn export_markdown(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Chat exportieren")
            .set_file_name("nemotron-chat.md")
            .add_filter("Markdown", &["md"])
            .save_file()
        else {
            return;
        };
        let notice = match std::fs::write(
            &path,
            conversation_markdown(&self.messages, &self.settings.model),
        ) {
            Ok(()) => Notice {
                text: format!("Chat gespeichert: {}", path.display()),
                error: false,
            },
            Err(e) => Notice {
                text: format!("Export fehlgeschlagen: {e}"),
                error: true,
            },
        };
        self.notice = Some(notice);
    }

    // --------------------------------------------------------------- input

    fn handle_global_input(&mut self, ctx: &egui::Context) {
        let (settings, new_chat, open_project, escape) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::O)),
                i.key_pressed(Key::Escape),
            )
        });
        if settings {
            self.open_settings();
        }
        if new_chat {
            // A failed save leaves the chat open and says why in the notice.
            let _ = self.new_chat(ctx);
        }
        if open_project {
            self.pick_workspace(ctx);
        }
        let dialog_open =
            self.settings_draft.is_some() || self.url_draft.is_some() || self.diff_window.is_some();
        if escape && self.active.is_some() && !dialog_open {
            self.stop();
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        // A dropped folder opens as project; files are attached.
        let (dirs, files): (Vec<_>, Vec<_>) = dropped.into_iter().partition(|p| p.is_dir());
        if let Some(dir) = dirs.first() {
            self.open_workspace(dir, ctx);
        }
        self.load_paths(files, ctx);
    }

    // -------------------------------------------------------------- remote

    fn start_remote(&mut self, ctx: &egui::Context) {
        if self.settings.remote_token.trim().is_empty() {
            self.settings.remote_token = remote::new_token();
            self.save_settings();
        }
        // Free the port before binding it again.
        self.remote = None;
        self.remote_qr = None;
        self.install_qr = None;
        self.remote_published = None;
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        match RemoteServer::start(
            self.settings.remote_port,
            self.settings.remote_token.clone(),
            self.device_name.clone(),
            wake,
        ) {
            Ok(server) => {
                let apk = remote::bundled_app_file();
                self.apk_offered = apk.is_some();
                server.offer_app(apk);
                self.remote = Some(server);
                self.remote_error = None;
            }
            Err(e) => self.remote_error = Some(e),
        }
    }

    fn set_remote_enabled(&mut self, on: bool, ctx: &egui::Context) {
        self.settings.remote_enabled = on;
        self.save_settings();
        if on {
            self.start_remote(ctx);
        } else {
            self.remote = None;
            self.remote_error = None;
            self.remote_qr = None;
            self.install_qr = None;
        }
    }

    /// Locks out every paired phone.
    fn renew_remote_token(&mut self, ctx: &egui::Context) {
        self.settings.remote_token = remote::new_token();
        self.save_settings();
        if self.remote.is_some() {
            self.start_remote(ctx);
        }
    }

    /// Keeps the Mac awake while the phone can reach it or an agent works
    /// (with the screen on, as it may look at it), and takes in the answer
    /// of the password dialog for sleeping with the lid closed.
    fn update_awake(&mut self, ctx: &egui::Context) {
        let level = if self.active.is_some() && self.settings.keep_awake {
            awake::Awake::Screen
        } else if self.remote.is_some() && self.settings.keep_awake {
            awake::Awake::System
        } else {
            awake::Awake::No
        };
        self.keep_awake.set(level);
        if let Some(task) = &self.lid_task {
            match task.try_recv() {
                Ok(result) => {
                    self.lid_task = None;
                    self.lid_message = result.err();
                    self.lid_disabled = awake::lid_sleep_disabled();
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(300));
                }
                Err(TryRecvError::Disconnected) => self.lid_task = None,
            }
        }
    }

    /// Runs what the phone asked for and publishes the new state.
    fn handle_remote(&mut self, ctx: &egui::Context) {
        if let Some(server) = &self.remote {
            server.set_max_image_dim(self.settings.max_image_dim);
        }
        self.prune_uploads();
        let mut handled = false;
        while let Some(request) = self.remote.as_ref().and_then(RemoteServer::next_command) {
            let mut request = request;
            let reply = self.remote_command(&mut request.command, ctx);
            // Publish before answering, so the phone's next look sees the result.
            self.remote_published = None;
            self.publish_remote(ctx);
            request.answer(reply);
            handled = true;
        }
        if !handled {
            self.publish_remote(ctx);
        }
    }

    fn remote_command(&mut self, command: &mut Command, ctx: &egui::Context) -> Reply {
        let busy = busy_error;
        let ok = Ok(json!({ "ok": true }));
        match command {
            Command::Upload { attachment } => self.take_upload(attachment.clone()),
            Command::OpenChat { id } => self.open_chat(id, ctx).map(|()| json!({ "ok": true })),
            Command::DeleteChat { id } => self.delete_chat(id, ctx).map(|()| json!({ "ok": true })),
            Command::Send {
                text,
                attachments: inline,
                uploads,
                interrupt,
            } => {
                if self.active.is_some() && !*interrupt {
                    return Err(busy());
                }
                if self.needs_api_key() {
                    return Err((
                        400,
                        "Am Mac ist noch kein NVIDIA-API-Schlüssel eingetragen.".into(),
                    ));
                }
                self.prune_uploads();
                if let Some(missing) = uploads
                    .iter()
                    .find(|id| !self.uploads.iter().any(|(u, _, _)| u == *id))
                {
                    return Err((
                        404,
                        format!("Anhang {missing} ist abgelaufen – bitte erneut senden."),
                    ));
                }
                let mut attachments = std::mem::take(inline);
                for id in uploads.iter() {
                    if let Some(i) = self.uploads.iter().position(|(u, _, _)| u == id) {
                        attachments.push(self.uploads.remove(i).2);
                    }
                }
                // The phone typed on while the server hung: that run ends here.
                self.stop();
                self.history.push(text);
                self.messages
                    .push(Message::user(text.trim().to_owned(), attachments));
                self.notice = None;
                self.save_chat();
                self.handoff_chain = 0;
                self.start_request(ctx);
                ok
            }
            Command::Stop => {
                self.stop();
                ok
            }
            Command::Continue => {
                if self.active.is_some() {
                    return Err(busy());
                }
                if self.interrupted_by().is_none() {
                    return Err((409, "Es gibt nichts zum Weitermachen.".into()));
                }
                self.continue_after_error(ctx);
                ok
            }
            Command::Decide { call_id, decision } => {
                let waiting = self.active.is_some()
                    && self
                        .messages
                        .last_mut()
                        .and_then(|m| m.tool_mut(call_id))
                        .is_some_and(|t| t.state == ToolState::AwaitingApproval);
                if !waiting {
                    return Err((404, "Diese Freigabe ist nicht mehr offen.".into()));
                }
                self.decide(call_id, *decision);
                ok
            }
            Command::NewChat => {
                if self.active.is_some() {
                    return Err(busy());
                }
                self.new_chat(ctx).map_err(|e| (500, e))?;
                ok
            }
            Command::Retry => {
                if self.active.is_some() {
                    return Err(busy());
                }
                if self.messages.is_empty() {
                    return Err((409, "Es gibt noch nichts zu wiederholen.".into()));
                }
                self.retry(ctx);
                ok
            }
            Command::Settings { approval, thinking } => {
                if let Some(mode) = approval {
                    self.set_mode(*mode);
                }
                if let Some(on) = thinking {
                    self.settings.thinking = *on;
                }
                if let Some(draft) = &mut self.settings_draft {
                    draft.settings.thinking = self.settings.thinking;
                }
                self.save_settings();
                ok
            }
            Command::Undo { path } => {
                if self.active.is_some() {
                    return Err(busy());
                }
                let Some(index) = self.changes.iter().position(|c| &c.path == path) else {
                    return Err((404, format!("{path} steht nicht in der Änderungsliste.")));
                };
                if self.needs_undo_confirm(&self.changes[index]) {
                    return Err((
                        409,
                        format!(
                            "{path} wurde inzwischen anders verändert. Bitte am Mac zurücksetzen und bestätigen."
                        ),
                    ));
                }
                self.undo_change(index, true, false);
                match &self.notice {
                    Some(n) if n.error => Err((500, n.text.clone())),
                    _ => ok,
                }
            }
            Command::Diff { path } => match self.changes.iter().find(|c| &c.path == path) {
                Some(change) => Ok(json!({ "path": path, "diff": change.diff() })),
                None => Err((404, format!("{path} steht nicht in der Änderungsliste."))),
            },
        }
    }

    /// Hands the current state to the server, at most a few times a second.
    fn publish_remote(&mut self, ctx: &egui::Context) {
        const EVERY: Duration = Duration::from_millis(150);
        // Building the snapshot costs time on the window's thread; nobody
        // looking, nothing to build.
        if !self
            .remote
            .as_ref()
            .is_some_and(|s| s.recently_seen(PHONE_RECENTLY))
        {
            return;
        }
        if let Some(last) = self.remote_published
            && last.elapsed() < EVERY
        {
            ctx.request_repaint_after(EVERY - last.elapsed());
            return;
        }
        let snapshot = self.remote_snapshot();
        if let Some(server) = &self.remote {
            server.publish(snapshot);
        }
        self.remote_published = Some(Instant::now());
    }

    /// What the phone shows, as described in docs/remote-api.md.
    fn remote_snapshot(&self) -> Value {
        let messages: Vec<Value> = self
            .messages
            .iter()
            .map(|msg| match msg.role {
                Role::User => json!({
                    "role": "user",
                    "text": msg.text,
                    "attachments": msg.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
                }),
                Role::Assistant => {
                    let (status, error) = match &msg.status {
                        Status::Streaming => ("streaming", None),
                        Status::Done => ("done", None),
                        Status::Cancelled => ("cancelled", None),
                        Status::Failed(e) => ("failed", Some(e.as_str())),
                    };
                    let steps: Vec<Value> = msg
                        .steps
                        .iter()
                        .map(|step| {
                            let (thinking, answer) = step.parts();
                            let tools: Vec<Value> = step
                                .tools
                                .iter()
                                .map(|t| {
                                    json!({
                                        "id": t.call.id,
                                        "name": crate::tools::canonical_tool_name(&t.call.name),
                                        "title": widgets::tool_title(t),
                                        "state": match t.state {
                                            ToolState::Planned => "planned",
                                            ToolState::AwaitingApproval => "awaiting_approval",
                                            ToolState::Running => "running",
                                            ToolState::Done => "done",
                                            ToolState::Declined => "declined",
                                            ToolState::NotRun => "not_run",
                                            ToolState::Interrupted => "interrupted",
                                        },
                                        "ok": t.ok,
                                        "output": tail_chars(&t.output, 4_000),
                                        "diff": head_chars(&t.diff, 8_000),
                                        "added": t.stats.map_or(0, |s| s.0),
                                        "removed": t.stats.map_or(0, |s| s.1),
                                        "warning": t.warning,
                                    })
                                })
                                .collect();
                            json!({
                                "reasoning": tail_chars(thinking.unwrap_or(""), 6_000),
                                "content": answer,
                                "tools": tools,
                            })
                        })
                        .collect();
                    json!({
                        "role": "assistant",
                        "status": status,
                        "error": error,
                        "finish_reason": msg.finish_reason,
                        "notices": msg.notices,
                        "continued_in": msg.continued_in,
                        "steps": steps,
                    })
                }
            })
            .collect();
        let changes: Vec<Value> = self
            .changes
            .iter()
            .map(|c| json!({"path": c.path, "added": c.stats.0, "removed": c.stats.1, "created": c.original.is_none()}))
            .collect();
        json!({
            "busy": self.active.is_some(),
            "stalled": self.stalled_for().is_some(),
            "error_marks": self.error_marks.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
            "model": self.settings.model,
            "thinking": self.settings.thinking,
            "approval": match self.current_mode() {
                ApprovalMode::AskAll => "AskAll",
                ApprovalMode::AutoEdits => "AutoEdits",
                ApprovalMode::AutoAll => "AutoAll",
            },
            "workspace": json!({
                "name": self.workspace.name(),
                "path": self.workspace.root().display().to_string(),
                "home": self.workspace.is_home(),
            }),
            "notice": self.notice.as_ref().map(|n| json!({"text": n.text, "error": n.error})),
            "chat": {"id": self.chat_id, "title": self.chat_title()},
            "chats": self.chat_list_json(),
            "changes": changes,
            "messages": messages,
        })
    }

    fn remote_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        if !self.remote_window {
            return;
        }
        let mut open = true;
        let mut enabled = self.remote.is_some();
        let mut toggled = false;
        let mut renew = false;
        let mut restart = false;
        let mut copy = None;
        let mut awake_changed = false;
        let mut lid_wanted = None;
        let addresses = remote::local_addresses();
        let port = self
            .remote
            .as_ref()
            .map_or(self.settings.remote_port, RemoteServer::port);
        let payload = remote::pairing_payload(
            &self.device_name,
            port,
            &self.settings.remote_token,
            &addresses,
        );
        let install_url = remote::install_url(&addresses, port);
        if self.remote.is_some() {
            if self.remote_qr.as_ref().is_none_or(|(p, _)| *p != payload) {
                self.remote_qr = qr_texture(ctx, &payload).map(|t| (payload.clone(), t));
            }
            match &install_url {
                Some(url) if self.install_qr.as_ref().is_none_or(|(u, _)| u != url) => {
                    self.install_qr = qr_texture(ctx, url).map(|t| (url.clone(), t));
                }
                None => self.install_qr = None,
                Some(_) => {}
            }
            // Asking the system takes a moment; not every frame.
            if self
                .lid_checked
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
                && self.lid_task.is_none()
            {
                self.lid_disabled = awake::lid_sleep_disabled();
                self.lid_checked = Some(Instant::now());
            }
        }
        let has_apk = self.apk_offered;
        egui::Window::new("Fernzugriff fürs Handy")
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(640.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height((screen.height() - 110.0).max(240.0))
                    .show(ui, |ui| {
                toggled = ui
                    .checkbox(&mut enabled, RichText::new("Fernzugriff an").strong())
                    .on_hover_text("Die Android-App kann dann Aufträge schicken, Antworten sehen und Freigaben geben.")
                    .changed();
                if let Some(e) = &self.remote_error {
                    ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {e}"));
                }
                let Some(server) = &self.remote else {
                    ui.label("Schalte den Fernzugriff ein, um dein Handy einzurichten: erst die App aufs Handy bringen, dann koppeln. Er bleibt aus, bis du ihn einschaltest.");
                    ui.add_space(6.0);
                    if ui.button(RichText::new("📱 Handy einrichten").size(16.0)).clicked() {
                        toggled = true;
                        enabled = true;
                    }
                    return;
                };
                ui.label(RichText::new(format!("Läuft auf Port {} · Gerät „{}“", server.port(), self.device_name)).weak());

                ui.add_space(8.0);
                ui.label(RichText::new("Schritt 1 · App aufs Handy bringen").strong().size(16.0));
                ui.horizontal_top(|ui| {
                    if has_apk && let Some((_, texture)) = &self.install_qr {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(170.0, 170.0)));
                    }
                    ui.vertical(|ui| {
                        if has_apk {
                            ui.label("Scanne diesen Code mit der Kamera deines Android-Handys (gleiches WLAN wie der Mac).");
                            ui.label("Die Seite öffnet sich: „Fadis Nemotron laden“ tippen, die Datei öffnen und installieren. Android fragt einmal, ob es Installationen aus dieser Quelle erlauben soll – erlauben.");
                            if let Some(url) = &install_url {
                                ui.label(RichText::new(format!("Oder im Handy-Browser öffnen: {url}")).monospace().small());
                            }
                            if server.app_downloads() > 0 {
                                ui.colored_label(widgets::accent(ui), "✔ Das Handy hat die App geladen. Weiter mit Schritt 2.");
                            } else {
                                ui.label(RichText::new("Noch nicht geladen. App schon installiert? Dann gleich weiter mit Schritt 2.").weak());
                            }
                        } else {
                            ui.colored_label(ui.visuals().warn_fg_color, "Die Android-App steckt nur in der fertigen Mac-App (DMG), nicht in diesem Programm.");
                            ui.hyperlink_to("FadisNemotron.apk von GitHub laden", APK_RELEASE_URL);
                            ui.label(RichText::new("Auf dem Handy öffnen und installieren, danach weiter mit Schritt 2.").weak());
                        }
                    });
                });

                ui.add_space(10.0);
                ui.label(RichText::new("Schritt 2 · Handy koppeln").strong().size(16.0));
                ui.horizontal_top(|ui| {
                    if let Some((_, texture)) = &self.remote_qr {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(170.0, 170.0)));
                    }
                    ui.vertical(|ui| {
                        ui.label("1. Die App „Fadis Nemotron“ auf dem Handy öffnen.");
                        ui.label("2. „QR-Code scannen“ tippen und diesen Code scannen.");
                        ui.label("3. Fertig – alle Chats sind sofort auch auf dem Handy, Aufträge und Freigaben laufen von dort.");
                        ui.add_space(4.0);
                        match server.last_seen() {
                            Some((when, from)) => {
                                ui.label(RichText::new(format!("Zuletzt verbunden: vor {} s von {from}", when.elapsed().as_secs())).weak());
                            }
                            None => {
                                ui.label(RichText::new("Noch kein Handy verbunden.").weak());
                            }
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new("Unterwegs").strong());
                        ui.label("Tailscale auf Mac und Handy installieren und bei beiden mit demselben Konto anmelden: dann klappt es überall, verschlüsselt, ohne Router-Einstellungen.");
                        ui.hyperlink_to("tailscale.com/download", "https://tailscale.com/download");
                    });
                });

                ui.add_space(10.0);
                ui.label(RichText::new("Im Hintergrund weiterlaufen").strong().size(16.0));
                awake_changed = ui
                    .checkbox(&mut self.settings.keep_awake, "Mac wach halten, solange der Fernzugriff an ist")
                    .on_hover_text("Der Mac schläft nicht von selbst ein. Bildschirm aus und App im Hintergrund ist in Ordnung; wenn Nemotron arbeitet, bleibt auch der Bildschirm an, damit er ihn sehen kann.")
                    .changed();
                if let Some(task) = &self.lid_task {
                    let _ = task;
                    ui.label(RichText::new("Warte auf das Passwort …").weak());
                } else {
                    match self.lid_disabled {
                        Some(true) => {
                            ui.colored_label(widgets::accent(ui), "✔ Der Mac läuft auch bei zugeklapptem Deckel weiter.");
                            if ui.button("Wieder normal schlafen lassen").clicked() {
                                lid_wanted = Some(false);
                            }
                        }
                        Some(false) | None => {
                            if ui
                                .button("🔒 Auch bei zugeklapptem Deckel weiterlaufen (Passwort)")
                                .on_hover_text("Führt mit deinem Passwort „pmset -a disablesleep 1“ aus. Rückgängig mit dem Knopf, der danach hier steht.")
                                .clicked()
                            {
                                lid_wanted = Some(true);
                            }
                        }
                    }
                }
                if let Some(message) = &self.lid_message {
                    ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {message}"));
                }
                ui.label(
                    RichText::new(
                        "Ein zugeklapptes MacBook schläft sonst immer ein – das kann keine App verhindern. \
                         Mit dem Schalter wird es warm, wenn es im Rucksack steckt: danach wieder normal schlafen lassen. \
                         Ohne Passwort geht es mit Netzteil, externem Bildschirm und Tastatur.",
                    )
                    .small()
                    .weak(),
                );

                ui.add_space(6.0);
                egui::CollapsingHeader::new("Technik: Adressen, Schlüssel, Port").show(ui, |ui| {
                    if addresses.is_empty() {
                        ui.label(RichText::new("Keine Netzwerkverbindung gefunden.").weak());
                    }
                    for addr in &addresses {
                        let kind = if remote::is_tailscale(*addr) {
                            "Tailscale (auch unterwegs)"
                        } else if addr.is_private() {
                            "Heimnetz / WLAN"
                        } else {
                            "anderes Netz"
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("http://{addr}:{}", server.port())).monospace());
                            ui.label(RichText::new(kind).small().weak());
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Schlüssel").strong());
                        let shown = if self.remote_show_token {
                            self.settings.remote_token.clone()
                        } else {
                            "•".repeat(12)
                        };
                        ui.label(RichText::new(shown).monospace());
                        ui.checkbox(&mut self.remote_show_token, "anzeigen");
                        if ui.small_button("📋 Kopieren").clicked() {
                            copy = Some(self.settings.remote_token.clone());
                        }
                    });
                    ui.horizontal(|ui| {
                        renew = ui
                            .button("Neuen Schlüssel erzeugen")
                            .on_hover_text("Alle gekoppelten Handys müssen danach neu scannen.")
                            .clicked();
                        ui.label("Port");
                        let port_changed = ui.add(egui::DragValue::new(&mut self.settings.remote_port).range(1024..=65535)).changed();
                        restart = port_changed;
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "macOS fragt beim ersten Mal, ob eingehende Verbindungen erlaubt sind – „Erlauben“ wählen. \
                         Nicht über eine Portfreigabe im Router ins Internet stellen; dafür ist Tailscale da.",
                    )
                    .small()
                    .weak(),
                );
                    });
            });
        if toggled {
            self.set_remote_enabled(enabled, ctx);
        }
        if awake_changed {
            self.save_settings();
        }
        if let Some(disabled) = lid_wanted
            && self.lid_task.is_none()
        {
            self.lid_message = None;
            self.lid_task = Some(awake::set_lid_sleep_disabled(disabled));
        }
        if renew {
            self.renew_remote_token(ctx);
        }
        if restart && self.remote.is_some() {
            self.save_settings();
            self.start_remote(ctx);
        }
        if let Some(token) = copy {
            ctx.copy_text(token);
        }
        if !open {
            self.remote_window = false;
            if !self.settings.phone_setup_seen {
                self.settings.phone_setup_seen = true;
                self.save_settings();
                // The key window was held back on the first start.
                if self.needs_api_key() {
                    self.open_settings();
                }
            }
        }
    }

    // ------------------------------------------------------------------ UI

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("kopfzeile").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Fadis Nemotron").strong().size(18.0).color(widgets::accent(ui)));
                ui.label(RichText::new("Agent").small().strong().color(Color32::BLACK).background_color(widgets::accent(ui)))
                    .on_hover_text(if self.workspace.is_home() {
                        "Nemotron liest und ändert Dateien in deinem Benutzerordner (Downloads, Schreibtisch, Dokumente …)."
                    } else {
                        "Nemotron liest und ändert Dateien im geöffneten Projektordner."
                    });
                ui.hyperlink_to(RichText::new(&self.settings.model).small().weak(), widgets::MODEL_URL)
                    .on_hover_text("Modellseite auf build.nvidia.com öffnen");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Einstellungen (⌘ ,)").clicked() {
                        self.open_settings();
                    }
                    let phone = if self.remote.is_some() {
                        RichText::new("📱 ●").color(widgets::accent(ui))
                    } else {
                        RichText::new("📱")
                    };
                    let hover = if self.remote.is_some() {
                        "Fernzugriff ist an: Handy kann verbinden"
                    } else {
                        "Fernzugriff fürs Handy"
                    };
                    if ui.button(phone).on_hover_text(hover).clicked() {
                        self.remote_window = true;
                    }
                    if ui
                        .add_enabled(!self.messages.is_empty(), egui::Button::new("💾"))
                        .on_hover_text("Chat als Markdown speichern")
                        .clicked()
                    {
                        self.export_markdown();
                    }
                    if ui.button("➕ Neuer Chat").on_hover_text("⌘ N").clicked() {
                        let _ = self.new_chat(ctx);
                    }
                    if self.workspace.is_home()
                        && ui.button("📁 Projekt öffnen").on_hover_text("Ordner wählen, in dem Nemotron arbeiten soll (⌘ O)").clicked()
                    {
                        self.pick_workspace(ctx);
                    }
                    let mut thinking = self.settings.thinking;
                    if ui
                        .checkbox(&mut thinking, "Denkmodus")
                        .on_hover_text("An: Nemotron denkt vor der Antwort nach (besser bei schweren Aufgaben).\nAus: schnellere, direkte Antworten.")
                        .changed()
                    {
                        self.settings.thinking = thinking;
                        if let Some(draft) = &mut self.settings_draft {
                            draft.settings.thinking = thinking;
                        }
                        self.save_settings();
                    }
                });
            });
            ui.add_space(6.0);
        });
    }

    /// Saved chats on the left; the phone shows the same list.
    fn chat_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let busy = self.active.is_some();
        let now = chats::now();
        let mut open: Option<String> = None;
        let mut new = false;
        egui::Panel::left("chats")
            .resizable(true)
            .default_size(210.0)
            .min_size(150.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                new = ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::new("➕ Neuer Chat"),
                    )
                    .on_hover_text("⌘ N")
                    .clicked();
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("chatliste")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let unsaved = !self.chats.iter().any(|c| c.id == self.chat_id);
                        let entries = unsaved
                            .then(|| (self.chat_id.clone(), self.chat_title(), self.chat_created))
                            .into_iter()
                            .chain(
                                self.chats
                                    .iter()
                                    .map(|c| (c.id.clone(), c.title.clone(), c.updated)),
                            );
                        for (id, title, updated) in entries {
                            let selected = id == self.chat_id;
                            ui.horizontal(|ui| {
                                let width = ui.available_width() - 26.0;
                                let label = ui
                                    .add_enabled_ui(!busy || selected, |ui| {
                                        ui.add_sized(
                                            [width.max(60.0), 20.0],
                                            egui::Button::selectable(
                                                selected,
                                                egui::RichText::new(&title),
                                            )
                                            .truncate(),
                                        )
                                    })
                                    .inner
                                    .on_hover_text(format!(
                                        "{title}\n{}",
                                        chats::relative_time(updated, now)
                                    ));
                                if label.clicked() && !selected {
                                    open = Some(id.clone());
                                }
                                if !(unsaved && selected)
                                    && ui
                                        .add_enabled(
                                            !(busy && selected),
                                            egui::Button::new("🗑").small().frame(false),
                                        )
                                        .on_hover_text("Chat löschen")
                                        .clicked()
                                {
                                    self.delete_chat_confirm = Some(id.clone());
                                }
                            });
                            ui.label(
                                RichText::new(chats::relative_time(updated, now))
                                    .small()
                                    .weak(),
                            );
                            ui.add_space(2.0);
                        }
                    });
            });
        if new {
            let _ = self.new_chat(ctx);
        }
        if let Some(id) = open
            && let Err((_, message)) = self.open_chat(&id, ctx)
        {
            self.notice = Some(Notice {
                text: message,
                error: true,
            });
        }
    }

    fn delete_chat_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some(id) = self.delete_chat_confirm.clone() else {
            return;
        };
        let title = self
            .chats
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.title.clone())
            .unwrap_or_else(|| self.chat_title());
        let mut yes = false;
        let mut no = false;
        egui::Window::new("Chat löschen?")
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "„{title}“ wird endgültig gelöscht, auch auf dem Handy."
                ));
                ui.horizontal(|ui| {
                    yes = ui.button("Löschen").clicked();
                    no = ui.button("Abbrechen").clicked();
                });
            });
        if yes {
            self.delete_chat_confirm = None;
            if let Err((_, message)) = self.delete_chat(&id, ctx) {
                self.notice = Some(Notice {
                    text: message,
                    error: true,
                });
            }
        } else if no {
            self.delete_chat_confirm = None;
        }
    }

    fn project_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let home = self.workspace.is_home();
        let name = if home {
            "Benutzerordner".to_owned()
        } else {
            self.workspace.name()
        };
        let root = self.workspace.root().to_path_buf();
        let busy = self.active.is_some();
        let mut action: Option<UiAction> = None;
        let mut switch = false;
        let mut close = false;
        egui::Panel::right("projekt").resizable(true).default_size(270.0).min_size(200.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{} {name}", if home { "🏠" } else { "📁" })).strong().size(16.0));
                ui.add(egui::Label::new(RichText::new(root.display().to_string()).small().weak()).truncate())
                    .on_hover_text(root.display().to_string());
                if home {
                    ui.label(
                        RichText::new("Nemotron arbeitet in deinem ganzen Benutzerordner: Downloads, Schreibtisch, Dokumente …")
                            .small()
                            .weak(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    switch = ui
                        .add_enabled(!busy, egui::Button::new(if home { "Projekt öffnen" } else { "Wechseln" }))
                        .clicked();
                    if ui.button("Im Finder").on_hover_text("Ordner im Dateimanager zeigen").clicked() {
                        Self::reveal_in_file_manager(&root);
                    }
                    if !home {
                        close = ui
                            .add_enabled(!busy, egui::Button::new("Schließen"))
                            .on_hover_text("Zurück zum Benutzerordner")
                            .clicked();
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Nemotron darf").strong());
                let mut mode = self.current_mode();
                for choice in ApprovalMode::ALL {
                    ui.radio_value(&mut mode, choice, choice.label());
                }
                if mode != self.current_mode() {
                    self.set_mode(mode);
                }
                ui.label(
                    RichText::new("Lesen und Suchen geht immer ohne Nachfrage. Gilt sofort, auch für den laufenden Auftrag. Schneller umstellen: Schalter über dem Eingabefeld.")
                        .small()
                        .weak(),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Änderungen ({})", self.changes.len())).strong());
                });
                if self.changes.is_empty() {
                    ui.label(RichText::new("Noch keine Dateien geändert.").weak());
                }
                for (i, change) in self.changes.iter().enumerate() {
                    let (added, removed) = change.stats;
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&change.path).monospace().size(12.0))
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text(if change.original.is_none() { "neu erstellt" } else { "geändert" });
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("+{added}")).color(widgets::ADDED).small());
                            ui.label(RichText::new(format!("−{removed}")).color(widgets::REMOVED).small());
                            if ui.small_button("Diff").clicked() {
                                action = Some(UiAction::ShowChange(i));
                            }
                            if ui
                                .add_enabled(!busy, egui::Button::new("↶ Rückgängig").small())
                                .on_hover_text("Datei wieder auf den Stand vor Nemotrons Änderung setzen")
                                .clicked()
                            {
                                action = Some(UiAction::Undo(i));
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        });
        if switch {
            self.pick_workspace(ctx);
        }
        if close {
            self.close_workspace(ctx);
        }
        if let Some(action) = action {
            self.apply_ui_action(action, ctx);
        }
    }

    /// ↑ and ↓ in the input field call back what was sent before, like in a
    /// shell: ↑ when the field is empty, the cursor is at its very start, or
    /// an earlier message is shown; ↓ while one is shown, or at the very end.
    /// Otherwise the keys move the cursor as usual.
    fn history_keys(&mut self, ctx: &egui::Context, id: egui::Id) {
        if self.history.is_empty() || !ctx.memory(|m| m.has_focus(id)) {
            return;
        }
        let total = self.input.chars().count();
        let cursor = egui::TextEdit::load_state(ctx, id)
            .and_then(|s| s.cursor.char_range())
            .map_or(total, |range| range.primary.index.into());
        let browsing = self.history.is_browsing(&self.input);
        let up_allowed = self.input.is_empty() || cursor == 0 || browsing;
        let down_allowed = self.history.has_newer() && (browsing || cursor == total);
        let up = up_allowed && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp));
        let down = !up
            && down_allowed
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown));
        let text = if up {
            self.history.older(&self.input)
        } else if down {
            self.history.newer()
        } else {
            None
        };
        if let Some(text) = text {
            self.input = text;
            // The cursor goes to the end, as in a shell.
            if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
                let end = egui::text::CCursor::new(self.input.chars().count());
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ctx, id);
            }
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::bottom("eingabe").show(ui, |ui| {
            ui.add_space(8.0);
            if let Some(i) = widgets::attachment_row(ui, &self.pending, true) {
                let att = self.pending.remove(i);
                ctx.forget_image(&att.preview_uri());
            }
            if self.loading_attachments > 0 {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Anhänge werden vorbereitet …");
                });
            }

            let mut send = false;
            let mut stop = false;
            let mut pick = false;
            let mut add_url = false;
            let mut after_error = None;
            // An error ended the last answer: one click goes on from there.
            if let Some(error) = self.interrupted_by() {
                let first_line = error.lines().next().unwrap_or(error);
                let short: String = first_line.chars().take(110).collect();
                let marked: Vec<&str> = self.error_marks.iter().map(|m| m.path.as_str()).collect();
                let first_marked = self
                    .error_marks
                    .first()
                    .and_then(|m| self.changes.iter().position(|c| c.abs == m.abs));
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, format!("⚠ Unterbrochen: {short}"));
                });
                if !marked.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "Der Fehler steht als Markierung im Code: {} (Zeile „{}“).",
                            marked.join(", "),
                            markers::MARK
                        ))
                        .small(),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    let go_on = if marked.is_empty() {
                        "▶ Weitermachen"
                    } else {
                        "▶ Weitermachen und Markierung entfernen"
                    };
                    if ui
                        .add(egui::Button::new(RichText::new(go_on).strong().color(Color32::BLACK)).fill(widgets::ACCENT))
                        .on_hover_text("Nemotron macht genau dort weiter, wo der Fehler ihn gestoppt hat. Die Markierung verschwindet dabei aus dem Code.")
                        .clicked()
                    {
                        after_error = Some(UiAction::ContinueAfterError);
                    }
                    if let Some(index) = first_marked
                        && ui
                            .button("Markierung im Diff ansehen")
                            .on_hover_text("Zeigt die Datei Zeile für Zeile; die Markierung ist eine grüne Zeile.")
                            .clicked()
                    {
                        after_error = Some(UiAction::ShowChange(index));
                    }
                    if !marked.is_empty()
                        && ui
                            .button("Nur Markierung entfernen")
                            .clicked()
                    {
                        after_error = Some(UiAction::RemoveErrorMarks);
                    }
                    if ui
                        .button("↻ Erneut versuchen")
                        .on_hover_text("Fragt noch einmal. Bereits geänderte Dateien bleiben so, die Markierung wird entfernt.")
                        .clicked()
                    {
                        after_error = Some(UiAction::Retry);
                    }
                });
                ui.add_space(4.0);
            }
            // The mode is chosen here, where one types, not in the settings.
            if let Some(mode) = widgets::mode_switcher(ui, self.current_mode()) {
                self.set_mode(mode);
            }
            ui.add_space(4.0);
            if let Some(silent) = self.stalled_for() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "⏳ Seit {} s keine Antwort von NVIDIA (Server hängt oder ist überlastet). Du kannst ⏹ stoppen – oder einfach weiterschreiben und Enter drücken: dann geht es sofort mit deiner neuen Nachricht weiter.",
                            silent.as_secs()
                        ),
                    );
                });
                ui.add_space(4.0);
            }
            let hint = if self.workspace.is_home() {
                "Sag Nemotron, was passieren soll – z. B. „Räum meinen Downloads-Ordner auf“ … (Enter senden, Shift+Enter neue Zeile)"
            } else {
                "Sag Nemotron, was im Projekt passieren soll … (Enter senden, Shift+Enter neue Zeile)"
            };
            let input_id = egui::Id::unique("eingabe_text");
            self.history_keys(ctx, input_id);
            // `horizontal` keeps the row as tall as its content; a free
            // layout here would stretch the panel over the whole window.
            ui.horizontal(|ui| {
                let spacing = ui.spacing().item_spacing.x;
                let working = self.active.is_some();
                // 📎 and 🔗, Send, and Stop beside Send while Nemotron works.
                let buttons = if working {
                    34.0 + 34.0 + 96.0 + 96.0 + 4.0 * spacing
                } else {
                    34.0 + 34.0 + 96.0 + 3.0 * spacing
                };
                let input_width = (ui.available_width() - buttons).max(120.0);
                // A long prompt scrolls inside the box instead of pushing the
                // chat out of the window.
                let response = ui
                    .allocate_ui_with_layout(egui::vec2(input_width, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(input_width);
                        egui::ScrollArea::vertical()
                            .id_salt("eingabe_scroll")
                            .max_height(180.0)
                            .min_scrolled_height(180.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.input)
                                        .id(input_id)
                                        .hint_text(hint)
                                        .desired_rows(2)
                                        .desired_width(f32::INFINITY)
                                        .min_size(egui::vec2(0.0, 56.0))
                                        .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter)),
                                )
                            })
                            .inner
                    })
                    .inner;
                if self.focus_input {
                    response.request_focus();
                    self.focus_input = false;
                }
                if response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift) {
                    send = true;
                }

                pick = ui
                    .add_sized([34.0, 34.0], egui::Button::new("📎"))
                    .on_hover_text("Bilder oder Videos anhängen (oder ins Fenster ziehen)")
                    .clicked();
                add_url = ui
                    .add_sized([34.0, 34.0], egui::Button::new("🔗"))
                    .on_hover_text("Bild- oder Video-Link anhängen")
                    .clicked();
                if working {
                    stop = ui.add_sized([96.0, 34.0], egui::Button::new("⏹ Stop")).on_hover_text("Esc").clicked();
                }
                let label = RichText::new(if working { "Jetzt ➤" } else { "Senden ➤" }).strong().color(Color32::BLACK);
                send |= ui
                    .add_enabled_ui(self.can_send(), |ui| {
                        ui.add_sized([96.0, 34.0], egui::Button::new(label).fill(widgets::ACCENT))
                    })
                    .inner
                    .on_hover_text(if working {
                        "Enter: stoppt den laufenden Auftrag und schickt dieses hier sofort"
                    } else {
                        "Enter"
                    })
                    .clicked();
            });
            ui.add_space(8.0);

            if let Some(action) = after_error {
                self.apply_ui_action(action, ctx);
            }
            if send {
                self.send(ctx);
                self.focus_input = true;
            }
            if stop {
                self.stop();
            }
            if pick {
                self.pick_files(ctx);
            }
            if add_url {
                self.url_draft = Some(UrlDraft { url: String::new(), kind: AttachmentKind::Image, kind_chosen: false, error: None });
            }
        });
    }

    fn chat_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(notice) = &self.notice {
                let mut close = false;
                ui.horizontal(|ui| {
                    let color = if notice.error {
                        ui.visuals().error_fg_color
                    } else {
                        widgets::accent(ui)
                    };
                    ui.label(RichText::new(&notice.text).color(color));
                    close = ui.small_button("✖").on_hover_text("Ausblenden").clicked();
                });
                if close {
                    self.notice = None;
                }
                ui.separator();
            }

            if self.messages.is_empty() {
                self.welcome(ui, ctx);
                return;
            }

            let mut actions = Vec::new();
            let count = self.messages.len();
            let busy = self.active.is_some();
            // The only scroll area in the chat: nested ones would catch the
            // mouse wheel and make the chat feel stuck.
            let jump_to_end = std::mem::take(&mut self.scroll_to_bottom);
            egui::ScrollArea::vertical()
                .id_salt("chat")
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    widgets::centered_column(ui, |ui| {
                        ui.add_space(8.0);
                        for (i, msg) in self.messages.iter_mut().enumerate() {
                            let is_last = i + 1 == count;
                            ui.push_id(i, |ui| {
                                widgets::message_ui(
                                    ui,
                                    msg,
                                    is_last,
                                    busy,
                                    &mut self.markdown,
                                    &mut actions,
                                );
                            });
                            ui.add_space(10.0);
                        }
                    });
                    if jump_to_end {
                        // After sending, show the new message even if the user had scrolled up.
                        ui.scroll_to_cursor(Some(Align::BOTTOM));
                    }
                });
            for action in actions {
                self.apply_ui_action(action, ctx);
            }
        });
    }

    fn apply_ui_action(&mut self, action: UiAction, ctx: &egui::Context) {
        match action {
            UiAction::Copy(text) => {
                ctx.copy_text(text);
                self.notice = Some(Notice {
                    text: "In die Zwischenablage kopiert.".into(),
                    error: false,
                });
            }
            UiAction::Retry => self.retry(ctx),
            UiAction::Decide { call_id, decision } => self.decide(&call_id, decision),
            UiAction::ShowDiff { title, diff } => {
                self.diff_window = Some(DiffWindow {
                    title,
                    abs: None,
                    diff,
                    current: None,
                    state: DiffState::Showing,
                    error: None,
                });
            }
            UiAction::ShowChange(index) => {
                if let Some(tracked) = self.changes.get(index) {
                    self.diff_window = Some(DiffWindow::for_change(tracked, DiffState::Showing));
                }
            }
            UiAction::Undo(index) => self.undo_change(index, false, true),
            UiAction::OpenProject => self.pick_workspace(ctx),
            UiAction::OpenSettings => self.open_settings(),
            UiAction::OpenPhoneSetup => self.remote_window = true,
            UiAction::ContinueAfterError => self.continue_after_error(ctx),
            UiAction::RemoveErrorMarks => {
                let removed = self.remove_error_marks();
                self.notice = Some(Notice {
                    text: format!("{removed} Fehler-Markierung(en) aus dem Code entfernt."),
                    error: false,
                });
            }
            UiAction::OpenChat(id) => {
                if let Err((_, e)) = self.open_chat(&id, ctx) {
                    self.notice = Some(Notice {
                        text: e,
                        error: true,
                    });
                }
            }
            UiAction::UsePrompt(text) => {
                self.input = text;
                self.focus_input = true;
            }
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut actions = Vec::new();
        widgets::welcome(
            ui,
            (!self.workspace.is_home())
                .then(|| self.workspace.name())
                .as_deref(),
            self.needs_api_key(),
            &mut actions,
        );
        for action in actions {
            self.apply_ui_action(action, ctx);
        }
    }

    /// What a click in the change window does; returns the window as it looks next.
    fn apply_diff_action(
        &mut self,
        mut window: DiffWindow,
        tracked: Option<usize>,
        action: DiffAction,
    ) -> DiffWindow {
        match action {
            DiffAction::Undo | DiffAction::ForceUndo => {
                let Some(i) = tracked else {
                    return window;
                };
                match self.try_undo(i, action == DiffAction::ForceUndo) {
                    UndoResult::Done(done) => {
                        self.notice = Some(Notice {
                            text: if done.original.is_none() {
                                format!("{} gelöscht (war neu).", done.path)
                            } else {
                                format!("{} wiederhergestellt.", done.path)
                            },
                            error: false,
                        });
                        window = DiffWindow::undone(done);
                    }
                    UndoResult::NeedsConfirm => window.state = DiffState::Confirming,
                    UndoResult::Failed(text) => window.error = Some(text),
                }
            }
            DiffAction::Cancel => window.state = DiffState::Showing,
            DiffAction::Redo => {
                if let DiffState::Undone { change, .. } = &mut window.state
                    && let Some(done) = change.take()
                {
                    match self.redo_change(done) {
                        Ok(()) => {
                            let again = self.changes.last().expect("just added");
                            window = DiffWindow::for_change(again, DiffState::Showing);
                        }
                        Err(e) => window.error = Some(e),
                    }
                }
            }
        }
        window
    }

    /// The change window: the diff line by line, and undo (with its result)
    /// right in it. It stays in front of everything else.
    fn diff_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some(mut window) = self.diff_window.take() else {
            return;
        };
        let busy = self.active.is_some();
        let tracked = window
            .abs
            .as_ref()
            .and_then(|abs| self.changes.iter().position(|c| &c.abs == abs));
        // Nemotron may still be editing this file: the diff follows.
        if let (Some(i), DiffState::Showing) = (tracked, &window.state)
            && window.current.as_deref() != Some(self.changes[i].current.as_str())
        {
            window.diff = self.changes[i].diff();
            window.current = Some(self.changes[i].current.clone());
        }
        let (added, removed) = diff_stats(&window.diff);
        let mut open = true;
        let mut action = None;
        egui::Window::new(format!("Änderungen: {}", window.title))
            .open(&mut open)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .default_size([760.0, 560.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&window.title).monospace().strong());
                    ui.label(RichText::new(format!("+{added}")).color(widgets::ADDED));
                    ui.label(RichText::new(format!("−{removed}")).color(widgets::REMOVED));
                });
                match &window.state {
                    DiffState::Showing if tracked.is_some() => {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("↶ Rückgängig"))
                                .on_hover_text("Die Datei wieder auf den Stand vor Nemotrons Änderung setzen – das Ergebnis siehst du gleich hier.")
                                .clicked()
                            {
                                action = Some(DiffAction::Undo);
                            }
                            if busy {
                                ui.label(RichText::new("Erst wenn Nemotron fertig ist.").small().weak());
                            }
                        });
                    }
                    DiffState::Showing => {
                        if window.abs.is_some() {
                            ui.label(RichText::new("Diese Änderung steht nicht mehr in der Liste.").weak());
                        }
                    }
                    DiffState::Confirming => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "⚠ Die Datei wurde nach Nemotrons Änderung noch einmal verändert. Rückgängig setzt sie auf den Stand vor Nemotron zurück; spätere Änderungen gehen dabei verloren.",
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Trotzdem zurücksetzen"))
                                .clicked()
                            {
                                action = Some(DiffAction::ForceUndo);
                            }
                            if ui.button("Abbrechen").clicked() {
                                action = Some(DiffAction::Cancel);
                            }
                        });
                    }
                    DiffState::Undone { change, deleted } => {
                        ui.colored_label(
                            widgets::ADDED,
                            if *deleted {
                                "✓ Rückgängig gemacht: die Datei war neu und ist wieder gelöscht."
                            } else {
                                "✓ Rückgängig gemacht: die Datei ist wieder auf dem Stand vor Nemotron."
                            },
                        );
                        if change.is_some()
                            && ui
                                .add_enabled(!busy, egui::Button::new("↷ Wieder anwenden"))
                                .on_hover_text("Nemotrons Änderung noch einmal einsetzen")
                                .clicked()
                        {
                            action = Some(DiffAction::Redo);
                        }
                    }
                }
                if let Some(error) = &window.error {
                    ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {error}"));
                }
                ui.label(
                    RichText::new(if matches!(window.state, DiffState::Undone { .. }) {
                        "Rot: was jetzt wieder weg ist · Grün: was jetzt wieder da ist"
                    } else {
                        "Rot: was entfernt wurde · Grün: was stattdessen eingefügt wird"
                    })
                    .small()
                    .weak(),
                );
                ui.separator();
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    widgets::diff_label(ui, &window.diff, usize::MAX);
                });
            });

        if let Some(action) = action {
            window = self.apply_diff_action(window, tracked, action);
        }
        if open {
            self.diff_window = Some(window);
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some(draft) = &mut self.settings_draft else {
            return;
        };

        if let Some(rx) = &draft.check
            && let Ok(result) = rx.try_recv()
        {
            draft.check_result = Some(result);
            draft.check = None;
        }

        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        let mut check = false;
        egui::Window::new("Einstellungen")
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .show(ctx, |ui| {
                let s = &mut draft.settings;
                egui::Grid::new("einstellungen").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                    ui.label("API-Schlüssel");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut s.api_key).password(!draft.show_key).hint_text("nvapi-…").desired_width(340.0));
                            ui.checkbox(&mut draft.show_key, "anzeigen");
                        });
                        ui.hyperlink_to("Kostenlosen Schlüssel auf build.nvidia.com holen", widgets::API_KEY_URL);
                        if s.api_key.trim().is_empty() && std::env::var("NVIDIA_API_KEY").is_ok_and(|k| !k.is_empty()) {
                            ui.label(RichText::new("Leer: NVIDIA_API_KEY aus der Umgebung wird benutzt.").small().weak());
                        }
                    });
                    ui.end_row();

                    ui.label("Server (Basis-URL)");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.base_url).desired_width(340.0));
                        if ui.small_button("Standard").clicked() {
                            s.base_url = api::DEFAULT_BASE_URL.to_owned();
                        }
                    })
                    .response
                    .on_hover_text("NVIDIA-Cloud oder eigener OpenAI-kompatibler Server (vLLM, NIM, SGLang), z. B. http://localhost:8000/v1");
                    ui.end_row();

                    ui.label("Modell");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.model).desired_width(340.0));
                        if ui.small_button("Standard").clicked() {
                            s.model = api::DEFAULT_MODEL.to_owned();
                        }
                    });
                    ui.end_row();

                    ui.label("Denkmodus");
                    ui.checkbox(&mut s.thinking, "an (Nemotron denkt vor der Antwort nach)");
                    ui.end_row();

                    ui.label("Bei Fehlern");
                    ui.checkbox(&mut s.mark_errors, "Markierung in den Code schreiben (hinter die zuletzt geänderte Zeile; „Weitermachen“ entfernt sie)");
                    ui.end_row();

                    ui.label("Agent darf");
                    egui::ComboBox::from_id_salt("freigabe")
                        .selected_text(s.approval.label())
                        .width(340.0)
                        .show_ui(ui, |ui| {
                            for mode in ApprovalMode::ALL {
                                ui.selectable_value(&mut s.approval, mode, mode.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Max. Schritte");
                    ui.add(egui::DragValue::new(&mut s.max_steps).range(1..=300))
                        .on_hover_text("So viele Anfragen darf Nemotron pro Nachricht stellen (lesen, ändern, prüfen …), bevor er pausiert.");
                    ui.end_row();

                    ui.label("Temperatur");
                    ui.add(egui::Slider::new(&mut s.temperature, 0.0..=2.0).step_by(0.05))
                        .on_hover_text("NVIDIA empfiehlt 1.0 für Text, 0.6 für Bilder und Agenten.");
                    ui.end_row();

                    ui.label("Top-p");
                    ui.add(egui::Slider::new(&mut s.top_p, 0.0..=1.0).step_by(0.01)).on_hover_text("NVIDIA empfiehlt 0.95");
                    ui.end_row();

                    ui.label("Max. Tokens");
                    ui.add(egui::DragValue::new(&mut s.max_tokens).range(256..=131_072).speed(128))
                        .on_hover_text("Länge von Denken + Antwort je Schritt. Bei leeren oder abgeschnittenen Antworten erhöhen (NVIDIA-Cloud: max. 131072).");
                    ui.end_row();

                    ui.label("Max. Bildgröße");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.max_image_dim).range(0..=8192).speed(16).suffix(" px"));
                        ui.label(RichText::new("längste Seite, 0 = nicht verkleinern").small().weak());
                    });
                    ui.end_row();

                    ui.label("Eigene Anweisungen");
                    ui.add(egui::TextEdit::multiline(&mut s.system_prompt).desired_rows(3).desired_width(400.0))
                        .on_hover_text("Im Chat der System-Prompt; im Agent-Modus zusätzliche Anweisungen.");
                    ui.end_row();
                });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    save = ui.button(RichText::new("Speichern").strong()).clicked();
                    cancel = ui.button("Abbrechen").clicked();
                    check = ui.add_enabled(draft.check.is_none(), egui::Button::new("Verbindung testen")).clicked();
                    if ui.button("Standardwerte").on_hover_text("Alles außer API-Schlüssel und Projekt zurücksetzen").clicked() {
                        let key = std::mem::take(&mut draft.settings.api_key);
                        let workspace = draft.settings.workspace.take();
                        draft.settings = Settings { api_key: key, workspace, ..Settings::default() };
                    }
                });
                if draft.check.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Prüfe Verbindung …");
                    });
                }
                match &draft.check_result {
                    Some(Ok(msg)) => {
                        ui.label(RichText::new(format!("✔ {msg}")).color(widgets::accent(ui)));
                    }
                    Some(Err(msg)) => {
                        ui.label(RichText::new(format!("⚠ {msg}")).color(ui.visuals().error_fg_color));
                    }
                    None => {}
                }
            });

        if check {
            let (tx, rx) = mpsc::channel();
            let http = self.http.clone();
            let s = draft.settings.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = api::check_connection(
                    &http,
                    &s.base_url,
                    &s.effective_api_key(),
                    s.model.trim(),
                );
                tx.send(result).ok();
                ctx.request_repaint();
            });
            draft.check = Some(rx);
            draft.check_result = None;
        }
        if save {
            let mut settings = draft.settings.clone();
            settings.base_url = settings.base_url.trim().to_owned();
            settings.model = settings.model.trim().to_owned();
            settings.api_key = settings.api_key.trim().to_owned();
            // The project folder and remote access are managed elsewhere.
            settings.workspace = self.settings.workspace.clone();
            settings.remote_enabled = self.settings.remote_enabled;
            settings.remote_port = self.settings.remote_port;
            settings.remote_token = self.settings.remote_token.clone();
            self.settings = settings;
            self.settings_draft = None;
            self.save_settings();
            self.focus_input = true;
        } else if cancel || !open {
            self.settings_draft = None;
        }
    }

    fn url_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some(draft) = &mut self.url_draft else {
            return;
        };
        let mut open = true;
        let mut add = false;
        let mut cancel = false;
        egui::Window::new("Link anhängen")
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Öffentlicher Link zu einem Bild (JPEG/PNG) oder Video (MP4):");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut draft.url)
                        .hint_text("https://…")
                        .desired_width(420.0),
                );
                if response.changed() && !draft.kind_chosen {
                    draft.kind = attachments::guess_kind_from_url(&draft.url);
                }
                ui.horizontal(|ui| {
                    let image = ui.radio_value(&mut draft.kind, AttachmentKind::Image, "Bild");
                    let video = ui.radio_value(&mut draft.kind, AttachmentKind::Video, "Video");
                    if image.clicked() || video.clicked() {
                        draft.kind_chosen = true;
                    }
                });
                if let Some(e) = &draft.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.horizontal(|ui| {
                    add = ui.button("Anhängen").clicked()
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                    cancel = ui.button("Abbrechen").clicked();
                });
            });

        if add {
            if attachments::is_supported_url(&draft.url) {
                self.pending
                    .push(Attachment::from_url(&draft.url, draft.kind));
                self.url_draft = None;
                self.focus_input = true;
            } else {
                draft.error =
                    Some("Bitte einen Link beginnend mit https:// oder http:// eingeben.".into());
            }
        } else if cancel || !open {
            self.url_draft = None;
        }
    }
}

/// Applies one agent event to the streaming message. Returns true when the
/// request is over. File changes are collected for the change list.
fn apply_event(msg: &mut Message, event: AgentEvent, changes: &mut Vec<FileChange>) -> bool {
    match event {
        AgentEvent::StepStarted => msg.steps.push(msg.new_step()),
        AgentEvent::Text(delta) => {
            if msg.steps.is_empty() {
                msg.steps.push(msg.new_step());
            }
            let step = msg.steps.last_mut().expect("step exists");
            match delta {
                TextDelta::Reasoning(r) => step.reasoning.push_str(&r),
                TextDelta::Content(c) => {
                    let was_answering = !step.parts().1.is_empty();
                    step.content.push_str(&c);
                    if !was_answering && !step.parts().1.is_empty() {
                        step.collapse_once();
                    }
                }
            }
        }
        AgentEvent::StepFinished {
            content,
            reasoning,
            tool_calls,
        } => {
            if msg.steps.is_empty() {
                msg.steps.push(msg.new_step());
            }
            let step = msg.steps.last_mut().expect("step exists");
            step.content = content;
            step.reasoning = reasoning;
            step.tools = tool_calls.into_iter().map(ToolView::planned).collect();
            if !step.tools.is_empty() || !step.parts().1.is_empty() {
                step.collapse_once();
            }
        }
        AgentEvent::ApprovalNeeded {
            call_id,
            preview,
            warning,
        } => {
            if let Some(tool) = msg.tool_mut(&call_id) {
                tool.state = ToolState::AwaitingApproval;
                tool.set_file_change(preview.as_ref());
                tool.preview = preview;
                tool.warning = warning;
            }
        }
        AgentEvent::ToolStarted { call_id } => {
            if let Some(tool) = msg.tool_mut(&call_id) {
                tool.state = ToolState::Running;
            }
        }
        AgentEvent::ToolFinished {
            call_id,
            output,
            ok,
            change,
        } => {
            if let Some(tool) = msg.tool_mut(&call_id) {
                if tool.state != ToolState::Declined {
                    tool.state = ToolState::Done;
                }
                tool.output = output;
                tool.ok = ok;
                tool.preview = None;
                tool.set_file_change(change.as_ref());
                if let Some(change) = &change {
                    changes.push(change.clone());
                }
                tool.change = change;
            }
        }
        AgentEvent::Notice(text) => msg.notices.push(text),
        AgentEvent::Handoff { .. } => {
            msg.status = Status::Done;
            msg.settle_tools();
            return true;
        }
        AgentEvent::Finished { finish_reason } => {
            msg.status = Status::Done;
            msg.finish_reason = finish_reason;
            msg.settle_tools();
            return true;
        }
        AgentEvent::Failed(e) => {
            msg.status = Status::Failed(e);
            msg.settle_tools();
            return true;
        }
        AgentEvent::Cancelled => {
            msg.status = Status::Cancelled;
            msg.settle_tools();
            return true;
        }
    }
    false
}

impl eframe::App for NemotronApp {
    /// Commands still running would otherwise live on without the app.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(active) = &self.active {
            active.cancel.store(true, Ordering::Relaxed);
        }
        crate::tools::kill_running_commands();
        self.save_chat();
    }

    /// Also runs while the window is hidden, so the phone keeps working
    /// when the Mac app is minimised.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_agent();
        self.continue_in_new_chat(ctx);
        self.poll_stopped();
        self.poll_attachments();
        self.handle_remote(ctx);
        self.update_awake(ctx);
        if self.active.is_some() || self.loading_attachments > 0 {
            // Keeps spinners moving even when no tokens arrive (long thinking).
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let full_rect = ui.max_rect();

        self.handle_global_input(&ctx);

        self.top_bar(ui, &ctx);
        self.composer(ui, &ctx);
        self.chat_list(ui, &ctx);
        self.project_panel(ui, &ctx);
        self.chat_area(ui, &ctx);
        self.settings_window(&ctx, full_rect);
        self.url_window(&ctx, full_rect);
        self.diff_window(&ctx, full_rect);
        self.delete_chat_window(&ctx, full_rect);
        self.remote_window(&ctx, full_rect);
        widgets::drop_overlay(&ctx, full_rect);
    }
}

fn attachment_bytes(attachment: &Attachment) -> usize {
    match &attachment.source {
        attachments::Source::Data { bytes, .. } => bytes.len(),
        attachments::Source::Url(_) => 0,
    }
}

fn busy_error() -> (u16, String) {
    (
        409,
        "Nemotron arbeitet noch. Erst stoppen oder warten.".to_owned(),
    )
}

/// A tool output from an earlier answer: its start, enough to remember
/// what happened.
fn shorten_old_output(output: &str) -> String {
    if output.chars().count() <= OLD_TOOL_OUTPUT_CHARS {
        return output.to_owned();
    }
    format!(
        "{}\n[… output shortened in the history; run the tool again if you need it …]",
        head_chars(output, OLD_TOOL_OUTPUT_CHARS)
    )
}

fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        text.to_owned()
    } else {
        format!("…{}", text.chars().skip(count - max).collect::<String>())
    }
}

fn head_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// The pairing QR code as a crisp texture (4-module quiet zone, 6 px per module).
fn qr_texture(ctx: &egui::Context, payload: &str) -> Option<egui::TextureHandle> {
    const QUIET: usize = 4;
    const SCALE: usize = 6;
    let code =
        qrcode::QrCode::with_error_correction_level(payload.as_bytes(), qrcode::EcLevel::M).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let size = (width + 2 * QUIET) * SCALE;
    let mut pixels = vec![Color32::WHITE; size * size];
    for (i, color) in colors.iter().enumerate() {
        if *color != qrcode::Color::Dark {
            continue;
        }
        let (mx, my) = (i % width + QUIET, i / width + QUIET);
        for y in my * SCALE..(my + 1) * SCALE {
            pixels[y * size + mx * SCALE..y * size + (mx + 1) * SCALE].fill(Color32::BLACK);
        }
    }
    let image = egui::ColorImage::new([size, size], pixels);
    Some(ctx.load_texture("kopplung", image, egui::TextureOptions::NEAREST))
}

/// The conversation as Markdown, for export.
fn conversation_markdown(messages: &[Message], model: &str) -> String {
    let mut out = format!("# Chat mit {model}\n");
    for msg in messages {
        match msg.role {
            Role::User => {
                out.push_str("\n## Du\n\n");
                if !msg.text.is_empty() {
                    out.push_str(msg.text.trim());
                    out.push('\n');
                }
                if !msg.attachments.is_empty() {
                    let names: Vec<&str> =
                        msg.attachments.iter().map(|a| a.name.as_str()).collect();
                    out.push_str(&format!("\n*Anhänge: {}*\n", names.join(", ")));
                }
            }
            Role::Assistant => {
                out.push_str("\n## Nemotron\n\n");
                for step in &msg.steps {
                    let (thinking, answer) = step.parts();
                    if let Some(thinking) = thinking {
                        out.push_str("<details><summary>Denkprozess</summary>\n\n");
                        out.push_str(thinking);
                        out.push_str("\n\n</details>\n\n");
                    }
                    if !answer.trim().is_empty() {
                        out.push_str(answer.trim());
                        out.push_str("\n\n");
                    }
                    for tool in &step.tools {
                        out.push_str(&format!(
                            "- `{}`: {}\n",
                            tool.call.name,
                            widgets::tool_title(tool)
                        ));
                    }
                    if !step.tools.is_empty() {
                        out.push('\n');
                    }
                }
                match &msg.status {
                    Status::Failed(e) => out.push_str(&format!("*Fehler: {e}*\n")),
                    Status::Cancelled => out.push_str("*Abgebrochen.*\n"),
                    _ => {}
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    fn run_events(events: Vec<AgentEvent>) -> (Message, Vec<FileChange>) {
        let mut msg = Message::assistant(true);
        let mut changes = Vec::new();
        for e in events {
            apply_event(&mut msg, e, &mut changes);
        }
        (msg, changes)
    }

    #[test]
    fn events_build_steps_tools_and_changes() {
        let change = FileChange {
            path: "a.rs".into(),
            abs: "/tmp/a.rs".into(),
            before: Some("1\n".into()),
            after: "2\n".into(),
        };
        let (msg, changes) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::Text(TextDelta::Reasoning("Ich lese.".into())),
            AgentEvent::StepFinished {
                content: String::new(),
                reasoning: "Ich lese.".into(),
                tool_calls: vec![
                    tool("c1", "edit_file", "{}"),
                    tool("c2", "run_command", "{}"),
                ],
            },
            AgentEvent::ToolStarted {
                call_id: "c1".into(),
            },
            AgentEvent::ToolFinished {
                call_id: "c1".into(),
                output: "Updated a.rs".into(),
                ok: true,
                change: Some(change.clone()),
            },
            AgentEvent::ApprovalNeeded {
                call_id: "c2".into(),
                preview: None,
                warning: Some("git push".into()),
            },
        ]);
        assert_eq!(msg.steps.len(), 1);
        let step = &msg.steps[0];
        assert!(step.collapse_reasoning);
        assert_eq!(step.tools[0].state, ToolState::Done);
        assert_eq!(step.tools[1].state, ToolState::AwaitingApproval);
        assert_eq!(changes, vec![change]);

        let mut msg = msg;
        assert!(apply_event(
            &mut msg,
            AgentEvent::Cancelled,
            &mut Vec::new()
        ));
        assert_eq!(msg.steps[0].tools[1].state, ToolState::NotRun);
        assert_eq!(msg.status, Status::Cancelled);
    }

    #[test]
    fn inline_thinking_collapses_when_the_answer_starts() {
        let (msg, _) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::Text(TextDelta::Content("<think>hmm".into())),
            AgentEvent::Text(TextDelta::Content("</think>".into())),
        ]);
        assert!(!msg.steps[0].collapse_reasoning);
        let (msg, _) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::Text(TextDelta::Content("<think>hmm</think>".into())),
            AgentEvent::Text(TextDelta::Content("Antwort".into())),
        ]);
        assert!(msg.steps[0].collapse_reasoning);
        assert_eq!(msg.steps[0].parts(), (Some("hmm"), "Antwort"));
    }

    #[test]
    fn history_keeps_tool_calls_paired_with_results() {
        let mut app_messages = vec![Message::user("Ändere a.rs".into(), Vec::new())];
        let (mut answer, _) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: String::new(),
                reasoning: "denken".into(),
                tool_calls: vec![
                    tool("c1", "read_file", r#"{"path":"a.rs"}"#),
                    tool("c2", "edit_file", "{}"),
                ],
            },
            AgentEvent::ToolFinished {
                call_id: "c1".into(),
                output: "1\tx".into(),
                ok: true,
                change: None,
            },
        ]);
        apply_event(&mut answer, AgentEvent::Cancelled, &mut Vec::new());
        app_messages.push(answer);

        let (done, _) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: "Fertig.".into(),
                reasoning: String::new(),
                tool_calls: vec![],
            },
            AgentEvent::Finished {
                finish_reason: Some("stop".into()),
            },
        ]);
        app_messages.push(Message::user("Weiter".into(), Vec::new()));
        app_messages.push(done);

        let app = HistoryOnly(app_messages);
        let history = app.history();
        let kinds: Vec<String> = history
            .iter()
            .map(|m| match m {
                ChatMessage::User { text, .. } => format!("user:{text}"),
                ChatMessage::Assistant {
                    content,
                    tool_calls,
                    reasoning,
                } => {
                    assert!(reasoning.is_none());
                    format!("assistant:{content}:{}", tool_calls.len())
                }
                ChatMessage::Tool { call_id, content } => format!("tool:{call_id}:{content}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "user:Ändere a.rs".to_owned(),
                "assistant::2".into(),
                "tool:c1:1\tx".into(),
                format!("tool:c2:{NOT_RUN}"),
                "user:Weiter".into(),
                "assistant:Fertig.:0".into(),
            ]
        );
    }

    /// Runs `api_history` without a window.
    struct HistoryOnly(Vec<Message>);

    impl HistoryOnly {
        fn history(self) -> Vec<ChatMessage> {
            let mut app = NemotronApp::with_settings(Settings::default(), None);
            app.messages = self.0;
            app.api_history()
        }
    }

    #[test]
    fn markdown_export_contains_turns_thinking_tools_and_attachments() {
        let (mut answer, _) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: String::new(),
                reasoning: "Hmm.".into(),
                tool_calls: vec![tool("c1", "read_file", r#"{"path":"src/main.rs"}"#)],
            },
            AgentEvent::ToolFinished {
                call_id: "c1".into(),
                output: "x".into(),
                ok: true,
                change: None,
            },
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: "Eine Katze.".into(),
                reasoning: String::new(),
                tool_calls: vec![],
            },
        ]);
        answer.status = Status::Done;
        let messages = vec![
            Message::user(
                "Was ist das?".into(),
                vec![Attachment::from_url(
                    "https://example.com/katze.jpg",
                    AttachmentKind::Image,
                )],
            ),
            answer,
        ];
        let md = conversation_markdown(&messages, "nvidia/test");
        assert!(md.starts_with("# Chat mit nvidia/test\n"));
        assert!(md.contains("## Du\n\nWas ist das?\n\n*Anhänge: katze.jpg*"));
        assert!(md.contains("<summary>Denkprozess</summary>\n\nHmm."));
        assert!(md.contains("- `read_file`: Gelesen: src/main.rs"), "{md}");
        assert!(md.contains("Eine Katze."));
    }

    /// Pumps the app like eframe would until `done` returns true.
    fn pump(app: &mut NemotronApp, ctx: &egui::Context, mut done: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(15);
        while !done() && Instant::now() < until {
            app.poll_agent();
            app.continue_in_new_chat(ctx);
            app.handle_remote(ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn remote_client() -> ureq::Agent {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into()
    }

    fn call(agent: &ureq::Agent, method: &str, url: &str, body: Option<Value>) -> (u16, Value) {
        let mut res = match (method, body) {
            ("GET", _) => agent.get(url).header("Authorization", "Bearer tok").call(),
            (_, body) => agent
                .post(url)
                .header("Authorization", "Bearer tok")
                .send(body.unwrap_or(json!({})).to_string()),
        }
        .unwrap();
        let text = res.body_mut().read_to_string().unwrap_or_default();
        (
            res.status().as_u16(),
            serde_json::from_str(&text).unwrap_or(Value::Null),
        )
    }

    /// Runs one frame of the input field with the given key presses.
    fn frame_with_keys(app: &mut NemotronApp, ctx: &egui::Context, keys: &[Key]) {
        let id = egui::Id::unique("eingabe_text");
        let events = keys
            .iter()
            .map(|key| egui::Event::Key {
                key: *key,
                physical_key: Some(*key),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            })
            .collect();
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            app.history_keys(&ctx, id);
            ui.add(egui::TextEdit::multiline(&mut app.input).id(id));
            ctx.memory_mut(|m| m.request_focus(id));
        });
        // Nothing is painted in a test.
        output.textures_delta.clear();
    }

    #[test]
    fn arrow_keys_in_the_input_field_call_back_earlier_messages() {
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        let ctx = egui::Context::default();
        app.history.push("Räum Downloads auf");
        app.history.push("Bau das Projekt");
        // First frame: the field gets the focus.
        frame_with_keys(&mut app, &ctx, &[]);

        frame_with_keys(&mut app, &ctx, &[Key::ArrowUp]);
        assert_eq!(app.input, "Bau das Projekt");
        frame_with_keys(&mut app, &ctx, &[Key::ArrowUp]);
        assert_eq!(app.input, "Räum Downloads auf");
        frame_with_keys(&mut app, &ctx, &[Key::ArrowDown]);
        assert_eq!(app.input, "Bau das Projekt");
        frame_with_keys(&mut app, &ctx, &[Key::ArrowDown]);
        assert_eq!(app.input, "", "back to the empty draft");

        // In the middle of a text the arrow keys keep moving the cursor.
        app.input = "halber Satz".into();
        app.history.stop_browsing();
        frame_with_keys(&mut app, &ctx, &[]);
        let id = egui::Id::unique("eingabe_text");
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        let end = egui::text::CCursor::new(app.input.chars().count());
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(&ctx, id);
        frame_with_keys(&mut app, &ctx, &[Key::ArrowUp]);
        assert_eq!(app.input, "halber Satz", "the cursor is at the end");
        // At the very start of the text, ↑ calls back the last message.
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(0),
            )));
        state.store(&ctx, id);
        frame_with_keys(&mut app, &ctx, &[Key::ArrowUp]);
        assert_eq!(app.input, "Bau das Projekt");
    }

    #[test]
    fn typing_on_while_the_server_hangs_goes_on_with_the_new_message() {
        use crate::api::tests::mock_server;
        let answer = "data: {\"choices\":[{\"delta\":{\"content\":\"Gleich da.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (base_url, nvidia) =
            mock_server(vec![("200 OK", "text/event-stream", answer.to_owned())]);
        let settings = Settings {
            base_url,
            api_key: "k".into(),
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        let ctx = egui::Context::default();
        app.messages
            .push(Message::user("Lies alles".into(), vec![]));
        app.messages.push(Message::assistant(true));
        let (_tx, rx) = mpsc::channel();
        let (decisions, _) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.active = Some(ActiveRequest {
            rx,
            cancel: cancel.clone(),
            decisions,
            approval: SharedApproval::new(ApprovalMode::AutoEdits),
            last_event: Instant::now() - Duration::from_secs(25),
        });
        // Silent for 25 s: the window says so, and the phone is told too.
        assert!(app.stalled_for().is_some());
        assert_eq!(app.remote_snapshot()["stalled"], true);

        app.input = "Dann nimm nur die erste Datei".into();
        assert!(app.can_send(), "typing on is allowed while Nemotron works");
        app.send(&ctx);
        assert!(
            cancel.load(Ordering::Relaxed),
            "the hanging run was stopped"
        );
        assert_eq!(app.messages[1].status, Status::Cancelled);
        assert_eq!(app.messages[2].text, "Dann nimm nur die erste Datei");
        let until = Instant::now() + Duration::from_secs(15);
        while app.active.is_some() && Instant::now() < until {
            app.poll_agent();
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = nvidia.join().unwrap();
        assert!(requests[0].contains("Dann nimm nur die erste Datei"));
        assert!(requests[0].contains("Lies alles"), "the chat goes on");
        assert_eq!(app.messages[3].steps[0].content, "Gleich da.");
        assert!(app.stalled_for().is_none(), "nothing runs any more");
    }

    #[test]
    fn an_error_after_a_change_leaves_a_mark_in_the_code_and_one_click_removes_it() {
        use crate::api::tests::mock_server;
        let dir = std::env::temp_dir().join(format!("fadis-mark-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "fn a() {}\nfn zahl() -> u32 { 41 }\nfn b() {}\n").unwrap();
        let edit = |text: &str| {
            format!(
                "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"choices":[{"delta":{"role":"assistant","content":""}}]}),
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"edit_file","arguments": json!({"path":"lib.rs","old_string":"41","new_string":text}).to_string()}}]}}]}),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
            )
        };
        let answer = "data: {\"choices\":[{\"delta\":{\"content\":\"Fertig.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (base_url, nvidia) = mock_server(vec![
            ("200 OK", "text/event-stream", edit("42")),
            // The server drops out right after the edit.
            (
                "401 Unauthorized",
                "application/json",
                r#"{"detail":"Server weg"}"#.to_owned(),
            ),
            ("200 OK", "text/event-stream", answer.to_owned()),
        ]);
        let settings = Settings {
            base_url,
            api_key: "k".into(),
            approval: ApprovalMode::AutoAll,
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        app.workspace = Workspace::open(&dir).unwrap();
        let ctx = egui::Context::default();
        app.input = "Mach daraus 42".into();
        app.send(&ctx);
        let until = Instant::now() + Duration::from_secs(15);
        while app.active.is_some() && Instant::now() < until {
            app.poll_agent();
            std::thread::sleep(Duration::from_millis(10));
        }

        // The edit stays, and one comment line stands right behind the changed line.
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "fn zahl() -> u32 { 42 }", "{text}");
        assert!(
            lines[2].starts_with("// FADIS-FEHLER: Auftrag unterbrochen"),
            "{text}"
        );
        assert_eq!(lines[3], "fn b() {}");
        assert_eq!(app.error_marks.len(), 1);
        assert!(app.interrupted_by().is_some());
        assert_eq!(app.remote_snapshot()["error_marks"], json!(["lib.rs"]));
        // The mark is part of the change: the diff shows it as a new line, and
        // Undo still sees the file as Nemotron left it (it does not ask).
        assert!(app.changes[0].diff().contains("+// FADIS-FEHLER"));
        assert!(!app.needs_undo_confirm(&app.changes[0]));

        // One click: the mark leaves the code, Nemotron goes on.
        app.continue_after_error(&ctx);
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "fn a() {}\nfn zahl() -> u32 { 42 }\nfn b() {}\n");
        assert!(app.error_marks.is_empty());
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| n.text.contains("1 Fehler-Markierung"))
        );
        assert!(!app.changes[0].diff().contains("FADIS-FEHLER"));
        assert!(!app.needs_undo_confirm(&app.changes[0]));
        while app.active.is_some() && Instant::now() < until {
            app.poll_agent();
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = nvidia.join().unwrap();
        assert!(requests[2].contains("wegen des Fehlers"));
        assert_eq!(app.messages.last().unwrap().steps[0].content, "Fertig.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_mark_without_a_change_in_a_file_with_comments_or_when_switched_off() {
        let dir = std::env::temp_dir().join(format!("fadis-nomark-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let json_file = dir.join("daten.json");
        std::fs::write(&json_file, "{\"a\": 2}\n").unwrap();
        let rust_file = dir.join("lib.rs");
        std::fs::write(&rust_file, "fn a() {}\n").unwrap();
        let mut app = NemotronApp::with_settings(Settings::default(), None);

        // Nothing was changed in this run: nowhere to write.
        app.mark_error_in_code("Server weg");
        assert!(app.error_marks.is_empty());

        // JSON has no comments: left alone.
        app.run_edits.push(FileChange {
            path: "daten.json".into(),
            abs: json_file.clone(),
            before: Some("{\"a\": 1}\n".into()),
            after: "{\"a\": 2}\n".into(),
        });
        app.mark_error_in_code("Server weg");
        assert!(app.error_marks.is_empty());
        assert_eq!(std::fs::read_to_string(&json_file).unwrap(), "{\"a\": 2}\n");

        // The file was changed by hand after Nemotron: not touched.
        app.run_edits.push(FileChange {
            path: "lib.rs".into(),
            abs: rust_file.clone(),
            before: Some("fn a() {}\n".into()),
            after: "fn b() {}\n".into(),
        });
        app.mark_error_in_code("Server weg");
        assert!(app.error_marks.is_empty());
        assert_eq!(std::fs::read_to_string(&rust_file).unwrap(), "fn a() {}\n");

        // As Nemotron left it, but the setting is off: not touched either.
        std::fs::write(&rust_file, "fn b() {}\n").unwrap();
        app.settings.mark_errors = false;
        app.mark_error_in_code("Server weg");
        assert!(app.error_marks.is_empty());
        assert_eq!(std::fs::read_to_string(&rust_file).unwrap(), "fn b() {}\n");

        // On: one mark, never two in the same file.
        app.settings.mark_errors = true;
        app.mark_error_in_code(
            "Verbindung zu https://x/v1/chat/completions fehlgeschlagen: timeout: receive response",
        );
        app.mark_error_in_code("noch ein Fehler");
        assert_eq!(app.error_marks.len(), 1);
        let text = std::fs::read_to_string(&rust_file).unwrap();
        assert_eq!(text.matches("FADIS-FEHLER").count(), 1);
        assert!(
            text.contains("Server nicht erreichbar (timeout: receive response)"),
            "{text}"
        );
        // "Nur entfernen" works without going on.
        assert_eq!(app.remove_error_marks(), 1);
        assert_eq!(std::fs::read_to_string(&rust_file).unwrap(), "fn b() {}\n");
        assert_eq!(app.remove_error_marks(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn after_an_error_one_click_goes_on_where_it_stopped() {
        use crate::api::tests::mock_server;
        let answer = "data: {\"choices\":[{\"delta\":{\"content\":\"Weiter geht es.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (base_url, nvidia) =
            mock_server(vec![("200 OK", "text/event-stream", answer.to_owned())]);
        let settings = Settings {
            base_url,
            api_key: "k".into(),
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        let ctx = egui::Context::default();
        // An answer that is fine offers nothing; the click does nothing then.
        let mut fine = Message::assistant(true);
        fine.status = Status::Done;
        app.messages = vec![Message::user("Bau das Projekt".into(), vec![]), fine];
        assert!(app.interrupted_by().is_none());
        app.continue_after_error(&ctx);
        assert!(app.active.is_none());
        assert_eq!(app.messages.len(), 2);

        // The server was not reachable: the hint shows the reason.
        let mut failed = Message::assistant(true);
        failed.status = Status::Failed(
            "Verbindung zu https://integrate.api.nvidia.com/v1/chat/completions fehlgeschlagen: timeout: receive response".into(),
        );
        app.messages[1] = failed;
        assert!(app.interrupted_by().is_some_and(|e| e.contains("timeout")));
        app.input = "halb getippt".into();
        app.continue_after_error(&ctx);
        assert!(app.active.is_some());
        assert_eq!(app.messages[2].text, CONTINUE_TEXT);
        assert_eq!(app.input, "halb getippt", "what was typed stays");
        assert!(
            app.interrupted_by().is_none(),
            "the hint is gone while it works"
        );
        let until = Instant::now() + Duration::from_secs(15);
        while app.active.is_some() && Instant::now() < until {
            app.poll_agent();
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = nvidia.join().unwrap();
        assert!(
            requests[0].contains("Bau das Projekt") && requests[0].contains("wegen des Fehlers")
        );
        assert_eq!(app.messages[3].steps[0].content, "Weiter geht es.");
        assert!(app.interrupted_by().is_none());
    }

    #[test]
    fn a_waiting_proposal_or_a_running_command_is_not_a_hanging_server() {
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        let (_tx, rx) = mpsc::channel();
        let (decisions, _) = mpsc::channel();
        app.active = Some(ActiveRequest {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            decisions,
            approval: SharedApproval::new(ApprovalMode::AskAll),
            last_event: Instant::now() - Duration::from_secs(60),
        });
        let mut msg = Message::assistant(true);
        for e in [
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: String::new(),
                reasoning: String::new(),
                tool_calls: vec![tool("c1", "run_command", r#"{"command":"make"}"#)],
            },
            AgentEvent::ApprovalNeeded {
                call_id: "c1".into(),
                preview: None,
                warning: None,
            },
        ] {
            apply_event(&mut msg, e, &mut Vec::new());
        }
        app.messages.push(Message::user("Bau".into(), vec![]));
        app.messages.push(msg);
        assert!(app.stalled_for().is_none(), "the user is being waited for");
        apply_event(
            app.messages.last_mut().unwrap(),
            AgentEvent::ToolStarted {
                call_id: "c1".into(),
            },
            &mut Vec::new(),
        );
        assert!(app.stalled_for().is_none(), "a command may run for minutes");
    }

    #[test]
    fn the_mode_switch_reaches_the_running_request_and_the_phone() {
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        assert_eq!(app.current_mode(), ApprovalMode::AutoEdits);
        let (_tx, rx) = mpsc::channel();
        let (decisions, _) = mpsc::channel();
        let approval = SharedApproval::new(ApprovalMode::AutoEdits);
        app.active = Some(ActiveRequest {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            decisions,
            approval: approval.clone(),
            last_event: Instant::now(),
        });
        app.set_mode(ApprovalMode::AskAll);
        assert_eq!(approval.get(), ApprovalMode::AskAll);
        assert_eq!(app.settings.approval, ApprovalMode::AskAll);
        assert_eq!(app.remote_snapshot()["approval"], "AskAll");
        // "Ab jetzt autonom" on a card changes the run, not the saved setting.
        approval.set(ApprovalMode::AutoAll);
        assert_eq!(app.current_mode(), ApprovalMode::AutoAll);
        assert_eq!(app.settings.approval, ApprovalMode::AskAll);
        assert_eq!(app.remote_snapshot()["approval"], "AutoAll");
        // Choosing the saved one again still switches the run back.
        app.set_mode(ApprovalMode::AskAll);
        assert_eq!(approval.get(), ApprovalMode::AskAll);
    }

    #[test]
    fn a_chat_that_got_too_long_goes_on_in_a_new_chat_with_the_note() {
        use crate::api::tests::mock_server;
        let answer = "data: {\"choices\":[{\"delta\":{\"content\":\"Weiter geht es.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (base_url, nvidia) =
            mock_server(vec![("200 OK", "text/event-stream", answer.to_owned())]);
        let settings = Settings {
            base_url,
            api_key: "k".into(),
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        // Chats go to a folder of their own in this test.
        let store_dir = std::env::temp_dir().join(format!("fadis-handoff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store_dir);
        app.chat_store = Some(ChatStore::new(store_dir.clone()));
        let ctx = egui::Context::default();
        let mut old = Message::assistant(true);
        old.status = Status::Done;
        app.messages = vec![Message::user("Bau das Projekt".into(), vec![]), old];
        let old_id = app.chat_id.clone();
        app.pending_handoff = Some((
            "Gelesen: a.rs, b.rs. Offen: Tests.".into(),
            ApprovalMode::AutoAll,
        ));

        app.continue_in_new_chat(&ctx);
        assert_ne!(app.chat_id, old_id, "a new chat");
        assert_eq!(app.handoff_chain, 1);
        assert_eq!(app.messages[0].role, Role::User);
        let first = &app.messages[0].text;
        // The first line is the title in the chat list.
        assert!(first.starts_with("↪ Bau das Projekt\n"), "{first}");
        assert!(first.contains("Auftrag aus dem vorigen Chat"), "{first}");
        let new_id = app.chat_id.clone();
        assert!(
            first.contains("Gelesen: a.rs, b.rs. Offen: Tests."),
            "{first}"
        );
        assert!(app.active.is_some(), "Nemotron goes on by himself");
        let until = Instant::now() + Duration::from_secs(15);
        while app.active.is_some() && Instant::now() < until {
            app.poll_agent();
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = nvidia.join().unwrap();
        assert!(
            requests[0].contains("Offen: Tests."),
            "the note reaches the model"
        );
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[1].steps[0].content, "Weiter geht es.");
        // Nothing is left to continue.
        app.continue_in_new_chat(&ctx);
        assert_eq!(app.handoff_chain, 1);

        // The old chat shows a button to the new one, also after a restart,
        // and the phone is told about it.
        app.open_chat(&old_id, &ctx).unwrap();
        let old = &app.messages[1];
        assert_eq!(old.continued_in.as_deref(), Some(new_id.as_str()));
        assert_eq!(
            app.remote_snapshot()["messages"][1]["continued_in"],
            json!(new_id)
        );
        // The list tells the continuation by its arrow.
        let titles: Vec<String> = app.chats.iter().map(|c| c.title.clone()).collect();
        assert!(titles.iter().any(|t| t.starts_with("↪ ")), "{titles:?}");
        let _ = std::fs::remove_dir_all(&store_dir);
    }

    #[test]
    fn phone_sends_a_message_and_sees_the_answer() {
        use crate::api::tests::mock_server;
        let answer = "data: {\"choices\":[{\"delta\":{\"content\":\"Hallo vom Mac!\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (base_url, nvidia) =
            mock_server(vec![("200 OK", "text/event-stream", answer.to_owned())]);
        let settings = Settings {
            base_url,
            api_key: "k".into(),
            remote_port: 0,
            remote_token: "tok".into(),
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        let ctx = egui::Context::default();
        app.start_remote(&ctx);
        let base = format!(
            "http://127.0.0.1:{}/api/v1",
            app.remote.as_ref().unwrap().port()
        );

        let phone = std::thread::spawn(move || {
            let agent = remote_client();
            let (status, _) = call(
                &agent,
                "POST",
                &format!("{base}/send"),
                Some(json!({"text": "Hi vom Handy"})),
            );
            assert_eq!(status, 200);
            let (status, body) = call(
                &agent,
                "POST",
                &format!("{base}/send"),
                Some(json!({"text": "noch was"})),
            );
            // Either still busy, or already done and accepted: both are valid answers.
            assert!(status == 409 || status == 200, "{status} {body}");
            let mut version = 0;
            for _ in 0..20 {
                let (_, state) = call(
                    &agent,
                    "GET",
                    &format!("{base}/state?after={version}&wait=5"),
                    None,
                );
                version = state["version"].as_u64().unwrap();
                let done = state["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["role"] == "assistant" && m["status"] == "done");
                if done {
                    return state;
                }
            }
            panic!("no answer arrived");
        });
        pump(&mut app, &ctx, || phone.is_finished());
        let state = phone.join().unwrap();
        nvidia.join().unwrap();

        assert_eq!(state["messages"][0]["text"], "Hi vom Handy");
        assert_eq!(
            state["messages"][1]["steps"][0]["content"],
            "Hallo vom Mac!"
        );
        assert_eq!(state["workspace"]["home"], true);
        assert_eq!(state["approval"], "AutoEdits");
        assert_eq!(app.messages[0].text, "Hi vom Handy");
    }

    #[test]
    fn phone_sees_saved_chats_and_sends_videos() {
        use crate::api::tests::mock_server;
        let answer = |text: &str| {
            format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
            )
        };
        let (base_url, nvidia) = mock_server(vec![(
            "200 OK",
            "text/event-stream",
            answer("Ein Hund rennt."),
        )]);
        let dir = std::env::temp_dir().join(format!("fadis-app-chats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings {
            base_url,
            api_key: "k".into(),
            remote_port: 0,
            remote_token: "tok".into(),
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        app.chat_store = Some(ChatStore::new(dir.clone()));
        let ctx = egui::Context::default();
        app.start_remote(&ctx);
        let base = format!(
            "http://127.0.0.1:{}/api/v1",
            app.remote.as_ref().unwrap().port()
        );

        let phone = std::thread::spawn(move || {
            let agent = remote_client();
            let upload = |name: &str, content_type: &str, bytes: &[u8]| -> (u16, Value) {
                let mut res = agent
                    .post(format!("{base}/upload?name={name}"))
                    .header("Authorization", "Bearer tok")
                    .header("Content-Type", content_type)
                    .send(bytes)
                    .unwrap();
                let text = res.body_mut().read_to_string().unwrap_or_default();
                (
                    res.status().as_u16(),
                    serde_json::from_str(&text).unwrap_or(Value::Null),
                )
            };
            let (status, up) = upload(
                "clip.mp4",
                "video/mp4",
                &[0, 0, 0, 24, b'f', b't', b'y', b'p'],
            );
            assert_eq!((status, up["kind"].as_str()), (200, Some("video")), "{up}");
            assert_eq!(upload("notiz.txt", "text/plain", b"hallo").0, 400);
            assert_eq!(
                call(
                    &agent,
                    "POST",
                    &format!("{base}/send"),
                    Some(json!({"text": "x", "uploads": ["up-gibtsnicht"]}))
                )
                .0,
                404
            );
            let (status, _) = call(
                &agent,
                "POST",
                &format!("{base}/send"),
                Some(json!({"text": "Was passiert im Video?", "uploads": [up["id"]]})),
            );
            assert_eq!(status, 200);
            let mut state = Value::Null;
            for _ in 0..20 {
                let version = state["version"].as_u64().unwrap_or(0);
                state = call(
                    &agent,
                    "GET",
                    &format!("{base}/state?after={version}&wait=5"),
                    None,
                )
                .1;
                if state["busy"] == false
                    && state["messages"].as_array().is_some_and(|m| m.len() == 2)
                {
                    break;
                }
            }
            let first_chat = state["chat"]["id"].as_str().unwrap().to_owned();
            assert_eq!(state["chat"]["title"], "Was passiert im Video?");
            assert_eq!(state["messages"][0]["attachments"], json!(["clip.mp4"]));
            assert_eq!(state["chats"][0]["id"], first_chat.as_str());
            assert_eq!(state["chats"][0]["messages"], 2);

            // A new chat on the phone is a new chat on the Mac; the old one stays listed.
            assert_eq!(
                call(&agent, "POST", &format!("{base}/new_chat"), None).0,
                200
            );
            let state = call(&agent, "GET", &format!("{base}/state"), None).1;
            assert_ne!(state["chat"]["id"], first_chat.as_str());
            assert_eq!(state["chat"]["title"], chats::UNTITLED);
            assert_eq!(state["messages"], json!([]));
            assert_eq!(state["chats"].as_array().unwrap().len(), 2);

            assert_eq!(
                call(
                    &agent,
                    "POST",
                    &format!("{base}/open_chat"),
                    Some(json!({"id": first_chat}))
                )
                .0,
                200
            );
            let state = call(&agent, "GET", &format!("{base}/state"), None).1;
            assert_eq!(state["chat"]["id"], first_chat.as_str());
            assert_eq!(
                state["messages"][1]["steps"][0]["content"],
                "Ein Hund rennt."
            );
            assert_eq!(
                call(
                    &agent,
                    "POST",
                    &format!("{base}/open_chat"),
                    Some(json!({"id": "c-0-gibtsnicht"}))
                )
                .0,
                404
            );
            assert_eq!(
                call(
                    &agent,
                    "POST",
                    &format!("{base}/open_chat"),
                    Some(json!({"id": "../settings"}))
                )
                .0,
                404
            );

            assert_eq!(
                call(
                    &agent,
                    "POST",
                    &format!("{base}/delete_chat"),
                    Some(json!({"id": first_chat}))
                )
                .0,
                200
            );
            let state = call(&agent, "GET", &format!("{base}/state"), None).1;
            assert_ne!(state["chat"]["id"], first_chat.as_str());
            assert!(
                state["chats"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|c| c["id"] != first_chat.as_str())
            );
            first_chat
        });
        pump(&mut app, &ctx, || phone.is_finished());
        let first_chat = phone.join().unwrap();
        let requests = nvidia.join().unwrap();
        // The video went to NVIDIA as a data URL.
        assert!(requests[0].contains(r#""type":"video_url""#));
        assert!(requests[0].contains("data:video/mp4;base64,AAAAGGZ0eXA="));
        assert!(!dir.join(format!("{first_chat}.json")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chat_that_cannot_be_saved_is_not_thrown_away() {
        let dir = std::env::temp_dir().join(format!("fadis-app-nosave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the chats folder should be: every save fails.
        std::fs::write(dir.join("chats"), "ich bin eine Datei").unwrap();
        let ctx = egui::Context::default();
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.chat_store = Some(ChatStore::new(dir.join("chats")));
        app.messages
            .push(Message::user("Wichtige Arbeit".into(), Vec::new()));
        let id = app.chat_id.clone();
        let err = app.new_chat(&ctx).unwrap_err();
        assert!(err.contains("nicht gespeichert"), "{err}");
        assert_eq!((app.messages.len(), &app.chat_id), (1, &id));
        assert!(app.notice.as_ref().is_some_and(|n| n.error));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn viewing_old_chats_does_not_reorder_the_list() {
        let dir = std::env::temp_dir().join(format!("fadis-app-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.chat_store = Some(ChatStore::new(dir.clone()));
        app.messages
            .push(Message::user("Erster".into(), Vec::new()));
        let first = app.chat_id.clone();
        app.save_chat();
        std::thread::sleep(Duration::from_millis(1_100));
        app.new_chat(&ctx).unwrap();
        app.messages
            .push(Message::user("Zweiter".into(), Vec::new()));
        let second = app.chat_id.clone();
        app.save_chat();
        let order = |app: &NemotronApp| app.chats.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(order(&app), [second.clone(), first.clone()]);
        // Looking at the older chat and back changes nothing about who is newest.
        app.open_chat(&first, &ctx).unwrap();
        app.open_chat(&second, &ctx).unwrap();
        assert_eq!(order(&app), [second.clone(), first.clone()]);
        // Deleting the open (newest) chat opens the newest of the others.
        app.delete_chat(&second, &ctx).unwrap();
        assert_eq!(app.chat_id, first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chats_are_saved_and_come_back_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("fadis-app-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.chat_store = Some(ChatStore::new(dir.clone()));
        app.messages.push(Message::user(
            "Räum Downloads auf".into(),
            vec![Attachment::from_bytes(
                "a.png",
                AttachmentKind::Image,
                "image/png",
                vec![9],
            )],
        ));
        let (mut answer, _) = run_events(vec![
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: String::new(),
                reasoning: "Erst schauen.".into(),
                tool_calls: vec![tool("c1", "list_dir", r#"{"path":"~/Downloads"}"#)],
            },
            AgentEvent::ToolStarted {
                call_id: "c1".into(),
            },
        ]);
        // Quit while the tool ran.
        answer.status = Status::Streaming;
        app.messages.push(answer);
        let id = app.chat_id.clone();
        app.save_chat();
        assert_eq!(app.chats[0].title, "Räum Downloads auf");

        let mut later = NemotronApp::with_settings(Settings::default(), None);
        later.chat_store = Some(ChatStore::new(dir.clone()));
        later.chats = later.chat_store.as_ref().unwrap().list();
        assert_eq!(later.chats.len(), 1);
        later.open_chat(&id, &ctx).unwrap();
        assert_eq!(later.messages.len(), 2);
        assert_eq!(later.messages[0].attachments[0].name, "a.png");
        let answer = &later.messages[1];
        assert_eq!(answer.status, Status::Cancelled);
        assert_eq!(answer.steps[0].reasoning, "Erst schauen.");
        assert_eq!(answer.steps[0].tools[0].state, ToolState::Interrupted);
        assert_eq!(answer.steps[0].tools[0].call.name, "list_dir");
        // The reopened chat goes to the model like before.
        assert!(
            matches!(&later.api_history()[0], ChatMessage::User { attachments, .. } if attachments.len() == 1)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phone_changes_settings_and_undoes_files() {
        let dir = std::env::temp_dir().join(format!("fadis-remote-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "neu\n").unwrap();
        let settings = Settings {
            remote_port: 0,
            remote_token: "tok".into(),
            ..Settings::default()
        };
        let mut app = NemotronApp::with_settings(settings, None);
        app.record_change(&FileChange {
            path: "a.txt".into(),
            abs: file.clone(),
            before: Some("alt\n".into()),
            after: "neu\n".into(),
        });
        let ctx = egui::Context::default();
        app.start_remote(&ctx);
        let base = format!(
            "http://127.0.0.1:{}/api/v1",
            app.remote.as_ref().unwrap().port()
        );

        let phone = std::thread::spawn(move || {
            let agent = remote_client();
            let settings = call(
                &agent,
                "POST",
                &format!("{base}/settings"),
                Some(json!({"approval": "AskAll", "thinking": false})),
            );
            let diff = call(&agent, "GET", &format!("{base}/diff?path=a.txt"), None);
            let missing = call(&agent, "GET", &format!("{base}/diff?path=b.txt"), None);
            let decide = call(
                &agent,
                "POST",
                &format!("{base}/decide"),
                Some(json!({"call_id": "x", "decision": "allow"})),
            );
            let undo = call(
                &agent,
                "POST",
                &format!("{base}/undo"),
                Some(json!({"path": "a.txt"})),
            );
            let (_, state) = call(&agent, "GET", &format!("{base}/state"), None);
            (settings, diff, missing, decide, undo, state)
        });
        pump(&mut app, &ctx, || phone.is_finished());
        let (settings, diff, missing, decide, undo, state) = phone.join().unwrap();

        assert_eq!(settings.0, 200);
        assert_eq!(app.settings.approval, ApprovalMode::AskAll);
        assert!(!app.settings.thinking);
        assert_eq!(diff.0, 200);
        assert!(diff.1["diff"].as_str().unwrap().contains("-alt\n+neu"));
        assert_eq!(missing.0, 404);
        assert_eq!(decide.0, 404, "nothing is waiting for approval");
        assert_eq!(undo.0, 200);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alt\n");
        assert_eq!(state["changes"], json!([]));
        assert_eq!(state["approval"], "AskAll");
        assert_eq!(state["thinking"], false);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undo_restores_and_deletes() {
        let dir = std::env::temp_dir().join(format!("fadis-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let edited = dir.join("alt.txt");
        let created = dir.join("neu.txt");
        std::fs::write(&edited, "nachher").unwrap();
        std::fs::write(&created, "neu").unwrap();

        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.record_change(&FileChange {
            path: "alt.txt".into(),
            abs: edited.clone(),
            before: Some("vorher".into()),
            after: "zwischen".into(),
        });
        app.record_change(&FileChange {
            path: "alt.txt".into(),
            abs: edited.clone(),
            before: Some("zwischen".into()),
            after: "nachher".into(),
        });
        app.record_change(&FileChange {
            path: "neu.txt".into(),
            abs: created.clone(),
            before: None,
            after: "neu".into(),
        });
        assert_eq!(app.changes.len(), 2);
        assert_eq!(app.changes[0].original.as_deref(), Some("vorher"));

        app.undo_change(0, false, false);
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "vorher");
        app.undo_change(0, false, false);
        assert!(!created.exists());
        assert!(app.changes.is_empty());

        // Changed again outside the agent: needs a confirmation first.
        std::fs::write(&edited, "von Hand").unwrap();
        app.record_change(&FileChange {
            path: "alt.txt".into(),
            abs: edited.clone(),
            before: Some("vorher".into()),
            after: "agent".into(),
        });
        app.undo_change(0, false, false);
        assert!(matches!(
            app.diff_window.as_ref().map(|w| &w.state),
            Some(DiffState::Confirming)
        ));
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "von Hand");
        app.undo_change(0, true, false);
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "vorher");

        // Changed by hand between two agent edits: undo would lose that.
        app.record_change(&FileChange {
            path: "alt.txt".into(),
            abs: edited.clone(),
            before: Some("vorher".into()),
            after: "agent 1".into(),
        });
        app.record_change(&FileChange {
            path: "alt.txt".into(),
            abs: edited.clone(),
            before: Some("agent 1 und Hand".into()),
            after: "agent 2".into(),
        });
        std::fs::write(&edited, "agent 2").unwrap();
        app.undo_change(0, false, false);
        assert!(app.diff_window.as_ref().is_some_and(
            |w| w.abs.as_ref() == Some(&edited) && matches!(w.state, DiffState::Confirming)
        ));
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "agent 2");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Draws the change window for one frame, as the real app does.
    fn frame_with_diff_window(app: &mut NemotronApp, ctx: &egui::Context) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let screen = ui.max_rect();
            app.diff_window(&ui.ctx().clone(), screen);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn undoing_in_the_change_window_shows_what_happened_right_there() {
        let dir = std::env::temp_dir().join(format!("fadis-diffwin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.rs");
        std::fs::write(&file, "eins\nzwei\ndrei\n").unwrap();
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.record_change(&FileChange {
            path: "lib.rs".into(),
            abs: file.clone(),
            before: Some("eins\nalt\ndrei\n".into()),
            after: "eins\nzwei\ndrei\n".into(),
        });
        let ctx = egui::Context::default();

        // The panel's "Diff": red for what went away, green for what came in.
        app.apply_ui_action(UiAction::ShowChange(0), &ctx);
        frame_with_diff_window(&mut app, &ctx);
        let window = app.diff_window.as_ref().unwrap();
        assert!(
            window.diff.contains("-alt") && window.diff.contains("+zwei"),
            "{}",
            window.diff
        );
        assert!(matches!(window.state, DiffState::Showing));

        // "↶ Rückgängig" inside the window: the file is back, and the window
        // shows the undo itself (what is gone again, what is back).
        let window = app.diff_window.take().unwrap();
        let window = app.apply_diff_action(window, Some(0), DiffAction::Undo);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "eins\nalt\ndrei\n");
        assert!(app.changes.is_empty());
        assert!(
            window.diff.contains("-zwei") && window.diff.contains("+alt"),
            "{}",
            window.diff
        );
        assert!(matches!(
            window.state,
            DiffState::Undone { deleted: false, .. }
        ));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| !n.error && n.text.contains("wiederhergestellt"))
        );
        app.diff_window = Some(window);
        frame_with_diff_window(&mut app, &ctx);
        assert!(
            app.diff_window.is_some(),
            "the window stays open for the result"
        );

        // "↷ Wieder anwenden" puts Nemotron's version back and the change on the list.
        let window = app.diff_window.take().unwrap();
        let window = app.apply_diff_action(window, None, DiffAction::Redo);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "eins\nzwei\ndrei\n"
        );
        assert_eq!(app.changes.len(), 1);
        assert!(matches!(window.state, DiffState::Showing));
        assert!(window.diff.contains("-alt") && window.diff.contains("+zwei"));

        // Changed by hand meanwhile: the window asks first, in itself.
        std::fs::write(&file, "von Hand\n").unwrap();
        let window = app.apply_diff_action(window, Some(0), DiffAction::Undo);
        assert!(matches!(window.state, DiffState::Confirming));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "von Hand\n");
        let window = app.apply_diff_action(window, Some(0), DiffAction::Cancel);
        assert!(matches!(window.state, DiffState::Showing));
        assert_eq!(app.changes.len(), 1);
        let window = app.apply_diff_action(window, Some(0), DiffAction::ForceUndo);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "eins\nalt\ndrei\n");
        assert!(matches!(window.state, DiffState::Undone { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undoing_a_new_file_shows_it_going_away() {
        let dir = std::env::temp_dir().join(format!("fadis-diffnew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("neu.txt");
        std::fs::write(&file, "a\nb\n").unwrap();
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.record_change(&FileChange {
            path: "neu.txt".into(),
            abs: file.clone(),
            before: None,
            after: "a\nb\n".into(),
        });
        // From the panel's "↶ Rückgängig" button: the window opens with the result.
        app.undo_change(0, false, true);
        assert!(!file.exists());
        let window = app.diff_window.as_ref().unwrap();
        assert!(matches!(
            window.state,
            DiffState::Undone { deleted: true, .. }
        ));
        assert_eq!(
            diff_stats(&window.diff),
            (0, 2),
            "both lines are gone again"
        );
        // The phone's undo does not pop up a window on the Mac.
        app.diff_window = None;
        app.record_change(&FileChange {
            path: "neu.txt".into(),
            abs: file.clone(),
            before: None,
            after: "a\n".into(),
        });
        std::fs::write(&file, "a\n").unwrap();
        app.undo_change(0, true, false);
        assert!(app.diff_window.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_while_a_tool_runs_marks_it_interrupted_until_it_reports() {
        let mut app = NemotronApp::with_settings(Settings::default(), None);
        app.messages.push(Message::user("Bau".into(), Vec::new()));
        let mut msg = Message::assistant(true);
        for e in [
            AgentEvent::StepStarted,
            AgentEvent::StepFinished {
                content: String::new(),
                reasoning: String::new(),
                tool_calls: vec![
                    tool("c1", "run_command", r#"{"command":"make"}"#),
                    tool("c2", "read_file", "{}"),
                ],
            },
            AgentEvent::ToolStarted {
                call_id: "c1".into(),
            },
        ] {
            apply_event(&mut msg, e, &mut Vec::new());
        }
        app.messages.push(msg);
        let (tx, rx) = mpsc::channel();
        let (decisions, _) = mpsc::channel();
        app.active = Some(ActiveRequest {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            decisions,
            approval: SharedApproval::new(ApprovalMode::AutoEdits),
            last_event: Instant::now(),
        });
        app.stop();
        let tools = &app.messages[1].steps[0].tools;
        assert_eq!(tools[0].state, ToolState::Interrupted);
        assert_eq!(tools[1].state, ToolState::NotRun);
        let history = app.api_history();
        assert!(matches!(&history[2], ChatMessage::Tool { content, .. } if content == INTERRUPTED));
        assert!(matches!(&history[3], ChatMessage::Tool { content, .. } if content == NOT_RUN));

        // The command ends after the stop and reports what it did.
        tx.send(AgentEvent::ToolFinished {
            call_id: "c1".into(),
            output: "$ make\nstopped by the user".into(),
            ok: false,
            change: None,
        })
        .unwrap();
        tx.send(AgentEvent::Cancelled).unwrap();
        drop(tx);
        app.poll_stopped();
        let tool = &app.messages[1].steps[0].tools[0];
        assert_eq!(tool.state, ToolState::Done);
        assert!(tool.output.contains("stopped by the user"));
        assert!(app.stopped.is_empty());
    }

    #[test]
    fn old_tool_outputs_are_shortened_in_the_history() {
        let long = "x".repeat(10_000);
        let answer = |id: &str| {
            run_events(vec![
                AgentEvent::StepStarted,
                AgentEvent::StepFinished {
                    content: String::new(),
                    reasoning: String::new(),
                    tool_calls: vec![tool(id, "read_file", "{}")],
                },
                AgentEvent::ToolFinished {
                    call_id: id.into(),
                    output: long.clone(),
                    ok: true,
                    change: None,
                },
                AgentEvent::Finished {
                    finish_reason: Some("stop".into()),
                },
            ])
            .0
        };
        let history = HistoryOnly(vec![
            Message::user("eins".into(), Vec::new()),
            answer("c1"),
            Message::user("zwei".into(), Vec::new()),
            answer("c2"),
        ])
        .history();
        let outputs: Vec<usize> = history
            .iter()
            .filter_map(|m| match m {
                ChatMessage::Tool { content, .. } => Some(content.len()),
                _ => None,
            })
            .collect();
        assert!(outputs[0] < 2_200, "{outputs:?}");
        assert_eq!(outputs[1], 10_000);
    }

    #[test]
    fn think_tags_are_text_when_thinking_is_off() {
        let mut msg = Message::assistant(false);
        for e in [
            AgentEvent::StepStarted,
            AgentEvent::Text(TextDelta::Content("Schreib </think> nie so.".into())),
        ] {
            apply_event(&mut msg, e, &mut Vec::new());
        }
        assert_eq!(msg.steps[0].parts(), (None, "Schreib </think> nie so."));
    }
}
