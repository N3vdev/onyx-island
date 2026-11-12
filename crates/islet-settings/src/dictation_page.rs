//! The Dictation page: hotkeys, microphone, text options, and speech models.
//!
//! Model downloads, verification, and deletion run here (in the settings
//! process) on worker threads, so the island needs no protocol for them: it
//! simply finds the files on its next dictation.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use islet_asr::{AddOn, ModelManager, ModelSpec, ModelStatus, AI_CLEANUP, MODELS};
use islet_audio::{list_input_devices, InputDevice};
use islet_core::config::{InjectionMethod, Replacement};
use islet_core::recordings;
use islet_core::hotkey::{modifier_bit, name_for_vk, Combo, Modifiers, VK_ESCAPE};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use crate::ui::{card, divider, header, row, segmented, toggle};
use crate::SettingsApp;

const MUTED: Color32 = Color32::from_rgb(140, 145, 160);
const DANGER: Color32 = Color32::from_rgb(255, 110, 110);
const OK: Color32 = Color32::from_rgb(64, 210, 122);

#[derive(Clone, Copy, PartialEq, Eq)]
enum HotkeyTarget {
    PushToTalk,
    HandsFree,
    Assistant,
}

/// A hotkey being captured: held keys are polled until all are released.
struct Capture {
    target: HotkeyTarget,
    /// Every modifier key pressed during the capture, one bit per physical
    /// key (so Right Ctrl and Left Ctrl are different).
    down: u8,
    key: Option<u16>,
    seen_any: bool,
}

enum JobKind {
    Download,
    Verify,
}

/// A background download or verification.
struct Job {
    kind: JobKind,
    progress: Arc<Mutex<(u64, u64)>>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

pub struct DictationUi {
    manager: ModelManager,
    devices: Vec<InputDevice>,
    devices_error: Option<String>,
    devices_loaded: bool,
    capture: Option<Capture>,
    capture_error: Option<String>,
    jobs: Vec<(&'static str, Job)>,
    /// Latest outcome per model id, shown under its row.
    messages: Vec<(&'static str, String, bool)>,
    new_word: Replacement,
    new_snippet: Replacement,
    /// Saved corrections, re-read every few seconds while the page is open.
    learned: Learned,
    /// How many recordings are saved and their size, re-read the same way.
    recordings: RecordingsUsage,
    /// The "choose a folder" dialog for recordings, while it is open.
    folder_pick: Option<crate::folder_picker::FolderPick>,
    folder_error: Option<String>,
}

#[derive(Default)]
struct RecordingsUsage {
    read_at: Option<std::time::Instant>,
    dir: PathBuf,
    count: usize,
    bytes: u64,
}

impl RecordingsUsage {
    fn refresh(&mut self, dir: PathBuf) {
        if dir == self.dir && self.read_at.is_some_and(|at| at.elapsed() < Duration::from_secs(3)) {
            return;
        }
        self.read_at = Some(std::time::Instant::now());
        (self.count, self.bytes) = recordings::usage(&dir);
        self.dir = dir;
    }
}

/// Whether recordings can be written to `dir` (creating it if needed).
fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".onyx-write-check");
    std::fs::create_dir_all(dir).is_ok()
        && std::fs::write(&probe, b"").is_ok()
        && std::fs::remove_file(&probe).is_ok()
}

fn size_text(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

fn keep_text(days: u32) -> String {
    match days {
        0 => "Never".to_owned(),
        1 => "After 1 day".to_owned(),
        days => format!("After {days} days"),
    }
}

fn recordings_card(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let dictation = &mut app.config.dictation;
    let state = &mut app.dictation_ui;
    row(
        ui,
        "Save voice recordings",
        "Keep the audio of each dictation as a WAV file, about 2 MB a minute. Only the sound is saved, never the text. Assistant commands and discarded dictations are not saved.",
        |ui| {
            toggle(ui, &mut dictation.save_recordings, "Save voice recordings");
        },
    );
    let dir = recordings::dir(&dictation.recordings_folder);
    state.recordings.refresh(dir.clone());
    if !dictation.save_recordings && state.recordings.count == 0 {
        return;
    }
    if let Some(pick) = &state.folder_pick {
        if let Some(picked) = pick.take() {
            state.folder_pick = None;
            if let Some(folder) = picked {
                if writable(&folder) {
                    state.folder_error = None;
                    dictation.recordings_folder = if folder == recordings::default_dir() {
                        String::new()
                    } else {
                        folder.display().to_string()
                    };
                } else {
                    state.folder_error = Some(format!(
                        "Onyx can't save files in {}. Choose another folder.",
                        folder.display()
                    ));
                }
            }
        }
    }
    divider(ui);
    let custom = !dictation.recordings_folder.trim().is_empty();
    let location = if custom {
        dir.display().to_string()
    } else {
        format!("Onyx's data folder ({})", dir.display())
    };
    row(ui, "Save recordings in", &location, |ui| {
        let picking = state.folder_pick.is_some();
        let label = if picking { "Choosing\u{2026}" } else { "Change\u{2026}" };
        if ui
            .add_enabled(!picking, egui::Button::new(label))
            .on_hover_text("Recordings can get large, so pick a drive with room to spare")
            .clicked()
        {
            state.folder_pick = Some(crate::folder_picker::FolderPick::open(
                "Choose where to save voice recordings",
                &dir,
            ));
        }
        if custom && ui.button("Use default").clicked() {
            dictation.recordings_folder.clear();
            state.folder_error = None;
        }
        if ui.button("Open").clicked() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
        }
    });
    if let Some(error) = &state.folder_error {
        ui.label(RichText::new(error).small().color(DANGER));
    }
    divider(ui);
    row(
        ui,
        "Delete recordings",
        "Older recordings are deleted automatically. Only recordings Onyx saved are ever deleted, never your other files.",
        |ui| {
            egui::ComboBox::from_id_salt("delete_recordings_after")
                .selected_text(keep_text(dictation.delete_recordings_after_days))
                .width(140.0)
                .show_ui(ui, |ui| {
                    for days in [1, 3, 7, 30, 90, 0] {
                        ui.selectable_value(
                            &mut dictation.delete_recordings_after_days,
                            days,
                            keep_text(days),
                        );
                    }
                });
        },
    );
    let usage = &state.recordings;
    ui.add_space(4.0);
    ui.label(
        RichText::new(match usage.count {
            0 => "No recordings saved yet.".to_owned(),
            1 => format!("1 recording, {}", size_text(usage.bytes)),
            count => format!("{count} recordings, {}", size_text(usage.bytes)),
        })
        .small()
        .color(MUTED),
    );
}

/// What the corrections file holds: how many, and the word swaps you made
/// more than once (most frequent first).
#[derive(Default)]
struct Learned {
    read_at: Option<std::time::Instant>,
    count: usize,
    swaps: Vec<(String, String, usize)>,
}

fn corrections_file() -> PathBuf {
    // Tests point this at their own file, never the user's.
    if let Some(path) = std::env::var_os("ONYX_CORRECTIONS_FILE") {
        return PathBuf::from(path);
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Islet")
        .join("corrections.jsonl")
}

impl Learned {
    fn refresh(&mut self) {
        if self.read_at.is_some_and(|at| at.elapsed() < Duration::from_secs(3)) {
            return;
        }
        self.read_at = Some(std::time::Instant::now());
        let text = std::fs::read_to_string(corrections_file()).unwrap_or_default();
        let mut counts: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
        self.count = 0;
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            self.count += 1;
            for swap in record["swaps"].as_array().into_iter().flatten() {
                if let (Some(heard), Some(meant)) = (swap[0].as_str(), swap[1].as_str()) {
                    *counts.entry((heard.to_owned(), meant.to_owned())).or_default() += 1;
                }
            }
        }
        let mut swaps: Vec<_> = counts.into_iter().filter(|(_, n)| *n >= 2).map(|((h, m), n)| (h, m, n)).collect();
        swaps.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        swaps.truncate(8);
        self.swaps = swaps;
    }
}

fn models_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Islet")
        .join("models")
}

