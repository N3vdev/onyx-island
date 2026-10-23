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

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Select, Sender};
use islet_asr::{AsrClient, AsrEvent, LoadRequest, ModelManager, ModelStatus};
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
    /// Show (or update) the dictation pill; `None` hides it.
    Card(Option<DictationCard>),
    /// A transient message ("model not installed", an error).
    Notice(NotificationCard),
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

/// How often the pill's clock and waveform update while recording.
const TICK: Duration = Duration::from_millis(33);
/// A dictation shorter than this is an accident, not speech.
const MIN_SPEECH_SECONDS: f32 = 0.30;
/// Safety stop for a forgotten hands-free recording.
const MAX_RECORDING: Duration = Duration::from_secs(10 * 60);
/// How long the check mark stays up.
const DONE_VISIBLE: Duration = Duration::from_millis(700);
/// Audio is forwarded to the worker in chunks of about this many samples.
const FORWARD_SAMPLES: usize = 480;
/// Audio held while the model is still loading (about two minutes).
const MAX_PENDING_SAMPLES: usize = 16_000 * 120;
/// Notices about a missing model are not repeated more often than this.
const NOTICE_COOLDOWN: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Recording,
    /// The user stopped; waiting for the final text.
    Transcribing,
    /// Text inserted; the check mark is showing.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelState {
    NotLoaded,
    Loading,
    Ready,
}

struct Session {
    started: Instant,
    hands_free: bool,
    /// `Begin` has been sent to the worker.
    began: bool,
    /// The user stopped before the model finished loading.
    stop_requested: bool,
    /// Audio gathered while the model was loading.
    pending: Vec<f32>,
    /// Audio waiting to be forwarded in a full chunk.
    outgoing: Vec<f32>,
    samples: usize,
    stopped_at: Option<Instant>,
    /// When `End` was sent to the worker.
    end_sent_at: Option<Instant>,
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

    phase: Phase,
    session: Option<Session>,
    done_until: Option<Instant>,
    last_used: Instant,
    last_notice: Option<Instant>,

    // Meter state for the pill.
    sum_squares: f64,
    sample_count: usize,
    level: f32,
    bars: [u8; DICTATION_BARS],
    next_tick: Instant,
}

impl Controller {
    fn new(
        config: DictationConfig,
        models_dir: PathBuf,
        ui: Sender<UiUpdate>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let hook = match KeyboardHook::start(&config.push_to_talk, &config.hands_free) {
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
            manager: ModelManager::new(models_dir),
            model: ModelState::NotLoaded,
            loaded_model: String::new(),
            phase: Phase::Idle,
            session: None,
            done_until: None,
            last_used: Instant::now(),
            last_notice: None,
            sum_squares: 0.0,
            sample_count: 0,
            level: 0.0,
            bars: [0; DICTATION_BARS],
            next_tick: Instant::now(),
        };
        controller.update_hook_enabled();
        controller.maybe_preload();
        controller
    }

