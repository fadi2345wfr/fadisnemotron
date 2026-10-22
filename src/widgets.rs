//! Drawing helpers for the chat: messages, tool cards, diffs, attachments.
//!
//! Nothing in here opens a scroll area inside the chat: a nested scroll area
//! catches the mouse wheel and makes the whole chat feel stuck.

use eframe::egui::{self, Color32, RichText, TextStyle};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde_json::Value;

use crate::agent::Decision;
use crate::api::Role;
use crate::app::{Message, Status, Step, ToolState, ToolView};
use crate::attachments::{self, Attachment, AttachmentKind};
use crate::tools::canonical_tool_name;

pub const ACCENT: Color32 = Color32::from_rgb(118, 185, 0);
const ACCENT_LIGHT_THEME: Color32 = Color32::from_rgb(64, 118, 0);
pub const ADDED: Color32 = Color32::from_rgb(110, 190, 90);
pub const REMOVED: Color32 = Color32::from_rgb(225, 95, 85);
pub const MODEL_URL: &str = "https://build.nvidia.com/nvidia/nemotron-3.5-super-vl-120b-a12b";
/// Messages are laid out in a column no wider than this.
const MAX_COLUMN_WIDTH: f32 = 900.0;
/// Lines of tool output or diff shown inline; the rest via copy or diff window.
const INLINE_OUTPUT_LINES: usize = 40;
const INLINE_DIFF_LINES: usize = 120;

/// Something the user clicked inside the chat; handled by the app afterwards.
pub enum UiAction {
    Copy(String),
    Retry,
    Decide { call_id: String, decision: Decision },
    ShowDiff { title: String, diff: String },
    Undo(usize),
    OpenProject,
    OpenSettings,
    UsePrompt(String),
}

pub fn accent(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        ACCENT
    } else {
        ACCENT_LIGHT_THEME
    }
}

pub fn setup_style(ctx: &egui::Context) {
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
pub fn centered_column(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
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

pub fn welcome(
    ui: &mut egui::Ui,
    workspace: Option<&str>,
    needs_key: bool,
    actions: &mut Vec<UiAction>,
) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.16).max(16.0));
        ui.label(RichText::new("Nemotron 3.5 Super VL").size(26.0).strong().color(accent(ui)));
        ui.add_space(4.0);
        ui.label(RichText::new("NVIDIAs offenes Multimodal-Modell · 120 Mrd. Parameter (12 Mrd. aktiv) · bis 1 Mio. Token Kontext").weak());
        ui.add_space(18.0);
        match workspace {
            Some(name) => {
                ui.label(RichText::new(format!("Agent-Modus in „{name}“")).size(18.0).strong());
                ui.label("Nemotron liest, durchsucht und ändert die Dateien in diesem Ordner selbst und kann Befehle ausführen.");
                ui.label(RichText::new("Jede Änderung siehst du rechts mit Diff und kannst sie rückgängig machen.").weak());
                ui.add_space(12.0);
                for prompt in [
                    "Erkläre mir, wie dieses Projekt aufgebaut ist.",
                    "Bau das Projekt, finde die Fehler und behebe sie.",
                    "Schreib Tests für die wichtigste Funktion und lass sie laufen.",
                ] {
                    if ui.button(prompt).clicked() {
                        actions.push(UiAction::UsePrompt(prompt.to_owned()));
                    }
                }
            }
            None => {
                ui.label(RichText::new("Nemotron arbeitet direkt auf deinem Mac").size(18.0).strong());
                ui.label("Er sieht deinen Benutzerordner – Downloads, Schreibtisch, Dokumente – legt Ordner und Dateien an, räumt auf und führt Befehle aus.");
                ui.add_space(8.0);
                for prompt in [
                    "Was liegt in meinem Downloads-Ordner? Sortiere es nach Art.",
                    "Leg auf dem Schreibtisch einen Ordner „Rechnungen“ an und verschiebe alle PDFs mit „Rechnung“ im Namen aus Downloads hinein.",
                ] {
                    if ui.button(prompt).clicked() {
                        actions.push(UiAction::UsePrompt(prompt.to_owned()));
                    }
                }
                ui.add_space(8.0);
                if ui.button(RichText::new("📁 Projektordner öffnen").size(16.0)).on_hover_text("Für Programmierprojekte: Nemotron arbeitet dann in diesem Ordner").clicked() {
                    actions.push(UiAction::OpenProject);
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Oder stell einfach eine Frage – auch mit Bildern, Screenshots oder einem MP4-Video (ins Fenster ziehen).").weak());
            }
        }
        if needs_key {
            ui.add_space(14.0);
            if ui.button("🔑 API-Schlüssel eintragen").clicked() {
                actions.push(UiAction::OpenSettings);
            }
        }
    });
}