impl DictationUi {
    pub fn new() -> Self {
        Self {
            manager: ModelManager::new(models_dir()),
            devices: Vec::new(),
            devices_error: None,
            devices_loaded: false,
            capture: None,
            capture_error: None,
            jobs: Vec::new(),
            messages: Vec::new(),
            new_word: Replacement::default(),
            new_snippet: Replacement::default(),
            learned: Learned::default(),
            recordings: RecordingsUsage::default(),
            folder_pick: None,
            folder_error: None,
        }
    }

    fn refresh_devices(&mut self) {
        match list_input_devices() {
            Ok(devices) => {
                self.devices = devices;
                self.devices_error = None;
            }
            Err(error) => {
                self.devices.clear();
                self.devices_error = Some(error.to_string());
            }
        }
        self.devices_loaded = true;
    }

    pub(crate) fn manager(&self) -> &ModelManager {
        &self.manager
    }

    /// Bytes done and total of a running download, by model id.
    pub(crate) fn download_progress(&self, id: &str) -> Option<(u64, u64)> {
        self.job_for(id)
            .filter(|job| matches!(job.kind, JobKind::Download))
            .map(|job| job.progress.lock().map_or((0, 0), |p| *p))
    }

    /// Stops a running download; its partial files stay for a resume.
    pub(crate) fn cancel_download(&self, id: &str) {
        if let Some(job) = self.job_for(id) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The last outcome for a model: its text and whether it went well.
    pub(crate) fn message(&self, id: &str) -> Option<(String, bool)> {
        self.messages
            .iter()
            .find(|(existing, _, _)| *existing == id)
            .map(|(_, text, good)| (text.clone(), *good))
    }

    /// Microphones, listed on first use.
    pub(crate) fn microphones(&mut self) -> &[InputDevice] {
        if !self.devices_loaded {
            self.refresh_devices();
        }
        &self.devices
    }

    fn job_for(&self, id: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|(job_id, _)| *job_id == id)
            .map(|(_, job)| job)
    }

    fn set_message(&mut self, id: &'static str, text: String, good: bool) {
        self.messages.retain(|(existing, _, _)| *existing != id);
        self.messages.push((id, text, good));
    }

    /// Collects finished jobs into messages.
    pub(crate) fn poll_jobs(&mut self) {
        let mut finished = Vec::new();
        for (index, (id, job)) in self.jobs.iter().enumerate() {
            if let Ok(mut result) = job.result.lock() {
                if let Some(outcome) = result.take() {
                    finished.push((index, *id, matches!(job.kind, JobKind::Verify), outcome));
                }
            }
        }
        for (index, id, verify, outcome) in finished.into_iter().rev() {
            self.jobs.remove(index);
            match outcome {
                Ok(()) if verify => self.set_message(id, "Checksums match.".to_owned(), true),
                Ok(()) => self.set_message(id, "Downloaded and verified.".to_owned(), true),
                Err(text) => self.set_message(id, text, false),
            }
        }
    }

