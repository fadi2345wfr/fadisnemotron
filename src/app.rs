//! The main window: chat with Nemotron, or, with a project folder open, an
//! agent that reads, edits and runs things in that folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use egui_commonmark::CommonMarkCache;

use crate::agent::{self, AgentConfig, AgentEvent, Decision};
use crate::api::{self, ChatMessage, Role, TextDelta, ToolCall};
use crate::attachments::{self, Attachment, AttachmentKind};
use crate::config::Settings;
use crate::tools::{ApprovalMode, FileChange, Workspace};
use crate::widgets::{self, UiAction};

const API_KEY_URL: &str = "https://build.nvidia.com/settings/api-keys";
const NOT_RUN: &str = "Not run: the request was stopped before this tool ran.";

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Streaming,
    Done,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolState {
    Planned,
    AwaitingApproval,
    Running,
    Done,
    Declined,
    NotRun,
}

pub struct ToolView {
    pub call: ToolCall,
    pub state: ToolState,
    pub output: String,
    pub ok: bool,
    /// What the tool changed on disk.
    pub change: Option<FileChange>,
    /// What it would change, shown while it waits for approval.
    pub preview: Option<FileChange>,
    /// Why this command always needs a yes (e.g. `sudo`).
    pub warning: Option<String>,
}

/// One model request within an answer: its thinking, text and tool calls.
#[derive(Default)]
pub struct Step {
    pub reasoning: String,
    pub content: String,
    pub tools: Vec<ToolView>,
    /// Set once so the thinking folds away when the answer or tools arrive.
    pub collapse_reasoning: bool,
    reasoning_collapsed: bool,
}

