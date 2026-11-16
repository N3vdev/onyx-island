//! Gmail codes in Settings: connect a Google account, and switch it on or off.
//! The island does the signing in and the checking; Settings only asks it to.

use std::path::PathBuf;

use eframe::egui;
use windows::core::w;

use crate::ui::{row, toggle};
use crate::SettingsApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// This build of Onyx has no Google sign-in app.
    Unavailable,
    Disconnected,
    Connecting,
    Connected,
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let status = status();
    crate::ui::card(ui, "Gmail codes", |ui| {
        let detail = match status {
            Status::Unavailable => "Gmail sign-in isn't set up in this build of Onyx.",
            Status::Disconnected => {
                "Show sign-in codes from Gmail on the island. Read-only: Onyx never sends or changes mail."
            }
            Status::Connecting => "Finish signing in in your browser.",
            Status::Connected => "Connected. A code that arrives shows on the island for a minute; click it to copy.",
        };
        let enabled = &mut app.config.mail.gmail_codes;
        row(ui, "Gmail codes", detail, |ui| {
            ui.horizontal(|ui| {
                if *enabled && status != Status::Unavailable {
                    let connected = status == Status::Connected;
                    let label = match status {
                        Status::Connected => "Disconnect",
                        Status::Connecting => "Connecting\u{2026}",
                        _ => "Connect Google account",
                    };
                    if ui.button(label).clicked() {
                        tell_island(!connected);
                    }
                }
                toggle(ui, enabled, "Gmail codes");
            });
        });
    });
    // The island writes the status as sign-in goes.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

/// What the island last wrote (Settings never sees the sign-in itself).
fn status() -> Status {
    let path = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("Islet").join("gmail-status.txt");
    match std::fs::read_to_string(path).as_deref().map(str::trim) {
        Ok("connected") => Status::Connected,
        Ok("connecting") => Status::Connecting,
        Ok("no-client") => Status::Unavailable,
        _ => Status::Disconnected,
    }
}

/// Asks the island to connect (it opens Google's sign-in page) or to forget
/// the account.
fn tell_island(connect: bool) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};
    // SAFETY: registers a message name and posts it to the island window.
    unsafe {
        let message = RegisterWindowMessageW(w!("Onyx.Gmail"));
        if let Ok(island) = FindWindowW(w!("IsletIslandWindow"), None) {
            let _ = PostMessageW(Some(island), message, WPARAM(usize::from(connect)), LPARAM(0));
        }
    }
}
