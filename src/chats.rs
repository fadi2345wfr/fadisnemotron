//! Saved chats: one JSON file per chat in the app's data folder, with its
//! photos and videos in a folder next to it. The Mac shows them in the side
//! bar and the phone gets the same list.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::attachments::{Attachment, AttachmentKind, Source};

pub const UNTITLED: &str = "Neuer Chat";
const TITLE_CHARS: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    /// Unix time in seconds.
    pub updated: u64,
    pub messages: usize,
}

/// A chat as stored: its messages, each with its attachments.
pub struct Chat<M> {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub messages: Vec<(M, Vec<Attachment>)>,
}

/// What the file holds besides the messages; read on its own for the list.
#[derive(Serialize, Deserialize)]
struct Header {
    id: String,
    title: String,
    created: u64,
    updated: u64,
    #[serde(default)]
    message_count: usize,
}

#[derive(Serialize, Deserialize)]
struct SavedAttachment {
    name: String,
    kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
    /// File name in the chat's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

pub struct ChatStore {
    dir: PathBuf,
}

impl ChatStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `~/Library/Application Support/de.fadi.FadisNemotron/chats` on macOS.
    pub fn default_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("de", "fadi", "FadisNemotron")
            .map(|d| d.data_dir().join("chats"))
    }

    pub fn new_id() -> String {
        let mut random = [0u8; 3];
        let _ = getrandom::fill(&mut random);
        let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
        format!("c-{}-{hex}", now())
    }

    /// Ids come from the phone too; only our own format may touch the disk.
    pub fn is_valid_id(id: &str) -> bool {
        id.strip_prefix("c-").is_some_and(|rest| {
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    }

    /// All saved chats, newest first.
    pub fn list(&self) -> Vec<ChatSummary> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut chats: Vec<ChatSummary> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                let header: Header = serde_json::from_str(&text).ok()?;
                Self::is_valid_id(&header.id).then_some(ChatSummary {
                    id: header.id,
                    title: header.title,
                    updated: header.updated,
                    messages: header.message_count,
                })
            })
            .collect();
        chats.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        chats
    }

    pub fn exists(&self, id: &str) -> bool {
        Self::is_valid_id(id) && self.file(id).is_file()
    }

    /// Writes the chat and its attachments; returns its new list entry.
    pub fn save<M: Serialize>(&self, chat: &Chat<M>) -> std::io::Result<ChatSummary> {
        if !Self::is_valid_id(&chat.id) {
            return Err(std::io::Error::other(format!(
                "ungültige Chat-ID {}",
                chat.id
            )));
        }
        let files = self.dir.join(&chat.id);
        let mut messages = Vec::with_capacity(chat.messages.len());
        for (index, (message, attachments)) in chat.messages.iter().enumerate() {
            let mut value = serde_json::to_value(message).map_err(std::io::Error::other)?;
            let mut saved = Vec::with_capacity(attachments.len());
            for (n, att) in attachments.iter().enumerate() {
                saved.push(match &att.source {
                    Source::Url(url) => SavedAttachment {
                        name: att.name.clone(),
                        kind: att.kind,
                        mime: None,
                        file: None,
                        url: Some(url.clone()),
                    },
                    Source::Data { mime, bytes } => {
                        let file = format!("{index}-{n}-{}", safe_file_name(&att.name));
                        let path = files.join(&file);
                        // Attachments never change, so each is written once.
                        if !path.exists() {
                            std::fs::create_dir_all(&files)?;
                            crate::config::write_private(&path, bytes)?;
                        }
                        SavedAttachment {
                            name: att.name.clone(),
                            kind: att.kind,
                            mime: Some(mime.clone()),
                            file: Some(file),
                            url: None,
                        }
                    }
                });
            }
            value["attachments"] = serde_json::to_value(saved).map_err(std::io::Error::other)?;
            messages.push(value);
        }
        let header = Header {
            id: chat.id.clone(),
            title: chat.title.clone(),
            created: chat.created,
            updated: now(),
            message_count: chat.messages.len(),
        };
        let mut body = serde_json::to_value(&header).map_err(std::io::Error::other)?;
        body["version"] = json!(1);
        body["messages"] = Value::Array(messages);
        std::fs::create_dir_all(&self.dir)?;
        let path = self.file(&chat.id);
        let tmp = path.with_extension("json.tmp");
        crate::config::write_private(
            &tmp,
            serde_json::to_string(&body)
                .map_err(std::io::Error::other)?
                .as_bytes(),
        )?;
        std::fs::rename(&tmp, &path)?;
        Ok(ChatSummary {
            id: header.id,
            title: header.title,
            updated: header.updated,
            messages: header.message_count,
        })
    }

    /// Reads a chat back. Attachments whose files are gone are left out.
    pub fn load<M: DeserializeOwned>(&self, id: &str) -> Result<Chat<M>, String> {
        if !Self::is_valid_id(id) {
            return Err(format!("Unbekannter Chat {id}."));
        }
        let text = std::fs::read_to_string(self.file(id))
            .map_err(|e| format!("Chat {id} konnte nicht gelesen werden: {e}"))?;
        let mut body: Value =
            serde_json::from_str(&text).map_err(|e| format!("Chat {id} ist beschädigt: {e}"))?;
        let header: Header = serde_json::from_value(body.clone())
            .map_err(|e| format!("Chat {id} ist beschädigt: {e}"))?;
        let files = self.dir.join(id);
        let mut messages = Vec::new();
        if let Some(Value::Array(values)) = body.get_mut("messages").map(Value::take) {
            for mut value in values {
                let saved: Vec<SavedAttachment> = value
                    .get_mut("attachments")
                    .map(Value::take)
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                let message: M = serde_json::from_value(value)
                    .map_err(|e| format!("Chat {id} ist beschädigt: {e}"))?;
                let attachments = saved
                    .into_iter()
                    .filter_map(|s| match (s.url, s.file, s.mime) {
                        (Some(url), _, _) => {
                            let mut att = Attachment::from_url(&url, s.kind);
                            att.name = s.name;
                            Some(att)
                        }
                        (None, Some(file), Some(mime)) => {
                            let bytes = std::fs::read(files.join(safe_file_name(&file))).ok()?;
                            Some(Attachment::from_bytes(&s.name, s.kind, &mime, bytes))
                        }
                        _ => None,
                    })
                    .collect();
                messages.push((message, attachments));
            }
        }
        Ok(Chat {
            id: header.id,
            title: header.title,
            created: header.created,
            messages,
        })
    }

    pub fn delete(&self, id: &str) -> std::io::Result<()> {
        if !Self::is_valid_id(id) {
            return Err(std::io::Error::other(format!("ungültige Chat-ID {id}")));
        }
        std::fs::remove_file(self.file(id))?;
        match std::fs::remove_dir_all(self.dir.join(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// The first line of the first message, shortened; or the first attachment.
pub fn title_from(text: &str, attachment: Option<&str>) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    let title = match (line, attachment) {
        (Some(line), _) => line,
        (None, Some(name)) => name,
        (None, None) => return UNTITLED.to_owned(),
    };
    if title.chars().count() > TITLE_CHARS {
        let short: String = title.chars().take(TITLE_CHARS - 1).collect();
        format!("{}…", short.trim_end())
    } else {
        title.to_owned()
    }
}

/// "gerade eben", "vor 5 Min.", "gestern" … for the chat list.
pub fn relative_time(updated: u64, now: u64) -> String {
    let secs = now.saturating_sub(updated);
    match secs {
        0..60 => "gerade eben".to_owned(),
        60..3_600 => format!("vor {} Min.", secs / 60),
        3_600..86_400 => format!("vor {} Std.", secs / 3_600),
        86_400..172_800 => "gestern".to_owned(),
        _ => format!("vor {} Tagen", secs / 86_400),
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Keeps letters, digits, dots, dashes and underscores; never a path.
fn safe_file_name(name: &str) -> String {
    let cleaned: String = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_start_matches('.').to_owned();
    if cleaned.is_empty() {
        "anhang".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Msg {
        text: String,
    }

    fn store(name: &str) -> (ChatStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("fadis-chats-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (ChatStore::new(dir.clone()), dir)
    }

    #[test]
    fn chats_round_trip_with_attachments() {
        let (store, dir) = store("roundtrip");
        assert!(store.list().is_empty());
        let id = ChatStore::new_id();
        assert!(ChatStore::is_valid_id(&id));
        let photo = Attachment::from_bytes(
            "foto 1.jpg",
            AttachmentKind::Image,
            "image/jpeg",
            vec![1, 2, 3],
        );
        let link = Attachment::from_url("https://x.de/clip.mp4", AttachmentKind::Video);
        let chat = Chat {
            id: id.clone(),
            title: title_from("Was ist auf dem Foto?", None),
            created: 1,
            messages: vec![
                (
                    Msg {
                        text: "Was ist auf dem Foto?".into(),
                    },
                    vec![photo, link],
                ),
                (
                    Msg {
                        text: "Eine Katze.".into(),
                    },
                    vec![],
                ),
            ],
        };
        let summary = store.save(&chat).unwrap();
        assert_eq!(summary.messages, 2);
        assert!(store.exists(&id));
        // Saving again does not rewrite or duplicate the attachment files.
        store.save(&chat).unwrap();
        assert_eq!(std::fs::read_dir(dir.join(&id)).unwrap().count(), 1);

        let loaded: Chat<Msg> = store.load(&id).unwrap();
        assert_eq!(loaded.title, "Was ist auf dem Foto?");
        assert_eq!(
            loaded.messages[1].0,
            Msg {
                text: "Eine Katze.".into()
            }
        );
        let atts = &loaded.messages[0].1;
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0].name, "foto 1.jpg");
        assert!(
            matches!(&atts[0].source, Source::Data { bytes, mime } if bytes[..] == [1, 2, 3] && mime == "image/jpeg")
        );
        assert!(matches!(&atts[1].source, Source::Url(u) if u == "https://x.de/clip.mp4"));

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Was ist auf dem Foto?");
        store.delete(&id).unwrap();
        assert!(store.list().is_empty() && !dir.join(&id).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_from_outside_cannot_reach_other_files() {
        let (store, dir) = store("ids");
        for bad in ["../settings", "c-../../x", "c-", "x-1", "c-a/b", ""] {
            assert!(!ChatStore::is_valid_id(bad), "{bad}");
            assert!(store.load::<Msg>(bad).is_err());
            assert!(store.delete(bad).is_err());
        }
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name(".versteckt"), "versteckt");
        assert_eq!(safe_file_name("Mein Clip (1).mp4"), "Mein_Clip__1_.mp4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn titles_and_times() {
        assert_eq!(
            title_from("\n  Ordner anlegen \nund mehr", None),
            "Ordner anlegen"
        );
        assert_eq!(title_from("", Some("clip.mp4")), "clip.mp4");
        assert_eq!(title_from("", None), UNTITLED);
        let long = "a".repeat(100);
        assert_eq!(title_from(&long, None).chars().count(), TITLE_CHARS);
        assert_eq!(relative_time(100, 130), "gerade eben");
        assert_eq!(relative_time(0, 300), "vor 5 Min.");
        assert_eq!(relative_time(0, 7_200), "vor 2 Std.");
        assert_eq!(relative_time(0, 90_000), "gestern");
        assert_eq!(relative_time(0, 3 * 86_400), "vor 3 Tagen");
        assert_eq!(relative_time(500, 100), "gerade eben");
    }
}