impl Step {
    /// `(thinking, answer)`, also when the server sent the thinking inline.
    pub fn parts(&self) -> (Option<&str>, &str) {
        if self.reasoning.trim().is_empty() {
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

pub struct Message {
    pub role: Role,
    /// The user's text.
    pub text: String,
    pub attachments: Vec<Attachment>,
    /// The assistant's steps.
    pub steps: Vec<Step>,
    pub status: Status,
    pub finish_reason: Option<String>,
    pub notices: Vec<String>,
}

impl Message {
    fn user(text: String, attachments: Vec<Attachment>) -> Self {
        Self {
            role: Role::User,
            text,
            attachments,
            steps: Vec::new(),
            status: Status::Done,
            finish_reason: None,
            notices: Vec::new(),
        }
    }

    fn assistant() -> Self {
        Self {
            role: Role::Assistant,
            status: Status::Streaming,
            ..Self::user(String::new(), Vec::new())
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

    /// Marks tools that never got to run, after a stop or an error.
    fn settle_tools(&mut self) {
        for tool in self.steps.iter_mut().flat_map(|s| &mut s.tools) {
            if matches!(
                tool.state,
                ToolState::Planned | ToolState::AwaitingApproval | ToolState::Running
            ) {
                tool.state = ToolState::NotRun;
                tool.preview = None;
            }
        }
    }
}

/// A file the agent changed during this session, with its original content.
pub struct TrackedChange {
    pub path: String,
    pub abs: PathBuf,
    /// `None` if the agent created the file.
    pub original: Option<String>,
    pub current: String,
}

impl TrackedChange {
    pub fn line_stats(&self) -> (usize, usize) {
        crate::tools::line_stats(self.original.as_deref().unwrap_or(""), &self.current)
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

/// An undo whose file was changed again outside the agent; needs a yes.
struct UndoConfirm {
    index: usize,
}

pub struct NemotronApp {
    settings: Settings,
    settings_path: Option<PathBuf>,
    settings_draft: Option<SettingsDraft>,
    workspace: Option<Workspace>,
    changes: Vec<TrackedChange>,
    messages: Vec<Message>,
    input: String,
    pending: Vec<Attachment>,
    loading_attachments: usize,
    attach_tx: Sender<Result<Attachment, String>>,
    attach_rx: Receiver<Result<Attachment, String>>,
    url_draft: Option<UrlDraft>,
    active: Option<ActiveRequest>,
    /// Workers that were stopped but may still report a file they wrote
    /// just before noticing; kept so the change can still be undone.
    stopped: Vec<Receiver<AgentEvent>>,
    notice: Option<Notice>,
    diff_view: Option<(String, String)>,
    undo_confirm: Option<UndoConfirm>,
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
        if app.needs_api_key() {
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
        app
    }

    fn with_settings(settings: Settings, settings_path: Option<PathBuf>) -> Self {
        let (attach_tx, attach_rx) = mpsc::channel();
        let workspace = settings
            .workspace
            .as_deref()
            .and_then(|p| Workspace::open(p).ok());
        Self {
            settings,
            settings_path,
            settings_draft: None,
            workspace,
            changes: Vec::new(),
            messages: Vec::new(),
            input: String::new(),
            pending: Vec::new(),
            loading_attachments: 0,
            attach_tx,
            attach_rx,
            url_draft: None,
            active: None,
            stopped: Vec::new(),
            notice: None,
            diff_view: None,
            undo_confirm: None,
            markdown: CommonMarkCache::default(),
            http: api::agent(),
            focus_input: true,
            scroll_to_bottom: false,
        }
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
        let title = match &self.workspace {
            Some(ws) => format!("Fadis Nemotron – {}", ws.name()),
            None => "Fadis Nemotron".to_owned(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    // ------------------------------------------------------------- project

    fn pick_workspace(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Projektordner öffnen");
        if let Some(ws) = &self.workspace {
            dialog = dialog.set_directory(ws.root());
        }
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
                self.settings.workspace = Some(ws.root().to_path_buf());
                self.workspace = Some(ws);
                self.changes.clear();
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
        self.workspace = None;
        self.changes.clear();
        self.settings.workspace = None;
        self.save_settings();
        self.update_title(ctx);
    }

    fn record_change(&mut self, change: &FileChange) {
        if let Some(tracked) = self.changes.iter_mut().find(|t| t.abs == change.abs) {
            tracked.current = change.after.clone();
        } else {
            self.changes.push(TrackedChange {
                path: change.path.clone(),
                abs: change.abs.clone(),
                original: change.before.clone(),
                current: change.after.clone(),
            });
        }
    }

    /// Restores a file to how it was before the agent first touched it.
    fn undo_change(&mut self, index: usize, force: bool) {
        let Some(tracked) = self.changes.get(index) else {
            return;
        };
        let on_disk = std::fs::read_to_string(&tracked.abs).ok();
        if !force && on_disk.as_deref() != Some(tracked.current.as_str()) {
            self.undo_confirm = Some(UndoConfirm { index });
            return;
        }
        let tracked = self.changes.remove(index);
        let result = match &tracked.original {
            Some(text) => std::fs::write(&tracked.abs, text),
            None => match std::fs::remove_file(&tracked.abs) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        self.notice = Some(match result {
            Ok(()) if tracked.original.is_some() => Notice {
                text: format!("{} wiederhergestellt.", tracked.path),
                error: false,
            },
            Ok(()) => Notice {
                text: format!("{} gelöscht (war neu).", tracked.path),
                error: false,
            },
            Err(e) => {
                let text = format!("{} konnte nicht zurückgesetzt werden: {e}", tracked.path);
                self.changes.insert(index.min(self.changes.len()), tracked);
                Notice { text, error: true }
            }
        });
    }

    fn reveal_in_file_manager(path: &Path) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(path).spawn();
    }

    fn new_chat(&mut self, ctx: &egui::Context) {
        self.stop();
        for att in self
            .messages
            .iter()
            .flat_map(|m| &m.attachments)
            .chain(&self.pending)
        {
            ctx.forget_image(&att.preview_uri());
        }
        self.messages.clear();
        self.pending.clear();
        self.input.clear();
        self.notice = None;
        self.markdown = CommonMarkCache::default();
        self.focus_input = true;
    }

    // ------------------------------------------------------------- sending

    fn can_send(&self) -> bool {
        self.active.is_none()
            && self.loading_attachments == 0
            && (!self.input.trim().is_empty() || !self.pending.is_empty())
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
        self.messages
            .push(Message::user(text, std::mem::take(&mut self.pending)));
        self.input.clear();
        self.notice = None;
        self.start_request(ctx);
    }

    /// The conversation so far in API form, including every tool call and
    /// its result, so follow-up messages keep the context.
    fn api_history(&self) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        for msg in &self.messages {
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
                                ToolState::Done | ToolState::Declined => tool.output.clone(),
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
        self.messages.push(Message::assistant());
        self.scroll_to_bottom = true;

        let (tx, rx) = mpsc::channel();
        let (decision_tx, decision_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let system_prompt = match &self.workspace {
            Some(ws) => agent::system_prompt(&self.settings.system_prompt, ws),
            None => self.settings.system_prompt.clone(),
        };
        let config = AgentConfig {
            base_url: self.settings.base_url.clone(),
            api_key: self.settings.effective_api_key(),
            system_prompt,
            options: self.settings.generation_options(),
            workspace: self.workspace.clone(),
            approval: self.settings.approval,
            max_steps: self.settings.max_steps.max(1),
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
            self.stopped.push(active.rx);
        }
    }

    /// Records file changes that stopped workers still report.
    fn poll_stopped(&mut self) {
        let mut changes = Vec::new();
        self.stopped.retain(|rx| {
            loop {
                match rx.try_recv() {
                    Ok(AgentEvent::ToolFinished {
                        change: Some(change),
                        ..
                    }) => changes.push(change),
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break true,
                    Err(TryRecvError::Disconnected) => break false,
                }
            }
        });
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
        }
    }

    fn poll_agent(&mut self) {
        let Some(active) = &self.active else { return };
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
        let mut finished = false;
        let mut changes = Vec::new();
        if let Some(msg) = self.messages.last_mut() {
            for event in events {
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
        }
        if finished {
            self.active = None;
        }
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
            self.new_chat(ctx);
        }
        if open_project {
            self.pick_workspace(ctx);
        }
        let dialog_open =
            self.settings_draft.is_some() || self.url_draft.is_some() || self.diff_view.is_some();
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

    // ------------------------------------------------------------------ UI

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("kopfzeile").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Fadis Nemotron").strong().size(18.0).color(widgets::accent(ui)));
                let mode = if self.workspace.is_some() { "Agent" } else { "Chat" };
                ui.label(RichText::new(mode).small().strong().color(Color32::BLACK).background_color(widgets::accent(ui)))
                    .on_hover_text(if self.workspace.is_some() {
                        "Nemotron liest und ändert Dateien im geöffneten Projektordner."
                    } else {
                        "Nur Chat. Öffne einen Projektordner, damit Nemotron Dateien lesen und ändern kann."
                    });
                ui.hyperlink_to(RichText::new(&self.settings.model).small().weak(), widgets::MODEL_URL)
                    .on_hover_text("Modellseite auf build.nvidia.com öffnen");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Einstellungen (⌘ ,)").clicked() {
                        self.open_settings();
                    }
                    if ui
                        .add_enabled(!self.messages.is_empty(), egui::Button::new("💾"))
                        .on_hover_text("Chat als Markdown speichern")
                        .clicked()
                    {
                        self.export_markdown();
                    }
                    if ui.button("➕ Neuer Chat").on_hover_text("⌘ N").clicked() {
                        self.new_chat(ctx);
                    }
                    if self.workspace.is_none()
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

    fn project_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(ws) = &self.workspace else { return };
        let (name, root) = (ws.name(), ws.root().to_path_buf());
        let busy = self.active.is_some();
        let mut action: Option<UiAction> = None;
        let mut switch = false;
        let mut close = false;
        egui::Panel::right("projekt").resizable(true).default_size(270.0).min_size(200.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("📁 {name}")).strong().size(16.0));
                ui.add(egui::Label::new(RichText::new(root.display().to_string()).small().weak()).truncate())
                    .on_hover_text(root.display().to_string());
                ui.horizontal_wrapped(|ui| {
                    switch = ui.add_enabled(!busy, egui::Button::new("Wechseln")).clicked();
                    if ui.button("Im Finder").on_hover_text("Ordner im Dateimanager zeigen").clicked() {
                        Self::reveal_in_file_manager(&root);
                    }
                    close = ui
                        .add_enabled(!busy, egui::Button::new("Schließen"))
                        .on_hover_text("Zurück zum reinen Chat")
                        .clicked();
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Nemotron darf").strong());
                let before = self.settings.approval;
                for mode in ApprovalMode::ALL {
                    ui.radio_value(&mut self.settings.approval, mode, mode.label());
                }
                if self.settings.approval != before {
                    self.save_settings();
                }
                ui.label(
                    RichText::new("Lesen und Suchen geht immer ohne Nachfrage. Gilt ab der nächsten Nachricht.")
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
                    let (added, removed) = change.line_stats();
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
                                action = Some(UiAction::ShowDiff { title: change.path.clone(), diff: change.diff() });
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
            let hint = if self.workspace.is_some() {
                "Sag Nemotron, was im Projekt passieren soll … (Enter senden, Shift+Enter neue Zeile)"
            } else {
                "Nachricht an Nemotron … (Enter senden, Shift+Enter neue Zeile)"
            };
            // `horizontal` keeps the row as tall as its content; a free
            // layout here would stretch the panel over the whole window.
            ui.horizontal(|ui| {
                let spacing = ui.spacing().item_spacing.x;
                let buttons = 34.0 + 34.0 + 96.0 + 3.0 * spacing;
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
                if self.active.is_some() {
                    stop = ui.add_sized([96.0, 34.0], egui::Button::new("⏹ Stop")).on_hover_text("Esc").clicked();
                } else {
                    let label = RichText::new("Senden ➤").strong().color(Color32::BLACK);
                    send |= ui
                        .add_enabled_ui(self.can_send(), |ui| {
                            ui.add_sized([96.0, 34.0], egui::Button::new(label).fill(widgets::ACCENT))
                        })
                        .inner
                        .on_hover_text("Enter")
                        .clicked();
                }
            });
            ui.add_space(8.0);

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
            UiAction::ShowDiff { title, diff } => self.diff_view = Some((title, diff)),
            UiAction::Undo(index) => self.undo_change(index, false),
            UiAction::OpenProject => self.pick_workspace(ctx),
            UiAction::OpenSettings => self.open_settings(),
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
            self.workspace.as_ref().map(Workspace::name).as_deref(),
            self.needs_api_key(),
            &mut actions,
        );
        for action in actions {
            self.apply_ui_action(action, ctx);
        }
    }

    fn diff_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some((title, diff)) = &self.diff_view else {
            return;
        };
        let mut open = true;
        egui::Window::new(format!("Änderungen: {title}"))
            .open(&mut open)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .default_size([720.0, 520.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    widgets::diff_label(ui, diff, usize::MAX);
                });
            });
        if !open {
            self.diff_view = None;
        }
    }

    fn undo_confirm_window(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some(confirm) = &self.undo_confirm else {
            return;
        };
        let index = confirm.index;
        let Some(path) = self.changes.get(index).map(|c| c.path.clone()) else {
            self.undo_confirm = None;
            return;
        };
        let mut yes = false;
        let mut no = false;
        egui::Window::new("Trotzdem rückgängig machen?")
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{path} wurde nach Nemotrons Änderung noch einmal verändert. Rückgängig setzt die Datei auf den Stand vor Nemotron zurück; spätere Änderungen gehen verloren."
                ));
                ui.horizontal(|ui| {
                    yes = ui.button("Zurücksetzen").clicked();
                    no = ui.button("Abbrechen").clicked();
                });
            });
        if yes {
            self.undo_confirm = None;
            self.undo_change(index, true);
        } else if no {
            self.undo_confirm = None;
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
                        ui.hyperlink_to("Kostenlosen Schlüssel auf build.nvidia.com holen", API_KEY_URL);
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
            // The project folder is managed in the side panel, not here.
            settings.workspace = self.settings.workspace.clone();
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
        AgentEvent::StepStarted => msg.steps.push(Step::default()),
        AgentEvent::Text(delta) => {
            if msg.steps.is_empty() {
                msg.steps.push(Step::default());
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
                msg.steps.push(Step::default());
            }
            let step = msg.steps.last_mut().expect("step exists");
            step.content = content;
            step.reasoning = reasoning;
            step.tools = tool_calls
                .into_iter()
                .map(|call| ToolView {
                    call,
                    state: ToolState::Planned,
                    output: String::new(),
                    ok: false,
                    change: None,
                    preview: None,
                    warning: None,
                })
                .collect();
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
                if let Some(change) = &change {
                    changes.push(change.clone());
                }
                tool.change = change;
            }
        }
        AgentEvent::Notice(text) => msg.notices.push(text),
        AgentEvent::Finished { finish_reason } => {
            msg.status = Status::Done;
            msg.finish_reason = finish_reason;
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let full_rect = ui.max_rect();

        self.poll_agent();
        self.poll_stopped();
        self.poll_attachments();
        self.handle_global_input(&ctx);

        self.top_bar(ui, &ctx);
        self.composer(ui, &ctx);
        self.project_panel(ui, &ctx);
        self.chat_area(ui, &ctx);
        self.settings_window(&ctx, full_rect);
        self.url_window(&ctx, full_rect);
        self.diff_window(&ctx, full_rect);
        self.undo_confirm_window(&ctx, full_rect);
        widgets::drop_overlay(&ctx, full_rect);

        if self.active.is_some() || self.loading_attachments > 0 {
            // Keeps spinners moving even when no tokens arrive (long thinking).
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
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
        let mut msg = Message::assistant();
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

        app.undo_change(0, false);
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "vorher");
        app.undo_change(0, false);
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
        app.undo_change(0, false);
        assert!(app.undo_confirm.is_some());
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "von Hand");
        app.undo_change(0, true);
        assert_eq!(std::fs::read_to_string(&edited).unwrap(), "vorher");
        std::fs::remove_dir_all(&dir).ok();
    }
}
