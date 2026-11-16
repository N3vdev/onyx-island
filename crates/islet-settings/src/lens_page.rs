//! The Lens page: Onyx Lens on or off, its shortcut, and a way into the
//! lens's own settings (look, AI and search, actions), which live in the
//! helper's window.

use eframe::egui::{self, RichText};
use windows::core::w;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};

use crate::ui::{card, divider, header, row, toggle, MUTED};
use crate::SettingsApp;

fn helper() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?.with_file_name("onyx-lens.exe");
    exe.exists().then_some(exe)
}

/// Asks the running helper to open its settings window. Onyx creates the
/// event while the lens is on; if it cannot be opened, the helper is started
/// with `--settings` instead (a running one gets that through its
/// single-instance check).
fn open_lens_settings() {
    // SAFETY: opens, signals and closes a named event.
    let signalled = unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\Onyx.Lens.Settings")) {
            Ok(event) => {
                let done = SetEvent(event).is_ok();
                let _ = CloseHandle(event);
                done
            }
            Err(_) => false,
        }
    };
    if !signalled {
        if let Some(exe) = helper() {
            let _ = std::process::Command::new(exe).arg("--settings").spawn();
        }
    }
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(
        ui,
        "Onyx Lens",
        "Circle anything on your screen to copy its text, translate it, search it, or ask AI about it.",
    );
    let installed = helper().is_some();
    card(ui, "Onyx Lens", |ui| {
        row(
            ui,
            "Onyx Lens",
            "Freezes the screen when you press the shortcut, so you can draw around anything.",
            |ui| {
                toggle(ui, &mut app.config.lens.enabled, "Onyx Lens");
            },
        );
        if !app.config.lens.enabled {
            return;
        }
        divider(ui);
        crate::dictation_page::lens_hotkey_row(app, ui);
        divider(ui);
        row(
            ui,
            "Look, AI and actions",
            "The lens colours, which AI and search engine it uses, and which buttons it shows.",
            |ui| {
                let button = ui.add_enabled(installed, egui::Button::new("Open lens settings"));
                if button.clicked() {
                    open_lens_settings();
                }
            },
        );
        if !installed {
            ui.label(
                RichText::new("Onyx Lens isn't installed with this copy of Onyx.")
                    .small()
                    .color(MUTED),
            );
        }
    });
    ui.add_space(18.0);
    card(ui, "How to use it", |ui| {
        for line in [
            "Press the shortcut. The screen freezes, and the island moves to the top with the lens's tools.",
            "Draw around anything. Choose Select text, Translate, Search, Ask AI, Save or Pin.",
            "Or drag straight across text to select it. Esc or a right-click closes the lens.",
        ] {
            ui.label(RichText::new(line).color(MUTED));
        }
    });
}
