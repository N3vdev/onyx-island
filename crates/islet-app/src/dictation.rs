//! The dictation controller: hotkey -> microphone -> speech model -> text.
//!
//! One thread owns the keyboard hook, the microphone, and the ASR worker
//! process, and turns their events into the island's dictation states. It
//! never logs what was said or typed; only timings are logged.
//!
//! Flow of one dictation:
//! 1. `Start` (hotkey down): check the model and microphone, begin loading
//!    the model if it is not loaded, start the microphone, show the pill.
//! 2. Audio streams to the worker, which cuts it into utterances with voice
//!    detection and recognises each as it closes.
//! 3. `Stop` (hotkey up): finish the last utterance, clean the text, insert
//!    it, show a check mark.
//! 4. With AI cleanup on, the cleaned text is first rewritten by the
//!    on-device language model (`islet_asr::Polisher`, started while the user
//!    speaks). A slow, failed, or suspicious rewrite falls back to the
//!    rule-cleaned text, so dictation never gets worse than without it.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Select, Sender};
use islet_asr::{
    AsrClient, AsrEvent, LoadRequest, ModelManager, ModelStatus, PolishError, Polisher, AI_CLEANUP,
};
use islet_audio::{tone, MicError, Microphone, Tone};
use islet_core::config::{DictationConfig, InjectionMethod};
use islet_core::hotkey::Action;
use islet_core::island::{
    AlertLevel, DictationCard, DictationPhase, NotificationCard, DICTATION_BARS,
};
use islet_core::transcript::{self, TextOptions};
use islet_input::{HookEvent, KeyboardHook, Method};

use crate::settings;

/// What the controller tells the island.
#[derive(Clone, Debug)]
pub enum UiUpdate {
    /// One-shot request after a real recording has been confirmed.
    PauseMedia,
    /// Show (or update) the dictation pill; `None` hides it.
    Card(Option<DictationCard>),
    /// A transient message ("model not installed", an error).
    Notice(NotificationCard),
    /// A music control said to the assistant.
    Media(islet_core::command::MediaVerb),
}

enum Command {
    Configure(Box<DictationConfig>),
    SetPaused(bool),
    Shutdown,
}

/// Handle to the controller thread. Dropping it stops dictation.
pub struct DictationHandle {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl DictationHandle {
    pub fn configure(&self, config: &DictationConfig) {
        let _ = self
            .commands
            .send(Command::Configure(Box::new(config.clone())));
    }

    /// Pauses dictation (tray menu): the hotkeys pass through untouched.
    pub fn set_paused(&self, paused: bool) {
        let _ = self.commands.send(Command::SetPaused(paused));
    }
}

impl Drop for DictationHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the controller. `notify` is called (from the controller thread)
/// after every message queued on `ui`, to wake the island's UI thread.
pub fn spawn(
    config: &DictationConfig,
    models_dir: PathBuf,
    ui: Sender<UiUpdate>,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> io::Result<DictationHandle> {
    let (commands, commands_rx) = unbounded();
    let config = config.clone();
    let thread = std::thread::Builder::new()
        .name("islet-dictation".to_owned())
        .spawn(move || {
            Controller::new(config, models_dir, ui, notify).run(&commands_rx);
        })?;
    Ok(DictationHandle {
        commands,
        thread: Some(thread),
    })
}

/// The hands-free hotkey text, or nothing while hands-free mode is off.
/// The kind of command, for the log (never the words).
fn command_kind(command: &islet_core::command::Command) -> &'static str {
    use islet_core::command::Command;
    match command {
        Command::Open(_) => "open",
        Command::Close(_) => "close",
        Command::Minimize(_) => "minimize",
        Command::Maximize => "maximize",
        Command::ShowDesktop => "show desktop",
        Command::Media(_) => "media",
        Command::Play { .. } => "play",
        Command::Volume(_) => "volume",
        Command::Type(_) => "type",
        Command::Search { .. } => "search",
        Command::Screenshot => "screenshot",
        Command::Lock => "lock",
    }
}

fn hands_free_hotkey(config: &DictationConfig) -> &str {
    if config.hands_free_enabled {
        &config.hands_free
    } else {
        ""
    }
}

/// How often the pill's clock and waveform update while recording.
const TICK: Duration = Duration::from_millis(33);
/// A press must last this long before anything is shown or played. Taps of
/// a single-key hotkey that is really part of another shortcut (Right
/// Ctrl+C) then cause no flash, tone, or model load.
const REVEAL_DELAY: Duration = Duration::from_millis(120);
/// A dictation shorter than this is an accident, not speech.
const MIN_SPEECH_SECONDS: f32 = 0.30;
/// Safety stop for a forgotten hands-free recording.
const MAX_RECORDING: Duration = Duration::from_secs(10 * 60);
/// How long the check mark stays up.
/// Audio is forwarded to the worker in chunks of about this many samples.
const FORWARD_SAMPLES: usize = 480;
/// Audio held while the model is still loading (about two minutes).
const MAX_PENDING_SAMPLES: usize = 16_000 * 120;
/// A recording whose loudest sample is below this (about -54 dBFS) is
/// silence, not a quiet room: the device is muted, virtual, or wrong.
const SILENCE_PEAK: f32 = 0.002;
/// Time allowed for the AI rewrite: a base plus a little per word.
const POLISH_BASE: Duration = Duration::from_millis(2500);
const POLISH_PER_WORD: Duration = Duration::from_millis(60);
const POLISH_MAX: Duration = Duration::from_secs(15);
/// Notices about a missing model are not repeated more often than this.
const NOTICE_COOLDOWN: Duration = Duration::from_secs(8);
/// How long "open X and type ..." waits for X's window.
const OPEN_WAIT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Recording,
    /// The user stopped; waiting for the final text.
    Transcribing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelState {
    NotLoaded,
    Loading,
    Ready,
}

struct Session {
    started: Instant,
    /// When the pill and tone appear (and the model starts loading if it
    /// is not loaded yet).
    reveal_at: Instant,
    revealed: bool,
    hands_free: bool,
    /// Spoken to the assistant (the command hotkey), not text to type.
    command: bool,
    /// `Begin` has been sent to the worker.
    began: bool,
    /// The user stopped before the model finished loading.
    stop_requested: bool,
    /// Audio gathered while the model was loading.
    pending: Vec<f32>,
    /// Audio waiting to be forwarded in a full chunk.
    outgoing: Vec<f32>,
    samples: usize,
    /// Loudest sample heard; ~0 means the microphone delivered silence.
    peak: f32,
    stopped_at: Option<Instant>,
    /// When `End` was sent to the worker.
    end_sent_at: Option<Instant>,
}

/// Timings of one dictation, logged when its text is inserted.
#[derive(Clone, Copy)]
struct Timing {
    stopped_at: Option<Instant>,
    worker_ms: Option<u64>,
    tail_audio_ms: u64,
    tail_decode_ms: u64,
}

/// A rewrite in flight: the rule-cleaned text is typed if it fails.
struct Polishing {
    events: Receiver<Result<String, PolishError>>,
    fallback: String,
    timing: Timing,
    started: Instant,
}

struct Controller {
    config: DictationConfig,
    text_options: TextOptions,
    paused: bool,
    ui: Sender<UiUpdate>,
    notify: Arc<dyn Fn() + Send + Sync>,

