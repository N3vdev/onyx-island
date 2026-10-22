use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use islet_core::Config;
use notify::{RecursiveMode, Watcher};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::tray::WM_APP_CONFIG_CHANGED;

/// Watches the config's parent directory and reloads on atomic writes.
///
/// Watching the directory (not the file) is deliberate: the settings process
/// writes `config.toml.tmp`, then renames it over `config.toml`. A file watch
/// would be invalidated by that rename.
pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
}

pub fn spawn(
    config_file: PathBuf,
    shared: Arc<RwLock<Config>>,
    hwnd: HWND,
) -> Result<ConfigWatcher, Box<dyn std::error::Error>> {
    let parent = config_file
        .parent()
        .ok_or("config file has no parent directory")?
        .to_owned();
    std::fs::create_dir_all(&parent)?;

    let (sender, receiver) = crossbeam_channel::unbounded::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = sender.send(result);
    })?;
    watcher.watch(&parent, RecursiveMode::NonRecursive)?;

    let watched_path = config_file.clone();
    let hwnd_value = hwnd.0 as isize;
    thread::Builder::new()
        .name("config-watch".to_owned())
        .spawn(move || {
            while let Ok(result) = receiver.recv() {
                match result {
                    Ok(event) => {
                        if !event
                            .paths
                            .iter()
                            .any(|path| paths_match(path, &watched_path))
                        {
                            continue;
                        }
                        // Coalesce the burst of events produced by an atomic
                        // replace and by editors that write in stages.
                        thread::sleep(Duration::from_millis(150));
                        while receiver.try_recv().is_ok() {}

                        match Config::load(&watched_path) {
                            Ok(new_config) => {
                                match shared.write() {
                                    Ok(mut current) => *current = new_config,
                                    Err(poisoned) => {
                                        tracing::error!("config lock poisoned; recovering");
                                        *poisoned.into_inner() = new_config;
                                    }
                                }
                                tracing::info!(
                                    path = %watched_path.display(),
                                    "config reloaded"
                                );
                                let target = HWND(hwnd_value as *mut _);
                                if let Err(error) = unsafe {
                                    PostMessageW(
                                        Some(target),
                                        WM_APP_CONFIG_CHANGED,
                                        WPARAM(0),
                                        LPARAM(0),
                                    )
                                } {
                                    tracing::warn!(%error, "could not notify UI of config change");
                                }
                            }
                            Err(error) => {
                                tracing::warn!(
                                    path = %watched_path.display(),
                                    %error,
                                    "config reload rejected; keeping previous config"
                                );
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "config watcher error");
                    }
                }
            }
        })?;

    Ok(ConfigWatcher { _watcher: watcher })
}

fn paths_match(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    // Some editors report the canonical path; compare case-insensitively on
    // Windows without requiring the file to exist right now.
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}
