//! The chat window.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use eframe::egui::{
    self, Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText, TextStyle,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::api::{self, Role, StreamEvent, Turn};
use crate::attachments::{self, Attachment, AttachmentKind};
use crate::config::Settings;

const ACCENT: Color32 = Color32::from_rgb(118, 185, 0);
const ACCENT_LIGHT_THEME: Color32 = Color32::from_rgb(64, 118, 0);
const API_KEY_URL: &str = "https://build.nvidia.com/settings/api-keys";
const MODEL_URL: &str = "https://build.nvidia.com/nvidia/nemotron-3.5-super-vl-120b-a12b";
/// Messages are laid out in a column no wider than this.
const MAX_COLUMN_WIDTH: f32 = 880.0;

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Streaming,
    Done,
    Cancelled,
    Failed(String),
}

struct Message {
    role: Role,
    /// The user's text, or the assistant's answer.
    text: String,
    /// The assistant's thinking, if the server sent it separately.
    reasoning: String,
    attachments: Vec<Attachment>,
    status: Status,
    finish_reason: Option<String>,
    /// Set when the answer starts so the thinking folds away once.
    collapse_reasoning: bool,
}

impl Message {
    fn user(text: String, attachments: Vec<Attachment>) -> Self {
        Self {
            role: Role::User,
            text,
            reasoning: String::new(),
            attachments,
            status: Status::Done,
            finish_reason: None,
            collapse_reasoning: false,
        }
    }

    fn assistant() -> Self {
        Self {
            role: Role::Assistant,
            status: Status::Streaming,
            ..Self::user(String::new(), Vec::new())
        }
    }

    /// `(thinking, answer)`, also when the server sent the thinking inline.
    fn parts(&self) -> (Option<&str>, &str) {
        if self.reasoning.trim().is_empty() {
            api::split_inline_thinking(&self.text)
        } else {
            (Some(self.reasoning.trim()), self.text.trim_start())
        }
    }
}

enum MessageAction {
    Copy(String),
    Retry,
}

struct ActiveRequest {
    rx: Receiver<StreamEvent>,
    cancel: Arc<AtomicBool>,
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

pub struct NemotronApp {
    settings: Settings,
    settings_path: Option<PathBuf>,
    settings_draft: Option<SettingsDraft>,
    messages: Vec<Message>,
    input: String,
    pending: Vec<Attachment>,
    loading_attachments: usize,
    attach_tx: Sender<Result<Attachment, String>>,
    attach_rx: Receiver<Result<Attachment, String>>,
    url_draft: Option<UrlDraft>,
    active: Option<ActiveRequest>,
    notice: Option<Notice>,
    markdown: CommonMarkCache,
    agent: ureq::Agent,
    focus_input: bool,
}

impl NemotronApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let settings_path = Settings::default_path();
        let settings = settings_path
            .as_deref()
            .map(Settings::load_from)
            .unwrap_or_default();
        let (attach_tx, attach_rx) = mpsc::channel();
        let mut app = Self {
            settings,
            settings_path,
            settings_draft: None,
            messages: Vec::new(),
            input: String::new(),
            pending: Vec::new(),
            loading_attachments: 0,
            attach_tx,
            attach_rx,
            url_draft: None,
            active: None,
            notice: None,
            markdown: CommonMarkCache::default(),
            agent: api::agent(),
            focus_input: true,
        };
        if app.needs_api_key() {
            app.open_settings();
        }
        // Files given on the command line (`open -a "Fadis Nemotron" --args bild.png`) are attached.
        let files: Vec<PathBuf> = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .collect();
        app.load_paths(files, &cc.egui_ctx);
        app
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

    // ---------------------------------------------------------------- sending

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