    hook: Option<KeyboardHook>,
    mic: Option<Microphone>,
    mic_device: String,
    asr: AsrClient,
    manager: ModelManager,
    model: ModelState,
    loaded_model: String,
    polisher: Arc<Polisher>,
    polishing: Option<Polishing>,

    phase: Phase,
    session: Option<Session>,
    last_used: Instant,
    last_notice: Option<Instant>,

    // Meter state for the pill.
    sum_squares: f64,
    sample_count: usize,
    level: f32,
    bars: [u8; DICTATION_BARS],
    /// The room's noise and the voice's loudness, learned as you speak, so
    /// the bars span quiet to loud for this mic instead of saturating.
    loudness: Loudness,
    /// The voice assistant, while it is turned on.
    assistant: Option<crate::assistant::Assistant>,
    /// What the dictation being typed was heard as and cleaned to, for
    /// learning from corrections (only kept while that is on).
    last_dictated: Option<(String, String)>,
    next_tick: Instant,
}

/// Tracks the background noise and the loudness of recent speech, in dB.
#[derive(Clone, Copy, Debug)]
struct Loudness {
    floor: f32,
    peak: f32,
}

impl Default for Loudness {
    fn default() -> Self {
        Self { floor: -60.0, peak: -30.0 }
    }
}

impl Loudness {
    /// Learns from one 33 ms reading and returns where it sits between the
    /// noise and the loudest recent speech, 0..=1. Ordinary speech lands in
    /// the middle and only stressed syllables reach the top.
    fn place(&mut self, db: f32) -> f32 {
        // The noise floor drops at once and creeps back up about 1 dB a
        // second, so speech never becomes the floor.
        self.floor = if db < self.floor { db } else { self.floor + 0.03 };
        self.floor = self.floor.clamp(-80.0, -35.0);
        // The loudness reference jumps to each peak and eases back over a
        // few seconds, so a loud phrase doesn't flatten what follows.
        self.peak = if db > self.peak { db } else { self.peak - 0.1 };
        self.peak = self.peak.max(self.floor + 20.0);
        let above = db - self.floor - 5.0;
        let span = (self.peak - self.floor - 5.0).max(15.0);
        (above / span).clamp(0.0, 1.0).powf(1.5)
    }
}

impl Controller {
    fn new(
        config: DictationConfig,
        models_dir: PathBuf,
        ui: Sender<UiUpdate>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let hook = match KeyboardHook::start(&config.push_to_talk, hands_free_hotkey(&config)) {
            Ok(hook) => Some(hook),
            Err(error) => {
                tracing::error!(%error, "dictation hotkeys are unavailable");
                None
            }
        };
        let mut controller = Self {
            text_options: TextOptions::from_config(&config),
            config,
            paused: false,
            ui,
            notify,
            hook,
            mic: None,
            mic_device: String::new(),
            asr: AsrClient::new(islet_asr::default_worker_path()),
            polisher: Arc::new(Polisher::new(
                ModelManager::new(&models_dir).addon_program(&AI_CLEANUP),
                ModelManager::new(&models_dir).addon_model(&AI_CLEANUP),
            )),
            polishing: None,
            manager: ModelManager::new(models_dir),
            model: ModelState::NotLoaded,
            loaded_model: String::new(),
            phase: Phase::Idle,
            session: None,
            last_used: Instant::now(),
            last_notice: None,
            sum_squares: 0.0,
            sample_count: 0,
            level: 0.0,
            bars: [0; DICTATION_BARS],
            loudness: Loudness::default(),
            assistant: None,
            last_dictated: None,
            next_tick: Instant::now(),
        };
        controller.update_assistant();
        controller.update_hook_enabled();
        controller.maybe_preload();
        controller
    }

    fn run(&mut self, commands: &Receiver<Command>) {
        let hook_events = self.hook.as_ref().map(|hook| hook.events().clone());
        self.run_with_hook_events(commands, hook_events);
    }

