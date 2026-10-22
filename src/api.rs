//! Client for NVIDIA's OpenAI-compatible chat completions API.
//!
//! Requests are streamed (Server-Sent Events). Nemotron sends its thinking in
//! `reasoning_content` (vLLM/NIM use `reasoning`), the answer in `content`,
//! and tool calls in `tool_calls` fragments that have to be stitched together.

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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON object as a string, exactly as the model produced it.
    pub arguments: String,
}

/// One message of the conversation as it is sent to the API.
#[derive(Clone, Debug)]
pub enum ChatMessage {
    User {
        text: String,
        attachments: Vec<Attachment>,
    },
    Assistant {
        content: String,
        /// Sent back as `reasoning_content` within a tool loop; the chat
        /// template drops it for earlier turns anyway.
        reasoning: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        content: String,
    },
}

impl ChatMessage {
    fn to_json(&self) -> Value {
        match self {
            Self::User { text, attachments } if attachments.is_empty() => {
                json!({ "role": "user", "content": text })
            }
            Self::User { text, attachments } => {
                let mut parts = Vec::with_capacity(attachments.len() + 1);
                if !text.is_empty() {
                    parts.push(json!({ "type": "text", "text": text }));
                }
                parts.extend(attachments.iter().map(Attachment::to_content_part));
                json!({ "role": "user", "content": parts })
            }
            Self::Assistant {
                content,
                reasoning,
                tool_calls,
            } => {
                let mut msg = json!({ "role": "assistant", "content": content });
                if let Some(r) = reasoning
                    .as_deref()
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                {
                    msg["reasoning_content"] = json!(r);
                }
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = tool_calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": { "name": c.name, "arguments": c.arguments },
                            })
                        })
                        .collect();
                }
                msg
            }
            Self::Tool { call_id, content } => {
                json!({ "role": "tool", "tool_call_id": call_id, "content": content })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerationOptions {
    pub model: String,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: u32,
    pub thinking: bool,
}

/// Builds the JSON body for `POST /chat/completions`.
///
/// `tools` is the OpenAI tools array; `None` sends a plain chat request
/// (an empty array is rejected by vLLM).
pub fn build_request_body(
    system_prompt: &str,
    messages: &[ChatMessage],
    opts: &GenerationOptions,
    tools: Option<&Value>,
    tool_choice: Option<&str>,
) -> Value {
    let mut all = Vec::with_capacity(messages.len() + 1);
    if !system_prompt.trim().is_empty() {
        all.push(json!({ "role": "system", "content": system_prompt }));
    }
    all.extend(messages.iter().map(ChatMessage::to_json));

    let mut kwargs = json!({ "enable_thinking": opts.thinking });
    let mut body = json!({
        "model": opts.model,
        "messages": all,
        "temperature": opts.temperature,
        "top_p": opts.top_p,
        "max_tokens": opts.max_tokens,
        "stream": true,
    });
    if let Some(tools) = tools.filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        body["tools"] = tools.clone();
        body["tool_choice"] = json!(tool_choice.unwrap_or("auto"));
        if opts.thinking {
            // NVIDIA's cookbook: keeps the answer from ending up empty when
            // the model calls a tool straight out of its thinking.
            kwargs["force_nonempty_content"] = json!(true);
        }
    }
    body["chat_template_kwargs"] = kwargs;
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

/// A piece of a tool call as it arrives in the stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// What one SSE `data:` payload contained.
#[derive(Debug, Default, PartialEq)]
pub struct ChunkDelta {
    pub reasoning: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCallDelta>,
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
    Some(parse_chunk(&value))
}

fn parse_chunk(value: &Value) -> ChunkDelta {
    if let Some(msg) = error_message(value) {
        return ChunkDelta {
            error: Some(msg),
            ..Default::default()
        };
    }
    let mut delta = ChunkDelta::default();
    let Some(choice) = value.get("choices").and_then(|c| c.get(0)) else {
        // e.g. the trailing usage chunk with "choices": []
        return delta;
    };
    if let Some(d) = choice.get("delta").or_else(|| choice.get("message")) {
        // Some servers send both fields; take whichever has text.
        delta.reasoning = ["reasoning_content", "reasoning"]
            .iter()
            .find_map(|k| d.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
            .map(str::to_owned);
        delta.content = d
            .get("content")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if let Some(calls) = d.get("tool_calls").and_then(Value::as_array) {
            delta.tool_calls = calls
                .iter()
                .enumerate()
                .map(|(pos, c)| {
                    let function = c.get("function");
                    ToolCallDelta {
                        index: c
                            .get("index")
                            .and_then(Value::as_u64)
                            .map_or(pos, |i| i as usize),
                        id: c
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                        name: function
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                        arguments: function.and_then(|f| f.get("arguments")).and_then(
                            |a| match a {
                                Value::String(s) => Some(s.clone()),
                                Value::Null => None,
                                // Not OpenAI-conformant, but some servers send an object.
                                other => Some(other.to_string()),
                            },
                        ),
                    }
                })
                .collect();
        }
    }
    delta.finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    delta
}

/// Stitches streamed tool-call fragments together by their `index`.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<ToolCall>,
}

impl ToolCallAccumulator {
    pub fn push(&mut self, delta: ToolCallDelta) {
        if self.calls.len() <= delta.index {
            self.calls.resize_with(delta.index + 1, ToolCall::default);
        }
        let call = &mut self.calls[delta.index];
        if let Some(id) = delta.id.filter(|_| call.id.is_empty()) {
            call.id = id;
        }
        if let Some(name) = delta.name.filter(|_| call.name.is_empty()) {
            call.name = name;
        }
        if let Some(args) = delta.arguments {
            call.arguments.push_str(&args);
        }
    }

    pub fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .enumerate()
            .map(|(i, mut c)| {
                if c.id.is_empty() {
                    c.id = format!("call_{i}");
                }
                if c.arguments.trim().is_empty() {
                    c.arguments = "{}".to_owned();
                }
                c
            })
            .collect()
    }
}