    /// Sends the whole conversation and appends a streaming answer.
    fn start_request(&mut self, ctx: &egui::Context) {
        let history: Vec<(Role, String, Vec<Attachment>)> = self
            .messages
            .iter()
            .filter_map(|m| match (m.role, &m.status) {
                (Role::User, _) => Some((Role::User, m.text.clone(), m.attachments.clone())),
                (Role::Assistant, Status::Done | Status::Cancelled) => {
                    let answer = m.parts().1.trim();
                    (!answer.is_empty()).then(|| (Role::Assistant, answer.to_owned(), Vec::new()))
                }
                (Role::Assistant, _) => None,
            })
            .collect();
        self.messages.push(Message::assistant());

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let system_prompt = self.settings.system_prompt.clone();
        let options = self.settings.generation_options();
        let base_url = self.settings.base_url.clone();
        let api_key = self.settings.effective_api_key();
        let agent = self.agent.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // Encoding large videos as base64 takes a moment, so it happens here and not in the UI thread.
            let turns: Vec<Turn<'_>> = history
                .iter()
                .map(|(role, text, attachments)| Turn {
                    role: *role,
                    text,
                    attachments,
                })
                .collect();
            let body = api::build_request_body(&system_prompt, &turns, &options);
            api::stream_chat(
                &agent,
                &base_url,
                &api_key,
                &body,
                &worker_cancel,
                |event| {
                    let delivered = tx.send(event).is_ok();
                    ctx.request_repaint();
                    delivered
                },
            );
        });
        self.active = Some(ActiveRequest { rx, cancel });
    }

    fn stop(&mut self) {
        if let Some(active) = self.active.take() {
            active.cancel.store(true, Ordering::Relaxed);
            if let Some(msg) = self.messages.last_mut()
                && msg.status == Status::Streaming
            {
                msg.status = Status::Cancelled;
            }
        }
    }

    /// Drops the last answer and asks again.
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

    fn poll_stream(&mut self) {
        let Some(active) = &self.active else { return };
        let Some(msg) = self.messages.last_mut() else {
            return;
        };
        let mut finished = false;
        loop {
            match active.rx.try_recv() {
                Ok(StreamEvent::Reasoning(r)) => msg.reasoning.push_str(&r),
                Ok(StreamEvent::Content(c)) => {
                    if msg.text.is_empty() && !msg.reasoning.trim().is_empty() {
                        msg.collapse_reasoning = true;
                    }
                    msg.text.push_str(&c);
                }
                Ok(StreamEvent::Done(reason)) => {
                    msg.status = Status::Done;
                    msg.finish_reason = reason;
                    finished = true;
                    break;
                }
                Ok(StreamEvent::Error(e)) => {
                    msg.status = Status::Failed(e);
                    finished = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    msg.status = Status::Failed("Die Verbindung wurde unerwartet beendet.".into());
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.active = None;
        }
    }

    // ------------------------------------------------------------ attachments

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
        let (settings, new_chat, escape) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N)),
                i.key_pressed(Key::Escape),
            )
        });
        if settings {
            self.open_settings();
        }
        if new_chat {
            self.new_chat(ctx);
        }
        if escape
            && self.active.is_some()
            && self.settings_draft.is_none()
            && self.url_draft.is_none()
        {
            self.stop();
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        self.load_paths(dropped, ctx);
    }

    // ------------------------------------------------------------------ UI

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("kopfzeile").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Fadis Nemotron").strong().size(18.0).color(accent(ui)));
                ui.hyperlink_to(RichText::new(&self.settings.model).small().weak(), MODEL_URL)
                    .on_hover_text("Modellseite auf build.nvidia.com öffnen");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("⚙ Einstellungen").on_hover_text("⌘ ,").clicked() {
                        self.open_settings();
                    }
                    if ui
                        .add_enabled(!self.messages.is_empty(), egui::Button::new("💾 Exportieren"))
                        .on_hover_text("Chat als Markdown speichern")
                        .clicked()
                    {
                        self.export_markdown();
                    }
                    if ui.button("➕ Neuer Chat").on_hover_text("⌘ N").clicked() {
                        self.new_chat(ctx);
                    }
                    let mut thinking = self.settings.thinking;
                    if ui
                        .checkbox(&mut thinking, "Denkmodus")
                        .on_hover_text(
                            "An: Nemotron denkt vor der Antwort nach (besser bei schweren Aufgaben).\n\
                             Aus: schnellere, direkte Antworten.",
                        )
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

    fn composer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::bottom("eingabe").show(ui, |ui| {
            ui.add_space(8.0);
            if let Some(i) = attachment_row(ui, &self.pending, true) {
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
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
                    if self.active.is_some() {
                        stop = ui
                            .add_sized([96.0, 34.0], egui::Button::new("⏹ Stop"))
                            .on_hover_text("Esc")
                            .clicked();
                    } else {
                        let label = RichText::new("Senden ➤").strong().color(Color32::BLACK);
                        send = ui
                            .add_enabled_ui(self.can_send(), |ui| {
                                ui.add_sized([96.0, 34.0], egui::Button::new(label).fill(ACCENT))
                            })
                            .inner
                            .on_hover_text("Enter")
                            .clicked();
                    }
                    add_url = ui
                        .add_sized([34.0, 34.0], egui::Button::new("🔗"))
                        .on_hover_text("Bild- oder Video-Link anhängen")
                        .clicked();
                    pick = ui
                        .add_sized([34.0, 34.0], egui::Button::new("📎"))
                        .on_hover_text("Bilder oder Videos anhängen (oder ins Fenster ziehen)")
                        .clicked();

                    let edit = egui::TextEdit::multiline(&mut self.input)
                        .hint_text("Nachricht an Nemotron … (Enter senden, Shift+Enter neue Zeile)")
                        .desired_rows(2)
                        .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
                    let response = ui.add_sized([ui.available_width(), 56.0], edit);
                    if self.focus_input {
                        response.request_focus();
                        self.focus_input = false;
                    }
                    if response.has_focus()
                        && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift)
                    {
                        send = true;
                    }
                });
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
                self.url_draft = Some(UrlDraft {
                    url: String::new(),
                    kind: AttachmentKind::Image,
                    kind_chosen: false,
                    error: None,
                });
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
                        accent(ui)
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
                self.welcome(ui);
                return;
            }

            let mut action = None;
            let count = self.messages.len();
            let busy = self.active.is_some();
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    centered_column(ui, |ui| {
                        ui.add_space(8.0);
                        for (i, msg) in self.messages.iter_mut().enumerate() {
                            let is_last = i + 1 == count;
                            ui.push_id(i, |ui| {
                                if let Some(a) =
                                    message_ui(ui, msg, is_last && !busy, &mut self.markdown)
                                {
                                    action = Some(a);
                                }
                            });
                            ui.add_space(10.0);
                        }
                    });
                });

            match action {
                Some(MessageAction::Copy(text)) => {
                    ctx.copy_text(text);
                    self.notice = Some(Notice {
                        text: "Antwort kopiert.".into(),
                        error: false,
                    });
                }
                Some(MessageAction::Retry) => self.retry(ctx),
                None => {}
            }
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.22).max(16.0));
            ui.label(RichText::new("Nemotron 3.5 Super VL").size(26.0).strong().color(accent(ui)));
            ui.add_space(4.0);
            ui.label("Offenes Multimodal-Modell von NVIDIA · 120 Mrd. Parameter (12 Mrd. aktiv) · bis 1 Mio. Token Kontext");
            ui.add_space(14.0);
            ui.label("Stell eine Frage, oder zieh Bilder, Screenshots, Dokument-Seiten oder ein MP4-Video ins Fenster.");
            ui.label(RichText::new("Tipp: Für Videos 1–2 Minuten lange Clips verwenden. PDFs vorher als PNG exportieren.").weak());
            if self.needs_api_key() {
                ui.add_space(14.0);
                if ui.button("🔑 API-Schlüssel eintragen").clicked() {
                    self.open_settings();
                }
            }
        });
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
            .default_width(600.0)
            .show(ctx, |ui| {
                let s = &mut draft.settings;
                egui::Grid::new("einstellungen").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                    ui.label("API-Schlüssel");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.api_key)
                                    .password(!draft.show_key)
                                    .hint_text("nvapi-…")
                                    .desired_width(340.0),
                            );
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
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.thinking, "an");
                        ui.add_enabled_ui(s.thinking, |ui| {
                            ui.label("Denk-Budget");
                            ui.add(egui::DragValue::new(&mut s.reasoning_budget).range(0..=1_000_000).speed(64))
                                .on_hover_text("Höchstzahl Tokens fürs Nachdenken. 0 = keine Grenze (Standard).");
                        });
                    });
                    ui.end_row();

                    ui.label("Temperatur");
                    ui.add(egui::Slider::new(&mut s.temperature, 0.0..=2.0).step_by(0.05))
                        .on_hover_text("NVIDIA empfiehlt 1.0");
                    ui.end_row();

                    ui.label("Top-p");
                    ui.add(egui::Slider::new(&mut s.top_p, 0.0..=1.0).step_by(0.01))
                        .on_hover_text("NVIDIA empfiehlt 0.95");
                    ui.end_row();

                    ui.label("Max. Tokens");
                    ui.add(egui::DragValue::new(&mut s.max_tokens).range(256..=262_144).speed(128))
                        .on_hover_text("Länge von Denken + Antwort zusammen. Bei leeren Antworten erhöhen.");
                    ui.end_row();

                    ui.label("Max. Bildgröße");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.max_image_dim).range(0..=8192).speed(16).suffix(" px"));
                        ui.label(RichText::new("längste Seite, 0 = nicht verkleinern").small().weak());
                    });
                    ui.end_row();

                    ui.label("System-Prompt");
                    ui.add(egui::TextEdit::multiline(&mut s.system_prompt).desired_rows(3).desired_width(400.0));
                    ui.end_row();
                });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    save = ui.button(RichText::new("Speichern").strong()).clicked();
                    cancel = ui.button("Abbrechen").clicked();
                    check = ui.add_enabled(draft.check.is_none(), egui::Button::new("Verbindung testen")).clicked();
                    if ui.button("Standardwerte").on_hover_text("Alles außer dem API-Schlüssel zurücksetzen").clicked() {
                        let key = std::mem::take(&mut draft.settings.api_key);
                        draft.settings = Settings { api_key: key, ..Settings::default() };
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
                        ui.label(RichText::new(format!("✔ {msg}")).color(accent(ui)));
                    }
                    Some(Err(msg)) => {
                        ui.label(RichText::new(format!("⚠ {msg}")).color(ui.visuals().error_fg_color));
                    }
                    None => {}
                }
            });

        if check {
            let (tx, rx) = mpsc::channel();
            let agent = self.agent.clone();
            let s = draft.settings.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = api::check_connection(
                    &agent,
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

impl eframe::App for NemotronApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let full_rect = ui.max_rect();

        self.poll_stream();
        self.poll_attachments();
        self.handle_global_input(&ctx);

        self.top_bar(ui, &ctx);
        self.composer(ui, &ctx);
        self.chat_area(ui, &ctx);
        self.settings_window(&ctx, full_rect);
        self.url_window(&ctx, full_rect);
        drop_overlay(&ctx, full_rect);

        if self.active.is_some() || self.loading_attachments > 0 {
            // Keeps spinners moving even when no tokens arrive (long thinking).
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}

// ------------------------------------------------------------------ helpers

fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ACCENT
    } else {
        ACCENT_LIGHT_THEME
    }
}

