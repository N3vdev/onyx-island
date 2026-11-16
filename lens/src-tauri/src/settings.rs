//! User-configurable settings: hotkey, theme, action-bar contents, AI
//! provider, search engine, and behavior toggles. Persisted as JSON in the
//! app's config directory. Rust owns this state (per CLAUDE.md's own rule);
//! both the overlay and settings windows read/write it through the
//! `get_settings`/`set_settings` commands below, and are kept in sync via a
//! `"settings-changed"` event rather than polling.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

pub const CURRENT_VERSION: u32 = 1;

/// Every built-in action id, in default display order. Must match the
/// builder registries in main.ts.
pub const CHOICE_ACTION_IDS: &[&str] = &["select-text", "translate-here", "lens", "image-to-ai", "save-png", "pin", "share"];
pub const TEXT_ACTION_IDS: &[&str] = &["copy", "copy-code", "redact", "translate-inline", "search", "ask-ai", "translate"];
pub const CUSTOM_ACTION_PREFIX: &str = "custom-";
const MAX_CUSTOM_ACTIONS: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    pub hotkey: String,
    pub theme: ThemeSettings,
    pub behavior: BehaviorSettings,
    pub ai_provider: AiProvider,
    pub custom_ai_url_template: String,
    pub search_engine: SearchEngine,
    pub custom_search_url_template: String,
    /// Order is display order. Both lists always contain every known action
    /// id (see main.ts's builder registries) — `enabled: false` hides one
    /// rather than removing it, so re-enabling later doesn't lose its place.
    pub choice_actions: Vec<ActionEntry>,
    pub text_actions: Vec<ActionEntry>,
    /// User-defined text actions (a Pro feature). Their ids also appear in
    /// `text_actions` for ordering/enabling. Kept even when the plan is Free
    /// — a lapsed subscription hides them rather than deleting the user's work.
    pub custom_actions: Vec<CustomAction>,
    pub launch_at_startup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomAction {
    /// Always `custom-<something>`, so it can never collide with a built-in id.
    pub id: String,
    pub label: String,
    /// http(s) URL with `{q}` where the selected text goes.
    pub url_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionEntry {
    pub id: String,
    pub enabled: bool,
}

impl ActionEntry {
    fn on(id: &str) -> Self {
        Self { id: id.to_string(), enabled: true }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AiProvider {
    #[default]
    #[serde(rename = "chatgpt", alias = "chat-gpt")]
    ChatGpt,
    Claude,
    Gemini,
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SearchEngine {
    #[default]
    Google,
    Bing,
    DuckDuckGo,
    Perplexity,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeSettings {
    /// Drives only the action bar's primary button and the active tool-bar
    /// button — defaults to the app's current hardcoded near-black so a
    /// fresh install looks pixel-identical to before this setting existed.
    /// Deliberately does not touch the lasso's fixed rainbow gradient, the
    /// text-selection highlight, or the selection-card border/glass, all of
    /// which stay as their current hand-tuned colors regardless of this.
    pub accent_color: String,
    pub spotlight_dim: f32,
    pub card_padding: f32,
    pub card_radius: f32,
    /// Overlay colour preset. "classic" is today's look and is free; the
    /// others are part of the Pro visual pack (gated in the UI by plan).
    pub preset: String,
    /// Lasso stroke style. "rainbow" is the default and free.
    pub lasso_style: String,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            accent_color: "#080808".to_string(),
            spotlight_dim: 0.42,
            card_padding: 10.0,
            card_radius: 20.0,
            preset: "classic".to_string(),
            lasso_style: "rainbow".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct BehaviorSettings {
    pub auto_copy_on_select_release: bool,
    pub translate_target_lang: String,
    /// Leaves the mouse pointer out of the captured frame. Off by default:
    /// Windows' own screenshot tools include it, and it's what a user who has
    /// never opened Settings would expect.
    pub hide_cursor: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            hotkey: "shift+enter".to_string(),
            theme: ThemeSettings::default(),
            behavior: BehaviorSettings { auto_copy_on_select_release: false, translate_target_lang: "en".to_string(), hide_cursor: false },
            ai_provider: AiProvider::default(),
            custom_ai_url_template: String::new(),
            search_engine: SearchEngine::default(),
            custom_search_url_template: String::new(),
            choice_actions: CHOICE_ACTION_IDS.iter().map(|id| ActionEntry::on(id)).collect(),
            // In-place translation supersedes opening Google Translate, so
            // the old action ships disabled for new installs (still one
            // toggle away in Settings).
            text_actions: TEXT_ACTION_IDS
                .iter()
                .map(|id| ActionEntry { id: id.to_string(), enabled: *id != "translate" })
                .collect(),
            custom_actions: Vec::new(),
            launch_at_startup: false,
        }
    }
}

impl Settings {
    /// Repairs action lists after loading or receiving settings. Saved lists
    /// fully replace the defaults (`serde(default)` only fills *missing*
    /// fields), so without this a built-in action added in a later version
    /// would never appear for anyone who had already saved settings. Also
    /// drops unknown/duplicate ids and malformed custom actions, so the
    /// overlay can trust every id it's handed.
    pub fn normalize(&mut self) {
        // This file is hand-editable JSON and every field here can also
        // arrive from the WebView, so nothing in it is trusted. Case-
        // insensitive scheme check: `new URL("HTTPS://…").protocol` in the
        // settings-window validator normalizes to lowercase, so a template
        // typed with an uppercase scheme passes that check — matching it
        // here (rather than an exact-case `starts_with`) avoids silently
        // dropping a URL the UI just accepted as valid.
        let valid_scheme = |url: &str| {
            let lower = url.to_ascii_lowercase();
            lower.starts_with("https://") || lower.starts_with("http://")
        };
        // Search/custom-action templates are always substituted into a
        // query string, so `{q}` is required; an AI provider template may
        // legitimately be a bare homepage with no placeholder (main.ts
        // falls back to opening it as-is — the same "no query-prefill"
        // path used for Claude/Gemini), so it only needs a valid scheme.
        let valid_query_url = |url: &str| valid_scheme(url) && url.contains("{q}");

        if !self.custom_ai_url_template.is_empty() && !valid_scheme(&self.custom_ai_url_template) {
            self.custom_ai_url_template.clear();
        }
        if !self.custom_search_url_template.is_empty() && !valid_query_url(&self.custom_search_url_template) {
            self.custom_search_url_template.clear();
        }

        let mut seen_custom = std::collections::HashSet::new();
        self.custom_actions.retain(|action| {
            action.id.starts_with(CUSTOM_ACTION_PREFIX)
                && action.id.len() > CUSTOM_ACTION_PREFIX.len()
                && !action.label.trim().is_empty()
                && valid_query_url(&action.url_template)
                && seen_custom.insert(action.id.clone())
        });
        self.custom_actions.truncate(MAX_CUSTOM_ACTIONS);

        normalize_list(&mut self.choice_actions, CHOICE_ACTION_IDS, &[]);
        let custom_ids: Vec<&str> = self.custom_actions.iter().map(|a| a.id.as_str()).collect();
        normalize_list(&mut self.text_actions, TEXT_ACTION_IDS, &custom_ids);
    }
}

/// Keeps entries whose id is known (built-in or an existing custom action),
/// first occurrence wins, then appends any known id that's missing — enabled,
/// at the end, so an upgrade adds new actions without reshuffling the user's
/// chosen order.
fn normalize_list(list: &mut Vec<ActionEntry>, built_in: &[&str], custom: &[&str]) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|entry| {
        let known = built_in.contains(&entry.id.as_str()) || custom.contains(&entry.id.as_str());
        known && seen.insert(entry.id.clone())
    });
    for id in built_in.iter().chain(custom) {
        if !seen.contains(*id) {
            list.push(ActionEntry::on(id));
        }
    }
}

fn config_path(app: &AppHandle) -> anyhow::Result<PathBuf> {
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("settings.json"))
}

/// Whether a settings file already exists, i.e. whether this is a fresh
/// install rather than a returning launch. Must be called before anything
/// else touches the config path this session, since `save` (called for the
/// first run immediately after this check) creates the file.
pub fn config_file_exists(app: &AppHandle) -> bool {
    config_path(app).map(|path| path.exists()).unwrap_or(false)
}

/// Loads persisted settings, or defaults on first run / a corrupt file.
/// Never fails outward — a bad settings file should not stop the app from
/// starting. Call once at startup, off the hotkey path.
pub fn load(app: &AppHandle) -> Settings {
    let Ok(path) = config_path(app) else {
        tracing::warn!("could not resolve settings path, using defaults");
        return Settings::default();
    };
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(%err, "settings.json invalid, using defaults");
            Settings::default()
        }),
        Err(_) => Settings::default(), // first run: no file yet
    };
    settings.normalize();
    settings
}