    pub(crate) fn start_download(&mut self, spec: &'static ModelSpec) {
        let job = Job {
            kind: JobKind::Download,
            progress: Arc::new(Mutex::new((0, spec.total_size()))),
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
        };
        let (progress, cancel, result) = (
            Arc::clone(&job.progress),
            Arc::clone(&job.cancel),
            Arc::clone(&job.result),
        );
        let manager = ModelManager::new(self.manager.root());
        std::thread::spawn(move || {
            let outcome = manager
                .download(
                    spec,
                    &mut |p| {
                        if let Ok(mut shared) = progress.lock() {
                            *shared = (p.done, p.total);
                        }
                    },
                    &cancel,
                )
                .map_err(|error| error.to_string());
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        });
        self.messages.retain(|(id, _, _)| *id != spec.id);
        self.jobs.push((spec.id, job));
    }

    pub(crate) fn start_addon_download(&mut self, addon: &'static AddOn) {
        let job = Job {
            kind: JobKind::Download,
            progress: Arc::new(Mutex::new((0, addon.total_size()))),
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
        };
        let (progress, cancel, result) = (
            Arc::clone(&job.progress),
            Arc::clone(&job.cancel),
            Arc::clone(&job.result),
        );
        let manager = ModelManager::new(self.manager.root());
        std::thread::spawn(move || {
            let outcome = manager
                .download_addon(
                    addon,
                    &mut |p| {
                        if let Ok(mut shared) = progress.lock() {
                            *shared = (p.done, p.total);
                        }
                    },
                    &cancel,
                )
                .map_err(|error| error.to_string());
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        });
        self.messages.retain(|(id, _, _)| *id != addon.id);
        self.jobs.push((addon.id, job));
    }

    fn start_verify(&mut self, spec: &'static ModelSpec) {
        let job = Job {
            kind: JobKind::Verify,
            progress: Arc::new(Mutex::new((0, 0))),
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
        };
        let result = Arc::clone(&job.result);
        let manager = ModelManager::new(self.manager.root());
        std::thread::spawn(move || {
            let outcome = manager.verify(spec).map_err(|error| error.to_string());
            if let Ok(mut slot) = result.lock() {
                *slot = Some(outcome);
            }
        });
        self.messages.retain(|(id, _, _)| *id != spec.id);
        self.jobs.push((spec.id, job));
    }

    /// Advances an in-progress hotkey capture by polling the keyboard.
    /// Returns the finished combo text, if the user completed one.
    fn poll_capture(&mut self) -> Option<(HotkeyTarget, String)> {
        let capture = self.capture.as_mut()?;
        let held = |vk: u16| -> bool {
            // SAFETY: reads global key state.
            unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
        };

        let mut down_now = 0u8;
        for vk in [0xA0u16, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C] {
            if held(vk) {
                down_now |= modifier_bit(vk).unwrap_or(0);
            }
        }
        let mut key_now = None;
        for vk in 0x08u16..=0xFE {
            let is_modifier = matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C);
            if !is_modifier && held(vk) {
                key_now = Some(vk);
                break;
            }
        }

        match step_capture(capture, down_now, key_now) {
            CaptureStep::Continue => None,
            CaptureStep::Cancelled => {
                self.capture = None;
                None
            }
            CaptureStep::Finished(result) => {
                let target = capture.target;
                self.capture = None;
                match result {
                    Ok(text) => {
                        self.capture_error = None;
                        Some((target, text))
                    }
                    Err(message) => {
                        self.capture_error = Some(message);
                        None
                    }
                }
            }
        }
    }
}

enum CaptureStep {
    Continue,
    Cancelled,
    /// The user let go of everything: the combo text, or why it is invalid.
    Finished(Result<String, String>),
}

/// One poll of a hotkey capture. Pure: the keyboard is read by the caller.
/// `down_now` is the held modifier keys (one bit per physical key). The combo
/// is everything pressed during the capture, finished when all keys are
/// released; Esc cancels.
fn step_capture(capture: &mut Capture, down_now: u8, key_now: Option<u16>) -> CaptureStep {
    if key_now == Some(VK_ESCAPE) {
        return CaptureStep::Cancelled;
    }
    capture.down |= down_now;
    if key_now.is_some() {
        capture.key = key_now;
    }
    if down_now != 0 || key_now.is_some() {
        capture.seen_any = true;
        return CaptureStep::Continue;
    }
    if !capture.seen_any {
        return CaptureStep::Continue;
    }

    let combo = Combo {
        modifiers: Modifiers::from_capture(capture.down, capture.key.is_some()),
        key: capture.key,
    };
    if combo.key.is_some_and(|vk| name_for_vk(vk).is_none()) {
        return CaptureStep::Finished(Err("That key isn't supported. Try another.".to_owned()));
    }
    let text = combo.to_string();
    CaptureStep::Finished(
        Combo::parse(&text)
            .map(|_| text)
            .map_err(|error| error.to_string()),
    )
}