pub fn message_ui(
    ui: &mut egui::Ui,
    msg: &mut Message,
    is_last: bool,
    busy: bool,
    markdown: &mut CommonMarkCache,
    actions: &mut Vec<UiAction>,
) {
    let user = msg.role == Role::User;
    let fill = if user {
        accent(ui).gamma_multiply(0.14)
    } else {
        ui.visuals().faint_bg_color
    };
    egui::Frame::new().fill(fill).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if user {
            ui.label(RichText::new("Du").strong());
            if !msg.attachments.is_empty() {
                attachment_row(ui, &msg.attachments, false);
            }
            if !msg.text.is_empty() {
                ui.add(egui::Label::new(&msg.text).selectable(true));
            }
            return;
        }

        let streaming = msg.status == Status::Streaming;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Nemotron").strong());
            if streaming {
                ui.spinner();
                ui.label(RichText::new(activity(msg)).weak());
            }
        });
        let step_count = msg.steps.len();
        for (i, step) in msg.steps.iter_mut().enumerate() {
            let live = streaming && i + 1 == step_count;
            ui.push_id(("schritt", i), |ui| step_ui(ui, step, live, markdown, actions));
        }
        for notice in &msg.notices {
            ui.label(RichText::new(format!("ℹ {notice}")).italics().weak());
        }

        let warn = ui.visuals().warn_fg_color;
        let has_tools = msg.steps.iter().any(|s| !s.tools.is_empty());
        match &msg.status {
            Status::Streaming => {}
            Status::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("⚠ {e}"));
            }
            Status::Cancelled => {
                ui.label(RichText::new("Gestoppt.").weak());
            }
            Status::Done => {
                if msg.finish_reason.as_deref() == Some("length") {
                    ui.colored_label(warn, "Die Antwort wurde bei „Max. Tokens“ abgeschnitten. Erhöhe den Wert in den Einstellungen.");
                } else if msg.answer().is_empty() && !has_tools {
                    ui.colored_label(warn, "Keine Antwort erhalten. Erhöhe „Max. Tokens“ oder schalte den Denkmodus aus.");
                }
            }
        }

        if !streaming {
            ui.horizontal(|ui| {
                let answer = msg.answer();
                if !answer.is_empty() && ui.small_button("📋 Kopieren").clicked() {
                    actions.push(UiAction::Copy(answer.to_owned()));
                }
                if is_last && !busy {
                    let label = if msg.status == Status::Done { "↻ Neu generieren" } else { "↻ Erneut versuchen" };
                    if ui.small_button(label).on_hover_text("Fragt noch einmal. Bereits geänderte Dateien bleiben so.").clicked() {
                        actions.push(UiAction::Retry);
                    }
                }
            });
        }
    });
}

/// What the agent is doing right now, for the header.
fn activity(msg: &Message) -> &'static str {
    let tools = msg
        .steps
        .last()
        .map(|s| s.tools.as_slice())
        .unwrap_or_default();
    if tools.iter().any(|t| t.state == ToolState::AwaitingApproval) {
        "wartet auf deine Freigabe"
    } else if tools
        .iter()
        .any(|t| matches!(t.state, ToolState::Running | ToolState::Planned))
    {
        "arbeitet …"
    } else if msg.steps.last().is_some_and(|s| !s.parts().1.is_empty()) {
        "schreibt …"
    } else {
        "denkt nach …"
    }
}