/// Pulls a human readable message out of the error shapes NVIDIA and
/// OpenAI-compatible servers use.
pub fn error_message(value: &Value) -> Option<String> {
    if let Some(err) = value.get("error").filter(|e| !e.is_null()) {
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
        let thinking = content[..end]
            .trim_start()
            .trim_start_matches("<think>")
            .trim();
        let answer = content[end + "</think>".len()..].trim_start();
        ((!thinking.is_empty()).then_some(thinking), answer)
    } else if let Some(rest) = content.trim_start().strip_prefix("<think>") {
        // Still thinking, the closing tag has not arrived yet.
        (Some(rest.trim()), "")
    } else {
        (None, content)
    }
}

/// True if the text looks like a tool call the server did not convert
/// (the check NVIDIA's own agent uses).
pub fn looks_like_tool_call_text(content: &str) -> bool {
    content.contains("<tool_call")
        || content.contains("<function=")
        || content.contains("<parameter=")
        || content.contains("<invoke")
}

/// Parses Nemotron's tool-call XML when it ends up in `content`:
///
/// ```text
/// <tool_call>
/// <function=read_file>
/// <parameter=path>
/// src/main.rs
/// </parameter>
/// </function>
/// </tool_call>
/// ```
///
/// Parameter values are converted to the types declared in `tools`.
/// Returns the text before the first call and the calls, or `None`.
pub fn parse_xml_tool_calls(content: &str, tools: &Value) -> Option<(String, Vec<ToolCall>)> {
    let first = content.find("<function=")?;
    let prefix_end = content[..first].rfind("<tool_call>").unwrap_or(first);
    let mut calls = Vec::new();
    let mut rest = &content[first..];
    while let Some(start) = rest.find("<function=") {
        rest = &rest[start + "<function=".len()..];
        let name_end = rest.find('>')?;
        let name = rest[..name_end].trim().to_owned();
        rest = &rest[name_end + 1..];
        let body_end = rest.find("</function>").unwrap_or(rest.len());
        let body = &rest[..body_end];
        let mut args = serde_json::Map::new();
        let mut params = body;
        while let Some(p) = params.find("<parameter=") {
            params = &params[p + "<parameter=".len()..];
            let Some(key_end) = params.find('>') else {
                break;
            };
            let key = params[..key_end].trim().to_owned();
            params = &params[key_end + 1..];
            let value_end = params.find("</parameter>").unwrap_or(params.len());
            let raw = &params[..value_end];
            // The template wraps values in one newline on each side.
            let raw = raw.strip_prefix('\n').unwrap_or(raw);
            let raw = raw.strip_suffix('\n').unwrap_or(raw);
            args.insert(key.clone(), typed_value(tools, &name, &key, raw));
            params = &params[(value_end + "</parameter>".len()).min(params.len())..];
        }
        if !name.is_empty() {
            calls.push(ToolCall {
                id: format!("call_xml_{}", calls.len()),
                name,
                arguments: Value::Object(args).to_string(),
            });
        }
        rest = &rest[(body_end + "</function>".len()).min(rest.len())..];
    }
    if calls.is_empty() {
        return None;
    }
    Some((content[..prefix_end].trim().to_owned(), calls))
}