/// A small "remove" cross drawn with lines, so it does not depend on the
/// font having a glyph for it.
fn cross_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(8),
                Color32::from_white_alpha(18),
            );
        }
        let color = if hovered {
            Color32::from_rgb(236, 237, 242)
        } else {
            MUTED
        };
        let stroke = egui::Stroke::new(1.6, color);
        let c = rect.center();
        let d = 4.5;
        ui.painter()
            .line_segment([c + egui::vec2(-d, -d), c + egui::vec2(d, d)], stroke);
        ui.painter()
            .line_segment([c + egui::vec2(-d, d), c + egui::vec2(d, -d)], stroke);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn hotkey_row(
    app: &mut SettingsApp,
    ui: &mut egui::Ui,
    target: HotkeyTarget,
    title: &str,
    description: &str,
) {
    let capturing = app
        .dictation_ui
        .capture
        .as_ref()
        .is_some_and(|capture| capture.target == target);
    let current = match target {
        HotkeyTarget::PushToTalk => app.config.dictation.push_to_talk.clone(),
        HotkeyTarget::HandsFree => app.config.dictation.hands_free.clone(),
        HotkeyTarget::Assistant => app.config.dictation.assistant_hotkey.clone(),
    };
    let is_set = !current.trim().is_empty();
    let mut start_capture = false;
    let mut clear = false;
    row(ui, title, description, |ui| {
        // Right to left: the clear button sits at the right edge.
        if is_set
            && cross_button(ui)
                .on_hover_text("Remove this hotkey")
                .clicked()
        {
            clear = true;
        }
        let text = if capturing {
            RichText::new("Press the keys\u{2026}").strong()
        } else if is_set {
            RichText::new(current).strong()
        } else {
            RichText::new("Not set").color(MUTED)
        };
        let button = egui::Button::new(text)
            .min_size(egui::vec2(170.0, 30.0))
            .selected(capturing);
        if ui
            .add(button)
            .on_hover_text("Click, then press a key or a combination")
            .clicked()
        {
            start_capture = true;
        }
    });
    if clear {
        match target {
            HotkeyTarget::PushToTalk => app.config.dictation.push_to_talk.clear(),
            HotkeyTarget::HandsFree => app.config.dictation.hands_free.clear(),
            HotkeyTarget::Assistant => app.config.dictation.assistant_hotkey.clear(),
        }
        app.dictation_ui.capture = None;
        app.dictation_ui.capture_error = None;
    }
    if start_capture && !capturing {
        app.dictation_ui.capture_error = None;
        app.dictation_ui.capture = Some(Capture {
            target,
            down: 0,
            key: None,
            seen_any: false,
        });
    }
}

fn entries_editor(
    ui: &mut egui::Ui,
    id: &str,
    entries: &mut Vec<Replacement>,
    draft: &mut Replacement,
    from_hint: &str,
    to_hint: &str,
) {
    let mut remove = None;
    for (index, entry) in entries.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&entry.from).strong());
            ui.label(RichText::new("\u{2192}").color(MUTED));
            ui.label(&entry.to);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if cross_button(ui).on_hover_text("Remove").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        entries.remove(index);
    }
    ui.horizontal(|ui| {
        let half = (ui.available_width() - 120.0) / 2.0;
        ui.add(
            egui::TextEdit::singleline(&mut draft.from)
                .id_salt((id, "from"))
                .hint_text(from_hint)
                .desired_width(half),
        );
        ui.add(
            egui::TextEdit::singleline(&mut draft.to)
                .id_salt((id, "to"))
                .hint_text(to_hint)
                .desired_width(half),
        );
        let ready = !draft.from.trim().is_empty() && !draft.to.is_empty();
        if ui.add_enabled(ready, egui::Button::new("Add")).clicked() {
            entries.push(Replacement {
                from: draft.from.trim().to_owned(),
                to: draft.to.clone(),
            });
            *draft = Replacement::default();
        }
    });
}

fn model_card(app: &mut SettingsApp, ui: &mut egui::Ui, spec: &'static ModelSpec) {
    let status = app.dictation_ui.manager.status(spec);
    let job_state = app.dictation_ui.job_for(spec.id).map(|job| {
        let (done, total) = job.progress.lock().map_or((0, 0), |p| *p);
        (matches!(job.kind, JobKind::Download), done, total)
    });
    let message = app
        .dictation_ui
        .messages
        .iter()
        .find(|(id, _, _)| *id == spec.id)
        .map(|(_, text, good)| (text.clone(), *good));

    let mut download = false;
    let mut cancel = false;
    let mut delete = false;
    let mut verify = false;

    // A model that isn't downloaded is drawn faded and can't be chosen; its
    // Download button (on the right) stays at full strength.
    let installed = matches!(status, ModelStatus::Installed);
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            if !installed {
                ui.multiply_opacity(0.4);
            }
            ui.add_enabled_ui(installed, |ui| {
                ui.radio_value(&mut app.config.dictation.model, spec.id.to_owned(), "")
            })
            .inner
            .on_disabled_hover_text("Download this model to use it.");
            ui.vertical(|ui| {
                ui.label(RichText::new(spec.name).strong());
                ui.label(
                    RichText::new(format!(
                        "{} \u{b7} {} \u{b7} {} \u{b7} {} MB",
                        spec.maker,
                        spec.languages,
                        spec.license,
                        spec.total_size() / 1_000_000
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.label(RichText::new(spec.summary).small().color(MUTED));
            });
        });
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| match (job_state, status) {
                (Some((true, done, total)), _) => {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(150.0)
                            .text(format!("{} / {} MB", done / 1_000_000, total / 1_000_000)),
                    );
                }
                (Some((false, ..)), _) => {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new("Checking\u{2026}").color(MUTED));
                }
                (None, ModelStatus::Installed) => {
                    if ui.button("Delete").clicked() {
                        delete = true;
                    }
                    if ui.button("Verify").clicked() {
                        verify = true;
                    }
                    ui.label(RichText::new("Installed").color(OK));
                }
                (None, ModelStatus::Partial { bytes }) => {
                    if ui.button("Resume").clicked() {
                        download = true;
                    }
                    ui.label(
                        RichText::new(format!("{} MB downloaded", bytes / 1_000_000)).color(MUTED),
                    );
                }
                (None, ModelStatus::Missing) => {
                    if ui
                        .button(format!("Download ({} MB)", spec.total_size() / 1_000_000))
                        .clicked()
                    {
                        download = true;
                    }
                }
            },
        );
    });
    if let Some((text, good)) = message {
        ui.label(
            RichText::new(text)
                .small()
                .color(if good { OK } else { DANGER }),
        );
    }

    if download {
        app.dictation_ui.start_download(spec);
    }
    if cancel {
        if let Some(job) = app.dictation_ui.job_for(spec.id) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
    if verify {
        app.dictation_ui.start_verify(spec);
    }
    if delete {
        match app.dictation_ui.manager.delete(spec) {
            Ok(()) => {
                app.dictation_ui
                    .set_message(spec.id, "Removed from this PC.".to_owned(), true)
            }
            Err(error) => {
                app.dictation_ui
                    .set_message(spec.id, format!("Could not remove: {error}"), false)
            }
        }
    }
}