fn step_ui(
    ui: &mut egui::Ui,
    step: &mut Step,
    live: bool,
    markdown: &mut CommonMarkCache,
    actions: &mut Vec<UiAction>,
) {
    let collapse = std::mem::take(&mut step.collapse_reasoning);
    let (thinking, answer) = step.parts();
    if let Some(thinking) = thinking {
        egui::CollapsingHeader::new(
            RichText::new(format!(
                "💭 Denkprozess ({} Zeichen)",
                thinking.chars().count()
            ))
            .weak(),
        )
        .id_salt("denkprozess")
        .default_open(live && answer.is_empty() && step.tools.is_empty())
        .open(collapse.then_some(false))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(thinking).weak()).selectable(true));
        });
    }
    if !answer.is_empty() {
        CommonMarkViewer::new().show(ui, markdown, answer);
    }
    for tool in &step.tools {
        ui.push_id(&tool.call.id, |ui| tool_card(ui, tool, actions));
    }
}

/// Short German description of a tool call, e.g. "Gelesen: src/main.rs".
pub fn tool_title(tool: &ToolView) -> String {
    let args: Value = serde_json::from_str(&tool.call.arguments).unwrap_or(Value::Null);
    let text = |key: &str| {
        let aliases: &[&str] = match key {
            "path" => &["path", "file_path", "filePath", "dir_path", "file"],
            "command" => &["command", "cmd"],
            "pattern" => &["pattern", "query", "glob", "regex"],
            _ => &[],
        };
        std::iter::once(key)
            .chain(aliases.iter().copied())
            .find_map(|k| args.get(k).and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let path = || {
        let p = text("path");
        if p.is_empty() || p == "." {
            "Projektordner".to_owned()
        } else {
            p
        }
    };
    let pending = matches!(tool.state, ToolState::Planned | ToolState::AwaitingApproval);
    match canonical_tool_name(&tool.call.name) {
        "list_dir" => format!("Ordner angesehen: {}", path()),
        "find_files" => format!("Dateien gesucht: {}", text("pattern")),
        "read_file" => match args.get("offset").and_then(Value::as_u64) {
            Some(from) if from > 1 => format!("Gelesen: {} (ab Zeile {from})", path()),
            _ => format!("Gelesen: {}", path()),
        },
        "search" => {
            let place = text("path");
            let glob = text("glob");
            let mut title = format!("Gesucht: „{}“", text("pattern"));
            if !place.is_empty() && place != "." {
                title.push_str(&format!(" in {place}"));
            }
            if !glob.is_empty() {
                title.push_str(&format!(" ({glob})"));
            }
            title
        }
        "edit_file" if pending => format!("Möchte bearbeiten: {}", path()),
        "edit_file" => format!("Bearbeitet: {}", path()),
        "write_file" if pending => format!("Möchte schreiben: {}", path()),
        "write_file" => format!("Geschrieben: {}", path()),
        "run_command" if pending => format!("Möchte ausführen: {}", text("command")),
        "run_command" => format!("Befehl: {}", text("command")),
        other => format!("Werkzeug „{other}“"),
    }
}

fn tool_icon(name: &str) -> &'static str {
    match canonical_tool_name(name) {
        "list_dir" => "📂",
        "find_files" => "🔎",
        "read_file" => "📄",
        "search" => "🔍",
        "edit_file" => "✏",
        "write_file" => "📝",
        "run_command" => "▶",
        _ => "🔧",
    }
}

fn tool_card(ui: &mut egui::Ui, tool: &ToolView, actions: &mut Vec<UiAction>) {
    let waiting = tool.state == ToolState::AwaitingApproval;
    let stroke = if waiting {
        egui::Stroke::new(1.5, accent(ui))
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    egui::Frame::new()
        .stroke(stroke)
        .corner_radius(6)
        .inner_margin(8)
        .outer_margin(egui::Margin::symmetric(0, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match tool.state {
                    ToolState::Running => {
                        ui.spinner();
                    }
                    ToolState::Done if tool.ok => {
                        ui.label(RichText::new("✔").color(ADDED));
                    }
                    ToolState::Done => {
                        ui.label(RichText::new("⚠").color(ui.visuals().warn_fg_color))
                            .on_hover_text(
                                "Das Werkzeug meldete einen Fehler; Nemotron sieht die Meldung.",
                            );
                    }
                    ToolState::AwaitingApproval => {
                        ui.label(RichText::new("⏸").color(accent(ui)));
                    }
                    ToolState::Declined => {
                        ui.label(RichText::new("✖").weak())
                            .on_hover_text("abgelehnt");
                    }
                    ToolState::NotRun => {
                        ui.label(RichText::new("–").weak())
                            .on_hover_text("nicht ausgeführt");
                    }
                    ToolState::Interrupted => {
                        ui.label(RichText::new("⏹").color(ui.visuals().warn_fg_color))
                            .on_hover_text("beim Stopp abgebrochen – kann teilweise gelaufen sein");
                    }
                    ToolState::Planned => {
                        ui.label(RichText::new("○").weak());
                    }
                }
                ui.label(tool_icon(&tool.call.name));
                let title = tool_title(tool);
                let strong = matches!(
                    canonical_tool_name(&tool.call.name),
                    "edit_file" | "write_file" | "run_command"
                );
                let text = if strong {
                    RichText::new(title.clone()).strong()
                } else {
                    RichText::new(title.clone())
                };
                ui.add(egui::Label::new(text).truncate())
                    .on_hover_text(title);
                if let Some((added, removed)) = tool.stats {
                    ui.label(RichText::new(format!("+{added}")).color(ADDED).small());
                    ui.label(RichText::new(format!("−{removed}")).color(REMOVED).small());
                }
                match tool.state {
                    ToolState::Declined => {
                        ui.label(RichText::new("abgelehnt").weak().small());
                    }
                    ToolState::NotRun => {
                        ui.label(RichText::new("nicht ausgeführt").weak().small());
                    }
                    _ => {}
                }
            });

            if waiting {
                ui.label(RichText::new("Nemotron möchte das tun. Erlauben?").strong());
                if let Some(warning) = &tool.warning {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("⚠ Achtung: Dieser Befehl {warning}. Er wird deshalb immer erst nachgefragt."),
                    );
                }
                match (&tool.preview, canonical_tool_name(&tool.call.name)) {
                    (Some(_), _) => diff_label(ui, &tool.diff, INLINE_DIFF_LINES),
                    (None, "run_command") => {
                        let args: Value =
                            serde_json::from_str(&tool.call.arguments).unwrap_or(Value::Null);
                        let command = args
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or(&tool.call.arguments);
                        code_frame(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format!("$ {command}")).monospace())
                                    .wrap(),
                            );
                        });
                        ui.label(
                            RichText::new("Läuft im Projektordner mit deinen Benutzerrechten.")
                                .small()
                                .weak(),
                        );
                    }
                    (None, _) => {
                        ui.label(RichText::new(&tool.call.arguments).monospace().small());
                    }
                }
                ui.horizontal(|ui| {
                    let allow = egui::Button::new(
                        RichText::new("✔ Erlauben").strong().color(Color32::BLACK),
                    )
                    .fill(ACCENT);
                    if ui.add(allow).clicked() {
                        actions.push(UiAction::Decide {
                            call_id: tool.call.id.clone(),
                            decision: Decision::Allow,
                        });
                    }
                    if ui
                        .button("Für diesen Auftrag alles erlauben")
                        .on_hover_text(
                            "Bis Nemotron mit dieser Nachricht fertig ist, ohne weitere Nachfrage.",
                        )
                        .clicked()
                    {
                        actions.push(UiAction::Decide {
                            call_id: tool.call.id.clone(),
                            decision: Decision::AllowForRun,
                        });
                    }
                    if ui.button("✖ Ablehnen").clicked() {
                        actions.push(UiAction::Decide {
                            call_id: tool.call.id.clone(),
                            decision: Decision::Deny,
                        });
                    }
                });
                return;
            }

            if tool.state != ToolState::Done || (tool.output.is_empty() && tool.change.is_none()) {
                return;
            }
            egui::CollapsingHeader::new(RichText::new("Details").small().weak())
                .id_salt("details")
                .default_open(false)
                .show(ui, |ui| {
                    if let Some(change) = &tool.change {
                        diff_label(ui, &tool.diff, INLINE_DIFF_LINES);
                        if ui.small_button("Ganzen Diff im Fenster zeigen").clicked() {
                            actions.push(UiAction::ShowDiff {
                                title: change.path.clone(),
                                diff: tool.diff.clone(),
                            });
                        }
                    } else {
                        output_label(ui, &tool.output, INLINE_OUTPUT_LINES);
                        if ui.small_button("📋 Ausgabe kopieren").clicked() {
                            actions.push(UiAction::Copy(tool.output.clone()));
                        }
                    }
                });
        });
}