fn setup_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        for (text_style, font) in &mut style.text_styles {
            font.size = match text_style {
                TextStyle::Small => 11.5,
                TextStyle::Body | TextStyle::Button => 14.5,
                TextStyle::Monospace => 13.0,
                TextStyle::Heading => 20.0,
                _ => font.size,
            };
        }
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.url_in_tooltip = true;
        let accent = if style.visuals.dark_mode {
            ACCENT
        } else {
            ACCENT_LIGHT_THEME
        };
        style.visuals.hyperlink_color = accent;
        style.visuals.selection.bg_fill = accent.gamma_multiply(0.45);
    });
}

/// Lays `add_contents` out in a centred column of at most [`MAX_COLUMN_WIDTH`].
fn centered_column(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    let column = width.min(MAX_COLUMN_WIDTH);
    let side = ((width - column) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(side);
        ui.vertical(|ui| {
            ui.set_width(column);
            add_contents(ui);
        });
    });
}

fn message_ui(
    ui: &mut egui::Ui,
    msg: &mut Message,
    is_last: bool,
    markdown: &mut CommonMarkCache,
) -> Option<MessageAction> {
    let user = msg.role == Role::User;
    let fill = if user {
        accent(ui).gamma_multiply(0.14)
    } else {
        ui.visuals().faint_bg_color
    };
    let collapse = std::mem::take(&mut msg.collapse_reasoning);
    let mut action = None;

    egui::Frame::new()
        .fill(fill)
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(if user { "Du" } else { "Nemotron" }).strong());
                if msg.status == Status::Streaming {
                    ui.spinner();
                    let (_, answer) = msg.parts();
                    ui.label(RichText::new(if answer.is_empty() { "denkt nach …" } else { "schreibt …" }).weak());
                }
            });

            if user {
                if !msg.attachments.is_empty() {
                    attachment_row(ui, &msg.attachments, false);
                }
                if !msg.text.is_empty() {
                    ui.add(egui::Label::new(&msg.text).selectable(true));
                }
                return;
            }

            let (thinking, answer) = msg.parts();
            if let Some(thinking) = thinking {
                egui::CollapsingHeader::new(
                    RichText::new(format!("💭 Denkprozess ({} Zeichen)", thinking.chars().count())).weak(),
                )
                .id_salt("denkprozess")
                .default_open(msg.status == Status::Streaming)
                .open(collapse.then_some(false))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(thinking).weak()).selectable(true));
                        });
                });
            }
            if !answer.is_empty() {
                CommonMarkViewer::new().show(ui, markdown, answer);
            }

            let warn = ui.visuals().warn_fg_color;
            match &msg.status {
                Status::Streaming => {}
                Status::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {e}"));
                }
                Status::Cancelled => {
                    ui.label(RichText::new("Abgebrochen.").weak());
                }
                Status::Done => {
                    if msg.finish_reason.as_deref() == Some("length") {
                        ui.colored_label(
                            warn,
                            "Die Antwort wurde bei „Max. Tokens“ abgeschnitten. Erhöhe den Wert in den Einstellungen.",
                        );
                    } else if answer.trim().is_empty() {
                        ui.colored_label(
                            warn,
                            "Keine Antwort erhalten. Erhöhe „Max. Tokens“ oder schalte den Denkmodus aus.",
                        );
                    }
                }
            }

            if msg.status != Status::Streaming {
                ui.horizontal(|ui| {
                    if !answer.trim().is_empty() && ui.small_button("📋 Kopieren").clicked() {
                        action = Some(MessageAction::Copy(answer.trim().to_owned()));
                    }
                    if is_last {
                        let label = if msg.status == Status::Done { "↻ Neu generieren" } else { "↻ Erneut versuchen" };
                        if ui.small_button(label).clicked() {
                            action = Some(MessageAction::Retry);
                        }
                    }
                });
            }
        });
    action
}