/// The AI cleanup download row and its on/off switch.
fn ai_cleanup_card(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let addon = &AI_CLEANUP;
    let status = app.dictation_ui.manager.addon_status(addon);
    let job_state = app.dictation_ui.job_for(addon.id).map(|job| {
        let (done, total) = job.progress.lock().map_or((0, 0), |p| *p);
        (done, total)
    });
    let message = app
        .dictation_ui
        .messages
        .iter()
        .find(|(id, _, _)| *id == addon.id)
        .map(|(_, text, good)| (text.clone(), *good));
    let download_size = app.dictation_ui.manager.addon_download_size(addon);
    let size_mb = download_size / 1_000_000;

    ui.label(
        RichText::new(
            "Onyx Cleanup runs Gemma 3 270M locally on your CPU. Long dictation is processed in short chunks, \
             with correction rules and fact checks before insertion. Rejected chunks keep the rule-cleaned text. \
             Download the 278 MiB model once to use AI cleanup offline. Cleanup quality and time vary with the passage."
        )
        .small()
        .color(MUTED),
    );
    ui.add_space(6.0);

    let mut download = false;
    let mut cancel = false;
    let mut delete = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(addon.name).strong());
            ui.label(
                RichText::new(format!(
                    "{} \u{b7} {} \u{b7} {size_mb} MB",
                    addon.maker, addon.license
                ))
                .small()
                .color(MUTED),
            );
            ui.label(RichText::new(addon.summary).small().color(MUTED));
        });
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| match (job_state, status) {
                (Some((done, total)), _) => {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(150.0)
                            .text(format!("{} / {} MB", done / 1_000_000, total / 1_000_000)),
                    );
                }
                (None, ModelStatus::Installed) => {
                    if ui.button("Delete").clicked() {
                        delete = true;
                    }
                    ui.label(RichText::new("Installed").color(OK));
                }
                (None, ModelStatus::Partial { bytes }) => {
                    if ui.button("Resume").clicked() {
                        download = true;
                    }
                    ui.label(
                        RichText::new(format!("{} MB downloaded", bytes / 1_000_000)).color(MUTED),
                    );
                }
                (None, ModelStatus::Missing) => {
                    if ui.button(format!("Install ({size_mb} MB)")).clicked() {
                        download = true;
                    }
                }
            },
        );
    });
    if let Some((text, good)) = message {
        ui.label(
            RichText::new(text)
                .small()
                .color(if good { OK } else { DANGER }),
        );
    }
    ui.add_space(6.0);
    divider(ui);
    let installed = status == ModelStatus::Installed;
    let dictation = &mut app.config.dictation;
    row(
        ui,
        "Rewrite with AI",
        if installed {
            "If a rewrite is slow or looks wrong, the normal cleaned text is typed instead."
        } else {
            "Install the local model supplied with this build to turn this on."
        },
        |ui| {
            ui.add_enabled_ui(installed, |ui| {
                toggle(ui, &mut dictation.ai_cleanup, "Rewrite with AI");
            });
        },
    );

    if download {
        app.dictation_ui.start_addon_download(addon);
    }
    if cancel {
        if let Some(job) = app.dictation_ui.job_for(addon.id) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
    if delete {
        // The switch keeps its position: without the model it does nothing,
        // and a later download picks up where the user left it.
        match app.dictation_ui.manager.delete_addon(addon) {
            Ok(()) => {
                app.dictation_ui
                    .set_message(addon.id, "Removed from this PC.".to_owned(), true)
            }
            Err(error) => {
                app.dictation_ui
                    .set_message(addon.id, format!("Could not remove: {error}"), false)
            }
        }
    }
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.dictation_ui.poll_jobs();
    if let Some((target, text)) = app.dictation_ui.poll_capture() {
        match target {
            HotkeyTarget::PushToTalk => app.config.dictation.push_to_talk = text,
            HotkeyTarget::HandsFree => app.config.dictation.hands_free = text,
            HotkeyTarget::Assistant => app.config.dictation.assistant_hotkey = text,
        }
    }
    if app.dictation_ui.capture.is_some()
        || !app.dictation_ui.jobs.is_empty()
        || app.dictation_ui.folder_pick.is_some()
    {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    if !app.dictation_ui.devices_loaded {
        app.dictation_ui.refresh_devices();
    }

    header(
        ui,
        "Dictation",
        "Hold a key, speak, and your words appear wherever you are typing.",
    );

    card(ui, "Voice typing", |ui| {
        row(
            ui,
            "Dictation",
            "Turn voice typing on or off. You can also pause it from the tray menu.",
            |ui| {
                toggle(ui, &mut app.config.dictation.enabled, "Dictation");
            },
        );
    });

    card(ui, "Hotkeys", |ui| {
        hotkey_row(
            app,
            ui,
            HotkeyTarget::PushToTalk,
            "Hotkey",
            "Hold to speak, release to insert. Press Esc while recording to cancel.",
        );
        divider(ui);
        row(
            ui,
            "Hands-free mode",
            "Use a second hotkey that starts recording on one press and stops it on the next.",
            |ui| {
                toggle(
                    ui,
                    &mut app.config.dictation.hands_free_enabled,
                    "Hands-free mode",
                );
            },
        );
        if app.config.dictation.hands_free_enabled {
            divider(ui);
            hotkey_row(
                app,
                ui,
                HotkeyTarget::HandsFree,
                "Hands-free hotkey",
                "Press once to start, press again to stop. Esc cancels.",
            );
        } else if app
            .dictation_ui
            .capture
            .as_ref()
            .is_some_and(|capture| capture.target == HotkeyTarget::HandsFree)
        {
            app.dictation_ui.capture = None;
        }
        if let Some(error) = &app.dictation_ui.capture_error {
            ui.label(RichText::new(error).small().color(DANGER));
        }
        let hands_free_usable = app.config.dictation.hands_free_enabled
            && !app.config.dictation.hands_free.trim().is_empty();
        if app.config.dictation.push_to_talk.trim().is_empty() && !hands_free_usable {
            ui.label(
                RichText::new("No hotkey is set, so dictation can't be started.")
                    .small()
                    .color(DANGER),
            );
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Use a single key (Right Ctrl, Right Alt, F9, CapsLock...) or a combination. \
                 In a combination, left-side keys work on either side; right-side keys match \
                 only the right one.",
            )
            .small()
            .color(MUTED),
        );
    });

    card(ui, "Voice assistant", |ui| {
        row(
            ui,
            "Voice assistant",
            "Hold the assistant hotkey and say what to do. It works offline, and nothing you say leaves this PC.",
            |ui| {
                toggle(ui, &mut app.config.dictation.assistant_enabled, "Voice assistant");
            },
        );
        if app.config.dictation.assistant_enabled {
            divider(ui);
            hotkey_row(
                app,
                ui,
                HotkeyTarget::Assistant,
                "Assistant hotkey",
                "Hold to speak a command, release to run it. The bars turn lavender while it listens.",
            );
            ui.add_space(4.0);
            for (what, examples) in [
                ("Apps", "\u{201c}Open Spotify\u{201d}, \u{201c}switch to Chrome\u{201d}, \u{201c}close Notepad\u{201d}"),
                ("Music", "\u{201c}Play\u{201d}, \u{201c}pause\u{201d}, \u{201c}next song\u{201d}, \u{201c}play Blinding Lights on Spotify\u{201d}"),
                ("Volume", "\u{201c}Volume up\u{201d}, \u{201c}set volume to 40\u{201d}, \u{201c}mute\u{201d}"),
                ("Typing", "\u{201c}Type see you at five\u{201d} types into the window you're in"),
                ("Web", "\u{201c}Search for flights to Dubai\u{201d}, \u{201c}search lo-fi on YouTube\u{201d}, \u{201c}open Gmail\u{201d}"),
                ("Windows", "\u{201c}Minimize this\u{201d}, \u{201c}show desktop\u{201d}, \u{201c}take a screenshot\u{201d}, \u{201c}lock my PC\u{201d}"),
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(what).small().strong());
                    ui.label(RichText::new(examples).small().color(MUTED));
                });
            }
        } else if app
            .dictation_ui
            .capture
            .as_ref()
            .is_some_and(|capture| capture.target == HotkeyTarget::Assistant)
        {
            app.dictation_ui.capture = None;
        }
    });

    card(ui, "Learn from corrections", |ui| {
        row(
            ui,
            "Learn from my corrections",
            "After a dictation, notice words you change in what Onyx typed. Only the dictated sentence and your edit are kept, on this PC. Password fields and other windows are never read.",
            |ui| {
                toggle(ui, &mut app.config.dictation.learn_corrections, "Learn from my corrections");
            },
        );
        app.dictation_ui.learned.refresh();
        let learned = &app.dictation_ui.learned;
        if learned.count > 0 || app.config.dictation.learn_corrections {
            divider(ui);
            let mut delete = false;
            row(
                ui,
                &format!("{} correction{} saved", learned.count, if learned.count == 1 { "" } else { "s" }),
                "Kept in corrections.jsonl in Onyx's data folder. Used to suggest dictionary words below, and later to train the cleanup model.",
                |ui| {
                    if learned.count > 0 && ui.button("Delete all").clicked() {
                        delete = true;
                    }
                    if ui.button("Show file").clicked() {
                        let file = corrections_file();
                        if file.exists() {
                            let _ = std::process::Command::new("explorer").arg("/select,").arg(&file).spawn();
                        } else if let Some(folder) = file.parent() {
                            let _ = std::process::Command::new("explorer").arg(folder).spawn();
                        }
                    }
                },
            );
            if delete {
                let _ = std::fs::remove_file(corrections_file());
                app.dictation_ui.learned = Learned::default();
            }
        }
        let suggestions: Vec<(String, String, usize)> = app
            .dictation_ui
            .learned
            .swaps
            .iter()
            .filter(|(heard, meant, _)| {
                !app.config.dictation.dictionary.iter().any(|entry| entry.from.eq_ignore_ascii_case(heard) && entry.to.eq_ignore_ascii_case(meant))
            })
            .cloned()
            .collect();
        if !suggestions.is_empty() {
            divider(ui);
            ui.label(RichText::new("Words you often correct").small().strong());
            for (heard, meant, times) in suggestions {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("\u{201c}{heard}\u{201d} corrected to \u{201c}{meant}\u{201d}, {times} times")).small());
                    if ui.small_button("Add to dictionary").on_hover_text("Always write this word as you corrected it").clicked() {
                        app.config.dictation.dictionary.push(Replacement { from: heard.clone(), to: meant.clone() });
                    }
                });
            }
        }
    });

    card(ui, "Recordings", |ui| recordings_card(app, ui));

    card(ui, "Microphone", |ui| {
        row(
            ui,
            "Pause media during dictation",
            "Pause playing music and videos when recording starts. Resume them yourself afterward.",
            |ui| {
                toggle(
                    ui,
                    &mut app.config.dictation.pause_media,
                    "Pause media during dictation",
                );
            },
        );
        divider(ui);
        let devices = app.dictation_ui.devices.clone();
        let selected_name = if app.config.dictation.microphone.is_empty() {
            "System default".to_owned()
        } else {
            devices
                .iter()
                .find(|device| device.id == app.config.dictation.microphone)
                .map_or_else(
                    || "Unavailable device".to_owned(),
                    |device| device.name.clone(),
                )
        };
        let mut refresh = false;
        row(
            ui,
            "Input device",
            "The microphone used for dictation. \"System default\" follows Windows.",
            |ui| {
                if ui
                    .add(egui::Button::new("\u{21bb}").frame(false))
                    .on_hover_text("Refresh the list")
                    .clicked()
                {
                    refresh = true;
                }
                egui::ComboBox::from_id_salt("microphone")
                    .selected_text(selected_name)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut app.config.dictation.microphone,
                            String::new(),
                            "System default",
                        );
                        for device in &devices {
                            ui.selectable_value(
                                &mut app.config.dictation.microphone,
                                device.id.clone(),
                                &device.name,
                            );
                        }
                    });
            },
        );
        if let Some(error) = &app.dictation_ui.devices_error {
            ui.label(RichText::new(error).small().color(DANGER));
        }
        if refresh {
            app.dictation_ui.refresh_devices();
        }
    });

    card(ui, "Text", |ui| {
        let dictation = &mut app.config.dictation;
        row(
            ui,
            "Remove filler words",
            "Drops \"um\", \"uh\", and a leading \"you know\".",
            |ui| {
                toggle(ui, &mut dictation.filler_removal, "Remove filler words");
            },
        );
        divider(ui);
        row(
            ui,
            "Spoken punctuation",
            "Say \"new line\", \"new paragraph\", \"comma\", \"question mark\", and so on.",
            |ui| {
                toggle(ui, &mut dictation.spoken_punctuation, "Spoken punctuation");
            },
        );
        divider(ui);
        row(
            ui,
            "Space after each dictation",
            "Keeps back-to-back dictations from running together.",
            |ui| {
                toggle(
                    ui,
                    &mut dictation.trailing_space,
                    "Space after each dictation",
                );
            },
        );
        divider(ui);
        row(
            ui,
            "How text is inserted",
            "Paste inserts through the clipboard. Type works where pasting does not.",
            |ui| {
                segmented(
                    ui,
                    &mut dictation.injection,
                    &[
                        (InjectionMethod::Clipboard, "Paste"),
                        (InjectionMethod::Unicode, "Type"),
                    ],
                );
            },
        );
        divider(ui);
        row(
            ui,
            "Copy dictation to clipboard",
            "Keep the finished text for Ctrl+V and Windows clipboard history (Win+V), when enabled. Turn off to preserve your previous clipboard.",
            |ui| {
                toggle(
                    ui,
                    &mut dictation.copy_to_clipboard,
                    "Copy dictation to clipboard",
                );
            },
        );
        divider(ui);
        row(
            ui,
            "Start and stop sounds",
            "A soft tone when recording begins and ends.",
            |ui| {
                toggle(ui, &mut dictation.tones, "Start and stop sounds");
            },
        );
    });

    card(ui, "Speech model", |ui| {
        ui.label(
            RichText::new(
                "Models run entirely on this PC. Pick one and download it once; it stays \
                 on disk and loads when you first dictate.",
            )
            .small()
            .color(MUTED),
        );
        ui.add_space(6.0);
        for (index, spec) in MODELS.iter().enumerate() {
            if index > 0 {
                divider(ui);
            }
            model_card(app, ui, spec);
        }
        ui.add_space(8.0);
        divider(ui);
        let dictation = &mut app.config.dictation;
        row(
            ui,
            "Keep model loaded",
            "Faster first dictation, but uses about 1 GB of memory all the time.",
            |ui| {
                toggle(ui, &mut dictation.keep_warm, "Keep model loaded");
            },
        );
        if !dictation.keep_warm {
            divider(ui);
            row(
                ui,
                "Unload after",
                "Frees the memory when you have not dictated for a while.",
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut dictation.unload_after_minutes, 1..=120)
                            .suffix(" min"),
                    );
                },
            );
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Parakeet models by NVIDIA (CC-BY-4.0). ONNX exports by the sherpa-onnx \
                 project; speech detection by Silero.",
            )
            .small()
            .color(MUTED),
        );
    });

    card(ui, "AI cleanup", |ui| ai_cleanup_card(app, ui));

    card(ui, "Personal dictionary", |ui| {
        ui.label(
            RichText::new(
                "Fix names and terms the model gets wrong. Say it the left way, get the right one.",
            )
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);
        let ui_state = &mut app.dictation_ui;
        entries_editor(
            ui,
            "dictionary",
            &mut app.config.dictation.dictionary,
            &mut ui_state.new_word,
            "when you say",
            "write",
        );
    });

    card(ui, "Snippets", |ui| {
        ui.label(
            RichText::new(
                "Say a short phrase to insert something longer, like your email address.",
            )
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);
        let ui_state = &mut app.dictation_ui;
        entries_editor(
            ui,
            "snippets",
            &mut app.config.dictation.snippets,
            &mut ui_state.new_snippet,
            "when you say",
            "insert",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bit(vk: u16) -> u8 {
        modifier_bit(vk).unwrap()
    }

    const LCTRL: u16 = 0xA2;
    const RCTRL: u16 = 0xA3;
    const LALT: u16 = 0xA4;
    const RALT: u16 = 0xA5;
    const LWIN: u16 = 0x5B;

    fn capture() -> Capture {
        Capture {
            target: HotkeyTarget::PushToTalk,
            down: 0,
            key: None,
            seen_any: false,
        }
    }

    /// Feeds (held modifier bits, held key) polls and returns the last step.
    fn run(polls: &[(u8, Option<u16>)]) -> CaptureStep {
        let mut capture = capture();
        let mut last = CaptureStep::Continue;
        for &(down, key) in polls {
            last = step_capture(&mut capture, down, key);
            if !matches!(last, CaptureStep::Continue) {
                break;
            }
        }
        last
    }

    const NOTHING: (u8, Option<u16>) = (0, None);

    fn finished(step: CaptureStep) -> Result<String, String> {
        match step {
            CaptureStep::Finished(result) => result,
            CaptureStep::Continue => panic!("not finished"),
            CaptureStep::Cancelled => panic!("cancelled"),
        }
    }

    #[test]
    fn ctrl_win_from_the_left_hand_means_either_side() {
        let step = run(&[
            NOTHING,
            (bit(LCTRL), None),
            (bit(LCTRL) | bit(LWIN), None),
            NOTHING,
        ]);
        assert_eq!(finished(step), Ok("Ctrl+Win".to_owned()));
    }

    #[test]
    fn a_lone_modifier_keeps_its_side() {
        assert_eq!(
            finished(run(&[(bit(RCTRL), None), NOTHING])),
            Ok("Right Ctrl".to_owned())
        );
        assert_eq!(
            finished(run(&[(bit(RALT), None), NOTHING])),
            Ok("Right Alt".to_owned())
        );
        assert_eq!(
            finished(run(&[(bit(LCTRL), None), NOTHING])),
            Ok("Left Ctrl".to_owned())
        );
    }

    #[test]
    fn combo_with_a_key_remembers_modifiers_released_first() {
        // Ctrl and Alt go up before D does; the combo still has all three.
        let both = bit(LCTRL) | bit(LALT);
        let step = run(&[
            (bit(LCTRL), None),
            (both, None),
            (both, Some(0x44)),
            (0, Some(0x44)),
            NOTHING,
        ]);
        assert_eq!(finished(step), Ok("Ctrl+Alt+D".to_owned()));
    }

    #[test]
    fn a_right_side_key_in_a_combo_stays_right_only() {
        let step = run(&[(bit(RALT), None), (bit(RALT), Some(0x44)), NOTHING]);
        assert_eq!(finished(step), Ok("Right Alt+D".to_owned()));
    }

    #[test]
    fn nothing_happens_until_a_key_is_pressed() {
        let mut capture = capture();
        for _ in 0..5 {
            assert!(matches!(
                step_capture(&mut capture, 0, None),
                CaptureStep::Continue
            ));
        }
    }

    #[test]
    fn escape_cancels() {
        let step = run(&[(bit(LCTRL), None), (bit(LCTRL), Some(VK_ESCAPE))]);
        assert!(matches!(step, CaptureStep::Cancelled));
    }

    #[test]
    fn invalid_combos_report_why() {
        // A bare letter would fire while typing.
        let bare = finished(run(&[(0, Some(0x41)), NOTHING]));
        assert!(bare.unwrap_err().contains("modifier"));
        // A key Islet has no name for.
        let odd = finished(run(&[(bit(LCTRL), Some(0xE8)), NOTHING]));
        assert!(odd.unwrap_err().contains("isn't supported"));
    }

    #[test]
    fn single_non_typing_keys_need_no_modifier() {
        assert_eq!(
            finished(run(&[(0, Some(0x78)), NOTHING])),
            Ok("F9".to_owned())
        );
        assert_eq!(
            finished(run(&[(0, Some(0x14)), NOTHING])),
            Ok("CapsLock".to_owned())
        );
    }
}
