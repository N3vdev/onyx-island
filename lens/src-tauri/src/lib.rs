mod aichat;
mod state;
mod hotkey;
mod island;
mod monitor;
mod objectnet;
mod overlay_memory;
mod capture;
pub mod layout;
pub mod ocr;
pub mod selection;
mod codeformat;
mod commands;
mod settings;
mod entitlements;
// The trial and purchase flow, unused until Onyx Pro (see entitlements.rs).
#[allow(dead_code)]
mod license;
mod pin;
mod rectsnap;
mod screenlinks;
mod redact;
mod share;
mod snap;
mod translate;

use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;

/// Debug builds log to the console the dev workflow already reads. Release
/// builds have no console at all (`windows_subsystem = "windows"`), so
/// without a file every diagnostic — including "your shortcut could not be
/// registered" — would go nowhere, leaving a support report with nothing to
/// look at. Truncated on each start so it stays one session's worth rather
/// than growing unbounded.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    #[cfg(debug_assertions)]
    tracing_subscriber::fmt().with_env_filter(filter).init();

    #[cfg(not(debug_assertions))]
    match log_file() {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }

    // The default hook writes to a stderr nobody can see in release; keep it
    // for debug builds and add a trail that survives either way.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "unhandled panic");
        default_hook(info);
    }));
}

#[cfg(not(debug_assertions))]
fn log_file() -> Option<std::fs::File> {
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("com.circlesearch.app");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::File::create(dir.join("onyx-lens.log")).ok()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    tauri::Builder::default()
        // Registered first so a second launch exits at once. Onyx's settings
        // launch `onyx-lens.exe --settings` when the helper is not running
        // yet; if it is, that request arrives here.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|arg| arg == "--settings") {
                show_settings_window(app, None);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(state::AppState::default())
        .manage(translate::Translator::default())
        .manage(pin::PinStore::default())
        .manage(share::ShareSession::default())
        .on_window_event(|window, event| {
            if window.label() == "overlay" && matches!(event, tauri::WindowEvent::Focused(false)) {
                commands::dismiss_overlay_on_focus_loss(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            island::island_tools,
            commands::dismiss_overlay,
            commands::prepare_region,
            commands::save_selection_png,
            commands::reveal_saved_image,
            commands::recognize_text,
            commands::search_with_lens,
            settings::get_settings,
            settings::set_settings,
            settings::default_settings,
            entitlements::get_entitlements,
            entitlements::get_license_status,
            license::start_trial,
            license::begin_purchase,
            license::poll_purchase_status,
            license::restore_purchase,
            translate::translate_paragraphs,
            codeformat::format_as_code,
            redact::detect_sensitive,
            pin::create_pin,
            pin::get_pin_image,
            share::share_selection,
            snap::snap_selection,
            rectsnap::detect_region,
            screenlinks::scan_codes,
            screenlinks::classify_text,
            snap::brush_selection,
            aichat::open_ai_provider,
            aichat::schedule_ai_paste,
        ])
        .setup(|app| {
            // Part of Onyx: Onyx starts this helper, owns the shortcut and
            // ends it with itself, so there is no tray icon, autostart or
            // global shortcut here.
            ocr::log_available_languages();

            let first_run = !settings::config_file_exists(app.handle());
            let loaded = settings::load(app.handle());
            if first_run {
                if let Err(err) = settings::save(app.handle(), &loaded) {
                    tracing::warn!(%err, "failed to persist first-run settings");
                }
            }
            app.state::<state::AppState>().set_settings(loaded);
            app.state::<state::AppState>().set_entitlements(entitlements::resolve(app.handle()));

            hotkey::listen(app.handle())?;
            if std::env::args().any(|arg| arg == "--settings") {
                show_settings_window(app.handle(), None);
            }

            // A process's first WGC capture pays a one-time D3D/driver
            // warm-up cost; doing a throwaway one here means the user's
            // first real shortcut press doesn't have to pay it.
            std::thread::spawn(capture::warm_up);

            // The overlay starts hidden; let its WebView2 idle lean from the start.
            if let Some(overlay) = app.get_webview_window("overlay") {
                overlay_memory::set_idle(&overlay, true);
            }

            tracing::info!(first_run, "onyx lens ready");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Built lazily on first open rather than pre-created hidden like the
/// overlay. The overlay pays that RAM cost because it has a <100ms
/// hotkey-to-visible non-negotiable; the settings window has no such
/// requirement, so a second always-resident WebView2 instance would be pure
/// idle-RAM cost for a window that may never be opened in a given session.
///
/// `section`, when given, opens straight to that sidebar section (e.g.
/// `"plan"` for the trial-expired paywall) instead of whatever was last
/// active — via a URL hash on first build, or a `navigate-section` event
/// settings.ts listens for if the window already existed.
pub(crate) fn show_settings_window(app: &tauri::AppHandle, section: Option<&str>) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
        if let Some(section) = section {
            let _ = app.emit("navigate-section", section);
        }
        return;
    }
    let url = match section {
        Some(section) => format!("settings.html#{section}"),
        None => "settings.html".to_string(),
    };
    // Frameless: the window ships its own dark titlebar (settings.html) so
    // it matches the app's own chrome instead of a bright native OS bar.
    if let Err(err) = tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App(url.into()))
        .title("Onyx Lens Settings")
        .inner_size(1280.0, 720.0)
        .min_inner_size(680.0, 480.0)
        .center()
        .decorations(false)
        .background_color(tauri::webview::Color(15, 15, 15, 255))
        .visible(true)
        .build()
    {
        tracing::error!(%err, "failed to open settings window");
    }
}