fn typed_value(tools: &Value, function: &str, param: &str, raw: &str) -> Value {
    let declared = tools
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["function"]["name"] == function)
        .and_then(|t| t["function"]["parameters"]["properties"][param]["type"].as_str());
    let trimmed = raw.trim();
    match declared {
        Some("integer") => trimmed
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(raw)),
        Some("number") => trimmed
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(raw)),
        Some("boolean") => match trimmed.to_ascii_lowercase().as_str() {
            "true" => json!(true),
            "false" => json!(false),
            _ => json!(raw),
        },
        Some("array" | "object") => serde_json::from_str(trimmed).unwrap_or_else(|_| json!(raw)),
        _ => json!(raw),
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

/// Live text while a step streams.
#[derive(Clone, Debug, PartialEq)]
pub enum TextDelta {
    Reasoning(String),
    Content(String),
}

/// Everything one model step produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepResult {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

impl StepResult {
    /// Fixes up quirks once the stream is complete: inline `<think>` tags,
    /// and the copy of the thinking that `force_nonempty_content` appends as
    /// content when the model never closed its thinking.
    fn finalize(mut self) -> Self {
        if self.reasoning.trim().is_empty() {
            let (thinking, answer) = split_inline_thinking(&self.content);
            if let Some(thinking) = thinking {
                let (thinking, answer) = (thinking.to_owned(), answer.to_owned());
                self.reasoning = thinking;
                self.content = answer;
            }
        } else if self.content.trim() == self.reasoning.trim() {
            self.content = std::mem::take(&mut self.reasoning);
        } else if self.reasoning.trim().len() > 80
            && let Some(rest) = self.content.strip_suffix(self.reasoning.as_str())
        {
            // Only a long thinking text can be such a copy; short endings may match by chance.
            self.content = rest.to_owned();
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepOutcome {
    Done(StepResult),
    Cancelled,
    Failed(String),
}

/// Sends one request and streams the reply. Text is passed to `on_text` as it
/// arrives; tool calls are collected and returned at the end.
///
/// Returns `Cancelled` when `cancel` is set or `on_text` returns `false`.
pub fn stream_step(
    agent: &ureq::Agent,
    base_url: &str,
    api_key: &str,
    body: &Value,
    cancel: &AtomicBool,
    mut on_text: impl FnMut(TextDelta) -> bool,
) -> StepOutcome {
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
        Err(e) => return StepOutcome::Failed(format!("Verbindung zu {url} fehlgeschlagen: {e}")),
    };
    if cancel.load(Ordering::Relaxed) {
        return StepOutcome::Cancelled;
    }

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let text = response
            .into_body()
            .with_config()
            .limit(MAX_ERROR_BODY)
            .lossy_utf8(true)
            .read_to_string()
            .unwrap_or_default();
        return StepOutcome::Failed(describe_http_error(status, &text));
    }

    let is_event_stream = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));
    let reader = BufReader::new(response.into_body().into_reader());

    let mut result = StepResult::default();
    let mut calls = ToolCallAccumulator::default();
    let mut apply = |delta: ChunkDelta, result: &mut StepResult| -> bool {
        if let Some(r) = delta.reasoning {
            result.reasoning.push_str(&r);
            if !on_text(TextDelta::Reasoning(r)) {
                return false;
            }
        }
        if let Some(c) = delta.content {
            result.content.push_str(&c);
            if !on_text(TextDelta::Content(c)) {
                return false;
            }
        }
        for call in delta.tool_calls {
            calls.push(call);
        }
        if delta.finish_reason.is_some() {
            result.finish_reason = delta.finish_reason;
        }
        true
    };

    if !is_event_stream {
        // Some servers ignore `stream: true` and answer in one JSON object.
        let value: Value = match serde_json::from_reader(reader) {
            Ok(v) => v,
            Err(e) => return StepOutcome::Failed(format!("Unlesbare Antwort vom Server: {e}")),
        };
        let delta = parse_chunk(&value);
        if let Some(err) = delta.error {
            return StepOutcome::Failed(err);
        }
        if !apply(delta, &mut result) {
            return StepOutcome::Cancelled;
        }
    } else {
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                return StepOutcome::Cancelled;
            }
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    return StepOutcome::Failed(format!(
                        "Verbindung während der Antwort abgebrochen: {e}"
                    ));
                }
            };
            let Some(delta) = parse_sse_line(&line) else {
                continue;
            };
            if let Some(err) = delta.error {
                return StepOutcome::Failed(err);
            }
            if delta.done {
                break;
            }
            if !apply(delta, &mut result) {
                return StepOutcome::Cancelled;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return StepOutcome::Cancelled;
        }
    }
    result.tool_calls = calls.finish();
    StepOutcome::Done(result.finalize())
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
pub(crate) mod tests {
    use super::*;
    use crate::attachments::{Attachment, AttachmentKind};

