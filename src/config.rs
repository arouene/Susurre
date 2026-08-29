use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const APP_ID: &str = "fr.rouene.Susurre";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    WhisperCpp,
    FasterWhisper,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::WhisperCpp, Engine::FasterWhisper];

    pub fn label(self) -> &'static str {
        match self {
            Engine::WhisperCpp => "whisper.cpp",
            Engine::FasterWhisper => "faster-whisper (CTranslate2)",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Engine::WhisperCpp => "whisper-cpp",
            Engine::FasterWhisper => "faster-whisper",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub engine: Engine,
    pub model: String,
    /// "auto" or an ISO code ("fr", "en", ...).
    pub language: String,
    /// Wanted trigger, in portal format: "CTRL+ALT+d". The compositor may bind
    /// something else; what it actually bound is reported back at runtime.
    pub shortcut: String,
    /// RemoteDesktop session restore token, so the permission is asked once.
    pub remote_desktop_token: Option<String>,
    /// Decoder prompt, in the user's own language. It steers punctuation,
    /// casing and vocabulary. Empty by default.
    pub prompt: String,
    /// Replacements applied to the transcript, e.g. `"new line" = "\n"`.
    /// Empty by default: no language is assumed.
    pub replacements: BTreeMap<String, String>,
    /// Former name of `replacements`. Read once so an existing table is not
    /// lost, then dropped from the file.
    /// The tables must stay the last fields, TOML requires them at the end of
    /// a document.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    commands: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: Engine::WhisperCpp,
            model: "small".into(),
            language: "auto".into(),
            shortcut: "CTRL+ALT+d".into(),
            remote_desktop_token: None,
            prompt: String::new(),
            replacements: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    gtk::glib::user_config_dir().join("susurre/config.toml")
}

pub fn data_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("susurre")
}

impl Config {
    pub fn load() -> Self {
        let mut cfg: Self = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        if !cfg.commands.is_empty() && cfg.replacements.is_empty() {
            cfg.replacements = std::mem::take(&mut cfg.commands);
        }
        cfg.commands.clear();
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[commands]` used to hold the text replacements. An existing table must
    /// survive the rename instead of silently vanishing.
    #[test]
    fn the_legacy_commands_table_becomes_replacements() {
        let mut cfg: Config = toml::from_str("[commands]\n\"a la ligne\" = \"\\n\"\n").unwrap();
        assert!(cfg.replacements.is_empty());
        cfg.replacements = std::mem::take(&mut cfg.commands);
        assert_eq!(cfg.replacements.get("a la ligne").map(String::as_str), Some("\n"));
        // Once migrated it is no longer written back out.
        assert!(!toml::to_string_pretty(&cfg).unwrap().contains("[commands]"));
    }
}
