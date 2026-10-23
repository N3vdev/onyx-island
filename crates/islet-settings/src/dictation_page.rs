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
use islet_asr::{ModelManager, ModelSpec, ModelStatus, MODELS};
use islet_audio::{list_input_devices, InputDevice};
use islet_core::config::{InjectionMethod, Replacement};
use islet_core::hotkey::{name_for_vk, Combo, Modifiers, VK_ESCAPE};
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
}

/// A hotkey being captured: held keys are polled until all are released.
struct Capture {
    target: HotkeyTarget,
    modifiers: Modifiers,
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
    fn poll_jobs(&mut self) {
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

    fn start_download(&mut self, spec: &'static ModelSpec) {
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

        let mut modifiers_now = Modifiers::NONE;
        for (vks, modifier) in [
            (&[0xA2u16, 0xA3][..], Modifiers::CTRL),
            (&[0xA0, 0xA1][..], Modifiers::SHIFT),
            (&[0xA4, 0xA5][..], Modifiers::ALT),
            (&[0x5B, 0x5C][..], Modifiers::WIN),
        ] {
            if vks.iter().any(|&vk| held(vk)) {
                modifiers_now = modifiers_now.union(modifier);
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

        match step_capture(capture, modifiers_now, key_now) {
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
/// The combo is everything pressed during the capture, finished when all
/// keys are released; Esc cancels.
fn step_capture(
    capture: &mut Capture,
    modifiers_now: Modifiers,
    key_now: Option<u16>,
) -> CaptureStep {
    if key_now == Some(VK_ESCAPE) {
        return CaptureStep::Cancelled;
    }
    capture.modifiers = capture.modifiers.union(modifiers_now);
    if key_now.is_some() {
        capture.key = key_now;
    }
    if !modifiers_now.is_empty() || key_now.is_some() {
        capture.seen_any = true;
        return CaptureStep::Continue;
    }
    if !capture.seen_any {
        return CaptureStep::Continue;
    }

    let combo = Combo {
        modifiers: capture.modifiers,
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
    };
    let mut start_capture = false;
    row(ui, title, description, |ui| {
        let label = if capturing {
            "Press the keys\u{2026}".to_owned()
        } else {
            current
        };
        let button = egui::Button::new(RichText::new(label).strong())
            .min_size(egui::vec2(170.0, 30.0))
            .selected(capturing);
        if ui
            .add(button)
            .on_hover_text("Click, then press the keys")
            .clicked()
        {
            start_capture = true;
        }
    });
    if start_capture && !capturing {
        app.dictation_ui.capture_error = None;
        app.dictation_ui.capture = Some(Capture {
            target,
            modifiers: Modifiers::NONE,
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
                if ui
                    .add(egui::Button::new(RichText::new("\u{2715}").color(MUTED)).frame(false))
                    .on_hover_text("Remove")
                    .clicked()
                {
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

    ui.horizontal(|ui| {
        ui.radio_value(&mut app.config.dictation.model, spec.id.to_owned(), "");
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

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.dictation_ui.poll_jobs();
    if let Some((target, text)) = app.dictation_ui.poll_capture() {
        match target {
            HotkeyTarget::PushToTalk => app.config.dictation.push_to_talk = text,
            HotkeyTarget::HandsFree => app.config.dictation.hands_free = text,
        }
    }
    if app.dictation_ui.capture.is_some() || !app.dictation_ui.jobs.is_empty() {
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
            "Push to talk",
            "Hold to record, release to insert. Tap twice to lock it on.",
        );
        divider(ui);
        hotkey_row(
            app,
            ui,
            HotkeyTarget::HandsFree,
            "Hands-free",
            "Press once to start, again to stop. Esc cancels any recording.",
        );
        if let Some(error) = &app.dictation_ui.capture_error {
            ui.label(RichText::new(error).small().color(DANGER));
        }
    });

    card(ui, "Microphone", |ui| {
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
            "Paste is fastest and keeps your clipboard. Type works where pasting does not.",
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

    fn capture() -> Capture {
        Capture {
            target: HotkeyTarget::PushToTalk,
            modifiers: Modifiers::NONE,
            key: None,
            seen_any: false,
        }
    }

    /// Feeds a list of (modifiers, key) polls and returns the last step.
    fn run(polls: &[(Modifiers, Option<u16>)]) -> CaptureStep {
        let mut capture = capture();
        let mut last = CaptureStep::Continue;
        for &(modifiers, key) in polls {
            last = step_capture(&mut capture, modifiers, key);
            if !matches!(last, CaptureStep::Continue) {
                break;
            }
        }
        last
    }

    const NOTHING: (Modifiers, Option<u16>) = (Modifiers::NONE, None);

    fn finished(step: CaptureStep) -> Result<String, String> {
        match step {
            CaptureStep::Finished(result) => result,
            CaptureStep::Continue => panic!("not finished"),
            CaptureStep::Cancelled => panic!("cancelled"),
        }
    }

    #[test]
    fn modifier_only_combo_is_captured_when_released() {
        let ctrl_win = Modifiers::CTRL.union(Modifiers::WIN);
        let step = run(&[NOTHING, (Modifiers::CTRL, None), (ctrl_win, None), NOTHING]);
        assert_eq!(finished(step), Ok("Ctrl+Win".to_owned()));
    }

    #[test]
    fn combo_with_a_key_remembers_modifiers_released_first() {
        let ctrl_alt = Modifiers::CTRL.union(Modifiers::ALT);
        // Ctrl and Alt go up before D does; the combo still has all three.
        let step = run(&[
            (Modifiers::CTRL, None),
            (ctrl_alt, None),
            (ctrl_alt, Some(0x44)),
            (Modifiers::NONE, Some(0x44)),
            NOTHING,
        ]);
        assert_eq!(finished(step), Ok("Ctrl+Alt+D".to_owned()));
    }

    #[test]
    fn nothing_happens_until_a_key_is_pressed() {
        let mut capture = capture();
        for _ in 0..5 {
            assert!(matches!(
                step_capture(&mut capture, Modifiers::NONE, None),
                CaptureStep::Continue
            ));
        }
    }

    #[test]
    fn escape_cancels() {
        let step = run(&[(Modifiers::CTRL, None), (Modifiers::CTRL, Some(VK_ESCAPE))]);
        assert!(matches!(step, CaptureStep::Cancelled));
    }

    #[test]
    fn invalid_combos_report_why() {
        // A lone modifier is too easy to hit by accident.
        let lone = finished(run(&[(Modifiers::CTRL, None), NOTHING]));
        assert!(lone.unwrap_err().contains("two modifiers"));
        // A bare letter would fire while typing.
        let bare = finished(run(&[(Modifiers::NONE, Some(0x41)), NOTHING]));
        assert!(bare.unwrap_err().contains("modifier"));
        // A key Islet has no name for.
        let odd = finished(run(&[(Modifiers::CTRL, Some(0xE8)), NOTHING]));
        assert!(odd.unwrap_err().contains("isn't supported"));
    }

    #[test]
    fn function_keys_need_no_modifier() {
        let step = run(&[(Modifiers::NONE, Some(0x78)), NOTHING]);
        assert_eq!(finished(step), Ok("F9".to_owned()));
    }
}
