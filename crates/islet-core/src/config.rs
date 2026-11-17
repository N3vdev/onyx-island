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
    pub widgets: WidgetConfig,
    pub night_light: NightLightConfig,
    pub lens: LensConfig,
    pub mail: MailConfig,
}

/// Email on the island.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MailConfig {
    /// Gmail on the island at all (once an account is connected). The name
    /// is from when it showed only codes, kept so settings carry over.
    pub gmail_codes: bool,
    /// What shows: codes only, every new email, or a chosen mix.
    pub show: MailShow,
    /// With [`MailShow::Custom`]: one-time codes,
    pub custom_codes: bool,
    /// mail Gmail marks important,
    pub custom_important: bool,
    /// mail in the Primary inbox tab (not Promotions, Social, Updates or
    /// Forums),
    pub custom_primary: bool,
    /// and mail from these senders: addresses ("ana@example.com") or domains
    /// ("github.com", which takes its subdomains too).
    pub custom_senders: Vec<String>,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            gmail_codes: true,
            show: MailShow::Codes,
            custom_codes: true,
            custom_important: true,
            custom_primary: false,
            custom_senders: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailShow {
    /// One-time codes only.
    #[default]
    Codes,
    /// Codes, and every new email in the inbox.
    All,
    /// The custom choices.
    Custom,
}

impl MailConfig {
    /// Codes found in new mail show.
    #[must_use]
    pub fn shows_codes(&self) -> bool {
        match self.show {
            MailShow::Codes | MailShow::All => true,
            MailShow::Custom => self.custom_codes,
        }
    }

    /// A new email (without a code to show) shows: Gmail's `important` and
    /// `primary` marks for it, and the sender's address.
    #[must_use]
    pub fn shows_email(&self, important: bool, primary: bool, address: &str) -> bool {
        match self.show {
            MailShow::Codes => false,
            MailShow::All => true,
            MailShow::Custom => {
                (self.custom_important && important)
                    || (self.custom_primary && primary)
                    || self.custom_senders.iter().any(|sender| sender_matches(sender, address))
            }
        }
    }
}

/// "ana@example.com" is that address; "example.com" or "@example.com" is the
/// domain and its subdomains.
fn sender_matches(sender: &str, address: &str) -> bool {
    let sender = sender.trim().to_ascii_lowercase();
    let address = address.trim().to_ascii_lowercase();
    if sender.is_empty() || address.is_empty() {
        return false;
    }
    if !sender.starts_with('@') && sender.contains('@') {
        return sender == address;
    }
    let domain = sender.trim_start_matches('@');
    let Some((_, from)) = address.rsplit_once('@') else { return false };
    from == domain || from.ends_with(&format!(".{domain}"))
}

/// Onyx Lens: press a shortcut, then circle anything on screen to copy its
/// text, translate it, search it or ask AI about it. It runs in its own
/// helper process (`onyx-lens.exe`), which Onyx starts and ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensConfig {
    pub enabled: bool,
    /// A key plus modifiers, as `"Ctrl+Shift+Space"`. A modifier-only combo
    /// cannot be a system-wide shortcut, so it is not accepted here.
    pub hotkey: String,
}

impl Default for LensConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: "Ctrl+Shift+Space".to_owned(),
        }
    }
}

/// Warms the screen at night (less blue light), like f.lux.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NightLightConfig {
    pub enabled: bool,
    /// 0 is no change, 1 the warmest (about candlelight).
    pub intensity: f32,
    /// When it comes on and goes off, as "20:00"; it may run past midnight.
    pub start: String,
    pub end: String,
}