    pub(crate) fn opts(thinking: bool) -> GenerationOptions {
        GenerationOptions {
            model: DEFAULT_MODEL.to_owned(),
            temperature: 1.0,
            top_p: 0.95,
            max_tokens: 4096,
            thinking,
        }
    }

    fn user(text: &str) -> ChatMessage {
        ChatMessage::User {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn body_has_model_sampling_and_thinking_flag() {
        let body = build_request_body("Sei hilfreich.", &[user("Hallo")], &opts(false), None, None);
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        assert!(body.get("tools").is_none() && body.get("tool_choice").is_none());
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "Sei hilfreich."})
        );
        assert_eq!(
            body["messages"][1],
            json!({"role": "user", "content": "Hallo"})
        );

        let body = build_request_body("  ", &[user("x")], &opts(true), None, None);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tools_are_sent_only_when_present() {
        let tools = json!([{"type": "function", "function": {"name": "read_file", "parameters": {"type": "object"}}}]);
        let body = build_request_body("", &[user("x")], &opts(true), Some(&tools), None);
        assert_eq!(body["tools"], tools);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": true, "force_nonempty_content": true})
        );
        let body = build_request_body(
            "",
            &[user("x")],
            &opts(false),
            Some(&tools),
            Some("required"),
        );
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        // vLLM rejects an empty tools array.
        let body = build_request_body("", &[user("x")], &opts(true), Some(&json!([])), None);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn tool_loop_messages_have_openai_shape() {
        let call = ToolCall {
            id: "chatcmpl-tool-1".into(),
            name: "read_file".into(),
            arguments: r#"{"path": "a.rs"}"#.into(),
        };
        let messages = [
            user("Lies a.rs"),
            ChatMessage::Assistant {
                content: String::new(),
                reasoning: Some("\nIch lese.".into()),
                tool_calls: vec![call],
            },
            ChatMessage::Tool {
                call_id: "chatcmpl-tool-1".into(),
                content: "1\tfn a() {}".into(),
            },
            ChatMessage::Assistant {
                content: "Fertig.".into(),
                reasoning: Some("  ".into()),
                tool_calls: vec![],
            },
        ];
        let body = build_request_body("", &messages, &opts(true), None, None);
        let m = body["messages"].as_array().unwrap();
        assert_eq!(
            m[1],
            json!({
                "role": "assistant", "content": "", "reasoning_content": "Ich lese.",
                "tool_calls": [{"id": "chatcmpl-tool-1", "type": "function",
                                "function": {"name": "read_file", "arguments": "{\"path\": \"a.rs\"}"}}]
            })
        );
        assert_eq!(
            m[2],
            json!({"role": "tool", "tool_call_id": "chatcmpl-tool-1", "content": "1\tfn a() {}"})
        );
        assert_eq!(m[3], json!({"role": "assistant", "content": "Fertig."}));
    }