/// Shows attachments as a row of thumbnails. Returns the index the user
/// asked to remove, if `removable`.
fn attachment_row(ui: &mut egui::Ui, items: &[Attachment], removable: bool) -> Option<usize> {
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, att) in items.iter().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_max_width(150.0);
                ui.vertical(|ui| {
                    match att.image_bytes() {
                        Some(bytes) => {
                            ui.add(
                                egui::Image::from_bytes(att.preview_uri(), bytes)
                                    .fit_to_exact_size(egui::vec2(140.0, 96.0))
                                    .corner_radius(4),
                            );
                        }
                        None => {
                            let icon = match (&att.source, att.kind) {
                                (attachments::Source::Url(_), AttachmentKind::Image) => "🔗🖼",
                                (attachments::Source::Url(_), AttachmentKind::Video) => "🔗🎬",
                                (_, _) => "🎬",
                            };
                            ui.label(RichText::new(icon).size(30.0));
                        }
                    }
                    ui.add(egui::Label::new(RichText::new(&att.name).small()).truncate())
                        .on_hover_text(&att.name);
                    ui.label(RichText::new(att.size_label()).small().weak());
                    if let Some(note) = &att.note {
                        ui.label(RichText::new(note).small().weak());
                    }
                    if removable && ui.small_button("✖ Entfernen").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
    });
    remove
}