fn code_frame(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(4)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

/// A unified diff with added lines green and removed lines red.
pub fn diff_label(ui: &mut egui::Ui, diff: &str, max_lines: usize) {
    let font = egui::FontId::monospace(12.5);
    let normal = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let mut job = egui::text::LayoutJob::default();
    let total = diff.lines().count();
    for line in diff.lines().take(max_lines) {
        let color = if line.starts_with("+++") || line.starts_with("---") {
            weak
        } else if line.starts_with('+') {
            ADDED
        } else if line.starts_with('-') {
            REMOVED
        } else if line.starts_with("@@") {
            accent(ui)
        } else {
            normal
        };
        job.append(line, 0.0, egui::TextFormat::simple(font.clone(), color));
        job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), color));
    }
    code_frame(ui, |ui| {
        ui.add(egui::Label::new(job).selectable(true));
        if total > max_lines {
            ui.label(
                RichText::new(format!("… {} weitere Zeilen", total - max_lines))
                    .small()
                    .weak(),
            );
        }
    });
}

fn output_label(ui: &mut egui::Ui, text: &str, max_lines: usize) {
    let total = text.lines().count();
    let shown: Vec<&str> = text.lines().take(max_lines).collect();
    code_frame(ui, |ui| {
        ui.add(
            egui::Label::new(RichText::new(shown.join("\n")).monospace().size(12.5))
                .selectable(true),
        );
        if total > max_lines {
            ui.label(
                RichText::new(format!(
                    "… {} weitere Zeilen (mit „Ausgabe kopieren“ alles holen)",
                    total - max_lines
                ))
                .small()
                .weak(),
            );
        }
    });
}

