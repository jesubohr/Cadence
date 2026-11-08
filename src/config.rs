//! Persisted user settings.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::media::{Kind, DEFAULT_MP3_KBPS};
use crate::storage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub audio_dir: PathBuf,
    pub video_dir: PathBuf,
    pub kind: Kind,
    pub mp3_kbps: u32,
    /// `None` means the best available height.
    pub video_height: Option<u32>,
    pub always_on_top: bool,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            audio_dir: dirs::audio_dir().unwrap_or_else(|| home.join("Music")),
            video_dir: dirs::video_dir().unwrap_or_else(|| home.join("Videos")),
            kind: Kind::Audio,
            mp3_kbps: DEFAULT_MP3_KBPS,
            video_height: None,
            always_on_top: false,
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        storage::data_dir().join("config.json")
    }

    pub fn load() -> Self {
        storage::load_json(&Self::path())
    }

    pub fn save(&self) {
        // A failed save only loses preferences, so the error is not fatal.
        if let Err(err) = storage::save_json(&Self::path(), self) {
            eprintln!("could not save config: {err}");
        }
    }

    pub fn dir_for(&self, kind: Kind) -> &PathBuf {
        match kind {
            Kind::Audio => &self.audio_dir,
            Kind::Video => &self.video_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_mp3_192() {
        let config = Config::default();
        assert_eq!(config.kind, Kind::Audio);
        assert_eq!(config.mp3_kbps, 192);
        assert!(config.audio_dir.ends_with("Music"));
    }

    #[test]
    fn partial_file_uses_defaults_for_missing_fields() {
        let config: Config = serde_json::from_str(r#"{"mp3_kbps":320}"#).unwrap();
        assert_eq!(config.mp3_kbps, 320);
        assert_eq!(config.kind, Kind::Audio);
    }
}
