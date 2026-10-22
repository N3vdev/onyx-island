//! User configuration: schema, defaults, validation, and atomic persistence.
//!
//! The island process and the settings process share this file. The settings
//! process writes it atomically; the island watches the parent directory and
//! reloads on change. Invalid edits never take down a running process.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level Islet configuration.
///
/// Unknown fields are ignored so a config written by a newer build does not
/// break an older one. Missing fields fall back to the documented defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub general: GeneralConfig,
    pub dictation: DictationConfig,
    pub media: MediaConfig,
    pub keysounds: KeysoundConfig,
    pub agents: AgentConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA_VERSION,
            general: GeneralConfig::default(),
            dictation: DictationConfig::default(),
            media: MediaConfig::default(),
            keysounds: KeysoundConfig::default(),
            agents: AgentConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub position: ScreenPosition,
    pub monitor: MonitorChoice,
    pub idle_style: IdleStyle,
    pub hide_in_fullscreen: bool,
    pub start_with_windows: bool,
    /// Slim down and dock to the screen edge while an app window is behind
    /// the island.
    pub tuck_behind_windows: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenPosition {
    #[default]
    Bottom,
    Top,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorChoice {
    #[default]
    Primary,
    FollowCursor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleStyle {
    #[default]
    Hidden,
    Sliver,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    pub enabled: bool,
    pub push_to_talk: String,
    pub hands_free: String,
    pub filler_removal: bool,
    pub spoken_punctuation: bool,
    pub injection: InjectionMethod,
    pub history: bool,
    pub model: String,
    pub keep_warm: bool,
    pub unload_after_minutes: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionMethod {
    #[default]
    Clipboard,
    Unicode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    pub enabled: bool,
    pub real_bars: bool,
    pub color_tint: bool,
    /// Seconds paused media stays on the island before closing (1-300).
    pub pause_hide_seconds: u32,
    /// Only show music: music apps, music sites, and music on youtube.com.
    pub music_only: bool,
    /// Sites whose browser playback counts as music. Names or URLs, matched
    /// against the site name in the tab title ("https://www.jiosaavn.com"
    /// and "JioSaavn" both work). youtube.com is handled separately: only
    /// music videos count, unless "youtube" itself is listed here.
    pub music_sites: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeysoundConfig {
    pub enabled: bool,
    pub pack: String,
    pub volume: f64,
    pub pitch_variation: bool,
    pub key_up_sounds: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub enabled: bool,
    pub sticky_needs_input: bool,
    pub show_done: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            position: ScreenPosition::Bottom,
            monitor: MonitorChoice::Primary,
            idle_style: IdleStyle::Hidden,
            hide_in_fullscreen: true,
            start_with_windows: false,
            tuck_behind_windows: true,
        }
    }
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            push_to_talk: "Ctrl+Win".to_owned(),
            hands_free: "Ctrl+Win+Space".to_owned(),
            filler_removal: true,
            spoken_punctuation: true,
            injection: InjectionMethod::Clipboard,
            history: false,
            model: "parakeet-tdt-0.6b-v3-int8".to_owned(),
            keep_warm: false,
            unload_after_minutes: 10,
        }
    }
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            real_bars: true,
            color_tint: true,
            pause_hide_seconds: 5,
            music_only: true,
            music_sites: crate::music::DEFAULT_MUSIC_SITES
                .iter()
                .map(|site| (*site).to_owned())
                .collect(),
        }
    }
}

impl Default for KeysoundConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pack: "thock".to_owned(),
            volume: 0.7,
            pitch_variation: true,
            key_up_sounds: true,
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sticky_needs_input: true,
            show_done: true,
        }
    }
}

