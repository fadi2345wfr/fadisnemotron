//! Client for NVIDIA's OpenAI-compatible chat completions API.
//!
//! Requests are streamed (Server-Sent Events). Nemotron sends its thinking in
//! `reasoning_content` (some servers use `reasoning`) and the answer in `content`.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::attachments::Attachment;

pub const DEFAULT_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const DEFAULT_MODEL: &str = "nvidia/nemotron-3.5-super-vl-120b-a12b";

/// Error bodies are short; anything larger is cut off.
const MAX_ERROR_BODY: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One turn of the conversation as it is sent to the API.
#[derive(Clone, Debug)]
pub struct Turn<'a> {
    pub role: Role,
    pub text: &'a str,
    pub attachments: &'a [Attachment],
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerationOptions {
    pub model: String,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: u32,
    pub thinking: bool,
    /// 0 means "no budget"; the field is then left out of the request.
    pub reasoning_budget: u32,
}

/// Builds the JSON body for `POST /chat/completions`.
pub fn build_request_body(
    system_prompt: &str,
    turns: &[Turn<'_>],
    opts: &GenerationOptions,
) -> Value {
    let mut messages = Vec::with_capacity(turns.len() + 1);
    if !system_prompt.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": system_prompt }));
    }
    for turn in turns {
        messages.push(match turn.role {
            Role::Assistant => json!({ "role": "assistant", "content": turn.text }),
            Role::User if turn.attachments.is_empty() => {
                json!({ "role": "user", "content": turn.text })
            }
            Role::User => {
                let mut parts = Vec::with_capacity(turn.attachments.len() + 1);
                if !turn.text.is_empty() {
                    parts.push(json!({ "type": "text", "text": turn.text }));
                }
                parts.extend(turn.attachments.iter().map(Attachment::to_content_part));
                json!({ "role": "user", "content": parts })
            }
        });
    }

    let mut body = json!({
        "model": opts.model,
        "messages": messages,
        "temperature": opts.temperature,
        "top_p": opts.top_p,
        "max_tokens": opts.max_tokens,
        "stream": true,
        "chat_template_kwargs": { "enable_thinking": opts.thinking },
    });
    if opts.thinking && opts.reasoning_budget > 0 {
        body["reasoning_budget"] = json!(opts.reasoning_budget);
    }
    body
}

/// Joins the base URL and an endpoint path without doubling slashes.
pub fn endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Reasoning(String),
    Content(String),
    /// The stream ended normally. Carries the `finish_reason` if the server sent one.
    Done(Option<String>),
    Error(String),
}

/// What one SSE `data:` payload contained.
#[derive(Debug, Default, PartialEq)]
pub struct ChunkDelta {
    pub reasoning: Option<String>,
    pub content: Option<String>,
    pub finish_reason: Option<String>,
    pub error: Option<String>,
    pub done: bool,
}

