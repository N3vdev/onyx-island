//! Launching the settings window (`islet-settings.exe`, a separate process).

use std::process::Command;

/// Opens settings, optionally on a page (`"media"`, `"about"`). The settings
/// process is single-instance and focuses itself if already open.
pub fn open(page: Option<&str>) {
    match page {
        Some(page) => launch(&["--page", page]),
        None => launch(&[]),
    }
}

/// Opens the welcome tour (its own window, beside any open settings).
pub fn open_onboarding() {
    launch(&["--onboarding"]);
}

fn launch(args: &[&str]) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe.with_file_name("islet-settings.exe"),
        Err(error) => {
            tracing::warn!(%error, "could not locate the settings app");
            return;
        }
    };
    let mut command = Command::new(&exe);
    command.args(args);
    if let Err(error) = command.spawn() {
        tracing::warn!(%error, path = %exe.display(), "could not open settings");
    }
}
