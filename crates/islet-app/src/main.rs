#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clock;
mod config_watch;
mod context_menu;
mod dictation;
mod converter;
mod file_drop;
mod icon;
mod last_music;
mod island_window;
mod logging;
mod occlusion;
mod paths;
mod restart;
mod settings;
mod sysstats;
mod tray;

use std::process::ExitCode;
use std::sync::{Arc, RwLock};

use islet_core::Config;
use paths::Paths;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

fn main() -> ExitCode {
    if let Err(error) = restart::wait_for_previous_instance() {
        eprintln!("could not restart Islet: {error}");
        return ExitCode::FAILURE;
    }
    let paths = Paths::discover();
    let _log_guard = match logging::init(&paths.log_dir) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("could not initialize Islet logging: {error}");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %paths.config_file.display(),
        data = %paths.data_dir.display(),
        models = %paths.model_dir.display(),
        "Islet starting"
    );

    match run(paths) {
        Ok(()) => {
            tracing::info!("Islet stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "Islet stopped with an error");
            ExitCode::FAILURE
        }
    }
}

fn run(paths: Paths) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let Some(_instance) = acquire_single_instance()? else {
        tracing::info!("another Islet instance is already running; exiting");
        return Ok(());
    };

    std::fs::create_dir_all(&paths.config_dir)?;
    std::fs::create_dir_all(&paths.data_dir)?;

    let existed = paths.config_file.exists();
    let config = match Config::load(&paths.config_file) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                path = %paths.config_file.display(),
                %error,
                "config could not be read; using defaults without overwriting the file"
            );
            Config::default()
        }
    };

    if !existed {
        config.save_atomic(&paths.config_file)?;
        tracing::info!(path = %paths.config_file.display(), "wrote default config");
        // First run after install: show the settings so the user can set
        // things up.
        settings::open(None);
    }

    let shared = Arc::new(RwLock::new(config));
    let mut demo = false;
    let mut demo_state = None;
    for arg in std::env::args().skip(1) {
        if arg == "--demo" {
            demo = true;
        } else if let Some(value) = arg.strip_prefix("--demo-state=") {
            match value.parse::<usize>() {
                Ok(step) => demo_state = Some(step % island_window::DEMO_STATES),
                Err(_) => eprintln!("ignoring invalid --demo-state value: {value}"),
            }
        }
    }
    tray::run(paths.config_file, shared, demo, demo_state)
}

/// Holds the per-session singleton mutex for the lifetime of the process.
struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn acquire_single_instance() -> Result<Option<InstanceGuard>, windows::core::Error> {
    // Local\ scopes the name to the current logon session, so two users can
    // each run their own Islet.
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\Islet.Singleton"))? };
    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        Ok(None)
    } else {
        Ok(Some(InstanceGuard(handle)))
    }
}