/// Parses a single SSE line. Returns `None` for lines that carry no data
/// (comments, `event:` lines, blank separators).
pub fn parse_sse_line(line: &str) -> Option<ChunkDelta> {
    let line = line.trim_end_matches(['\r', '\n']);
    let data = line.strip_prefix("data:")?.trim_start();
    if data.is_empty() {
        return None;
    }
    if data == "[DONE]" {
        return Some(ChunkDelta {
            done: true,
            ..Default::default()
        });
    }
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            return Some(ChunkDelta {
                error: Some(format!(
                    "Unlesbare Antwort vom Server ({e}): {}",
                    truncate(data, 200)
                )),
                ..Default::default()
            });
        }
    };
    if let Some(msg) = error_message(&value) {
        return Some(ChunkDelta {
            error: Some(msg),
            ..Default::default()
        });
    }

    let mut delta = ChunkDelta::default();
    if let Some(choice) = value.get("choices").and_then(|c| c.get(0)) {
        let d = choice.get("delta").or_else(|| choice.get("message"));
        if let Some(d) = d {
            delta.reasoning = ["reasoning_content", "reasoning"]
                .iter()
                .find_map(|k| d.get(*k).and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            delta.content = d
                .get("content")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
        }
        delta.finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    Some(delta)
}

/// Pulls a human readable message out of the error shapes NVIDIA and
/// OpenAI-compatible servers use.
pub fn error_message(value: &Value) -> Option<String> {
    if let Some(err) = value.get("error") {
        return Some(match err {
            Value::String(s) => s.clone(),
            other => other
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| other.to_string()),
        });
    }
    if value.get("object").and_then(Value::as_str) == Some("error") {
        return value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    // NVCF style: {"status":401,"title":"Unauthorized","detail":"..."}
    if value.get("choices").is_none() {
        if let Some(detail) = value.get("detail") {
            return Some(match detail {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
        if let Some(title) = value.get("title").and_then(Value::as_str) {
            return Some(title.to_owned());
        }
    }
    None
}

/// Turns an HTTP error status and body into a message for the user.
pub fn describe_http_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| error_message(&v))
        .unwrap_or_else(|| truncate(body.trim(), 300).to_owned());
    let hint = match status {
        401 | 403 => "API-Schlüssel fehlt, ist ungültig oder hat keinen Zugriff auf dieses Modell.",
        404 => {
            "Modell oder Adresse nicht gefunden. Prüfe Modell-ID und Basis-URL in den Einstellungen."
        }
        413 => "Die Anfrage ist zu groß. Verkleinere Bilder/Videos oder gib das Video als URL an.",
        422 | 400 => "Der Server hat die Anfrage abgelehnt.",
        429 => "Zu viele Anfragen. Kurz warten und erneut versuchen.",
        500..=599 => "Fehler auf dem Server. Später erneut versuchen.",
        _ => "Anfrage fehlgeschlagen.",
    };
    if detail.is_empty() {
        format!("HTTP {status}: {hint}")
    } else {
        format!("HTTP {status}: {hint}\n{detail}")
    }
}

/// Nemotron's thinking normally arrives in its own field. Servers without a
/// reasoning parser put it inline as `<think>…</think>`; this splits it off.
/// Returns `(thinking, answer)`.
pub fn split_inline_thinking(content: &str) -> (Option<&str>, &str) {
    if let Some(end) = content.find("</think>") {
        let thinking = content[..end].trim_start().trim_start_matches("<think>");
        let answer = content[end + "</think>".len()..].trim_start();
        let thinking = thinking.trim();
        (
            if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            answer,
        )
    } else if let Some(rest) = content.trim_start().strip_prefix("<think>") {
        // Still thinking, the closing tag has not arrived yet.
        (Some(rest.trim()), "")
    } else {
        (None, content)
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        // Thinking can take minutes before the first answer token, so only
        // guard against a silent connection, not a slow answer.
        .timeout_recv_response(Some(Duration::from_secs(300)))
        .timeout_recv_body(None)
        .build()
        .into()
}

/// Sends a chat request and streams the answer through `emit`.
///
/// Stops early when `cancel` is set or `emit` returns `false` (the receiver
/// is gone). Always finishes with exactly one `Done` or `Error` event unless
/// it was cancelled.
pub fn stream_chat(
    agent: &ureq::Agent,
    base_url: &str,
    api_key: &str,
    body: &Value,
    cancel: &AtomicBool,
    mut emit: impl FnMut(StreamEvent) -> bool,
) {
    let url = endpoint(base_url, "chat/completions");
    let mut request = agent
        .post(&url)
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json");
    if !api_key.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let response = match request.send(body.to_string()) {
        Ok(r) => r,
        Err(e) => {
            emit(StreamEvent::Error(format!(
                "Verbindung zu {url} fehlgeschlagen: {e}"
            )));
            return;
        }
    };

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let text = response
            .into_body()
            .with_config()
            .limit(MAX_ERROR_BODY)
            .lossy_utf8(true)
            .read_to_string()
            .unwrap_or_default();
        emit(StreamEvent::Error(describe_http_error(status, &text)));
        return;
    }

    let is_event_stream = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));
    let reader = BufReader::new(response.into_body().into_reader());

    if !is_event_stream {
        // Some servers ignore `stream: true` and answer in one JSON object.
        read_whole_response(reader, emit);
        return;
    }

    let mut finish_reason = None;
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                emit(StreamEvent::Error(format!(
                    "Verbindung während der Antwort abgebrochen: {e}"
                )));
                return;
            }
        };
        let Some(delta) = parse_sse_line(&line) else {
            continue;
        };
        if let Some(err) = delta.error {
            emit(StreamEvent::Error(err));
            return;
        }
        if delta.done {
            break;
        }
        if let Some(r) = delta.reasoning
            && !emit(StreamEvent::Reasoning(r))
        {
            return;
        }
        if let Some(c) = delta.content
            && !emit(StreamEvent::Content(c))
        {
            return;
        }
        if delta.finish_reason.is_some() {
            finish_reason = delta.finish_reason;
        }
    }
    if !cancel.load(Ordering::Relaxed) {
        emit(StreamEvent::Done(finish_reason));
    }
}

