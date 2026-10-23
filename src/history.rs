//! What the user sent before, to call it back with the arrow keys in the
//! input field (↑ older, ↓ newer), like in a shell. Kept across restarts.

use std::path::PathBuf;

const MAX_ENTRIES: usize = 200;
const MAX_ENTRY_CHARS: usize = 4_000;

pub struct InputHistory {
    /// Oldest first.
    entries: Vec<String>,
    /// Which entry is shown in the input field right now, if one is.
    pos: Option<usize>,
    /// What was typed before the first ↑, for coming back with ↓.
    draft: String,
    path: Option<PathBuf>,
}

impl InputHistory {
    /// Reads the saved history; without a path (tests) nothing is read or written.
    pub fn load(path: Option<PathBuf>) -> Self {
        let entries: Vec<String> = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let skip = entries.len().saturating_sub(MAX_ENTRIES);
        Self {
            entries: entries.into_iter().skip(skip).collect(),
            pos: None,
            draft: String::new(),
            path,
        }
    }

    /// Remembers a message that was sent. An older identical one moves up.
    pub fn push(&mut self, text: &str) {
        self.pos = None;
        self.draft.clear();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let text: String = text.chars().take(MAX_ENTRY_CHARS).collect();
        self.entries.retain(|e| *e != text);
        self.entries.push(text);
        let extra = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..extra);
        self.save();
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&self.entries) {
            let _ = crate::config::write_private(path, json.as_bytes());
        }
    }

    /// True while an entry is on screen, unchanged: ↑ and ↓ then walk through
    /// the history instead of moving the cursor.
    pub fn is_browsing(&self, current: &str) -> bool {
        self.pos
            .and_then(|p| self.entries.get(p))
            .is_some_and(|e| e == current)
    }

    /// A step back in time. `current` is what the field holds now. Returns the
    /// text to show, or `None` if there is nothing older.
    pub fn older(&mut self, current: &str) -> Option<String> {
        let target = match self.pos {
            None => {
                let last = self.entries.len().checked_sub(1)?;
                self.draft = current.to_owned();
                last
            }
            Some(0) => return None,
            Some(p) => p - 1,
        };
        self.pos = Some(target);
        self.entries.get(target).cloned()
    }

    /// A step forward; after the newest comes back what was typed before.
    pub fn newer(&mut self) -> Option<String> {
        let p = self.pos?;
        if p + 1 < self.entries.len() {
            self.pos = Some(p + 1);
            self.entries.get(p + 1).cloned()
        } else {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// True if ↓ has somewhere to go.
    pub fn has_newer(&self) -> bool {
        self.pos.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Back to normal typing (a chat was opened, text was cleared).
    pub fn stop_browsing(&mut self) {
        self.pos = None;
        self.draft.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("fadis-history-{}-{name}", std::process::id()))
            .join("history.json")
    }

    #[test]
    fn arrows_walk_back_and_forth_and_return_to_the_draft() {
        let mut h = InputHistory::load(None);
        assert!(h.older("halb").is_none(), "nothing yet");
        h.push("eins");
        h.push("zwei");
        h.push("drei");
        assert_eq!(h.older("halb getippt").as_deref(), Some("drei"));
        assert!(h.is_browsing("drei"));
        assert!(!h.is_browsing("drei!"), "an edited entry is not browsed");
        assert_eq!(h.older("drei").as_deref(), Some("zwei"));
        assert_eq!(h.older("zwei").as_deref(), Some("eins"));
        assert!(h.older("eins").is_none(), "already at the oldest");
        assert_eq!(h.newer().as_deref(), Some("zwei"));
        assert_eq!(h.newer().as_deref(), Some("drei"));
        assert_eq!(h.newer().as_deref(), Some("halb getippt"));
        assert!(!h.has_newer());
        assert!(h.newer().is_none());
    }

    #[test]
    fn repeated_and_empty_messages_are_handled() {
        let mut h = InputHistory::load(None);
        h.push("  Mach das  ");
        h.push("");
        h.push("   ");
        h.push("Anderes");
        h.push("Mach das");
        // The repeated one moved to the end instead of appearing twice.
        assert_eq!(h.older("").as_deref(), Some("Mach das"));
        assert_eq!(h.older("").as_deref(), Some("Anderes"));
        assert!(h.older("").is_none());
        h.push("neu");
        assert!(!h.has_newer(), "sending ends browsing");
    }

    #[test]
    fn history_survives_a_restart_and_stays_small() {
        let path = temp_file("persist");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let mut h = InputHistory::load(Some(path.clone()));
        for i in 0..MAX_ENTRIES + 25 {
            h.push(&format!("Auftrag {i}"));
        }
        h.push(&"x".repeat(MAX_ENTRY_CHARS + 500));
        let mut again = InputHistory::load(Some(path.clone()));
        let newest = again.older("").unwrap();
        assert_eq!(newest.chars().count(), MAX_ENTRY_CHARS);
        assert_eq!(
            again.older("").as_deref(),
            Some(&*format!("Auftrag {}", MAX_ENTRIES + 24))
        );
        assert_eq!(again.entries.len(), MAX_ENTRIES);
        assert!(!again.entries.iter().any(|e| e == "Auftrag 0"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the history may hold private text");
        }
        // A damaged file gives an empty history, not a crash.
        std::fs::write(&path, "{kaputt").unwrap();
        assert!(InputHistory::load(Some(path.clone())).is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