    fn run(&mut self, commands: &Receiver<Command>) {
        let hook_events = self.hook.as_ref().map(|hook| hook.events().clone());
        let asr_events = self.asr.events().clone();
        loop {
            let mic_audio = self.mic.as_ref().map(|mic| mic.audio().clone());

            let mut select = Select::new();
            let commands_index = select.recv(commands);
            let asr_index = select.recv(&asr_events);
            let hook_index = hook_events.as_ref().map(|events| select.recv(events));
            let mic_index = mic_audio.as_ref().map(|audio| select.recv(audio));

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
                    if let Ok(HookEvent::Action(action)) = operation.recv(events) {
                        self.on_action(action);
                    }
                }
            } else if Some(index) == mic_index {
                if let Some(audio) = &mic_audio {
                    if let Ok(chunk) = operation.recv(audio) {
                        self.on_audio(&chunk);
                    }
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
    }

    /// When the loop must wake even if nothing arrives.
    fn next_deadline(&self) -> Option<Instant> {
        let mut deadlines = Vec::with_capacity(3);
        if self.phase == Phase::Recording || self.phase == Phase::Transcribing {
            deadlines.push(self.next_tick);
        }
        if let Some(until) = self.done_until {
            deadlines.push(until);
        }
        if self.phase == Phase::Idle && self.asr.is_running() && !self.config.keep_warm {
            deadlines.push(self.last_used + self.unload_after());
        }
        deadlines.into_iter().min()
    }

    fn unload_after(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.unload_after_minutes.max(1)) * 60)
    }

    fn apply_config(&mut self, config: DictationConfig) {
        if let Some(hook) = &self.hook {
            hook.set_combos(&config.push_to_talk, &config.hands_free);
        }
        self.text_options = TextOptions::from_config(&config);

        let model_changed = config.model != self.config.model;
        let mic_changed = config.microphone != self.config.microphone;
        self.config = config;
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
            Action::Start => self.start(),
            Action::EnterHandsFree => {
                if let Some(session) = &mut self.session {
                    session.hands_free = true;
                }
                self.send_card();
            }
            Action::Stop => self.stop(),
            Action::Discard => self.discard(),
            Action::SuppressStartMenu => islet_input::suppress_start_menu(),
        }
    }

    fn start(&mut self) {
        if self.phase == Phase::Transcribing || self.phase == Phase::Recording {
            // Still busy with the previous dictation.
            self.recording_ended();
            return;
        }
        self.done_until = None;

        // Setting a hotkey in Islet's own settings window must not start a
        // dictation (and there is nowhere to type into there).
        if islet_input::foreground_exe().as_deref() == Some("islet-settings.exe") {
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
                "That window runs as administrator. Run Islet as administrator to dictate into it.",
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
        if self.model == ModelState::NotLoaded || self.loaded_model != spec.id {
            self.asr.shutdown();
            self.begin_load();
            if self.model == ModelState::NotLoaded {
                self.notice(
                    "Voice error",
                    "The voice model could not be started.",
                    AlertLevel::Error,
                );
                self.recording_ended();
                return;
            }
        }

        let mut session = Session {
            started: Instant::now(),
            hands_free: false,
            began: false,
            stop_requested: false,
            pending: Vec::new(),
            outgoing: Vec::new(),
            samples: 0,
            stopped_at: None,
            end_sent_at: None,
        };
        if self.model == ModelState::Ready {
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

        if self.config.tones {
            tone::play(Tone::Start);
        }
        if let Some(mic) = &self.mic {
            mic.start();
        }
        self.next_tick = Instant::now();
        self.send_card();
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
        if self.config.tones {
            tone::play(Tone::Stop);
        }
        let Some(session) = &mut self.session else {
            self.phase = Phase::Idle;
            return;
        };
        session.stopped_at = Some(Instant::now());
        if (session.samples as f32) < MIN_SPEECH_SECONDS * 16_000.0 {
            self.discard();
            return;
        }
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
        self.phase = Phase::Idle;
        self.last_used = Instant::now();
        self.recording_ended();
        self.hide();
    }

    fn fail(&mut self, title: &str, body: &str) {
        self.session = None;
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
        let samples = std::mem::take(&mut session.outgoing);
        if self.asr.audio(&samples).is_err() {
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
        let target = ((db + 55.0) / 45.0).clamp(0.0, 1.0).powf(0.85);
        // Fast attack, slower release.
        let rate = if target > self.level { 0.7 } else { 0.25 };
        self.level += (target - self.level) * rate;
        self.bars.copy_within(1.., 0);
        self.bars[DICTATION_BARS - 1] = (self.level * 255.0) as u8;
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        self.next_tick = now + TICK;

        if self.done_until.is_some_and(|until| now >= until) {
            self.done_until = None;
            if self.phase == Phase::Done {
                self.phase = Phase::Idle;
                self.hide();
            }
        }
        match self.phase {
            Phase::Recording => {
                self.update_meter();
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
                if self.asr.is_running()
                    && !self.config.keep_warm
                    && now.duration_since(self.last_used) >= self.unload_after()
                {
                    tracing::info!("unloading the speech model after idle");
                    self.asr.shutdown();
                    self.model = ModelState::NotLoaded;
                    self.loaded_model.clear();
                    self.mic = None;
                }
            }
            Phase::Done => {}
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
        self.session = None;
        let mut text = transcript::clean(raw, &self.text_options);
        if text.is_empty() {
            self.phase = Phase::Idle;
            self.recording_ended();
            self.hide();
            return;
        }
        if self.config.trailing_space {
            text.push(' ');
        }

        let method = match self.config.injection {
            InjectionMethod::Clipboard => Method::Clipboard,
            InjectionMethod::Unicode => Method::Unicode,
        };
        let text_ready = stopped_at.map(|at| at.elapsed());
        match islet_input::insert_text(&text, method) {
            Ok(report) => {
                tracing::info!(
                    tail_audio_ms,
                    tail_decode_ms,
                    worker_round_trip_ms = worker_ms,
                    release_to_text_ms = text_ready.map(|d| d.as_millis() as u64),
                    release_to_inserted_ms = stopped_at.map(|at| at.elapsed().as_millis() as u64),
                    method = ?report.method,
                    clipboard_restored = report.clipboard_restored,
                    "dictation inserted"
                );
                if self.config.tones {
                    tone::play(Tone::Done);
                }
                self.phase = Phase::Done;
                self.done_until = Some(Instant::now() + DONE_VISIBLE);
                self.recording_ended();
                self.send_card();
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
                let waiting = self.session.as_ref().is_some_and(|session| !session.began);
                if waiting {
                    DictationPhase::Loading
                } else {
                    DictationPhase::Recording
                }
            }
            Phase::Transcribing => DictationPhase::Transcribing,
            Phase::Done => DictationPhase::Done,
        };
        let (elapsed_ms, hands_free) = self.session.as_ref().map_or((0, false), |session| {
            (
                session.started.elapsed().as_millis() as u32,
                session.hands_free,
            )
        });
        let card = DictationCard {
            elapsed_ms,
            level: self.level,
            phase,
            bars: self.bars,
            hands_free,
        };
        self.send(UiUpdate::Card(Some(card)));
    }

    fn hide(&mut self) {
        self.send(UiUpdate::Card(None));
    }

    fn notice(&mut self, title: &str, body: &str, level: AlertLevel) {
        self.send(UiUpdate::Notice(NotificationCard {
            source: "Islet".to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            level,
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
