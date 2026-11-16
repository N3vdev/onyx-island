//! The To-do page: the list itself (add, tick off, remove), how reminders
//! are written, and the focus timer's lengths.
//!
//! The list is `tasks.toml` beside the config; the island edits it too.
//! After a change here the island is told to reload; changes the island
//! made are picked up when the file's time changes.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use islet_core::tasks::{self, TaskList};
use windows::core::w;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};

use crate::ui::{bold_font, card, divider, header, row, toggle, DANGER, MUTED, TEXT};
use crate::SettingsApp;

pub struct TasksUi {
    path: PathBuf,
    list: TaskList,
    modified: Option<SystemTime>,
    draft: String,
}

impl Default for TasksUi {
    fn default() -> Self {
        Self::at(
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Islet")
                .join("tasks.toml"),
        )
    }
}

impl TasksUi {
    pub fn at(path: PathBuf) -> Self {
        Self { list: TaskList::load(&path), modified: modified(&path), path, draft: String::new() }
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

impl TasksUi {
    /// Picks up what the island changed.
    fn refresh(&mut self) {
        let now = modified(&self.path);
        if now != self.modified {
            self.list = TaskList::load(&self.path);
            self.modified = now;
        }
    }

    fn save(&mut self) {
        if self.list.save(&self.path).is_ok() {
            self.modified = modified(&self.path);
            tell_island();
        }
    }
}

fn tell_island() {
    // SAFETY: registers a message name and posts it to the island window.
    unsafe {
        let message = RegisterWindowMessageW(w!("Onyx.Tasks.Changed"));
        if let Ok(island) = FindWindowW(w!("IsletIslandWindow"), None) {
            let _ = PostMessageW(Some(island), message, WPARAM(0), LPARAM(0));
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// Seconds the local clock is ahead of UTC, to the minute.
fn utc_offset() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // SAFETY: no arguments; both return times by value.
    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let seconds = |t: windows::Win32::Foundation::SYSTEMTIME| {
        tasks::civil_seconds(t.wYear.into(), t.wMonth.into(), t.wDay.into(), t.wHour.into(), t.wMinute.into(), t.wSecond.into())
    };
    ((seconds(local) - seconds(utc)) as f64 / 60.0).round() as i64 * 60
}

/// Whether Windows shows 12-hour times.
fn twelve_hour() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let mut buffer = [0u16; 32];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: reads a short string into a local buffer of that size.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\International"),
            w!("sTimeFormat"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if read.is_err() {
        return true;
    }
    let format = String::from_utf16_lossy(&buffer[..buffer.iter().position(|&c| c == 0).unwrap_or(0)]);
    !format.contains('H')
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "To-do", "Tasks, reminders and focus time, right on your island.");
    let widgets = &mut app.config.widgets;
    card(ui, "To-do list", |ui| {
        row(
            ui,
            "To-do list",
            "A page of your tasks when you open the island. Scroll the opened island to reach it.",
            |ui| {
                toggle(ui, &mut widgets.tasks, "To-do list");
            },
        );
    });
    if !widgets.tasks {
        return;
    }

    let state = &mut app.tasks_ui;
    state.refresh();
    let (now, offset, twelve) = (now(), utc_offset(), twelve_hour());
    let mut changed = false;
    card(ui, "Your tasks", |ui| {
        ui.add_space(4.0);
        let mut add = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut state.draft)
                    .hint_text("Add a task, like \u{201c}call Sam at 5pm\u{201d}")
                    .desired_width(ui.available_width() - 70.0),
            );
            add = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            add |= ui.button("Add").clicked();
            if add {
                field.request_focus();
            }
        });
        let preview = tasks::parse(&state.draft, now, offset);
        if let Some(at) = preview.remind_at.filter(|_| !preview.text.is_empty()) {
            ui.label(
                RichText::new(format!("\u{23f0} Reminds you {}", lowercase_first(&tasks::when(at, now, offset, twelve))))
                    .small()
                    .color(MUTED),
            );
        }
        if add && state.list.add(&state.draft, now, offset).is_some() {
            state.draft.clear();
            changed = true;
        }
        ui.add_space(6.0);

        let mut ids: Vec<u64> = state.list.open().iter().map(|task| task.id).collect();
        ids.extend(state.list.tasks.iter().filter(|task| task.done).map(|task| task.id));
        if ids.is_empty() {
            ui.label(RichText::new("No tasks yet.").color(MUTED));
        }
        for id in ids {
            let Some(task) = state.list.get(id).cloned() else { continue };
            divider(ui);
            ui.horizontal(|ui| {
                let mut done = task.done;
                if ui.checkbox(&mut done, "").changed() {
                    state.list.toggle(id);
                    changed = true;
                }
                let text = RichText::new(&task.text).color(if task.done { MUTED } else { TEXT });
                ui.label(if task.done { text.strikethrough() } else { text });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Remove").clicked() {
                        state.list.remove(id);
                        changed = true;
                    }
                    if let Some(at) = task.remind_at.filter(|_| !task.done) {
                        let overdue = at <= now;
                        let label = if task.reminded && !overdue {
                            "Reminded".to_owned()
                        } else {
                            tasks::when(at, now, offset, twelve)
                        };
                        ui.label(RichText::new(label).small().color(if overdue { DANGER } else { MUTED }));
                    }
                });
            });
        }
        if state.list.done_count() > 0 {
            divider(ui);
            if ui.button("Clear completed").clicked() && state.list.clear_done() {
                changed = true;
            }
        }
    });
    if changed {
        state.save();
    }

    card(ui, "Focus timer", |ui| {
        row(ui, "Focus session", "How long each session lasts. Start one from the To-do page on the island.", |ui| {
            ui.add(egui::Slider::new(&mut widgets.focus_minutes, 5..=90).suffix(" min"));
        });
        divider(ui);
        row(ui, "Break", "The break offered when a session ends.", |ui| {
            ui.add(egui::Slider::new(&mut widgets.break_minutes, 1..=30).suffix(" min"));
        });
        divider(ui);
        row(ui, "Sound", "A chime with reminders, and when a session or break ends.", |ui| {
            toggle(ui, &mut widgets.task_sound, "Sound");
        });
    });

    ui.add_space(4.0);
    ui.label(RichText::new("Writing reminders").font(bold_font(14.0)));
    for example in [
        "\u{201c}stretch in 20 min\u{201d}, \u{201c}call back in 2 hours\u{201d}",
        "\u{201c}call Sam at 5pm\u{201d}, \u{201c}standup at 9:30\u{201d}",
        "\u{201c}pay rent tomorrow\u{201d} (9 AM), \u{201c}gym tomorrow 7am\u{201d}, \u{201c}call mum tonight\u{201d}",
    ] {
        ui.label(RichText::new(example).color(MUTED));
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "On the island: open it, scroll to To-do, click the box at the bottom and type. Enter adds the task; \
             Esc clears what you typed. Click a task to tick it off. When a reminder is due the island shows it, \
             with a Done button.",
        )
        .small()
        .color(MUTED),
    );
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        // "Tomorrow 9:00 AM" reads "tomorrow 9:00 AM"; a time reads "at 5:00 PM".
        Some(first) if first.is_ascii_digit() => format!("at {text}"),
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