fn drop_overlay(ctx: &egui::Context, rect: egui::Rect) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("ablegen"),
    ));
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Bilder oder Videos hier ablegen",
        egui::FontId::proportional(24.0),
        Color32::WHITE,
    );
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
                let (thinking, answer) = msg.parts();
                if let Some(thinking) = thinking {
                    out.push_str("<details><summary>Denkprozess</summary>\n\n");
                    out.push_str(thinking);
                    out.push_str("\n\n</details>\n\n");
                }
                out.push_str(answer.trim());
                out.push('\n');
                match &msg.status {
                    Status::Failed(e) => out.push_str(&format!("\n*Fehler: {e}*\n")),
                    Status::Cancelled => out.push_str("\n*Abgebrochen.*\n"),
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

    #[test]
    fn parts_prefer_separate_reasoning_and_fall_back_to_inline_tags() {
        let mut m = Message::assistant();
        m.reasoning = "Ich überlege.".into();
        m.text = "\nAntwort".into();
        assert_eq!(m.parts(), (Some("Ich überlege."), "Antwort"));

        let mut m = Message::assistant();
        m.text = "<think>inline</think>Antwort".into();
        assert_eq!(m.parts(), (Some("inline"), "Antwort"));
    }

    #[test]
    fn markdown_export_contains_turns_thinking_and_attachments() {
        let mut answer = Message::assistant();
        answer.reasoning = "Hmm.".into();
        answer.text = "Eine Katze.".into();
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
        assert!(md.contains("## Nemotron"));
        assert!(md.trim_end().ends_with("Eine Katze."));
    }
}