fn read_whole_response(reader: impl std::io::Read, mut emit: impl FnMut(StreamEvent) -> bool) {
    let value: Value = match serde_json::from_reader(reader) {
        Ok(v) => v,
        Err(e) => {
            emit(StreamEvent::Error(format!(
                "Unlesbare Antwort vom Server: {e}"
            )));
            return;
        }
    };
    let Some(delta) = parse_sse_line(&format!("data: {value}")) else {
        emit(StreamEvent::Done(None));
        return;
    };
    if let Some(err) = delta.error {
        emit(StreamEvent::Error(err));
        return;
    }
    if let Some(r) = delta.reasoning {
        emit(StreamEvent::Reasoning(r));
    }
    if let Some(c) = delta.content {
        emit(StreamEvent::Content(c));
    }
    emit(StreamEvent::Done(delta.finish_reason));
}

/// Checks the key and base URL by listing the server's models, and reports
/// whether `model` is among them.
pub fn check_connection(
    agent: &ureq::Agent,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<String, String> {
    let url = endpoint(base_url, "models");
    let mut request = agent.get(&url);
    if !api_key.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", api_key.trim()));
    }
    let response = request
        .call()
        .map_err(|e| format!("Verbindung zu {url} fehlgeschlagen: {e}"))?;
    let status = response.status().as_u16();
    let text = response
        .into_body()
        .with_config()
        .limit(16 * 1024 * 1024)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|e| format!("Antwort konnte nicht gelesen werden: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(describe_http_error(status, &text));
    }
    let ids: Vec<String> = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("data").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    if ids.iter().any(|id| id == model) {
        Ok(format!("Verbindung ok. Modell „{model}“ ist verfügbar."))
    } else if ids.is_empty() {
        Ok("Verbindung ok. Der Server hat keine Modellliste geliefert.".to_owned())
    } else {
        Ok(format!(
            "Verbindung ok, aber „{model}“ steht nicht in der Modellliste des Servers ({} Modelle). \
             Prüfe die Modell-ID oder ob dein Konto Zugriff hat.",
            ids.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::{Attachment, AttachmentKind};

    fn opts(thinking: bool) -> GenerationOptions {
        GenerationOptions {
            model: DEFAULT_MODEL.to_owned(),
            temperature: 1.0,
            top_p: 0.95,
            max_tokens: 4096,
            thinking,
            reasoning_budget: 0,
        }
    }

    #[test]
    fn body_has_model_sampling_and_thinking_flag() {
        let turns = [Turn {
            role: Role::User,
            text: "Hallo",
            attachments: &[],
        }];
        let body = build_request_body("Sei hilfreich.", &turns, &opts(false));
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert!(body.get("reasoning_budget").is_none());
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "Sei hilfreich."})
        );
        assert_eq!(
            body["messages"][1],
            json!({"role": "user", "content": "Hallo"})
        );
    }

    #[test]
    fn empty_system_prompt_is_left_out_and_budget_only_with_thinking() {
        let turns = [Turn {
            role: Role::User,
            text: "x",
            attachments: &[],
        }];
        let mut o = opts(true);
        o.reasoning_budget = 2048;
        let body = build_request_body("  ", &turns, &o);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["reasoning_budget"], 2048);

        o.thinking = false;
        let body = build_request_body("", &turns, &o);
        assert!(body.get("reasoning_budget").is_none());
    }

    #[test]
    fn attachments_become_content_parts() {
        let atts = [
            Attachment::from_bytes("a.png", AttachmentKind::Image, "image/png", vec![1, 2, 3]),
            Attachment::from_url("https://example.com/v.mp4", AttachmentKind::Video),
        ];
        let turns = [
            Turn {
                role: Role::User,
                text: "Was siehst du?",
                attachments: &atts,
            },
            Turn {
                role: Role::Assistant,
                text: "Eine Katze.",
                attachments: &[],
            },
        ];
        let body = build_request_body("", &turns, &opts(true));
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0], json!({"type": "text", "text": "Was siehst du?"}));
        assert_eq!(
            parts[1],
            json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,AQID"}})
        );
        assert_eq!(
            parts[2],
            json!({"type": "video_url", "video_url": {"url": "https://example.com/v.mp4"}})
        );
        assert_eq!(
            body["messages"][1],
            json!({"role": "assistant", "content": "Eine Katze."})
        );
    }

    #[test]
    fn attachment_without_text_sends_only_media() {
        let atts = [Attachment::from_url(
            "https://example.com/i.jpg",
            AttachmentKind::Image,
        )];
        let turns = [Turn {
            role: Role::User,
            text: "",
            attachments: &atts,
        }];
        let body = build_request_body("", &turns, &opts(true));
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn endpoint_joins_cleanly() {
        assert_eq!(
            endpoint("https://integrate.api.nvidia.com/v1/", "/chat/completions"),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint(" http://localhost:8000/v1 ", "models"),
            "http://localhost:8000/v1/models"
        );
    }

    #[test]
    fn parses_reasoning_and_content_deltas() {
        let r = parse_sse_line(r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":"Hm"},"finish_reason":null}]}"#).unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("Hm"));
        assert_eq!(r.content, None);

        let r =
            parse_sse_line(r#"data:{"choices":[{"delta":{"reasoning":"vLLM-Feld"}}]}"#).unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("vLLM-Feld"));

        let c = parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Hallo","reasoning_content":null},"finish_reason":"stop"}]}"#).unwrap();
        assert_eq!(c.content.as_deref(), Some("Hallo"));
        assert_eq!(c.reasoning, None);
        assert_eq!(c.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn ignores_non_data_lines_and_detects_done() {
        assert_eq!(parse_sse_line(""), None);
        assert_eq!(parse_sse_line(": keep-alive"), None);
        assert_eq!(parse_sse_line("event: message"), None);
        assert!(parse_sse_line("data: [DONE]\r").unwrap().done);
    }

    #[test]
    fn reports_errors_in_stream_and_bodies() {
        let e =
            parse_sse_line(r#"data: {"error":{"message":"Kontext zu lang","code":400}}"#).unwrap();
        assert_eq!(e.error.as_deref(), Some("Kontext zu lang"));
        let e = parse_sse_line(r#"data: {"object":"error","message":"kaputt"}"#).unwrap();
        assert_eq!(e.error.as_deref(), Some("kaputt"));
        assert!(parse_sse_line("data: {nicht json").unwrap().error.is_some());

        let msg = describe_http_error(
            401,
            r#"{"status":401,"title":"Unauthorized","detail":"Authentication failed"}"#,
        );
        assert!(msg.starts_with("HTTP 401"));
        assert!(msg.contains("Authentication failed"));
        let msg = describe_http_error(500, "upstream exploded");
        assert!(msg.contains("upstream exploded"));
    }

    #[test]
    fn splits_inline_thinking() {
        assert_eq!(
            split_inline_thinking("<think>a\nb</think>\n\nAntwort"),
            (Some("a\nb"), "Antwort")
        );
        assert_eq!(
            split_inline_thinking("<think>noch dabei"),
            (Some("noch dabei"), "")
        );
        assert_eq!(
            split_inline_thinking("<think></think>Nur Antwort"),
            (None, "Nur Antwort")
        );
        assert_eq!(
            split_inline_thinking("Normale Antwort"),
            (None, "Normale Antwort")
        );
    }

    /// Serves exactly one HTTP response on a local port and hands back the raw request.
    fn mock_server(
        status: &str,
        content_type: &str,
        body: &str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (base_url, handle)
    }

    fn collect(base_url: &str, key: &str) -> Vec<StreamEvent> {
        let turns = [Turn {
            role: Role::User,
            text: "Hallo",
            attachments: &[],
        }];
        let body = build_request_body("", &turns, &opts(true));
        let mut events = Vec::new();
        stream_chat(
            &agent(),
            base_url,
            key,
            &body,
            &AtomicBool::new(false),
            |e| {
                events.push(e);
                true
            },
        );
        events
    }

    #[test]
    fn streams_thinking_then_answer_from_server() {
        let sse = concat!(
            ": ping\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null,\"reasoning_content\":\"Der Nutzer \"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"grüßt.\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hallo! \"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Wie kann ich helfen?\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (base_url, server) = mock_server("200 OK", "text/event-stream", sse);
        let events = collect(&base_url, "nvapi-test");
        assert_eq!(
            events,
            vec![
                StreamEvent::Reasoning("Der Nutzer ".into()),
                StreamEvent::Reasoning("grüßt.".into()),
                StreamEvent::Content("Hallo! ".into()),
                StreamEvent::Content("Wie kann ich helfen?".into()),
                StreamEvent::Done(Some("stop".into())),
            ]
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer nvapi-test")
        );
        assert!(request.contains(r#""model":"nvidia/nemotron-3.5-super-vl-120b-a12b""#));
        assert!(request.contains(r#""enable_thinking":true"#));
    }

    #[test]
    fn http_errors_become_readable_messages() {
        let (base_url, server) = mock_server(
            "401 Unauthorized",
            "application/json",
            r#"{"status":401,"title":"Unauthorized","detail":"Authentication failed"}"#,
        );
        let events = collect(&base_url, "falsch");
        server.join().unwrap();
        assert_eq!(events.len(), 1);
        let StreamEvent::Error(msg) = &events[0] else {
            panic!("{events:?}")
        };
        assert!(
            msg.contains("HTTP 401") && msg.contains("Authentication failed"),
            "{msg}"
        );
    }

    #[test]
    fn non_streaming_json_answer_is_accepted() {
        let (base_url, server) = mock_server(
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"role":"assistant","content":"Fertig.","reasoning_content":"kurz"},"finish_reason":"stop"}]}"#,
        );
        let events = collect(&base_url, "");
        let request = server.join().unwrap();
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert_eq!(
            events,
            vec![
                StreamEvent::Reasoning("kurz".into()),
                StreamEvent::Content("Fertig.".into()),
                StreamEvent::Done(Some("stop".into())),
            ]
        );
    }

    #[test]
    fn unreachable_server_reports_connection_error() {
        // Port 9 (discard) on localhost is closed in test environments.
        let events = collect("http://127.0.0.1:9/v1", "x");
        assert!(
            matches!(&events[..], [StreamEvent::Error(m)] if m.contains("Verbindung")),
            "{events:?}"
        );
    }

    #[test]
    fn connection_check_finds_model_in_list() {
        let (base_url, server) = mock_server(
            "200 OK",
            "application/json",
            r#"{"object":"list","data":[{"id":"meta/llama"},{"id":"nvidia/nemotron-3.5-super-vl-120b-a12b"}]}"#,
        );
        let result = check_connection(&agent(), &base_url, "k", DEFAULT_MODEL);
        assert!(server.join().unwrap().starts_with("GET /v1/models"));
        assert!(result.unwrap().contains("ist verfügbar"));
    }
}