/// Shows attachments as a row of thumbnails. Returns the index the user
/// asked to remove, if `removable`.
pub fn attachment_row(ui: &mut egui::Ui, items: &[Attachment], removable: bool) -> Option<usize> {
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

pub fn drop_overlay(ctx: &egui::Context, rect: egui::Rect) {
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
        "Bilder oder Videos anhängen – oder einen Ordner als Projekt öffnen",
        egui::FontId::proportional(22.0),
        Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ToolCall;

    fn view(name: &str, args: &str, state: ToolState) -> ToolView {
        ToolView {
            call: ToolCall {
                id: "c".into(),
                name: name.into(),
                arguments: args.into(),
            },
            state,
            output: String::new(),
            ok: true,
            change: None,
            preview: None,
            warning: None,
            diff: String::new(),
            stats: None,
        }
    }

    #[test]
    fn tool_titles_are_readable() {
        assert_eq!(
            tool_title(&view(
                "read_file",
                r#"{"path":"src/main.rs"}"#,
                ToolState::Done
            )),
            "Gelesen: src/main.rs"
        );
        assert_eq!(
            tool_title(&view(
                "read_file",
                r#"{"path":"a.rs","offset":401}"#,
                ToolState::Done
            )),
            "Gelesen: a.rs (ab Zeile 401)"
        );
        assert_eq!(
            tool_title(&view("list_dir", "{}", ToolState::Done)),
            "Ordner angesehen: Projektordner"
        );
        assert_eq!(
            tool_title(&view(
                "search",
                r#"{"pattern":"fn main","path":"src","glob":"*.rs"}"#,
                ToolState::Done
            )),
            "Gesucht: „fn main“ in src (*.rs)"
        );
        assert_eq!(
            tool_title(&view(
                "edit_file",
                r#"{"path":"a.rs"}"#,
                ToolState::AwaitingApproval
            )),
            "Möchte bearbeiten: a.rs"
        );
        assert_eq!(
            tool_title(&view("edit_file", r#"{"path":"a.rs"}"#, ToolState::Done)),
            "Bearbeitet: a.rs"
        );
        assert_eq!(
            tool_title(&view(
                "run_command",
                r#"{"command":"cargo test"}"#,
                ToolState::Done
            )),
            "Befehl: cargo test"
        );
        assert_eq!(
            tool_title(&view("rm_rf", "kaputt", ToolState::Done)),
            "Werkzeug „rm_rf“"
        );
    }
}