    fn run_with_hook_events(
        &mut self,
        commands: &Receiver<Command>,
        hook_events: Option<Receiver<HookEvent>>,
    ) {
        let asr_events = self.asr.events().clone();
        let mut hook_connected = true;
        loop {
            let mic_audio = self.mic.as_ref().map(|mic| mic.audio().clone());

            let mut select = Select::new();
            let commands_index = select.recv(commands);
            let asr_index = select.recv(&asr_events);
            let hook_index = hook_events
                .as_ref()
                .filter(|_| hook_connected)
                .map(|events| select.recv(events));
            let mic_index = mic_audio.as_ref().map(|audio| select.recv(audio));
            let polish_events = self.polishing.as_ref().map(|job| job.events.clone());
            let polish_index = polish_events.as_ref().map(|events| select.recv(events));

            let operation = match self.next_deadline() {
                Some(deadline) => select
                    .select_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok(),
                None => Some(select.select()),
            };
            let Some(operation) = operation else {
                self.on_tick();
                continue;
            };

            let index = operation.index();
            if index == commands_index {
                match operation.recv(commands) {
                    Ok(Command::Configure(config)) => self.apply_config(*config),
                    Ok(Command::SetPaused(paused)) => {
                        self.paused = paused;
                        if paused {
                            self.discard();
                        }
                        self.update_hook_enabled();
                    }
                    Ok(Command::Shutdown) | Err(_) => break,
                }
            } else if index == asr_index {
                if let Ok(event) = operation.recv(&asr_events) {
                    self.on_asr(event);
                }
            } else if Some(index) == hook_index {
                if let Some(events) = &hook_events {
                    match operation.recv(events) {
                        Ok(HookEvent::Action(action)) => self.on_action(action),
                        Err(_) => {
                            // A disconnected channel is always ready. Stop
                            // selecting it so a dead hook cannot spin a core.
                            hook_connected = false;
                            self.hook = None;
                            if self.phase == Phase::Recording {
                                self.discard();
                            }
                            self.notice(
                                "Hotkeys unavailable",
                                "Restart Onyx to enable dictation again.",
                                AlertLevel::Error,
                            );
                        }
                    }
                }
            } else if Some(index) == mic_index {
                if let Some(audio) = &mic_audio {
                    match operation.recv(audio) {
                        Ok(chunk) => self.on_audio(&chunk),
                        Err(_) => self.microphone_disconnected(),
                    }
                }
            } else if Some(index) == polish_index {
                if let Some(events) = &polish_events {
                    let result = operation
                        .recv(events)
                        .unwrap_or_else(|_| Err(PolishError::Request("stopped".to_owned())));
                    self.polished(result);
                }
            }
            if self.next_tick <= Instant::now() {
                self.on_tick();
            }
        }

        // Shutting down: never leave a recording or the worker behind.
        if let Some(mic) = &self.mic {
            mic.stop();
        }
        self.asr.shutdown();
        self.polisher.shutdown();
    }

