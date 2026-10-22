//! User settings, stored as JSON in the platform's config directory
//! (on macOS `~/Library/Application Support/de.fadi.FadisNemotron/settings.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::DEFAULT_MAX_STEPS;
use crate::api::{self, GenerationOptions};
use crate::tools::ApprovalMode;

pub const DEFAULT_SYSTEM_PROMPT: &str =
    "Du bist ein hilfreicher Assistent. Antworte in der Sprache des Nutzers.";
const OLD_DEFAULT_MAX_TOKENS: u32 = 16_384;
/// Bumped when stored settings need a one-time migration.
const SETTINGS_VERSION: u32 = 2;
/// The model card recommends 1536×1024 for document pages.
pub const DEFAULT_MAX_IMAGE_DIM: u32 = 1536;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 0 for files written before 0.3 (no such field).
    #[serde(default)]
    pub version: u32,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub system_prompt: String,
    pub thinking: bool,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: u32,
    pub max_image_dim: u32,
    /// What the agent may do in the project folder without asking.
    pub approval: ApprovalMode,
    /// Model requests per message before the agent pauses.
    pub max_steps: usize,
    /// The project folder that was open last time.
    pub workspace: Option<PathBuf>,
    /// Remote access for the Android app (docs/remote-api.md).
    pub remote_enabled: bool,
    pub remote_port: u16,
    /// Pairing key; created when remote access is first switched on.
    pub remote_token: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            api_key: String::new(),
            base_url: api::DEFAULT_BASE_URL.to_owned(),
            model: api::DEFAULT_MODEL.to_owned(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_owned(),
            thinking: true,
            // Recommended sampling from NVIDIA's model card.
            temperature: 1.0,
            top_p: 0.95,
            // Thinking counts toward max_tokens, so leave plenty of room.
            max_tokens: 32_768,
            max_image_dim: DEFAULT_MAX_IMAGE_DIM,
            approval: ApprovalMode::default(),
            max_steps: DEFAULT_MAX_STEPS,
            workspace: None,
            remote_enabled: false,
            remote_port: crate::remote::DEFAULT_PORT,
            remote_token: String::new(),
        }
    }
}

impl Settings {
    /// The key from the settings, or `NVIDIA_API_KEY` from the environment.
    pub fn effective_api_key(&self) -> String {
        if self.api_key.trim().is_empty() {
            std::env::var("NVIDIA_API_KEY").unwrap_or_default()
        } else {
            self.api_key.trim().to_owned()
        }
    }

    pub fn generation_options(&self) -> GenerationOptions {
        GenerationOptions {
            model: self.model.trim().to_owned(),
            temperature: self.temperature,
            top_p: self.top_p,
            max_tokens: self.max_tokens,
            thinking: self.thinking,
        }
    }

    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("de", "fadi", "FadisNemotron")
            .map(|d| d.config_dir().join("settings.json"))
    }

    /// Missing or unreadable files give the defaults.
    pub fn load_from(path: &Path) -> Self {
        let mut settings: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // 0.1 saved its default of 16384; agent steps need more room. Only
        // once, so a user who sets 16384 again keeps it.
        if settings.version < 2 && settings.max_tokens == OLD_DEFAULT_MAX_TOKENS {
            settings.max_tokens = Settings::default().max_tokens;
        }
        settings.version = SETTINGS_VERSION;
        settings
    }

    /// Writes the settings; the file holds the API key, so only the user may read it.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, json.as_bytes())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("fadis-nemotron-{}-{name}", std::process::id()))
            .join("settings.json")
    }

    #[test]
    fn roundtrip_and_private_permissions() {
        let path = temp_path("roundtrip");
        let settings = Settings {
            api_key: "nvapi-test".into(),
            thinking: false,
            max_tokens: 2048,
            ..Default::default()
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), settings);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let path = temp_path("partial");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"model":"eigenes/modell","reasoning_budget":0,"max_tokens":16384}"#,
        )
        .unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s.model, "eigenes/modell");
        assert_eq!(s.max_tokens, 32_768, "old default is raised for the agent");
        assert_eq!(s.approval, ApprovalMode::AutoEdits);
        assert_eq!(s.workspace, None);
        assert_eq!(s.base_url, api::DEFAULT_BASE_URL);
        assert!(s.thinking);
        // Saved again with the version, 16384 is the user's own choice.
        let mut s = s;
        s.max_tokens = 16_384;
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).max_tokens, 16_384);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();

        assert_eq!(
            Settings::load_from(&temp_path("fehlt")),
            Settings::default()
        );
    }

    #[test]
    fn defaults_match_model_card() {
        let o = Settings::default().generation_options();
        assert_eq!(o.model, "nvidia/nemotron-3.5-super-vl-120b-a12b");
        assert_eq!((o.temperature, o.top_p), (1.0, 0.95));
        assert!(o.thinking);
    }
}