pub fn save(app: &AppHandle, settings: &Settings) -> anyhow::Result<()> {
    let path = config_path(app)?;
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

/// Reads the in-memory copy held in `AppState` — never touches disk, so
/// it's cheap enough to call once from every window at startup.
#[tauri::command]
pub fn get_settings(state: tauri::State<AppState>) -> Settings {
    state.settings()
}

/// For the settings window's "Reset to defaults" button: hands back
/// `Settings::default()` without touching `AppState` or disk, so the form
/// can be repopulated without anything being persisted until Save.
#[tauri::command]
pub fn default_settings() -> Settings {
    Settings::default()
}

#[tauri::command]
pub async fn set_settings(app: AppHandle, state: tauri::State<'_, AppState>, mut settings: Settings) -> Result<(), String> {
    settings.normalize();
    let previous = state.settings();

    // The shortcut and launch-at-startup belong to Onyx now; whatever the
    // form sends for them is kept as it was.
    settings.hotkey = previous.hotkey.clone();
    settings.launch_at_startup = previous.launch_at_startup;
    state.set_settings(settings.clone());

    let app_for_save = app.clone();
    let to_save = settings.clone();
    tokio::task::spawn_blocking(move || save(&app_for_save, &to_save))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())?;

    app.emit("settings-changed", settings).map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_list_every_built_in_action() {
        let settings = Settings::default();
        let choice_ids: Vec<&str> = settings.choice_actions.iter().map(|a| a.id.as_str()).collect();
        let text_ids: Vec<&str> = settings.text_actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(choice_ids, CHOICE_ACTION_IDS);
        assert_eq!(text_ids, TEXT_ACTION_IDS);
        assert!(settings.choice_actions.iter().all(|a| a.enabled));
        // Only the old open-Google-Translate action starts off.
        let disabled: Vec<&str> = settings.text_actions.iter().filter(|a| !a.enabled).map(|a| a.id.as_str()).collect();
        assert_eq!(disabled, ["translate"]);
        assert_eq!(settings.theme.accent_color, "#080808");
        assert_eq!(settings.theme.preset, "classic");
        assert_eq!(settings.theme.lasso_style, "rainbow");
    }

    #[test]
    fn normalize_adds_built_ins_missing_from_an_older_save_without_reordering() {
        // A file saved before translate-inline/copy-code/redact existed,
        // with the user's own order. Missing built-ins are appended in
        // TEXT_ACTION_IDS order, not reshuffled into it.
        let json = r#"{"textActions":[{"id":"ask-ai","enabled":true},{"id":"copy","enabled":false},{"id":"search","enabled":true},{"id":"translate","enabled":true}]}"#;
        let mut settings: Settings = serde_json::from_str(json).unwrap();
        settings.normalize();
        let ids: Vec<(&str, bool)> = settings.text_actions.iter().map(|a| (a.id.as_str(), a.enabled)).collect();
        assert_eq!(
            ids,
            [
                ("ask-ai", true),
                ("copy", false),
                ("search", true),
                ("translate", true),
                ("copy-code", true),
                ("redact", true),
                ("translate-inline", true)
            ]
        );
    }

    #[test]
    fn normalize_drops_unknown_and_duplicate_ids() {
        let mut settings = Settings::default();
        settings.text_actions.insert(0, ActionEntry::on("no-such-action"));
        settings.text_actions.push(ActionEntry::on("copy"));
        settings.text_actions.push(ActionEntry::on("custom-gone"));
        settings.normalize();
        let ids: Vec<&str> = settings.text_actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, TEXT_ACTION_IDS);
    }

    #[test]
    fn normalize_keeps_valid_custom_actions_and_rejects_malformed_ones() {
        let custom = |id: &str, label: &str, url: &str| CustomAction {
            id: id.into(),
            label: label.into(),
            url_template: url.into(),
        };
        let mut settings = Settings {
            custom_actions: vec![
            custom("custom-jira", "Jira", "https://example.atlassian.net/issues/?jql=text~{q}"),
            custom("custom-jira", "Duplicate", "https://example.com/?q={q}"),
            custom("copy", "Hijack a built-in id", "https://example.com/?q={q}"),
            custom("custom-noq", "No placeholder", "https://example.com/"),
            custom("custom-js", "Script", "javascript:alert({q})"),
            custom("custom-blank", "  ", "https://example.com/?q={q}"),
            ],
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.custom_actions.len(), 1);
        assert_eq!(settings.custom_actions[0].label, "Jira");
        // The surviving custom action joins the text list so it can be ordered.
        assert_eq!(settings.text_actions.last().unwrap().id, "custom-jira");
        assert!(!settings.choice_actions.iter().any(|a| a.id.starts_with(CUSTOM_ACTION_PREFIX)));
    }

    #[test]
    fn normalize_accepts_an_uppercase_scheme() {
        // The settings-window validator parses with `new URL(...)`, whose
        // `.protocol` normalizes to lowercase, so a template typed/pasted
        // with an uppercase scheme passes there — normalize() must not then
        // turn around and silently drop it for being differently cased.
        let mut settings = Settings {
            custom_search_url_template: "HTTPS://example.com/search?q={q}".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.custom_search_url_template, "HTTPS://example.com/search?q={q}");
    }

    #[test]
    fn normalize_clears_an_invalid_ai_or_search_template_instead_of_rejecting_the_save() {
        let mut settings = Settings {
            custom_ai_url_template: "javascript:alert(1)".into(),
            custom_search_url_template: "https://example.com/search?q={q}".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.custom_ai_url_template, "");
        assert_eq!(settings.custom_search_url_template, "https://example.com/search?q={q}");
    }

    #[test]
    fn normalize_allows_an_ai_template_with_no_placeholder_but_requires_one_for_search() {
        // An AI provider template may be a bare homepage (main.ts falls back
        // to opening it as-is, same as the built-in Claude/Gemini entries);
        // a search template is always substituted into a query, so it can't.
        let mut settings = Settings {
            custom_ai_url_template: "https://example.com/chat".into(),
            custom_search_url_template: "https://example.com/search".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.custom_ai_url_template, "https://example.com/chat");
        assert_eq!(settings.custom_search_url_template, "");
    }

    #[test]
    fn round_trips_through_json() {
        let settings = Settings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hotkey, settings.hotkey);
        assert_eq!(restored.choice_actions.len(), settings.choice_actions.len());
    }

    #[test]
    fn chatgpt_provider_matches_the_action_bar_and_reads_older_saves() {
        assert_eq!(serde_json::to_string(&AiProvider::ChatGpt).unwrap(), "\"chatgpt\"");
        assert_eq!(serde_json::from_str::<AiProvider>("\"chat-gpt\"").unwrap(), AiProvider::ChatGpt);
    }

    #[test]
    fn tolerates_a_json_file_missing_fields_added_later() {
        // Simulates an old settings.json written before some field existed —
        // #[serde(default)] at the container level should fill it in from
        // Settings::default() rather than failing to deserialize.
        let partial = r#"{"hotkey": "ctrl+space"}"#;
        let settings: Settings = serde_json::from_str(partial).unwrap();
        assert_eq!(settings.hotkey, "ctrl+space");
        assert_eq!(settings.search_engine, SearchEngine::Google);
        assert_eq!(settings.choice_actions.len(), CHOICE_ACTION_IDS.len());
    }

    #[test]
    fn hide_cursor_defaults_off_and_round_trips() {
        // A behavior block saved before the option existed keeps everything
        // else it had and simply gets the pointer included, as before.
        let old = r#"{"behavior": {"autoCopyOnSelectRelease": true, "translateTargetLang": "de"}}"#;
        let settings: Settings = serde_json::from_str(old).unwrap();
        assert!(!settings.behavior.hide_cursor);
        assert!(settings.behavior.auto_copy_on_select_release);
        assert_eq!(settings.behavior.translate_target_lang, "de");
        assert!(!Settings::default().behavior.hide_cursor);

        let mut on = Settings::default();
        on.behavior.hide_cursor = true;
        let json = serde_json::to_string(&on).unwrap();
        assert!(json.contains(r#""hideCursor":true"#), "serialized as camelCase: {json}");
        assert!(serde_json::from_str::<Settings>(&json).unwrap().behavior.hide_cursor);
    }

    #[test]
    fn tolerates_an_empty_json_object() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.hotkey, Settings::default().hotkey);
    }
}