    #[test]
    fn attachments_become_content_parts() {
        let atts = vec![
            Attachment::from_bytes("a.png", AttachmentKind::Image, "image/png", vec![1, 2, 3]),
            Attachment::from_url("https://example.com/v.mp4", AttachmentKind::Video),
        ];
        let messages = [ChatMessage::User {
            text: "Was siehst du?".into(),
            attachments: atts,
        }];
        let body = build_request_body("", &messages, &opts(true), None, None);
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

        let only_media = [ChatMessage::User {
            text: String::new(),
            attachments: vec![Attachment::from_url(
                "https://example.com/i.jpg",
                AttachmentKind::Image,
            )],
        }];
        let body = build_request_body("", &only_media, &opts(true), None, None);
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
        let r = parse_sse_line(
            r#"data:{"choices":[{"delta":{"reasoning_content":"","reasoning":"zweites"}}]}"#,
        )
        .unwrap();
        assert_eq!(r.reasoning.as_deref(), Some("zweites"));

        let c = parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Hallo","reasoning_content":null},"finish_reason":"stop"}]}"#).unwrap();
        assert_eq!(c.content.as_deref(), Some("Hallo"));
        assert_eq!(c.reasoning, None);
        assert_eq!(c.finish_reason.as_deref(), Some("stop"));

        // Trailing usage chunk.
        let u = parse_sse_line(r#"data: {"choices":[],"usage":{"prompt_tokens":5}}"#).unwrap();
        assert_eq!(u, ChunkDelta::default());
    }

    #[test]
    fn tool_call_fragments_are_stitched_by_index() {
        let lines = [
            r#"data: {"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"chatcmpl-tool-a","type":"function","function":{"name":"read_file"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\": \""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"src/main.rs\"}"}},{"index":1,"id":"chatcmpl-tool-b","type":"function","function":{"name":"list_dir","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let mut acc = ToolCallAccumulator::default();
        let mut finish = None;
        for line in lines {
            let d = parse_sse_line(line).unwrap();
            for c in d.tool_calls {
                acc.push(c);
            }
            finish = d.finish_reason.or(finish);
        }
        assert_eq!(finish.as_deref(), Some("tool_calls"));
        assert_eq!(
            acc.finish(),
            vec![
                ToolCall {
                    id: "chatcmpl-tool-a".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path": "src/main.rs"}"#.into()
                },
                ToolCall {
                    id: "chatcmpl-tool-b".into(),
                    name: "list_dir".into(),
                    arguments: "{}".into()
                },
            ]
        );
    }

    #[test]
    fn whole_tool_call_in_one_chunk_and_object_arguments() {
        let d = parse_sse_line(r#"data: {"choices":[{"message":{"tool_calls":[{"id":"x","function":{"name":"search","arguments":{"pattern":"main"}}}]}}]}"#).unwrap();
        let mut acc = ToolCallAccumulator::default();
        for c in d.tool_calls {
            acc.push(c);
        }
        let calls = acc.finish();
        assert_eq!(calls[0].name, "search");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].arguments).unwrap(),
            json!({"pattern": "main"})
        );
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
        let ok = parse_sse_line(r#"data: {"error":null,"choices":[{"delta":{"content":"x"}}]}"#)
            .unwrap();
        assert_eq!(ok.error, None);

        let msg = describe_http_error(
            401,
            r#"{"status":401,"title":"Unauthorized","detail":"Authentication failed"}"#,
        );
        assert!(msg.starts_with("HTTP 401"));
        assert!(msg.contains("Authentication failed"));
        assert!(describe_http_error(500, "upstream exploded").contains("upstream exploded"));
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

    #[test]
    fn finalize_fixes_inline_thinking_and_duplicated_reasoning() {
        let r = StepResult {
            content: "<think>plane</think>Antwort".into(),
            ..Default::default()
        }
        .finalize();
        assert_eq!(
            (r.reasoning.as_str(), r.content.as_str()),
            ("plane", "Antwort")
        );

        let r = StepResult {
            reasoning: "nur gedacht".into(),
            content: "nur gedacht".into(),
            ..Default::default()
        }
        .finalize();
        assert_eq!(
            (r.reasoning.as_str(), r.content.as_str()),
            ("", "nur gedacht")
        );

        let long =
            "Ich überlege, welche Datei geändert werden muss und wie der Test danach aussieht."
                .repeat(2);
        let r = StepResult {
            reasoning: long.clone(),
            content: format!("Text {long}"),
            ..Default::default()
        }
        .finalize();
        assert_eq!(r.content, "Text ");
        let r = StepResult {
            reasoning: "OK".into(),
            content: "Alles OK".into(),
            ..Default::default()
        }
        .finalize();
        assert_eq!(r.content, "Alles OK");
    }

    #[test]
    fn xml_tool_calls_in_content_are_recovered() {
        let tools = crate::tools::definitions();
        let content = "Ich schaue nach.\n<tool_call>\n<function=read_file>\n<parameter=path>\nsrc/main.rs\n</parameter>\n<parameter=offset>\n10\n</parameter>\n</function>\n</tool_call>\n<tool_call>\n<function=edit_file>\n<parameter=path>\na.rs\n</parameter>\n<parameter=old_string>\n  let x = 1;\n</parameter>\n<parameter=new_string>\n  let x = 2;\n</parameter>\n<parameter=replace_all>\nfalse\n</parameter>\n</function>\n</tool_call>";
        assert!(looks_like_tool_call_text(content));
        let (prefix, calls) = parse_xml_tool_calls(content, &tools).unwrap();
        assert_eq!(prefix, "Ich schaue nach.");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].arguments).unwrap(),
            json!({"path": "src/main.rs", "offset": 10})
        );
        assert_eq!(
            serde_json::from_str::<Value>(&calls[1].arguments).unwrap(),
            json!({"path": "a.rs", "old_string": "  let x = 1;", "new_string": "  let x = 2;", "replace_all": false})
        );
        assert!(parse_xml_tool_calls("Kein Aufruf hier", &tools).is_none());
        assert!(!looks_like_tool_call_text(
            "Normale Antwort mit <b>HTML</b>"
        ));
    }

    /// Serves the given responses, one per connection, on a local port and
    /// hands back the raw requests.
    pub(crate) fn mock_server(
        responses: Vec<(&'static str, &'static str, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, content_type, body) in responses {
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
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                requests.push(String::from_utf8_lossy(&request).into_owned());
            }
            requests
        });
        (base_url, handle)
    }

    fn step(base_url: &str, key: &str) -> (StepOutcome, Vec<TextDelta>) {
        let body = build_request_body("", &[user("Hallo")], &opts(true), None, None);
        let mut deltas = Vec::new();
        let outcome = stream_step(
            &agent(),
            base_url,
            key,
            &body,
            &AtomicBool::new(false),
            |d| {
                deltas.push(d);
                true
            },
        );
        (outcome, deltas)
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
        let (base_url, server) = mock_server(vec![("200 OK", "text/event-stream", sse.into())]);
        let (outcome, deltas) = step(&base_url, "nvapi-test");
        assert_eq!(
            deltas,
            vec![
                TextDelta::Reasoning("Der Nutzer ".into()),
                TextDelta::Reasoning("grüßt.".into()),
                TextDelta::Content("Hallo! ".into()),
                TextDelta::Content("Wie kann ich helfen?".into()),
            ]
        );
        assert_eq!(
            outcome,
            StepOutcome::Done(StepResult {
                content: "Hallo! Wie kann ich helfen?".into(),
                reasoning: "Der Nutzer grüßt.".into(),
                tool_calls: vec![],
                finish_reason: Some("stop".into()),
            })
        );
        let request = &server.join().unwrap()[0];
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
        let (base_url, server) = mock_server(vec![(
            "401 Unauthorized",
            "application/json",
            r#"{"status":401,"title":"Unauthorized","detail":"Authentication failed"}"#.into(),
        )]);
        let (outcome, _) = step(&base_url, "falsch");
        server.join().unwrap();
        let StepOutcome::Failed(msg) = outcome else {
            panic!("{outcome:?}")
        };
        assert!(
            msg.contains("HTTP 401") && msg.contains("Authentication failed"),
            "{msg}"
        );
    }

    #[test]
    fn non_streaming_json_answer_is_accepted() {
        let (base_url, server) = mock_server(vec![(
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"role":"assistant","content":"Fertig.","reasoning_content":"kurz"},"finish_reason":"stop"}]}"#.into(),
        )]);
        let (outcome, deltas) = step(&base_url, "");
        let request = &server.join().unwrap()[0];
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert_eq!(
            deltas,
            vec![
                TextDelta::Reasoning("kurz".into()),
                TextDelta::Content("Fertig.".into())
            ]
        );
        assert!(matches!(outcome, StepOutcome::Done(r) if r.content == "Fertig."));
    }

    #[test]
    fn unreachable_server_reports_connection_error() {
        // Port 9 (discard) on localhost is closed in test environments.
        let (outcome, _) = step("http://127.0.0.1:9/v1", "x");
        assert!(
            matches!(&outcome, StepOutcome::Failed(m) if m.contains("Verbindung")),
            "{outcome:?}"
        );
    }

    #[test]
    fn connection_check_finds_model_in_list() {
        let (base_url, server) = mock_server(vec![(
            "200 OK",
            "application/json",
            r#"{"object":"list","data":[{"id":"meta/llama"},{"id":"nvidia/nemotron-3.5-super-vl-120b-a12b"}]}"#.into(),
        )]);
        let result = check_connection(&agent(), &base_url, "k", DEFAULT_MODEL);
        assert!(server.join().unwrap()[0].starts_with("GET /v1/models"));
        assert!(result.unwrap().contains("ist verfügbar"));
    }
}
