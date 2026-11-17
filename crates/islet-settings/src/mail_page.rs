//! Gmail in Settings: connect one or more Google accounts, choose what shows
//! on the island (codes only, every new email, or a custom mix), and switch
//! it on or off. The island does the signing in and the checking; Settings
//! only asks it to.

use std::path::PathBuf;
use std::sync::Mutex;

use eframe::egui::{self, RichText};
use islet_core::config::MailShow;
use windows::core::w;

use crate::ui::{divider, row, segmented, toggle};
use crate::SettingsApp;

const MUTED: egui::Color32 = egui::Color32::from_rgb(150, 156, 172);
/// The island's own limit.
const MOST_ACCOUNTS: usize = 5;
/// Messages to the island: add an account, or remove the n-th.
const ADD: usize = 1;
const REMOVE_ACCOUNT: usize = 0x100;

/// What the island last wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Status {
    /// This build of Onyx can sign in to Google.
    available: bool,
    connecting: bool,
    accounts: Vec<String>,
}

/// The senders box as typed (the list itself is in the settings).
static SENDERS: Mutex<Option<String>> = Mutex::new(None);

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let status = status();
    crate::ui::card(ui, "Gmail", |ui| {
        let mail = &mut app.config.mail;
        let detail = if !status.available {
            "Gmail sign-in isn't set up in this build of Onyx."
        } else if status.accounts.is_empty() {
            "Show sign-in codes, or new emails, from Gmail on the island. Read-only: Onyx never sends or changes mail."
        } else {
            "Checked every 15 seconds while you use your PC. Codes stay a minute; click one to copy it."
        };
        row(ui, "Gmail on the island", detail, |ui| {
            toggle(ui, &mut mail.gmail_codes, "Gmail on the island");
        });
        if !status.available || !mail.gmail_codes {
            return;
        }

        // The accounts, each with Remove, then Add.
        for (index, email) in status.accounts.iter().enumerate() {
            divider(ui);
            let shown = if email.is_empty() { "Gmail account" } else { email.as_str() };
            row(ui, shown, "Connected", |ui| {
                if ui.button("Remove").clicked() {
                    tell_island(REMOVE_ACCOUNT + index);
                }
            });
        }
        divider(ui);
        let (title, about) = if status.accounts.is_empty() {
            ("Google account", "Sign in with Google and allow read-only access.")
        } else {
            ("Another account", "Codes and mail from every account show on the island.")
        };
        row(ui, title, about, |ui| {
            if status.connecting {
                ui.add_enabled(false, egui::Button::new("Connecting\u{2026}"));
            } else if status.accounts.len() >= MOST_ACCOUNTS {
                ui.label(RichText::new("Up to 5 accounts").small().color(MUTED));
            } else {
                let label = if status.accounts.is_empty() { "Connect Google account" } else { "Add account" };
                if ui.button(label).clicked() {
                    tell_island(ADD);
                }
            }
        });

        // What shows.
        divider(ui);
        let about = match mail.show {
            MailShow::Codes => "Only one-time sign-in codes.",
            MailShow::All => "Codes, and every new email in your inbox.",
            MailShow::Custom => "Codes and the emails you choose below.",
        };
        row(ui, "Show", about, |ui| {
            segmented(ui, &mut mail.show, &[(MailShow::Codes, "Codes only"), (MailShow::All, "All new mail"), (MailShow::Custom, "Custom")]);
        });
        if mail.show == MailShow::Custom {
            divider(ui);
            row(ui, "One-time codes", "Sign-in and verification codes, ready to copy.", |ui| {
                toggle(ui, &mut mail.custom_codes, "One-time codes");
            });
            divider(ui);
            row(ui, "Important mail", "Mail Gmail marks as important.", |ui| {
                toggle(ui, &mut mail.custom_important, "Important mail");
            });
            divider(ui);
            row(ui, "Primary inbox", "Everything in the Primary tab: not Promotions, Social, Updates or Forums.", |ui| {
                toggle(ui, &mut mail.custom_primary, "Primary inbox");
            });
            divider(ui);
            row(ui, "From these senders", "Addresses or domains, separated by commas: ana@example.com, github.com", |ui| {
                senders_box(ui, &mut mail.custom_senders);
            });
        }
    });
    // The island writes the status as sign-in goes.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn senders_box(ui: &mut egui::Ui, senders: &mut Vec<String>) {
    let Ok(mut draft) = SENDERS.lock() else { return };
    // Kept as typed while it says the same as the settings (so a trailing
    // comma survives); otherwise taken from them.
    let text = draft.get_or_insert_with(|| senders.join(", "));
    if parse_senders(text) != *senders {
        *text = senders.join(", ");
    }
    let edit = egui::TextEdit::singleline(text).hint_text("github.com").desired_width(220.0);
    if ui.add(edit).changed() {
        *senders = parse_senders(text);
    }
}

fn parse_senders(text: &str) -> Vec<String> {
    text.split([',', ';', ' '])
        .map(str::trim)
        .filter(|sender| !sender.is_empty())
        .map(str::to_ascii_lowercase)
        .take(50)
        .collect()
}

/// What the island last wrote (Settings never sees the keys).
fn status() -> Status {
    let path = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("Islet").join("gmail-status.txt");
    let Some(json) = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()) else {
        // Not written yet: the island hasn't started Gmail. Connecting will.
        return Status { available: true, ..Status::default() };
    };
    Status {
        available: json["available"].as_bool().unwrap_or(true),
        connecting: json["connecting"].as_bool().unwrap_or(false),
        accounts: json["accounts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|email| email.as_str().map(str::to_owned))
            .take(MOST_ACCOUNTS)
            .collect(),
    }
}

/// Asks the island to add an account (it opens Google's sign-in page) or to
/// forget one or all.
fn tell_island(what: usize) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};
    // SAFETY: registers a message name and posts it to the island window.
    unsafe {
        let message = RegisterWindowMessageW(w!("Onyx.Gmail"));
        if let Ok(island) = FindWindowW(w!("IsletIslandWindow"), None) {
            let _ = PostMessageW(Some(island), message, WPARAM(what), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senders_are_read_from_what_is_typed() {
        assert_eq!(parse_senders("Ana@Example.com, github.com,"), ["ana@example.com", "github.com"]);
        assert_eq!(parse_senders(" ; "), Vec::<String>::new());
    }
}