impl Default for NightLightConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            intensity: 0.75,
            start: "20:00".to_owned(),
            end: "05:00".to_owned(),
        }
    }
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
            widgets: WidgetConfig::default(),
            night_light: NightLightConfig::default(),
            lens: LensConfig::default(),
            mail: MailConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub position: ScreenPosition,
    pub monitor: MonitorChoice,
    pub idle_style: IdleStyle,
    /// Never hide: with nothing going on, show `glance` instead.
    pub always_on: bool,
    /// What the always-on island shows when nothing else is happening.
    pub glance: IdleGlance,
    /// Whether the island opens its full view on hover or only on click.
    pub expand_on: ExpandTrigger,
    pub hide_in_fullscreen: bool,
    pub start_with_windows: bool,
    /// The welcome tour ran (or was skipped). Until then the island opens
    /// it once at startup.
    pub onboarded: bool,
    /// Read only, from configs written before `ScreenPosition::Dynamic`
    /// existed: `top` with this on meant what `dynamic` means now.
    /// `normalize` folds it into `position`; it is never written back.
    #[serde(skip_serializing)]
    pub tuck_behind_windows: Option<bool>,
    /// In dynamic mode: stay at the top when the part of the app under the
    /// island is empty (one flat colour).
    pub stay_over_empty_space: bool,
    pub top_margin_percent: f32,
    pub bottom_margin_percent: f32,
    pub top_expanded_scale: f32,
    pub bottom_expanded_scale: f32,
    /// With only the eyes showing, the island waits as a small circle in
    /// this corner (swiped there); anything else shows in the middle.
    pub corner: IslandCorner,
    /// What the island is made of.
    pub look: IslandLook,
    /// With the glass look: how dark its tint is, 0 (clear) to 1 (black).
    pub glass_tint: f32,
}

/// The island's material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandLook {
    /// Solid black, as on a phone.
    #[default]
    Black,
    /// Frosted glass: what is behind shows, blurred (while Windows'
    /// transparency effects are on; clear otherwise), under a dark tint.
    Glass,
}

/// Where the eyes-only island waits along its edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandCorner {
    /// In the middle, as the usual pill.
    #[default]
    Middle,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetConfig {
    /// OLE drag-to-convert; no background worker while idle.
    pub file_converter: bool,
    /// Opt-in: existing idle behavior stays unchanged until enabled.
    pub calculator: bool,
    /// Live CPU, GPU, memory, network, and temperature. Sampled once a second
    /// only while it is on screen; the CPU alone, every two seconds, for the
    /// hot-CPU glow otherwise.
    pub system_stats: bool,
    /// The to-do list page, with reminders and the focus timer.
    pub tasks: bool,
    /// A focus session's length, in minutes.
    pub focus_minutes: u32,
    /// The break after one, in minutes.
    pub break_minutes: u32,
    /// A sound with reminders and when a focus session or break ends.
    pub task_sound: bool,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            file_converter: true,
            calculator: false,
            system_stats: false,
            tasks: true,
            focus_minutes: 25,
            break_minutes: 5,
            task_sound: true,
        }
    }
}