    /// When the loop must wake even if nothing arrives.
    fn next_deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Recording | Phase::Transcribing => Some(self.next_tick),
            Phase::Idle if self.models_running() && !self.config.keep_warm => {
                Some(self.last_used + self.unload_after())
            }
            Phase::Idle => None,
        }
    }

    fn models_running(&self) -> bool {
        self.asr.is_running() || self.polisher.is_running()
    }

    /// AI cleanup is on and its files are on disk.
    fn ai_ready(&self) -> bool {
        self.config.ai_cleanup && self.manager.addon_status(&AI_CLEANUP) == ModelStatus::Installed
    }

    /// Starts the language model in the background while the user speaks,
    /// so the rewrite does not wait for it to load.
    fn warm_polisher(&self) {
        if !self.ai_ready() || self.polisher.is_running() {
            return;
        }
        let polisher = Arc::clone(&self.polisher);
        std::thread::spawn(move || {
            if let Err(error) = polisher.warm() {
                tracing::warn!(%error, "AI cleanup could not start");
            }
        });
    }

    fn unload_after(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.unload_after_minutes.max(1)) * 60)
    }

    fn apply_config(&mut self, config: DictationConfig) {
        if let Some(hook) = &self.hook {
            hook.set_combos(&config.push_to_talk, hands_free_hotkey(&config));
        }
        self.text_options = TextOptions::from_config(&config);
        if !config.ai_cleanup {
            self.polisher.shutdown();
        }

        let model_changed = config.model != self.config.model;
        let mic_changed = config.microphone != self.config.microphone;
        self.config = config;
        self.update_assistant();
        self.update_hook_enabled();

        if mic_changed && self.phase == Phase::Idle {
            self.mic = None;
        }
        if model_changed && self.phase == Phase::Idle {
            // Unload; the new model loads on next use (or now, if kept warm).
            self.asr.shutdown();
            self.model = ModelState::NotLoaded;
            self.loaded_model.clear();
        }
        self.maybe_preload();
    }

    /// Arms the assistant's hotkey while it is on, and lists the installed
    /// apps ahead of the first command.
    fn update_assistant(&mut self) {
        let on = self.config.assistant_enabled && !self.config.assistant_hotkey.trim().is_empty();
        if let Some(hook) = &self.hook {
            hook.set_command(if on { &self.config.assistant_hotkey } else { "" });
        }
        if on && self.assistant.is_none() {
            self.assistant = Some(crate::assistant::Assistant::new());
        } else if !on {
            self.assistant = None;
        }
    }

    fn update_hook_enabled(&self) {
        if let Some(hook) = &self.hook {
            hook.set_enabled(self.config.enabled && !self.paused);
        }
    }

    /// Loads the model right away when "keep warm" is on.
    fn maybe_preload(&mut self) {
        if self.config.enabled
            && self.config.keep_warm
            && self.model == ModelState::NotLoaded
            && self.model_installed()
        {
            self.begin_load();
        }
    }

    fn model_installed(&self) -> bool {
        let spec = islet_asr::find_or_default(&self.config.model);
        self.manager.status(spec) == ModelStatus::Installed
    }

    fn begin_load(&mut self) {
        let spec = islet_asr::find_or_default(&self.config.model);
        let threads = std::thread::available_parallelism()
            .map_or(2, |count| count.get())
            .clamp(2, 4) as u32;
        let request = LoadRequest {
            engine: spec.engine.wire_name().to_owned(),
            dir: self.manager.model_dir(spec).display().to_string(),
            vad: self.manager.vad_path().display().to_string(),
            threads,
        };
        match self.asr.load(request) {
            Ok(()) => {
                self.model = ModelState::Loading;
                self.loaded_model = spec.id.to_owned();
            }
            Err(error) => {
                tracing::error!(%error, "could not start the speech worker");
                self.model = ModelState::NotLoaded;
            }
        }
    }

    // ----- hotkey actions ------------------------------------------------

    fn on_action(&mut self, action: Action) {
        match action {
            Action::Start => self.start(false),
            Action::StartCommand => self.start(true),
            Action::EnterHandsFree => {
                if let Some(session) = &mut self.session {
                    session.hands_free = true;
                }
                self.send_card();
            }
            Action::Stop => self.stop(),
            Action::Discard => self.discard(),
            Action::Cancel => {
                // Esc while recording: throw the audio away and say so.
                if self.phase == Phase::Recording {
                    self.discard();
                    self.notice("Stopped", "Recording discarded.", AlertLevel::Info);
                }
            }
            Action::SuppressStartMenu => islet_input::suppress_start_menu(),
        }
    }

    /// Begins recording: text to type, or a `command` for the assistant.
    fn start(&mut self, command: bool) {
        if self.phase == Phase::Transcribing || self.phase == Phase::Recording {
            // Still busy with the previous dictation.
            self.recording_ended();
            return;
        }

        // Setting a hotkey in Islet's own settings window must not start a
        // dictation (and there is nowhere to type into there). The welcome
        // tour is the exception: it has a box for trying dictation out.
        if islet_input::foreground_exe().as_deref() == Some("islet-settings.exe")
            && islet_input::foreground_title() != islet_core::handoff::ONBOARDING_TITLE
        {
            self.recording_ended();
            return;
        }

        let spec = islet_asr::find_or_default(&self.config.model);
        if self.manager.status(spec) != ModelStatus::Installed {
            self.model_missing_notice();
            self.recording_ended();
            return;
        }
        if islet_input::foreground_is_blocked_by_elevation() {
            self.notice(
                "Admin window",
                "That window runs as administrator. Run Onyx as administrator to dictate into it.",
                AlertLevel::Error,
            );
            self.recording_ended();
            return;
        }
        if let Err(error) = self.ensure_mic() {
            self.notice("Mic problem", &error.to_string(), AlertLevel::Error);
            self.recording_ended();
            return;
        }
        let mut session = Session {
            started: Instant::now(),
            reveal_at: Instant::now() + REVEAL_DELAY,
            revealed: false,
            hands_free: false,
            command,
            began: false,
            stop_requested: false,
            pending: Vec::new(),
            outgoing: Vec::new(),
            samples: 0,
            peak: 0.0,
            stopped_at: None,
            end_sent_at: None,
        };
        if self.model == ModelState::Ready && self.loaded_model == spec.id {
            if self.asr.begin().is_err() {
                self.model = ModelState::NotLoaded;
                self.notice("Voice error", "Try again.", AlertLevel::Error);
                self.recording_ended();
                return;
            }
            session.began = true;
        }
        self.session = Some(session);
        self.phase = Phase::Recording;
        self.reset_meter();

        if let Some(mic) = &self.mic {
            mic.start();
        }
        // The pill and tone follow after `REVEAL_DELAY` (see `reveal`).
        self.next_tick = Instant::now();
    }

    /// The press has lasted long enough to be a real dictation: load the
    /// model if needed, play the start tone, and show the pill. Returns
    /// false if the model could not be started (the dictation is dropped).
    fn reveal(&mut self) -> bool {
        if self.session.is_none() {
            return false;
        }
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.revealed)
        {
            return true;
        }
        let spec = islet_asr::find_or_default(&self.config.model);
        if let Some(session) = &mut self.session {
            session.revealed = true;
        }
        if self.model == ModelState::NotLoaded || self.loaded_model != spec.id {
            self.asr.shutdown();
            self.begin_load();
            if self.model == ModelState::NotLoaded {
                self.fail("Voice error", "The voice model could not be started.");
                return false;
            }
        }
        if self.config.pause_media {
            self.send(UiUpdate::PauseMedia);
        }
        self.warm_polisher();
        if self.config.tones {
            tone::play(Tone::Start);
        }
        self.send_card();
        true
    }

    fn stop(&mut self) {
        if self.phase != Phase::Recording {
            return;
        }
        if let Some(mic) = &self.mic {
            mic.stop();
        }
        // Pull in whatever the device delivered before it stopped.
        let audio = self.mic.as_ref().map(|mic| mic.audio().clone());
        if let Some(audio) = audio {
            while let Ok(chunk) = audio.try_recv() {
                self.on_audio(&chunk);
            }
        }
        let Some(session) = &mut self.session else {
            self.phase = Phase::Idle;
            return;
        };
        let revealed = session.revealed;
        session.stopped_at = Some(Instant::now());
        if (session.samples as f32) < MIN_SPEECH_SECONDS * 16_000.0 {
            self.discard();
            return;
        }
        if !revealed && !self.reveal() {
            return;
        }
        if self.config.tones {
            tone::play(Tone::Stop);
        }
        let Some(session) = &mut self.session else {
            return;
        };
        self.phase = Phase::Transcribing;
        self.last_used = Instant::now();
        if session.began {
            self.flush_outgoing();
            if let Some(session) = &mut self.session {
                session.end_sent_at = Some(Instant::now());
            }
            if self.asr.end().is_err() {
                self.fail("Voice error", "Try again.");
                return;
            }
        } else {
            // The model is still loading; finish when it is ready.
            session.stop_requested = true;
        }
        self.next_tick = Instant::now();
        self.send_card();
    }

    fn discard(&mut self) {
        if self.phase == Phase::Idle {
            return;
        }
        if let Some(mic) = &self.mic {
            mic.stop();
            while mic.audio().try_recv().is_ok() {}
        }
        if self.session.as_ref().is_some_and(|session| session.began) {
            let _ = self.asr.cancel();
        }
        self.session = None;
        self.polishing = None;
        self.phase = Phase::Idle;
        self.last_used = Instant::now();
        self.recording_ended();
        self.hide();
    }

    fn fail(&mut self, title: &str, body: &str) {
        // Errors end capture too; otherwise the hidden mic keeps delivering
        // packets and waking this thread after the UI has returned to idle.
        self.mic = None;
        self.mic_device.clear();
        self.session = None;
        self.polishing = None;
        self.phase = Phase::Idle;
        self.model = ModelState::NotLoaded;
        self.asr.shutdown();
        self.recording_ended();
        self.hide();
        self.notice(title, body, AlertLevel::Error);
    }

    /// Tells the hotkey state machine a recording ended on our side.
    fn recording_ended(&self) {
        if let Some(hook) = &self.hook {
            hook.recording_ended();
        }
    }

    // ----- audio and meter -----------------------------------------------

    fn microphone_disconnected(&mut self) {
        self.mic = None;
        self.mic_device.clear();
        if self.phase == Phase::Recording {
            self.discard();
            self.notice(
                "Microphone disconnected",
                "Choose a microphone in Settings and try again.",
                AlertLevel::Error,
            );
        }
    }

    fn ensure_mic(&mut self) -> Result<(), MicError> {
        let wanted = self.config.microphone.clone();
        let usable = self
            .mic
            .as_ref()
            .is_some_and(|mic| mic.is_alive() && self.mic_device == wanted);
        if usable {
            return Ok(());
        }
        self.mic = None;
        // Testing hook: play a WAV file instead of listening to the room.
        let mic = match std::env::var_os("ISLET_FAKE_MIC") {
            Some(path) => Microphone::from_wav(std::path::Path::new(&path))?,
            None => Microphone::open((!wanted.is_empty()).then_some(wanted.as_str()))?,
        };
        self.mic = Some(mic);
        self.mic_device = wanted;
        Ok(())
    }

    fn on_audio(&mut self, chunk: &[f32]) {
        let Some(session) = &mut self.session else {
            return;
        };
        for sample in chunk {
            self.sum_squares += f64::from(sample * sample);
        }
        self.sample_count += chunk.len();
        session.samples += chunk.len();
        session.peak = chunk
            .iter()
            .fold(session.peak, |peak, sample| peak.max(sample.abs()));

        if session.began {
            session.outgoing.extend_from_slice(chunk);
            if session.outgoing.len() >= FORWARD_SAMPLES {
                self.flush_outgoing();
            }
        } else if session.pending.len() < MAX_PENDING_SAMPLES {
            session.pending.extend_from_slice(chunk);
        }
    }

    fn flush_outgoing(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.outgoing.is_empty() {
            return;
        }
        let result = self.asr.audio(&session.outgoing);
        session.outgoing.clear();
        if result.is_err() {
            self.fail("Voice error", "Try again.");
        }
    }

    fn reset_meter(&mut self) {
        self.sum_squares = 0.0;
        self.sample_count = 0;
        self.level = 0.0;
        self.bars = [0; DICTATION_BARS];
    }

    /// Folds the audio since the last tick into one waveform bar.
    fn update_meter(&mut self) {
        let rms = if self.sample_count == 0 {
            0.0
        } else {
            (self.sum_squares / self.sample_count as f64).sqrt() as f32
        };
        self.sum_squares = 0.0;
        self.sample_count = 0;
        let db = 20.0 * rms.max(1e-5).log10();
        let target = self.loudness.place(db);
        // Fast attack and a quick release, so syllables show as dips.
        let rate = if target > self.level { 0.75 } else { 0.42 };
        self.level += (target - self.level) * rate;
        self.bars.copy_within(1.., 0);
        self.bars[DICTATION_BARS - 1] = (self.level * 255.0) as u8;
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        self.next_tick = now + TICK;

        match self.phase {
            Phase::Recording => {
                self.update_meter();
                let due = self
                    .session
                    .as_ref()
                    .is_some_and(|session| !session.revealed && now >= session.reveal_at);
                if due && !self.reveal() {
                    return;
                }
                self.send_card();
                let over = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.started.elapsed() > MAX_RECORDING);
                if over {
                    self.stop();
                    self.recording_ended();
                }
            }
            Phase::Transcribing => self.send_card(),
            Phase::Idle => {
                if self.models_running()
                    && !self.config.keep_warm
                    && now.duration_since(self.last_used) >= self.unload_after()
                {
                    tracing::info!("unloading the speech model after idle");
                    self.asr.shutdown();
                    self.polisher.shutdown();
                    self.model = ModelState::NotLoaded;
                    self.loaded_model.clear();
                    self.mic = None;
                }
            }
        }
    }

    // ----- speech worker events ------------------------------------------

    fn on_asr(&mut self, event: AsrEvent) {
        match event {
            AsrEvent::Loaded { load_ms } => {
                tracing::info!(load_ms, "speech model loaded");
                self.model = ModelState::Ready;
                self.begin_pending_session();
            }
            AsrEvent::Segment(segment) => {
                tracing::debug!(
                    audio_ms = segment.audio_ms,
                    decode_ms = segment.decode_ms,
                    "utterance recognised"
                );
            }
            AsrEvent::Final(done) => {
                if self.phase == Phase::Transcribing {
                    self.finish(&done.text, done.tail_audio_ms, done.tail_decode_ms);
                }
            }
            AsrEvent::Error(message) => {
                tracing::error!(%message, "speech worker error");
                self.fail("Voice error", &message);
            }
            AsrEvent::Exited => {
                tracing::error!("speech worker exited unexpectedly");
                self.model = ModelState::NotLoaded;
                if self.phase != Phase::Idle {
                    self.fail("Voice error", "Try again.");
                }
            }
        }
    }

    /// The model finished loading: start the session that was waiting for it.
    fn begin_pending_session(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        if session.began {
            return;
        }
        if self.asr.begin().is_err() {
            self.fail("Voice error", "Try again.");
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        session.began = true;
        let pending = std::mem::take(&mut session.pending);
        let stop_requested = session.stop_requested;
        if !pending.is_empty() && self.asr.audio(&pending).is_err() {
            self.fail("Voice error", "Try again.");
            return;
        }
        if stop_requested && self.asr.end().is_err() {
            self.fail("Voice error", "Try again.");
            return;
        }
        self.send_card();
    }

    fn finish(&mut self, raw: &str, tail_audio_ms: u64, tail_decode_ms: u64) {
        let stopped_at = self.session.as_ref().and_then(|session| session.stopped_at);
        let worker_ms = self
            .session
            .as_ref()
            .and_then(|session| session.end_sent_at)
            .map(|at| at.elapsed().as_millis() as u64);
        let heard_silence = self
            .session
            .as_ref()
            .is_some_and(|session| session.peak < SILENCE_PEAK);
        let command = self.session.as_ref().is_some_and(|session| session.command);
        self.session = None;
        let text = transcript::clean(raw, &self.text_options);
        if command && !text.is_empty() {
            self.run_command(&text);
            return;
        }
        self.last_dictated = self.config.learn_corrections.then(|| (raw.to_owned(), text.clone()));
        tracing::info!(
            recognized_correction_cues = transcript::correction_cue_count(raw),
            remaining_correction_cues = transcript::correction_cue_count(&text),
            "Dictation correction summary"
        );
        if text.is_empty() {
            self.phase = Phase::Idle;
            self.recording_ended();
            self.hide();
            if heard_silence {
                // Most often the wrong input device (a virtual or muted one).
                self.notice(
                    "No sound",
                    "Onyx heard silence. Choose your microphone in Settings > Dictation.",
                    AlertLevel::NeedsInput,
                );
            }
            return;
        }
        let timing = Timing {
            stopped_at,
            worker_ms,
            tail_audio_ms,
            tail_decode_ms,
        };
        if self.ai_ready() {
            self.start_polish(text, timing);
        } else {
            self.insert(text, timing);
        }
    }

    /// Carries out what was said to the assistant. The words themselves are
    /// never logged; only whether it was understood.
    fn run_command(&mut self, said: &str) {
        self.phase = Phase::Idle;
        self.recording_ended();
        self.hide();
        self.last_used = Instant::now();
        use islet_core::command::Command;
        // What was heard, shown under a failure so it is clear why.
        let heard = format!("Heard \u{201c}{}\u{201d}", said.trim().trim_end_matches(['.', '?', '!']));
        let Some(commands) = islet_core::command::parse_all(said) else {
            tracing::info!("assistant: not a command");
            self.notice("Not sure what to do", &heard, AlertLevel::Info);
            return;
        };
        let mut last_notice = None;
        let mut opened: Option<String> = None;
        for command in &commands {
            // "Open Spotify and search for lo-fi", or "search lo-fi on
            // Spotify": the search happens in that app, not on the web.
            let in_app = match (command, &opened, &self.assistant) {
                (Command::Search { query, site: None }, Some(app), Some(_)) => Some((app.clone(), query)),
                (Command::Search { query, site: Some(site) }, _, Some(assistant)) if assistant.is_app(site) => {
                    let reply = assistant.run(&Command::Open(site.clone()));
                    if !reply.ok {
                        let title = reply.notice.map_or_else(|| "Couldn't open that".to_owned(), |(title, _)| title);
                        self.notice_with_icon(&title, &heard, AlertLevel::Info, reply.icon);
                        return;
                    }
                    Some((site.clone(), query))
                }
                _ => None,
            };
            if let (Some((app, query)), Some(assistant)) = (in_app, &self.assistant) {
                tracing::info!("assistant: search in app");
                let method = match self.config.injection {
                    InjectionMethod::Clipboard => Method::Clipboard,
                    InjectionMethod::Unicode => Method::Unicode,
                };
                let reply = assistant.search_in(&app, query, method, OPEN_WAIT);
                if !reply.ok {
                    let title = reply.notice.map_or_else(|| "Couldn't search there".to_owned(), |(title, _)| title);
                    self.notice_with_icon(&title, &heard, AlertLevel::Info, reply.icon);
                    return;
                }
                if let Some((title, body)) = reply.notice {
                    last_notice = Some((title, body, reply.icon));
                }
                opened = None;
                continue;
            }
            if let Command::Type(text) = command {
                tracing::info!("assistant: type");
                // Just opened something: type into it once its window is up.
                if let (Some(name), Some(assistant)) = (&opened, &self.assistant) {
                    if !assistant.wait_for_window(name, OPEN_WAIT) {
                        self.notice("The app didn't open in time", &heard, AlertLevel::Info);
                        return;
                    }
                }
                let method = match self.config.injection {
                    InjectionMethod::Clipboard => Method::Clipboard,
                    InjectionMethod::Unicode => Method::Unicode,
                };
                if let Err(error) = islet_input::insert_text_with_clipboard(text, method, false) {
                    self.notice("Can't type there", &error.to_string(), AlertLevel::Error);
                    return;
                }
                opened = None;
                continue;
            }
            let Some(assistant) = &self.assistant else {
                return;
            };
            let reply = assistant.run(command);
            tracing::info!(ok = reply.ok, kind = command_kind(command), "assistant: command");
            if !reply.ok {
                // Stop here: the next step would land in the wrong place.
                let title = reply.notice.map_or_else(|| "Couldn't do that".to_owned(), |(title, _)| title);
                self.notice_with_icon(&title, &heard, AlertLevel::Info, reply.icon);
                return;
            }
            opened = match command {
                Command::Open(name) => Some(name.clone()),
                _ => None,
            };
            if let Some(verb) = reply.media {
                self.send(UiUpdate::Media(verb));
            }
            if let Some((verb, after)) = reply.media_later {
                let ui = self.ui.clone();
                let notify = Arc::clone(&self.notify);
                std::thread::spawn(move || {
                    std::thread::sleep(after);
                    if ui.send(UiUpdate::Media(verb)).is_ok() {
                        notify();
                    }
                });
            }
            if let Some((title, body)) = reply.notice {
                last_notice = Some((title, body, reply.icon));
            }
        }
        if self.config.tones {
            tone::play(Tone::Done);
        }
        if let Some((title, body, icon)) = last_notice {
            self.notice_with_icon(&title, &body, AlertLevel::Success, icon);
        }
    }

    /// Hands the text to the language model on a helper thread; the pill
    /// keeps showing "transcribing" until `polished` types the result.
    fn start_polish(&mut self, text: String, timing: Timing) {
        let words = text.split_whitespace().count() as u32;
        let budget = (POLISH_BASE + POLISH_PER_WORD * words).min(POLISH_MAX);
        let (sender, events) = crossbeam_channel::bounded(1);
        let polisher = Arc::clone(&self.polisher);
        let input = text.clone();
        std::thread::spawn(move || {
            let _ = sender.send(polisher.rewrite(&input, budget));
        });
        self.polishing = Some(Polishing {
            events,
            fallback: text,
            timing,
            started: Instant::now(),
        });
    }

    fn polished(&mut self, result: Result<String, PolishError>) {
        let Some(job) = self.polishing.take() else {
            return;
        };
        if self.phase != Phase::Transcribing {
            return; // Cancelled meanwhile.
        }
        let polish_ms = job.started.elapsed().as_millis() as u64;
        let text = match result {
            Ok(rewrite) => {
                let rewrite = transcript::unwrap_rewrite(&rewrite);
                if transcript::accept_rewrite(&job.fallback, &rewrite) {
                    if rewrite == job.fallback {
                        tracing::info!(polish_ms, "AI cleanup retained rule-cleaned text");
                    } else {
                        tracing::info!(polish_ms, "AI cleanup applied");
                    }
                    rewrite
                } else {
                    tracing::info!(
                        polish_ms,
                        "AI cleanup rewrite refused; typing the cleaned text"
                    );
                    job.fallback
                }
            }
            Err(error) => {
                tracing::warn!(%error, polish_ms, "AI cleanup unavailable");
                if matches!(error, PolishError::MissingRuntime | PolishError::Start(_)) {
                    self.notice("AI cleanup off", &error.to_string(), AlertLevel::Error);
                }
                job.fallback
            }
        };
        self.insert(text, job.timing);
    }

    fn insert(&mut self, mut text: String, timing: Timing) {
        text = transcript::finish_layout(&transcript::format_numbers(&text));
        let Timing {
            stopped_at,
            worker_ms,
            tail_audio_ms,
            tail_decode_ms,
        } = timing;
        if self.config.trailing_space {
            text.push(' ');
        }

        let method = match self.config.injection {
            InjectionMethod::Clipboard => Method::Clipboard,
            InjectionMethod::Unicode => Method::Unicode,
        };
        let text_ready = stopped_at.map(|at| at.elapsed());
        match islet_input::insert_text_with_clipboard(&text, method, self.config.copy_to_clipboard) {
            Ok(report) => {
                tracing::info!(
                    tail_audio_ms,
                    tail_decode_ms,
                    worker_round_trip_ms = worker_ms,
                    release_to_text_ms = text_ready.map(|d| d.as_millis() as u64),
                    release_to_inserted_ms = stopped_at.map(|at| at.elapsed().as_millis() as u64),
                    method = ?report.method,
                    clipboard_restored = report.clipboard_restored,
                    clipboard_saved = report.clipboard_saved,
                    "dictation inserted"
                );
                if self.config.tones {
                    tone::play(Tone::Done);
                }
                if let Some((heard, rule)) = self.last_dictated.take() {
                    crate::learn::watch(crate::learn::Dictated { heard, rule, typed: text.trim().to_owned() });
                }
                // No "done" mark: the pill simply goes, back to the music
                // if it was playing.
                self.phase = Phase::Idle;
                self.recording_ended();
                self.hide();
                if self.config.copy_to_clipboard && !report.clipboard_saved {
                    self.notice(
                        "Text inserted",
                        "Could not copy it to the clipboard.",
                        AlertLevel::Error,
                    );
                }
            }
            Err(error) => {
                tracing::error!(%error, "could not insert the dictated text");
                self.phase = Phase::Idle;
                self.recording_ended();
                self.hide();
                self.notice("Can't insert", &error.to_string(), AlertLevel::Error);
            }
        }
        self.last_used = Instant::now();
    }

    // ----- talking to the island -----------------------------------------

    fn send_card(&mut self) {
        let phase = match self.phase {
            Phase::Idle => return,
            Phase::Recording => {
                let Some(session) = &self.session else {
                    return;
                };
                if !session.revealed {
                    return; // Still deciding whether this was a real press.
                }
                let waiting = !session.began;
                if waiting {
                    DictationPhase::Loading
                } else {
                    DictationPhase::Recording
                }
            }
            Phase::Transcribing => DictationPhase::Transcribing,
        };
        let (elapsed_ms, hands_free, command) =
            self.session.as_ref().map_or((0, false, false), |session| {
                (
                    session.started.elapsed().as_millis() as u32,
                    session.hands_free,
                    session.command,
                )
            });
        let card = DictationCard {
            elapsed_ms,
            level: self.level,
            phase,
            bars: self.bars,
            hands_free,
            command,
        };
        self.send(UiUpdate::Card(Some(card)));
    }

    fn hide(&mut self) {
        self.send(UiUpdate::Card(None));
    }

    /// One of Onyx's own messages: no sender line, the Onyx mark beside it.
    fn notice(&mut self, title: &str, body: &str, level: AlertLevel) {
        self.notice_with_icon(title, body, level, None);
    }

    /// The same, with the icon of the app it is about.
    fn notice_with_icon(
        &mut self,
        title: &str,
        body: &str,
        level: AlertLevel,
        icon: Option<islet_core::media::ArtImage>,
    ) {
        self.send(UiUpdate::Notice(NotificationCard {
            source: String::new(),
            title: title.to_owned(),
            body: body.to_owned(),
            level,
            icon,
        }));
    }

    fn model_missing_notice(&mut self) {
        let now = Instant::now();
        if self
            .last_notice
            .is_some_and(|last| now.duration_since(last) < NOTICE_COOLDOWN)
        {
            return;
        }
        self.last_notice = Some(now);
        self.notice(
            "Model needed",
            "Download it in Settings to start dictating.",
            AlertLevel::NeedsInput,
        );
        settings::open(Some("dictation"));
    }

    fn send(&self, update: UiUpdate) {
        if self.ui.send(update).is_ok() {
            (self.notify)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::RecvTimeoutError;

    fn controller(ui: Sender<UiUpdate>) -> Controller {
        let config = DictationConfig {
            enabled: false,
            ..Default::default()
        };
        Controller {
            text_options: TextOptions::from_config(&config),
            config,
            paused: false,
            ui,
            notify: Arc::new(|| {}),
            hook: None,
            mic: None,
            mic_device: String::new(),
            asr: AsrClient::new("unused-test-worker.exe"),
            polisher: Arc::new(Polisher::new(
                PathBuf::from("unused-llama-server.exe"),
                PathBuf::from("unused.gguf"),
            )),
            polishing: None,
            manager: ModelManager::new(std::env::temp_dir().join("islet-controller-test")),
            model: ModelState::NotLoaded,
            loaded_model: String::new(),
            phase: Phase::Idle,
            session: None,
            last_used: Instant::now(),
            last_notice: None,
            sum_squares: 0.0,
            sample_count: 0,
            level: 0.0,
            bars: [0; DICTATION_BARS],
            loudness: Loudness::default(),
            assistant: None,
            last_dictated: None,
            next_tick: Instant::now(),
        }
    }

    #[test]
    fn steady_speech_leaves_room_for_louder_syllables() {
        let mut loudness = Loudness::default();
        // A second of room noise, then several seconds of steady speech.
        for _ in 0..30 {
            loudness.place(-58.0);
        }
        let mut steady = 0.0;
        for i in 0..150 {
            steady = loudness.place(if i % 6 == 0 { -14.0 } else { -22.0 });
        }
        assert!(steady > 0.2 && steady < 0.8, "steady speech sits mid-range: {steady}");
        assert!(loudness.place(-14.0) > steady + 0.15, "stressed syllables stand out");
        assert!(loudness.place(-58.0) < 0.05, "pauses fall back to rest");
    }

    #[test]
    fn media_pause_is_opt_in_and_sent_once_per_confirmed_recording() {
        for enabled in [false, true] {
            let (tx, rx) = unbounded();
            let mut controller = controller(tx);
            controller.config.pause_media = enabled;
            controller.config.tones = false;
            controller.model = ModelState::Ready;
            controller.loaded_model = controller.config.model.clone();
            controller.phase = Phase::Recording;
            controller.session = Some(Session {
                started: Instant::now(),
                reveal_at: Instant::now(),
                revealed: false,
                hands_free: false,
                command: false,
                began: true,
                stop_requested: false,
                pending: Vec::new(),
                outgoing: Vec::new(),
                samples: 0,
                peak: 0.0,
                stopped_at: None,
                end_sent_at: None,
            });
            assert!(rx.is_empty());
            assert!(controller.reveal());
            assert!(controller.reveal());
            let pauses = rx
                .try_iter()
                .filter(|update| matches!(update, UiUpdate::PauseMedia))
                .count();
            assert_eq!(pauses, usize::from(enabled));
        }
    }

    #[test]
    fn closed_hook_is_removed_from_selection_and_reported_once() {
        let (ui_tx, ui_rx) = unbounded();
        let (commands, commands_rx) = unbounded();
        let (hook_tx, hook_rx) = unbounded();
        drop(hook_tx);
        let worker = std::thread::spawn(move || {
            controller(ui_tx).run_with_hook_events(&commands_rx, Some(hook_rx))
        });
        let first = ui_rx.recv_timeout(Duration::from_secs(1));
        let duplicate = ui_rx.recv_timeout(Duration::from_millis(50));
        commands.send(Command::Shutdown).unwrap();
        worker.join().unwrap();
        assert!(
            matches!(first, Ok(UiUpdate::Notice(notice)) if notice.title == "Hotkeys unavailable")
        );
        assert!(matches!(duplicate, Err(RecvTimeoutError::Timeout)));
    }

    #[test]
    fn disconnected_mic_cancels_recording_and_returns_to_blocking_idle() {
        let (tx, rx) = unbounded();
        let mut controller = controller(tx);
        controller.phase = Phase::Recording;
        controller.microphone_disconnected();
        assert_eq!(controller.phase, Phase::Idle);
        assert!(controller.mic.is_none());
        assert!(controller.next_deadline().is_none());
        let updates: Vec<_> = rx.try_iter().collect();
        assert!(matches!(&updates[0], UiUpdate::Card(None)));
        assert!(
            matches!(&updates[1], UiUpdate::Notice(notice) if notice.title == "Microphone disconnected")
        );
    }

    #[test]
    fn speech_failure_stops_audio_delivery_instead_of_leaving_capture_running() {
        // A synthetic one-sample WAV; no real microphone, hook, or model is used.
        let path = std::env::temp_dir().join(format!("islet-error-mic-{}.wav", std::process::id()));
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&38_u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&32_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2_u32.to_le_bytes());
        wav.extend_from_slice(&0_i16.to_le_bytes());
        std::fs::write(&path, wav).unwrap();
        let mic = Microphone::from_wav(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let audio = mic.audio().clone();
        mic.start();
        assert!(audio.recv_timeout(Duration::from_secs(1)).is_ok());
        let (ui, _) = unbounded();
        let mut controller = controller(ui);
        controller.mic = Some(mic);
        controller.phase = Phase::Recording;
        controller.fail("Voice error", "Try again.");
        while audio.try_recv().is_ok() {}
        assert_eq!(
            audio.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Disconnected)
        );
        assert_eq!(controller.phase, Phase::Idle);
        assert!(controller.next_deadline().is_none());
    }
}