impl Config {
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// Loads and validates a config. Missing files produce defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Io(error)),
        }
    }

    /// Parses and normalizes a config from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let mut config: Self = toml::from_str(text).map_err(ConfigError::Parse)?;
        config.normalize();
        Ok(config)
    }

    /// Serializes a config to TOML text.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(ConfigError::Serialize)
    }

    /// Writes the config atomically.
    ///
    /// The file is written to a sibling temporary file, flushed to disk, then
    /// renamed over the destination. Readers never observe a partial document.
    pub fn save_atomic(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        let parent = path.parent().ok_or_else(|| {
            ConfigError::InvalidPath(format!("{} has no parent directory", path.display()))
        })?;
        fs::create_dir_all(parent).map_err(ConfigError::Io)?;

        let temporary = temp_path(path);
        {
            let mut file = fs::File::create(&temporary).map_err(ConfigError::Io)?;
            file.write_all(text.as_bytes()).map_err(ConfigError::Io)?;
            file.sync_all().map_err(ConfigError::Io)?;
        }

        // std::fs::rename replaces an existing file on Windows.
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(ConfigError::Io(error));
        }
        Ok(())
    }

    /// Clamps values that must stay in range. Called after parsing.
    pub fn normalize(&mut self) {
        if self.schema_version == 0 {
            self.schema_version = Self::CURRENT_SCHEMA_VERSION;
        }
        self.keysounds.volume = self.keysounds.volume.clamp(0.0, 1.0);
        self.dictation.unload_after_minutes = self.dictation.unload_after_minutes.clamp(1, 240);
        self.dictation.push_to_talk = self.dictation.push_to_talk.trim().to_owned();
        self.dictation.hands_free = self.dictation.hands_free.trim().to_owned();
        self.media.pause_hide_seconds = self.media.pause_hide_seconds.clamp(1, 300);
        self.media
            .music_sites
            .retain(|site| !site.trim().is_empty());
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()))
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    InvalidPath(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "config I/O error: {error}"),
            Self::Parse(error) => write!(f, "invalid config: {error}"),
            Self::Serialize(error) => write!(f, "could not serialize config: {error}"),
            Self::InvalidPath(message) => write!(f, "invalid config path: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::InvalidPath(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("islet-core-test-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&directory).expect("create temp dir");
        directory
    }

    #[test]
    fn defaults_are_valid_and_stable() {
        let config = Config::default();
        assert_eq!(config.schema_version, Config::CURRENT_SCHEMA_VERSION);
        assert_eq!(config.dictation.push_to_talk, "Ctrl+Win");
        assert!(config.agents.sticky_needs_input);
        assert_eq!(config.general.position, ScreenPosition::Bottom);

        let normalized = {
            let mut config = config;
            config.normalize();
            config
        };
        assert_eq!(normalized.schema_version, Config::CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config::default();
        let text = config.to_toml().expect("serialize");
        let parsed = Config::from_toml(&text).expect("parse");
        assert_eq!(config, parsed);
        assert!(text.contains("[dictation]"));
        assert!(text.contains("push_to_talk = \"Ctrl+Win\""));
    }

    #[test]
    fn partial_config_uses_defaults() {
        let config = Config::from_toml(
            r#"
            [general]
            position = "top"

            [keysounds]
            volume = 4.0
            "#,
        )
        .expect("parse");
        assert_eq!(config.general.position, ScreenPosition::Top);
        assert!(config.general.hide_in_fullscreen);
        assert_eq!(config.keysounds.volume, 1.0);
        assert_eq!(config.dictation.push_to_talk, "Ctrl+Win");
    }

    #[test]
    fn invalid_toml_is_an_error() {
        let error = Config::from_toml("[general\nposition = ").unwrap_err();
        assert!(matches!(error, ConfigError::Parse(_)));
    }

    #[test]
    fn missing_file_returns_defaults() {
        let directory = temporary_directory();
        let config = Config::load(&directory.join("missing.toml")).expect("load");
        assert_eq!(config, Config::default());
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn atomic_save_replaces_file_without_leftovers() {
        let directory = temporary_directory();
        let path = directory.join("config.toml");

        let mut first = Config::default();
        first.general.position = ScreenPosition::Top;
        first.save_atomic(&path).expect("first save");

        let mut second = Config::default();
        second.dictation.push_to_talk = "Alt+Space".to_owned();
        second.save_atomic(&path).expect("second save");

        let loaded = Config::load(&path).expect("load");
        assert_eq!(loaded.dictation.push_to_talk, "Alt+Space");
        assert_eq!(loaded.general.position, ScreenPosition::Bottom);

        let leftovers: Vec<_> = fs::read_dir(&directory)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files left behind");

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn unknown_fields_do_not_break_forward_compatibility() {
        let config = Config::from_toml(
            r#"
            schema_version = 9
            [general]
            position = "bottom"
            future_option = true
            "#,
        )
        .expect("parse");
        assert_eq!(config.schema_version, 9);
    }
}