#[cfg(test)]
mod converter_config_tests {
    use super::*;
    #[test]
    fn converter_is_on_for_existing_configs_and_explicit_off_survives_save_load() {
        assert!(Config::from_toml("[widgets]\ncalculator = true\n").unwrap().widgets.file_converter);
        let off=Config::from_toml("[widgets]\nfile_converter = false\n").unwrap();
        assert!(!off.widgets.file_converter);
        assert!(!Config::from_toml(&off.to_toml().unwrap()).unwrap().widgets.file_converter);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenPosition {
    /// Always at the bottom edge.
    #[default]
    Bottom,
    /// Always at the top edge.
    Top,
    /// At the top, gliding to the bottom while an app window is behind it
    /// (unless that space is empty), and back when the way is clear.
    Dynamic,
}

impl ScreenPosition {
    /// The edge the island rests at when nothing is behind it.
    #[must_use]
    pub fn home_is_top(self) -> bool {
        matches!(self, Self::Top | Self::Dynamic)
    }

    /// Whether the island moves out of the way of app windows.
    #[must_use]
    pub fn docks_over_apps(self) -> bool {
        self == Self::Dynamic
    }
}

/// What an always-on island shows when nothing is playing or happening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleGlance {
    /// The time. (`resume_or_clock`, an earlier mode that kept paused music
    /// on the pill, reads as this.)
    #[default]
    #[serde(alias = "resume_or_clock")]
    Clock,
    /// CPU and memory use.
    Stats,
    /// Two eyes that blink and look the way the island is pulled.
    Eyes,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandTrigger {
    /// Resting the pointer on the island opens it.
    #[default]
    Hover,
    /// Only a click opens it; hovering does nothing.
    Click,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    pub enabled: bool,
    pub push_to_talk: String,
    /// Hotkey that starts recording on one press and stops it on the next.
    /// Only active while `hands_free_enabled` is on.
    pub hands_free: String,
    /// Hands-free mode: off by default, so the one hotkey is hold-to-speak.
    pub hands_free_enabled: bool,
    /// The voice assistant: hold `assistant_hotkey` and say a command
    /// ("open Spotify", "next song", "type see you soon").
    pub assistant_enabled: bool,
    pub assistant_hotkey: String,
    /// Learn from your corrections: after a dictation, notice words you
    /// changed in what Onyx typed and keep the pair on this PC only. Off by
    /// default.
    pub learn_corrections: bool,
    pub filler_removal: bool,
    pub spoken_punctuation: bool,
    pub injection: InjectionMethod,
    /// Keep the final dictated text available for another paste after insertion.
    pub copy_to_clipboard: bool,
    pub history: bool,
    pub model: String,
    pub keep_warm: bool,
    pub unload_after_minutes: u32,
    /// Microphone to use; empty means the system default.
    pub microphone: String,
    /// Play a soft tone when recording starts and stops.
    pub tones: bool,
    /// Pause playing Windows media sessions when dictation is revealed.
    pub pause_media: bool,
    /// End the inserted text with a space so back-to-back dictations do not
    /// run together.
    pub trailing_space: bool,
    /// Replace spoken words with preferred spellings ("claude code" ->
    /// "Claude Code").
    pub dictionary: Vec<Replacement>,
    /// Spoken triggers that expand to longer text ("insert my email").
    pub snippets: Vec<Replacement>,
    /// Rewrite each dictation with the optional on-device language model
    /// (fixes grammar, drops corrected words, lays out lists). On by default;
    /// it only runs once the model is downloaded, and until then dictation
    /// uses the rule-based cleanup.
    pub ai_cleanup: bool,
    /// Keep a WAV file of each dictation's audio (never its text). Off by
    /// default.
    pub save_recordings: bool,
    /// Where recordings are saved; empty means Onyx's own data folder.
    pub recordings_folder: String,
    /// Recordings older than this many days are deleted; 0 keeps them.
    pub delete_recordings_after_days: u32,
}

/// One "say this, get that" entry for the dictionary or snippets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    pub from: String,
    pub to: String,
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
    /// Say when Bluetooth earbuds or headphones connect and leave, and when
    /// they run low, with their battery.
    pub bluetooth: bool,
    /// Offer Spotify search and play on the island (with a connected
    /// Premium account).
    pub spotify: bool,
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
            always_on: true,
            glance: IdleGlance::Clock,
            expand_on: ExpandTrigger::Hover,
            hide_in_fullscreen: true,
            start_with_windows: false,
            onboarded: false,
            tuck_behind_windows: None,
            stay_over_empty_space: true,
            top_margin_percent: 0.0,
            bottom_margin_percent: 0.0,
            top_expanded_scale: 1.0,
            bottom_expanded_scale: 1.0,
            corner: IslandCorner::Middle,
            look: IslandLook::Black,
            glass_tint: 0.45,
        }
    }
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            push_to_talk: "Right Ctrl".to_owned(),
            hands_free: "Ctrl+Win+Space".to_owned(),
            hands_free_enabled: false,
            assistant_enabled: true,
            assistant_hotkey: "Right Alt".to_owned(),
            learn_corrections: false,
            filler_removal: true,
            spoken_punctuation: true,
            injection: InjectionMethod::Clipboard,
            copy_to_clipboard: true,
            history: false,
            // Same as islet_asr::DEFAULT_MODEL_ID (core doesn't depend on the ASR crate).
            model: "parakeet-tdt-0.6b-v2-int8".to_owned(),
            keep_warm: false,
            unload_after_minutes: 10,
            microphone: String::new(),
            tones: true,
            pause_media: false,
            trailing_space: true,
            dictionary: Vec::new(),
            snippets: Vec::new(),
            ai_cleanup: true,
            save_recordings: false,
            recordings_folder: String::new(),
            delete_recordings_after_days: 7,
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
            bluetooth: true,
            spotify: true,
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
        for margin in [
            &mut self.general.top_margin_percent,
            &mut self.general.bottom_margin_percent,
        ] {
            *margin = if margin.is_finite() {
                margin.clamp(0.0, 5.0)
            } else {
                0.0
            };
        }
        for scale in [
            &mut self.general.top_expanded_scale,
            &mut self.general.bottom_expanded_scale,
        ] {
            *scale = if scale.is_finite() {
                scale.clamp(0.8, 1.25)
            } else {
                1.0
            };
        }
        if self.schema_version == 0 {
            self.schema_version = Self::CURRENT_SCHEMA_VERSION;
        }
        if let Some(tuck) = self.general.tuck_behind_windows.take() {
            if tuck && self.general.position == ScreenPosition::Top {
                self.general.position = ScreenPosition::Dynamic;
            }
        }
        self.keysounds.volume = self.keysounds.volume.clamp(0.0, 1.0);
        self.dictation.unload_after_minutes = self.dictation.unload_after_minutes.clamp(1, 240);
        self.dictation.push_to_talk = self.dictation.push_to_talk.trim().to_owned();
        self.dictation.hands_free = self.dictation.hands_free.trim().to_owned();
        self.dictation.assistant_hotkey = self.dictation.assistant_hotkey.trim().to_owned();
        self.lens.hotkey = self.lens.hotkey.trim().to_owned();
        self.media.pause_hide_seconds = self.media.pause_hide_seconds.clamp(1, 300);
        for list in [&mut self.dictation.dictionary, &mut self.dictation.snippets] {
            list.retain(|entry| !entry.from.trim().is_empty());
        }
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

    #[test]
    fn the_old_resume_glance_reads_as_the_clock() {
        let config: Config = toml::from_str("[general]\nglance = \"resume_or_clock\"\n").unwrap();
        assert_eq!(config.general.glance, IdleGlance::Clock);
    }
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
    fn pause_media_defaults_off_and_persists_when_enabled() {
        assert!(!Config::default().dictation.pause_media);
        assert!(
            !Config::from_toml("[dictation]\nenabled = true")
                .unwrap()
                .dictation
                .pause_media
        );
        let mut config = Config::default();
        config.dictation.pause_media = true;
        assert!(
            Config::from_toml(&config.to_toml().unwrap())
                .unwrap()
                .dictation
                .pause_media
        );
    }

    #[test]
    fn expand_trigger_defaults_to_hover_and_persists_click() {
        assert_eq!(
            Config::from_toml("[general]\nposition = \"top\"")
                .unwrap()
                .general
                .expand_on,
            ExpandTrigger::Hover
        );
        let mut config = Config::default();
        config.general.expand_on = ExpandTrigger::Click;
        let text = config.to_toml().unwrap();
        assert!(text.contains("expand_on = \"click\""));
        assert_eq!(
            Config::from_toml(&text).unwrap().general.expand_on,
            ExpandTrigger::Click
        );
    }

    #[test]
    fn customization_is_bounded_and_existing_configs_keep_original_size() {
        let defaults = Config::from_toml("[general]\nposition = 'top'").unwrap();
        assert_eq!(defaults.general.top_margin_percent, 0.0);
        assert_eq!(defaults.general.bottom_expanded_scale, 1.0);
        let mut config = defaults;
        config.general.top_margin_percent = 50.0;
        config.general.bottom_margin_percent = f32::NAN;
        config.general.top_expanded_scale = 0.1;
        config.general.bottom_expanded_scale = 5.0;
        config.normalize();
        assert_eq!(config.general.top_margin_percent, 5.0);
        assert_eq!(config.general.bottom_margin_percent, 0.0);
        assert_eq!(config.general.top_expanded_scale, 0.8);
        assert_eq!(config.general.bottom_expanded_scale, 1.25);
        assert_eq!(
            Config::from_toml(&config.to_toml().unwrap()).unwrap(),
            config
        );
    }

    #[test]
    fn defaults_are_valid_and_stable() {
        let config = Config::default();
        assert_eq!(config.schema_version, Config::CURRENT_SCHEMA_VERSION);
        assert_eq!(config.dictation.push_to_talk, "Right Ctrl");
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
        assert!(text.contains("push_to_talk = \"Right Ctrl\""));
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
        assert_eq!(config.dictation.push_to_talk, "Right Ctrl");
    }

    #[test]
    fn clipboard_copy_defaults_on_and_an_explicit_off_choice_round_trips() {
        let legacy = Config::from_toml("[dictation]\ntrailing_space = false\n").unwrap();
        assert!(legacy.dictation.copy_to_clipboard);
        assert!(!legacy.dictation.trailing_space);
        let off = Config::from_toml("[dictation]\ncopy_to_clipboard = false\n").unwrap();
        assert!(!off.dictation.copy_to_clipboard);
        assert!(!Config::from_toml(&off.to_toml().unwrap())
            .unwrap()
            .dictation
            .copy_to_clipboard);
        assert!(Config::default().dictation.copy_to_clipboard);
    }

    #[test]
    fn old_top_with_docking_becomes_dynamic_and_other_edges_stay() {
        let from = |position: &str, tuck: &str| {
            Config::from_toml(&format!(
                "[general]\nposition = \"{position}\"\ntuck_behind_windows = {tuck}\n"
            ))
            .unwrap()
            .general
            .position
        };
        // What the config looked like before the third option existed.
        assert_eq!(from("top", "true"), ScreenPosition::Dynamic);
        assert_eq!(from("top", "false"), ScreenPosition::Top);
        assert_eq!(from("bottom", "true"), ScreenPosition::Bottom);
        assert_eq!(from("bottom", "false"), ScreenPosition::Bottom);
    }

    #[test]
    fn position_choices_round_trip_and_stay_put() {
        for position in [
            ScreenPosition::Dynamic,
            ScreenPosition::Top,
            ScreenPosition::Bottom,
        ] {
            let mut config = Config::default();
            config.general.position = position;
            let text = config.to_toml().unwrap();
            assert!(!text.contains("tuck_behind_windows"), "legacy key written");
            // Saving and loading again must not be re-migrated.
            let loaded = Config::from_toml(&text).unwrap();
            assert_eq!(loaded.general.position, position);
            assert_eq!(loaded, config);
        }
        assert!(ScreenPosition::Dynamic.home_is_top() && ScreenPosition::Dynamic.docks_over_apps());
        assert!(ScreenPosition::Top.home_is_top() && !ScreenPosition::Top.docks_over_apps());
        assert!(!ScreenPosition::Bottom.home_is_top() && !ScreenPosition::Bottom.docks_over_apps());
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

    #[test]
    fn mail_shows_codes_only_unless_chosen() {
        let mut mail = MailConfig::default();
        assert!(mail.shows_codes());
        assert!(!mail.shows_email(true, true, "ana@example.com"));
        mail.show = MailShow::All;
        assert!(mail.shows_email(false, false, "news@shop.example"));
        mail.show = MailShow::Custom;
        mail.custom_codes = false;
        mail.custom_senders = vec!["github.com".into(), "ana@example.com".into()];
        assert!(!mail.shows_codes());
        assert!(mail.shows_email(true, false, "x@y.z"), "important");
        assert!(!mail.shows_email(false, true, "x@y.z"), "primary is off by default");
        assert!(mail.shows_email(false, false, "noreply@github.com"));
        assert!(mail.shows_email(false, false, "a@mail.github.com"));
        assert!(!mail.shows_email(false, false, "a@notgithub.com"));
        assert!(mail.shows_email(false, false, "Ana@Example.com"));
        assert!(!mail.shows_email(false, false, "bo@example.com"));
        let saved: Config = toml::from_str("[mail]
gmail_codes = true
").expect("parse");
        assert_eq!(saved.mail.show, MailShow::Codes);
    }
}
