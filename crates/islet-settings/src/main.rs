//! Onyx settings: a small egui window in its own process.
//!
//! It costs nothing while closed: the process exits and its memory is freed.
//! Every change is written atomically to `config.toml`, which the island
//! watches and hot-reloads, so there is no Save button.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod customize;
mod dictation_page;
mod preview;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use islet_core::music::DEFAULT_MUSIC_SITES;
use islet_core::Config;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

const WINDOW_TITLE: &str = "Onyx Settings";

fn main() -> eframe::Result {
    // One settings window per session: a second launch focuses the first.
    // SAFETY: plain named-mutex creation; the handle lives until exit.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\Islet.Settings")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: window lookup and activation by title.
        unsafe {
            if let Ok(hwnd) = FindWindowW(None, w!("Onyx Settings")) {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            if let Ok(handle) = mutex {
                let _ = CloseHandle(handle);
            }
        }
        return Ok(());
    }

    // `--page media|about` opens a specific page (used by the island).
    let page = match std::env::args()
        .skip_while(|arg| arg != "--page")
        .nth(1)
        .as_deref()
    {
        Some("media" | "now-playing") => Page::NowPlaying,
        Some("dictation" | "models") => Page::Dictation,
        Some("widgets") => Page::Widgets,
        Some("about") => Page::About,
        _ => Page::General,
    };
    let config_file = config_path();
    let config = Config::load(&config_file).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size([1040.0, 820.0])
            .with_min_inner_size([940.0, 720.0])
            .with_icon(Arc::new(window_icon())),
        ..Default::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, config_file, config, page)))),
    )
}

/// The Onyx app icon (`assets/onyx.ico` at 64px, raw RGBA).
fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../../../assets/onyx-64.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}

/// Same location the island uses (`%APPDATA%\Islet\config.toml`).
fn config_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Islet")
        .join("config.toml")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    NowPlaying,
    Dictation,
    Widgets,
    About,
}

pub struct SettingsApp {
    customization: Option<customize::Customization>,
    preview: preview::PreviewLink,
    page_changed: std::time::Instant,
    animations: bool,
    config_file: PathBuf,
    config: Config,
    saved: Config,
    page: Page,
    new_site: String,
    status: Option<String>,
    dictation_ui: dictation_page::DictationUi,
    /// One-shot initial scroll position (`--scroll <px>`, for development).
    initial_scroll: Option<f32>,
}

impl SettingsApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        config_file: PathBuf,
        config: Config,
        page: Page,
    ) -> Self {
        ui::install_theme(&cc.egui_ctx);
        ui::install_scrolling(&cc.egui_ctx);
        ui::install_titlebar_theme();
        Self {
            customization: None,
            preview: preview::PreviewLink::default(),
            page_changed: std::time::Instant::now(),
            animations: preview::animations_enabled(),
            config_file,
            saved: config.clone(),
            config,
            page,
            new_site: String::new(),
            status: None,
            dictation_ui: dictation_page::DictationUi::new(),
            initial_scroll: std::env::args()
                .skip_while(|arg| arg != "--scroll")
                .nth(1)
                .and_then(|value| value.parse().ok()),
        }
    }

    fn open_customization(&mut self, ctx: &egui::Context) {
        self.customization = Some(customize::Customization::new(&self.config.general, ctx));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        ctx.request_repaint();
    }

    /// Writes the config whenever an edit changed it. The island picks the
    /// file up through its directory watcher.
    fn save_if_changed(&mut self) {
        // Hover and transition repaints do not edit settings. Avoid cloning
        // dictionaries, snippets, and site lists on every one of those frames.
        if self.config == self.saved {
            return;
        }
        let mut normalized = self.config.clone();
        normalized.normalize();
        if normalized == self.saved {
            return;
        }
        let mut startup_error = None;
        if normalized.general.start_with_windows != self.saved.general.start_with_windows {
            if let Err(error) = autostart::set(normalized.general.start_with_windows) {
                startup_error = Some(format!("Could not change startup setting: {error}"));
                normalized.general.start_with_windows = self.saved.general.start_with_windows;
                self.config.general.start_with_windows = self.saved.general.start_with_windows;
            }
        }
        if let Some(parent) = self.config_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match normalized.save_atomic(&self.config_file) {
            Ok(()) => {
                self.saved = normalized;
                self.status = startup_error;
            }
            Err(error) => self.status = Some(format!("Could not save settings: {error}")),
        }
    }

    fn add_site(&mut self) {
        let site = self.new_site.trim().to_owned();
        if site.is_empty() {
            return;
        }
        let token = islet_core::music::site_token(&site);
        let duplicate = self
            .config
            .media
            .music_sites
            .iter()
            .any(|existing| islet_core::music::site_token(existing) == token);
        if !duplicate && !token.is_empty() {
            self.config.media.music_sites.push(site);
        }
        self.new_site.clear();
    }

    fn reset_sites(&mut self) {
        self.config.media.music_sites = DEFAULT_MUSIC_SITES
            .iter()
            .map(|site| (*site).to_owned())
            .collect();
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ui::draw(self, ctx);
        self.save_if_changed();
        self.preview.close_requested(ctx);
    }
}
