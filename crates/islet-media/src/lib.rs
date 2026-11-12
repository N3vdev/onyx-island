//! Now-playing data from Windows' Global System Media Transport Controls.
//!
//! [`MediaService`] owns one MTA thread. GSMTC events (which arrive on WinRT
//! thread-pool threads) only post a "refresh" flag to that thread; it then
//! re-reads the session and reports a [`MediaInfo`] through the callback. No
//! polling: an idle system with nothing playing costs no wakeups.

#![allow(unsafe_op_in_unsafe_fn)]

mod tabs;

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use islet_core::media::{
    choose_session, dominant_color, source_display_name, ArtImage, SessionCandidate,
};
use islet_core::music::{is_browser_name, MusicFilter, SessionFacts};
use windows::core::{Ref, Result as WinResult, HSTRING};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as MediaProperties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    MediaPropertiesChangedEventArgs, PlaybackInfoChangedEventArgs, SessionsChangedEventArgs,
    TimelinePropertiesChangedEventArgs,
};
use windows::Media::MediaPlaybackType;
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeHighQualityCubic,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Shell::{
    FOLDERID_AppsFolder, IShellItem, SHCreateItemInKnownFolder, KF_FLAG_DEFAULT,
    SIGDN_NORMALDISPLAY,
};

/// Album art edge length in pixels. Drawn at up to 78 dp, so this covers
/// 150% scaling crisply; larger displays get a mild upscale.
pub const ART_SIZE: u32 = 128;
/// GSMTC fires several events per change; coalesce them.
const COALESCE: Duration = Duration::from_millis(40);
/// Tab titles can lag a track change; re-check a rejected browser track once.
const RECHECK_AFTER: Duration = Duration::from_millis(1_500);
/// The island shows the first player that started, plus the next one in a
/// bubble beside it.
const MAX_PLAYERS: usize = 2;

/// The current track as reported by the source app.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub app_id: String,
    /// Short name for display, e.g. "Spotify".
    pub source: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Seconds into the track, extrapolated to the moment this was sent.
    pub position: f64,
    /// Seconds; zero when the app reports no timeline.
    pub duration: f64,
    pub rate: f64,
    pub art: Option<ArtImage>,
    pub accent: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaCommand {
    TogglePlayPause,
    Next,
    Previous,
    /// Absolute position in seconds.
    Seek(f64),
}

/// One-shot pause independent of the island's music filter and visibility.
/// WinRT work stays off the recording/UI threads; nothing runs while disabled.
pub fn pause_playing_media() {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("islet-pause-media".to_owned())
        .spawn(|| {
            struct Reset;
            impl Drop for Reset {
                fn drop(&mut self) {
                    RUNNING.store(false, Ordering::Release);
                }
            }
            let _reset = Reset;
            // SAFETY: initialization and cleanup occur on this worker thread.
            if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                tracing::debug!(%error, "media pause initialization failed");
                return;
            }
            let result = (|| -> WinResult<()> {
                let manager = Manager::RequestAsync()?.join()?;
                let sessions = manager.GetSessions()?;
                // Issue every request before waiting for any individual player.
                let mut pending = Vec::new();
                for index in 0..sessions.Size()? {
                    let Ok(session) = sessions.GetAt(index) else {
                        continue;
                    };
                    let status = session
                        .GetPlaybackInfo()
                        .and_then(|info| info.PlaybackStatus());
                    if matches!(status, Ok(PlaybackStatus::Playing)) {
                        match session.TryPauseAsync() {
                            Ok(operation) => pending.push(operation),
                            Err(error) => tracing::debug!(%error, "media pause request failed"),
                        }
                    }
                }
                for operation in pending {
                    match operation.join() {
                        Ok(true) => {}
                        Ok(false) => tracing::debug!("media app declined pause"),
                        Err(error) => tracing::debug!(%error, "media pause failed"),
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                tracing::debug!(%error, "could not enumerate media for dictation");
            }
            unsafe { RoUninitialize() };
        });
    if let Err(error) = spawned {
        RUNNING.store(false, Ordering::Release);
        tracing::warn!(%error, "could not start media pause worker");
    }
}

enum Message {
    Refresh(Refresh),
    /// A transport command for the player with this app id.
    Command(String, MediaCommand),
    Filter(MusicFilter),
    PauseHold(Duration),
    Held(bool),
    Stop,
}

#[derive(Clone, Copy, Debug, Default)]
struct Refresh {
    sessions: bool,
    properties: bool,
    playback: bool,
    timeline: bool,
}

impl Refresh {
    const ALL: Self = Self {
        sessions: true,
        properties: true,
        playback: true,
        timeline: true,
    };

    fn merge(&mut self, other: Self) {
        self.sessions |= other.sessions;
        self.properties |= other.properties;
        self.playback |= other.playback;
        self.timeline |= other.timeline;
    }

    fn any(self) -> bool {
        self.sessions || self.properties || self.playback || self.timeline
    }
}

/// Handle to the media thread. Dropping it stops the thread.
pub struct MediaService {
    tx: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl MediaService {
    /// Starts the media thread. `on_update` runs on that thread whenever a
    /// shown track changes. Players come in the order they started playing
    /// (at most two); empty means nothing is playing.
    pub fn start(
        filter: MusicFilter,
        on_update: impl FnMut(Vec<MediaInfo>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = unbounded();
        let thread_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("islet-media".to_owned())
            .spawn(move || media_thread(thread_tx, rx, filter, on_update))?;
        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Sends `command` to the player whose [`MediaInfo::app_id`] is `app_id`.
    pub fn send(&self, app_id: &str, command: MediaCommand) {
        let _ = self.tx.send(Message::Command(app_id.to_owned(), command));
    }

    /// Replaces the music filter (config reload) and re-picks the session.
    pub fn set_filter(&self, filter: MusicFilter) {
        let _ = self.tx.send(Message::Filter(filter));
    }

    /// How long a paused player keeps its place while another one plays.
    pub fn set_pause_hold(&self, hold: Duration) {
        let _ = self.tx.send(Message::PauseHold(hold));
    }

    /// While held (the pointer is on the island), paused players keep their
    /// place, so nothing swaps out from under the cursor.
    pub fn set_held(&self, held: bool) {
        let _ = self.tx.send(Message::Held(held));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Subscription {
    session: Session,
    properties: i64,
    playback: i64,
    timeline: i64,
}

impl Subscription {
    fn new(session: Session, tx: &Sender<Message>) -> WinResult<Self> {
        let refresh = |flags: Refresh| {
            let tx = tx.clone();
            move || {
                let _ = tx.send(Message::Refresh(flags));
            }
        };
        let on_properties = refresh(Refresh {
            properties: true,
            sessions: true,
            ..Refresh::default()
        });
        let on_playback = refresh(Refresh {
            playback: true,
            timeline: true,
            ..Refresh::default()
        });
        let on_timeline = refresh(Refresh {
            timeline: true,
            ..Refresh::default()
        });
        let properties = session.MediaPropertiesChanged(&TypedEventHandler::new(
            move |_: Ref<Session>, _: Ref<MediaPropertiesChangedEventArgs>| {
                on_properties();
                Ok(())
            },
        ))?;
        let playback = session.PlaybackInfoChanged(&TypedEventHandler::new(
            move |_: Ref<Session>, _: Ref<PlaybackInfoChangedEventArgs>| {
                on_playback();
                Ok(())
            },
        ))?;
        let timeline = session.TimelinePropertiesChanged(&TypedEventHandler::new(
            move |_: Ref<Session>, _: Ref<TimelinePropertiesChangedEventArgs>| {
                on_timeline();
                Ok(())
            },
        ))?;
        Ok(Self {
            session,
            properties,
            playback,
            timeline,
        })
    }
}

/// Watches play/pause on a session we are *not* showing, so a browser that
/// starts playing replaces a paused Spotify (and vice versa).
struct Watcher {
    session: Session,
    playback: i64,
    properties: i64,
}

impl Watcher {
    fn new(session: Session, tx: &Sender<Message>) -> WinResult<Self> {
        let playback_tx = tx.clone();
        let playback = session.PlaybackInfoChanged(&TypedEventHandler::new(
            move |_: Ref<Session>, _: Ref<PlaybackInfoChangedEventArgs>| {
                let _ = playback_tx.send(Message::Refresh(Refresh {
                    sessions: true,
                    ..Refresh::default()
                }));
                Ok(())
            },
        ))?;
        let properties_tx = tx.clone();
        let properties = session.MediaPropertiesChanged(&TypedEventHandler::new(
            move |_: Ref<Session>, _: Ref<MediaPropertiesChangedEventArgs>| {
                let _ = properties_tx.send(Message::Refresh(Refresh {
                    sessions: true,
                    ..Refresh::default()
                }));
                Ok(())
            },
        ));
        let properties = match properties {
            Ok(token) => token,
            Err(error) => {
                let _ = session.RemovePlaybackInfoChanged(playback);
                return Err(error);
            }
        };
        Ok(Self {
            session,
            playback,
            properties,
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.session.RemovePlaybackInfoChanged(self.playback);
        let _ = self.session.RemoveMediaPropertiesChanged(self.properties);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.session.RemoveMediaPropertiesChanged(self.properties);
        let _ = self.session.RemovePlaybackInfoChanged(self.playback);
        let _ = self.session.RemoveTimelinePropertiesChanged(self.timeline);
    }
}

/// Cached per-track data that is expensive to re-read.
#[derive(Default)]
struct TrackCache {
    title: String,
    artist: String,
    art: Option<ArtImage>,
    art_hash: u64,
    accent: Option<[u8; 3]>,
}

struct Playback {
    playing: bool,
    rate: f64,
}

struct TimelineSample {
    position: f64,
    duration: f64,
    /// FILETIME ticks (100 ns since 1601) when `position` was true.
    updated: i64,
}

fn media_thread(
    tx: Sender<Message>,
    rx: Receiver<Message>,
    filter: MusicFilter,
    mut on_update: impl FnMut(Vec<MediaInfo>),
) {
    // SAFETY: balanced with RoUninitialize below on this same thread.
    if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        tracing::error!(%error, "media thread could not initialise WinRT");
        return;
    }
    if let Err(error) = run(&tx, &rx, filter, &mut on_update) {
        tracing::warn!(%error, "now playing is unavailable");
    }
    // SAFETY: matches the successful RoInitialize above.
    unsafe { RoUninitialize() };
}

fn run(
    tx: &Sender<Message>,
    rx: &Receiver<Message>,
    filter: MusicFilter,
    on_update: &mut impl FnMut(Vec<MediaInfo>),
) -> WinResult<()> {
    let manager = Manager::RequestAsync()?.join()?;
    let sessions_tx = tx.clone();
    let current_tx = tx.clone();
    let sessions_token = manager.SessionsChanged(&TypedEventHandler::new(
        move |_: Ref<Manager>, _: Ref<SessionsChangedEventArgs>| {
            let _ = sessions_tx.send(Message::Refresh(Refresh::ALL));
            Ok(())
        },
    ))?;
    let current_token = manager.CurrentSessionChanged(&TypedEventHandler::new(
        move |_: Ref<Manager>, _: Ref<CurrentSessionChangedEventArgs>| {
            let _ = current_tx.send(Message::Refresh(Refresh::ALL));
            Ok(())
        },
    ))?;
    // SAFETY: plain in-process COM activation on an initialised MTA thread.
    let wic: Option<IWICImagingFactory> =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| tracing::warn!(%error, "album art decoding unavailable"))
            .ok();

    let tabs = tabs::TabReader::new()
        .map_err(|error| tracing::warn!(%error, "browser tab titles unavailable"))
        .ok();
    let mut state = State {
        filter,
        hidden: None,
        hidden_poll_at: None,
        last_played: None,
        tabs,
        recheck_at: None,
        rechecked: std::collections::HashSet::new(),
        artist_badges: ArtistBadges::default(),
        players: Vec::new(),
        order: Vec::new(),
        pause_hold: Duration::from_millis(u64::from(islet_core::island::MEDIA_PAUSED_MS)),
        held: false,
        watchers: Vec::new(),
        last_sent: Vec::new(),
    };
    let mut pending = Refresh::ALL;

    loop {
        if pending.any() {
            // Coalesce bursts, but keep commands responsive.
            let deadline = std::time::Instant::now() + COALESCE;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                match rx.recv_timeout(remaining) {
                    Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        return cleanup(&manager, sessions_token, current_token);
                    }
                    Ok(message) => state.handle(message, &mut pending),
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            let flags = std::mem::take(&mut pending);
            if let Err(error) = state.refresh(&manager, flags, tx, wic.as_ref()) {
                tracing::debug!(%error, "media refresh failed");
                // A session probably went away; re-resolve next time.
                state.players.clear();
            }
            let info = state.info();
            if let Some(playing) = info.iter().find(|info| info.playing) {
                if state.last_played.as_deref() != Some(playing.app_id.as_str()) {
                    state.last_played = Some(playing.app_id.clone());
                }
            }
            if info != state.last_sent {
                state.last_sent.clone_from(&info);
                on_update(info);
            }
            continue;
        }

        // Idle: block, except for a pending tab re-check or a paused player
        // whose hold runs out.
        let wake = [state.recheck_at, state.next_expiry(), state.hidden_poll_at]
            .into_iter()
            .flatten()
            .min();
        let message = match wake {
            Some(at) => {
                match rx.recv_timeout(at.saturating_duration_since(std::time::Instant::now())) {
                    Ok(message) => Ok(message),
                    Err(RecvTimeoutError::Timeout) => {
                        if state
                            .recheck_at
                            .is_some_and(|at| at <= std::time::Instant::now())
                        {
                            state.recheck_at = None;
                        }
                        if state
                            .hidden_poll_at
                            .is_some_and(|at| at <= std::time::Instant::now())
                        {
                            state.hidden_poll_at = None;
                        }
                        pending.merge(Refresh {
                            sessions: true,
                            ..Refresh::default()
                        });
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => Err(()),
                }
            }
            None => rx.recv().map_err(|_| ()),
        };
        match message {
            Ok(Message::Stop) | Err(()) => return cleanup(&manager, sessions_token, current_token),
            Ok(message) => state.handle(message, &mut pending),
        }
    }
}

fn cleanup(manager: &Manager, sessions: i64, current: i64) -> WinResult<()> {
    let _ = manager.RemoveSessionsChanged(sessions);
    let _ = manager.RemoveCurrentSessionChanged(current);
    Ok(())
}

/// A player in line for the island, in the order it started playing.
struct Queued {
    app_id: String,
    /// When it paused. It keeps its place for the pause hold, so pausing
    /// the first player never swaps the second one in under your cursor.
    paused_since: Option<std::time::Instant>,
}

/// A session the island shows, with its own event subscription and cache.
struct Player {
    app_id: String,
    subscription: Subscription,
    /// Display name of the session's app, resolved once.
    source_name: String,
    cache: TrackCache,
    playback: Option<Playback>,
    timeline: Option<TimelineSample>,
}

impl Player {
    fn new(app_id: String, session: Session, tx: &Sender<Message>) -> WinResult<Self> {
        Ok(Self {
            source_name: app_display_name(&app_id),
            app_id,
            subscription: Subscription::new(session, tx)?,
            cache: TrackCache::default(),
            playback: None,
            timeline: None,
        })
    }

    fn session(&self) -> &Session {
        &self.subscription.session
    }

    fn refresh(&mut self, flags: Refresh, wic: Option<&IWICImagingFactory>) -> WinResult<()> {
        let session = self.session().clone();
        if flags.properties {
            let properties = session.TryGetMediaPropertiesAsync()?.join()?;
            let title = properties.Title()?.to_string_lossy();
            let artist = properties.Artist()?.to_string_lossy();
            let track_changed = title != self.cache.title || artist != self.cache.artist;
            self.cache.title = title;
            self.cache.artist = artist;
            match properties
                .Thumbnail()
                .ok()
                .and_then(|thumb| read_stream(&thumb).ok())
            {
                Some(bytes) => {
                    let hash = fnv1a(&bytes);
                    if hash != self.cache.art_hash || self.cache.art.is_none() {
                        self.cache.art_hash = hash;
                        self.cache.art = wic.and_then(|wic| {
                            decode_art(wic, &bytes)
                                .map_err(|error| tracing::debug!(%error, "album art decode failed"))
                                .ok()
                        });
                        self.cache.accent = self
                            .cache
                            .art
                            .as_ref()
                            .and_then(|art| dominant_color(&art.pixels, art.width, art.height));
                    }
                }
                None if track_changed => {
                    // Browsers often send the thumbnail a moment after the
                    // title; drop stale art from the previous track.
                    self.cache.art = None;
                    self.cache.art_hash = 0;
                    self.cache.accent = None;
                }
                None => {}
            }
        }

        if flags.playback {
            let info = session.GetPlaybackInfo()?;
            let playing = info.PlaybackStatus()? == PlaybackStatus::Playing;
            let rate = info
                .PlaybackRate()
                .and_then(|rate| rate.Value())
                .ok()
                .filter(|rate| *rate > 0.0)
                .unwrap_or(1.0);
            self.playback = Some(Playback { playing, rate });
        }

        if flags.timeline {
            let timeline = session.GetTimelineProperties()?;
            let start = timeline.StartTime()?.Duration;
            let end = timeline.EndTime()?.Duration;
            let position = timeline.Position()?.Duration;
            self.timeline = Some(TimelineSample {
                position: ticks_to_seconds(position - start),
                duration: ticks_to_seconds((end - start).max(0)),
                updated: timeline.LastUpdatedTime()?.UniversalTime,
            });
        }
        Ok(())
    }

    fn info(&self) -> Option<MediaInfo> {
        if self.cache.title.is_empty() && self.cache.artist.is_empty() {
            return None;
        }
        let (playing, rate) = self
            .playback
            .as_ref()
            .map_or((false, 1.0), |playback| (playback.playing, playback.rate));
        let (position, duration) = self.timeline.as_ref().map_or((0.0, 0.0), |timeline| {
            let mut position = timeline.position;
            if playing && timeline.updated > 0 {
                let elapsed = ticks_to_seconds((now_filetime() - timeline.updated).max(0));
                position += elapsed * rate;
            }
            if timeline.duration > 0.0 {
                position = position.clamp(0.0, timeline.duration);
            }
            (position, timeline.duration)
        });
        Some(MediaInfo {
            source: self.source_name.clone(),
            app_id: self.app_id.clone(),
            title: self.cache.title.clone(),
            artist: self.cache.artist.clone(),
            playing,
            position,
            duration,
            rate,
            art: self.cache.art.clone(),
            accent: self.cache.accent,
        })
    }

    fn command(&self, command: MediaCommand) {
        let session = self.session();
        let result = match command {
            MediaCommand::TogglePlayPause => session
                .TryTogglePlayPauseAsync()
                .and_then(|operation| operation.join()),
            MediaCommand::Next => session
                .TrySkipNextAsync()
                .and_then(|operation| operation.join()),
            MediaCommand::Previous => session
                .TrySkipPreviousAsync()
                .and_then(|operation| operation.join()),
            MediaCommand::Seek(seconds) => {
                let start = session
                    .GetTimelineProperties()
                    .and_then(|timeline| timeline.StartTime())
                    .map(|start| start.Duration)
                    .unwrap_or(0);
                session
                    .TryChangePlaybackPositionAsync(start + seconds_to_ticks(seconds))
                    .and_then(|operation| operation.join())
            }
        };
        match result {
            Ok(true) => {}
            Ok(false) => tracing::debug!(?command, "media app declined the command"),
            Err(error) => tracing::debug!(?command, %error, "media command failed"),
        }
    }
}

/// Music playing in a browser tab that the browser does not report: Firefox
/// gives Windows one session, for whichever tab played last, so a YouTube
/// Music tab playing behind a newer video tab is otherwise invisible. Read
/// from the tab itself (its speaker icon and, for YouTube Music, its player
/// bar), and controlled through that player bar.
#[derive(Clone, Debug, PartialEq)]
struct HiddenTab {
    browser: String,
    tab: String,
    title: String,
    artist: String,
    playing: bool,
    heard_at: std::time::Instant,
}

impl HiddenTab {
    fn app_id(&self) -> String {
        format!("{}#{}", self.browser, self.tab)
    }

    /// A player already on the island plays this tab's song: the browser
    /// reports the tab after all (it first looked like something else), so
    /// it is not shown twice.
    fn shown_by(&self, players: &[MediaInfo]) -> bool {
        let same = |a: &str, b: &str| !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim());
        players.iter().any(|player| {
            MusicFilter::tab_names_track(&self.tab, &player.title) || same(&self.title, &player.title)
        })
    }

    fn info(&self) -> MediaInfo {
        MediaInfo {
            app_id: self.app_id(),
            source: self.tab.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            playing: self.playing,
            position: 0.0,
            duration: 0.0,
            rate: 1.0,
            art: None,
            accent: None,
        }
    }
}

/// While a hidden tab plays, how often its player bar is read again (the
/// tab sends Windows nothing when its song changes).
const HIDDEN_TAB_POLL: Duration = Duration::from_secs(2);

struct State {
    filter: MusicFilter,
    hidden: Option<HiddenTab>,
    hidden_poll_at: Option<std::time::Instant>,
    /// App id of the music session most recently seen playing.
    last_played: Option<String>,
    tabs: Option<tabs::TabReader>,
    /// One-shot re-check for a browser track rejected before its tab title
    /// caught up.
    recheck_at: Option<std::time::Instant>,
    /// Rejected tracks already re-checked, so a non-music video playing for
    /// an hour costs one re-check, not a timer.
    rechecked: std::collections::HashSet<String>,
    artist_badges: ArtistBadges,
    /// The shown players, first-started first.
    players: Vec<Player>,
    /// Music sessions that started playing, in that order.
    order: Vec<Queued>,
    pause_hold: Duration,
    held: bool,
    watchers: Vec<Watcher>,
    last_sent: Vec<MediaInfo>,
}

impl State {
    fn handle(&mut self, message: Message, pending: &mut Refresh) {
        let sessions = Refresh {
            sessions: true,
            ..Refresh::default()
        };
        match message {
            Message::Refresh(flags) => pending.merge(flags),
            Message::Command(app_id, command) => {
                if let Some(player) = self.players.iter().find(|player| player.app_id == app_id) {
                    player.command(command);
                } else if let (Some(hidden), Some(reader)) = (&self.hidden, self.tabs.as_mut()) {
                    if hidden.app_id() == app_id {
                        let names: &[&str] = match command {
                            MediaCommand::TogglePlayPause => &["Pause", "Play"],
                            MediaCommand::Next => &["Next"],
                            MediaCommand::Previous => &["Previous"],
                            MediaCommand::Seek(_) => &[],
                        };
                        if !names.is_empty() && reader.press_player_button(&hidden.tab, names) {
                            pending.merge(sessions);
                        }
                    }
                }
            }
            Message::Filter(filter) => {
                self.filter = filter;
                pending.merge(Refresh::ALL);
            }
            Message::PauseHold(hold) => {
                self.pause_hold = hold;
                pending.merge(sessions);
            }
            Message::Held(held) => {
                self.held = held;
                if !held {
                    pending.merge(sessions);
                }
            }
            // Handled by the caller, which owns the manager.
            Message::Stop => {}
        }
    }

    /// When the longest-paused queued player's hold runs out.
    fn next_expiry(&self) -> Option<std::time::Instant> {
        if self.held {
            return None;
        }
        self.order
            .iter()
            .filter_map(|queued| queued.paused_since)
            .min()
            .map(|since| since + self.pause_hold)
    }

    /// Keeps `order` in step with the sessions: new players join the end,
    /// closed ones leave, and paused ones leave once their hold runs out.
    fn update_order(&mut self, music: &[MusicSession]) {
        let now = std::time::Instant::now();
        self.order
            .retain(|queued| music.iter().any(|session| session.app_id == queued.app_id));
        for session in music {
            match self
                .order
                .iter_mut()
                .find(|queued| queued.app_id == session.app_id)
            {
                Some(queued) if session.playing => queued.paused_since = None,
                Some(queued) => {
                    queued.paused_since.get_or_insert(now);
                }
                None if session.playing => self.order.push(Queued {
                    app_id: session.app_id.clone(),
                    paused_since: None,
                }),
                None => {}
            }
        }
        if !self.held {
            let hold = self.pause_hold;
            self.order.retain(|queued| {
                queued
                    .paused_since
                    .is_none_or(|since| now.saturating_duration_since(since) < hold)
            });
        }
    }

    fn refresh(
        &mut self,
        manager: &Manager,
        mut flags: Refresh,
        tx: &Sender<Message>,
        wic: Option<&IWICImagingFactory>,
    ) -> WinResult<()> {
        if flags.sessions || self.players.is_empty() {
            let pick = pick_session(
                manager,
                &self.filter,
                self.last_played.as_deref(),
                self.tabs.as_mut(),
                &mut self.artist_badges,
            )?;
            self.find_hidden_tab(&pick.rejected_browser_tracks);
            for key in pick.rejected_browser_tracks {
                if self.rechecked.len() > 256 {
                    self.rechecked.clear();
                }
                if self.rechecked.insert(key) {
                    self.recheck_at = Some(std::time::Instant::now() + RECHECK_AFTER);
                }
            }
            self.watchers.clear();
            for session in pick.all {
                match Watcher::new(session, tx) {
                    Ok(watcher) => self.watchers.push(watcher),
                    Err(error) => tracing::debug!(%error, "could not watch a media session"),
                }
            }
            self.update_order(&pick.music);
            // Players in line, first-started first. A player that is playing
            // always gets a slot before a paused one, so a paused player held
            // on the island never pushes out music that is playing now. With
            // none in line (all paused a while), fall back to the single best
            // session so a paused song stays until the island closes it.
            let mut in_line: Vec<&MusicSession> = self
                .order
                .iter()
                .filter_map(|queued| {
                    pick.music
                        .iter()
                        .find(|session| session.app_id == queued.app_id)
                })
                .collect();
            if in_line.len() > MAX_PLAYERS {
                in_line.sort_by_key(|session| !session.playing);
            }
            let mut wanted: Vec<(String, Session)> = in_line
                .into_iter()
                .take(MAX_PLAYERS)
                .map(|session| (session.app_id.clone(), session.session.clone()))
                .collect();
            if wanted.is_empty() {
                if let Some(session) = pick.chosen {
                    let app_id = session
                        .SourceAppUserModelId()
                        .map(|id| id.to_string_lossy())
                        .unwrap_or_default();
                    wanted.push((app_id, session));
                }
            }
            let mut previous = std::mem::take(&mut self.players);
            for (app_id, session) in wanted {
                if let Some(index) = previous.iter().position(|player| player.app_id == app_id) {
                    self.players.push(previous.remove(index));
                } else {
                    self.players.push(Player::new(app_id, session, tx)?);
                    flags = Refresh::ALL;
                }
            }
        }
        for player in &mut self.players {
            player.refresh(flags, wic)?;
        }
        Ok(())
    }

    fn info(&self) -> Vec<MediaInfo> {
        let mut info: Vec<MediaInfo> = self.players.iter().filter_map(Player::info).collect();
        if let Some(hidden) = self.hidden.as_ref().filter(|hidden| !hidden.shown_by(&info)) {
            // Playing now: it takes the main pill ahead of paused players.
            let place = if hidden.playing {
                info.iter().position(|player| !player.playing).unwrap_or(info.len())
            } else {
                info.len()
            };
            info.insert(place, hidden.info());
            info.truncate(MAX_PLAYERS);
        }
        info
    }

    /// Looks for music playing in a browser tab the browser is not
    /// reporting (its session shows another, non-music tab).
    fn find_hidden_tab(&mut self, rejected: &[String]) {
        let now = std::time::Instant::now();
        let Some(reader) = self.tabs.as_mut().filter(|_| self.filter.music_only) else {
            self.hidden = None;
            return;
        };
        if rejected.is_empty() && self.hidden.is_none() {
            return;
        }
        // "browser app id|title" of the browser tracks the filter turned down.
        let reported: Vec<(&str, &str)> = rejected
            .iter()
            .filter_map(|key| key.split_once('|'))
            .collect();
        let audible = reader.audible_titles();
        let found = audible.iter().find(|tab| {
            self.filter.is_music_site_tab(tab)
                && !reported
                    .iter()
                    .any(|(_, track)| MusicFilter::tab_names_track(tab, track))
        });
        match found {
            Some(tab) => {
                let browser = reported
                    .first()
                    .map(|(browser, _)| (*browser).to_owned())
                    .or_else(|| self.hidden.as_ref().map(|hidden| hidden.browser.clone()))
                    .unwrap_or_default();
                let (title, artist) = reader
                    .youtube_music_now_playing(tab)
                    .map(|(title, byline)| {
                        let artist = byline.split(" \u{2022} ").next().unwrap_or("").to_owned();
                        (title, artist)
                    })
                    .unwrap_or_else(|| (site_trimmed(tab), String::new()));
                self.hidden = Some(HiddenTab {
                    browser,
                    tab: tab.clone(),
                    title,
                    artist,
                    playing: true,
                    heard_at: now,
                });
            }
            None => {
                // Paused (no speaker icon): it stays for the pause hold, as
                // any paused player does, then goes.
                let keep = self
                    .hidden
                    .as_ref()
                    .is_some_and(|hidden| now.duration_since(hidden.heard_at) < self.pause_hold);
                if keep {
                    if let Some(hidden) = &mut self.hidden {
                        hidden.playing = false;
                    }
                } else {
                    self.hidden = None;
                }
            }
        }
        self.hidden_poll_at = self.hidden.as_ref().map(|_| now + HIDDEN_TAB_POLL);
    }
}

/// A music session the filter accepts. One per app: of several sessions an
/// app exposes (browser tabs), a playing one is kept.
struct MusicSession {
    app_id: String,
    playing: bool,
    session: Session,
}

struct Pick {
    /// The single best session, for when no player is in line.
    chosen: Option<Session>,
    music: Vec<MusicSession>,
    all: Vec<Session>,
    /// "app|title" of browser tracks the music filter rejected.
    rejected_browser_tracks: Vec<String>,
}

/// Answers to "does this YouTube page show the Official Artist Channel
/// badge?", by tab title, so each video's page is searched at most twice.
#[derive(Default)]
struct ArtistBadges {
    /// Tab title -> (badge found, times checked).
    seen: std::collections::HashMap<String, (bool, u8)>,
}

impl ArtistBadges {
    /// A "no" is checked once more (the page may still have been loading;
    /// the rejected track's re-check comes back shortly), then kept.
    const CHECKS: u8 = 2;

    fn check(&mut self, reader: &mut tabs::TabReader, tab: &str) -> bool {
        let checks = match self.seen.get(tab) {
            Some(&(found, checks)) if found || checks >= Self::CHECKS => return found,
            Some(&(_, checks)) => checks,
            None => 0,
        };
        // Not readable (e.g. a background tab): answer no without caching.
        let Some(found) = reader.official_artist(tab) else {
            return false;
        };
        if self.seen.len() >= 256 {
            self.seen.clear();
        }
        self.seen.insert(tab.to_owned(), (found, checks + 1));
        found
    }
}

/// For the `media_why` example: one line per media session saying how the
/// music filter judged it. Never called by the app; prints titles, so only
/// for the user's own console.
pub fn explain_sessions(filter: &MusicFilter) -> Vec<String> {
    // SAFETY: balanced with RoUninitialize below on success.
    let initialised = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
    let lines = (|| -> WinResult<Vec<String>> {
        let manager = Manager::RequestAsync()?.join()?;
        let mut reader = tabs::TabReader::new().ok();
        let titles = reader.as_mut().map(tabs::TabReader::titles).unwrap_or_default();
        let sessions = manager.GetSessions()?;
        let mut lines: Vec<String> = titles
            .iter()
            .filter(|tab| tab.to_lowercase().contains("music"))
            .map(|tab| format!("music tab: {tab}"))
            .collect();
        if let Some(reader) = reader.as_mut() {
            for tab in reader.audible_titles() {
                lines.push(format!("audible tab: {tab}"));
            }
            lines.push(format!(
                "youtube music bar: {:?}",
                reader.youtube_music_now_playing("YouTube Music")
            ));
        }
        for index in 0..sessions.Size()? {
            let session = sessions.GetAt(index)?;
            let app_id = session.SourceAppUserModelId()?.to_string_lossy();
            let properties = session.TryGetMediaPropertiesAsync()?.join()?;
            let title = properties.Title()?.to_string_lossy();
            let artist = properties.Artist()?.to_string_lossy();
            let is_browser = is_browser_name(&app_display_name(&app_id));
            let status = session
                .GetPlaybackInfo()
                .and_then(|info| info.PlaybackStatus())
                .map_or_else(|_| "unknown".to_owned(), |status| format!("{status:?}"));
            let facts = SessionFacts {
                app: &app_id,
                is_browser,
                flagged_video: false,
                title: &title,
                artist: &artist,
            };
            let accepted = filter.is_music(facts, &titles);
            let doubtful = filter.doubtful_youtube_tab(facts, &titles);
            let found = match (doubtful, reader.as_mut()) {
                (Some(tab), Some(reader)) => Some(reader.official_artist(tab)),
                _ => None,
            };
            let accepted = accepted || found.flatten() == Some(true);
            let badge = found.map_or_else(|| "not checked".to_owned(), |found| format!("{found:?}"));
            let tab = titles
                .iter()
                .find(|tab| tab.to_lowercase().contains(&title.to_lowercase()))
                .cloned()
                .unwrap_or_else(|| "(no tab names this title)".to_owned());
            lines.push(format!(
                "{app_id}  [{status}]\n  title:  {title}\n  artist: {artist}\n  browser: {is_browser}  tab: {tab}\n  \
                 title heuristic: {}  badge: {badge}  => accepted: {accepted}",
                islet_core::music::looks_like_music(&title, &artist)
            ));
        }
        Ok(lines)
    })()
    .unwrap_or_else(|error| vec![format!("error: {error}")]);
    if initialised {
        // SAFETY: matches the successful RoInitialize above.
        unsafe { RoUninitialize() };
    }
    lines
}

/// A tab title without its site name: "Song - SoundCloud" -> "Song".
fn site_trimmed(tab: &str) -> String {
    tab.rsplit_once(" - ")
        .or_else(|| tab.rsplit_once(" | "))
        .map_or(tab, |(page, _)| page)
        .trim()
        .to_owned()
}

/// Chooses the session to show among those the music filter accepts.
fn pick_session(
    manager: &Manager,
    filter: &MusicFilter,
    last_played: Option<&str>,
    mut tabs: Option<&mut tabs::TabReader>,
    artist_badges: &mut ArtistBadges,
) -> WinResult<Pick> {
    let sessions = manager.GetSessions()?;
    let count = sessions.Size()?;
    let mut all = Vec::with_capacity(count as usize);
    let mut ids = Vec::with_capacity(count as usize);
    let mut playing = Vec::with_capacity(count as usize);
    let mut music = Vec::with_capacity(count as usize);
    let mut rejected_browser_tracks = Vec::new();
    // Read lazily: only when a browser session needs classifying.
    let mut tab_titles: Option<Vec<String>> = None;

    for index in 0..count {
        let session = sessions.GetAt(index)?;
        let app_id = session
            .SourceAppUserModelId()
            .map(|id| id.to_string_lossy())
            .unwrap_or_default();
        let info = session.GetPlaybackInfo().ok();
        playing.push(
            info.as_ref()
                .and_then(|info| info.PlaybackStatus().ok())
                .is_some_and(|status| status == PlaybackStatus::Playing),
        );
        let flagged_video = info
            .as_ref()
            .and_then(|info| info.PlaybackType().ok())
            .and_then(|kind| kind.Value().ok())
            .is_some_and(|kind| kind == MediaPlaybackType::Video);
        let properties: Option<MediaProperties> = session
            .TryGetMediaPropertiesAsync()
            .and_then(|operation| operation.join())
            .ok();
        let text = |read: fn(&MediaProperties) -> WinResult<HSTRING>| {
            properties
                .as_ref()
                .and_then(|properties| read(properties).ok())
                .map(|value| value.to_string_lossy())
                .unwrap_or_default()
        };
        let title = text(MediaProperties::Title);
        let artist = text(MediaProperties::Artist);
        let is_browser = is_browser_name(&app_display_name(&app_id));
        let titles: &[String] = if is_browser && filter.music_only {
            tab_titles.get_or_insert_with(|| {
                tabs.as_deref_mut()
                    .map(tabs::TabReader::titles)
                    .unwrap_or_default()
            })
        } else {
            &[]
        };
        let facts = SessionFacts {
            app: &app_id,
            is_browser,
            flagged_video,
            title: &title,
            artist: &artist,
        };
        let mut is_music = filter.is_music(facts, titles);
        // The title did not look like a song: an artist's own channel still
        // makes it music.
        if !is_music {
            if let (Some(tab), Some(reader)) = (
                filter.doubtful_youtube_tab(facts, titles),
                tabs.as_deref_mut(),
            ) {
                is_music = artist_badges.check(reader, tab);
            }
        }
        if is_browser && !is_music && !title.is_empty() {
            rejected_browser_tracks.push(format!("{app_id}|{title}"));
        }
        music.push(is_music);
        ids.push(app_id);
        all.push(session);
    }

    let current_id: Option<HSTRING> = manager
        .GetCurrentSession()
        .ok()
        .and_then(|session| session.SourceAppUserModelId().ok());
    let current = current_id.and_then(|current| {
        let current = current.to_string_lossy();
        ids.iter().position(|id| *id == current)
    });
    // Only music sessions are candidates; keep their original indices.
    let eligible: Vec<usize> = (0..all.len()).filter(|&index| music[index]).collect();
    let candidates: Vec<SessionCandidate<'_>> = eligible
        .iter()
        .map(|&index| SessionCandidate {
            app_id: &ids[index],
            playing: playing[index],
            last_played: last_played == Some(ids[index].as_str()),
        })
        .collect();
    let current = current.and_then(|current| eligible.iter().position(|&index| index == current));
    let chosen = choose_session(&candidates, current).map(|slot| all[eligible[slot]].clone());
    let mut unique: Vec<MusicSession> = Vec::with_capacity(eligible.len());
    for &index in &eligible {
        match unique
            .iter_mut()
            .find(|session| session.app_id == ids[index])
        {
            Some(existing) if playing[index] && !existing.playing => {
                existing.playing = true;
                existing.session = all[index].clone();
            }
            Some(_) => {}
            None => unique.push(MusicSession {
                app_id: ids[index].clone(),
                playing: playing[index],
                session: all[index].clone(),
            }),
        }
    }
    Ok(Pick {
        chosen,
        music: unique,
        all,
        rejected_browser_tracks,
    })
}

/// The app's own name from the shell's Apps folder (e.g. "Google Chrome"
/// for Chromium's hashed AUMIDs), falling back to a heuristic.
fn app_display_name(app_id: &str) -> String {
    let fallback = source_display_name(app_id);
    if fallback != "Browser" && !app_id.contains('!') {
        // Known exe-style ids already have a good short name.
        return fallback;
    }
    // SAFETY: COM is initialised on this thread; the returned string is
    // freed with CoTaskMemFree exactly once.
    let resolved = unsafe {
        SHCreateItemInKnownFolder::<_, IShellItem>(
            &FOLDERID_AppsFolder,
            KF_FLAG_DEFAULT,
            &HSTRING::from(app_id),
        )
        .and_then(|item| item.GetDisplayName(SIGDN_NORMALDISPLAY))
        .map(|name| {
            let text = name.to_string().unwrap_or_default();
            CoTaskMemFree(Some(name.0.cast()));
            text
        })
    };
    match resolved {
        // "Mozilla Firefox" -> "Firefox"; unknown names pass through.
        Ok(name) if !name.trim().is_empty() && name != app_id => source_display_name(&name),
        _ => fallback,
    }
}

fn read_stream(reference: &IRandomAccessStreamReference) -> WinResult<Vec<u8>> {
    let stream = reference.OpenReadAsync()?.join()?;
    let size = u32::try_from(stream.Size()?)
        .unwrap_or(u32::MAX)
        .min(8 << 20);
    let reader = DataReader::CreateDataReader(&stream)?;
    let loaded = reader.LoadAsync(size)?.join()?;
    let mut bytes = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

/// Decodes any WIC-supported image, centre-crops it square, and scales it to
/// `ART_SIZE` premultiplied BGRA.
fn decode_art(wic: &IWICImagingFactory, bytes: &[u8]) -> WinResult<ArtImage> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    // SAFETY: WIC objects are created and used on this thread only; `bytes`
    // outlives the stream that reads from it.
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder =
            wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height)?;
        let side = width.min(height).max(1);
        let clipper = wic.CreateBitmapClipper()?;
        clipper.Initialize(
            &frame,
            &WICRect {
                X: ((width - side) / 2) as i32,
                Y: ((height - side) / 2) as i32,
                Width: side as i32,
                Height: side as i32,
            },
        )?;
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(
            &clipper,
            ART_SIZE,
            ART_SIZE,
            WICBitmapInterpolationModeHighQualityCubic,
        )?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let mut pixels = vec![0u8; (ART_SIZE * ART_SIZE * 4) as usize];
        converter.CopyPixels(std::ptr::null(), ART_SIZE * 4, &mut pixels)?;
        Ok(ArtImage {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            width: ART_SIZE,
            height: ART_SIZE,
            pixels: pixels.into(),
        })
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / 10_000_000.0
}

fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds.max(0.0) * 10_000_000.0) as i64
}

fn now_filetime() -> i64 {
    // SAFETY: no preconditions.
    let time = unsafe { GetSystemTimePreciseAsFileTime() };
    (i64::from(time.dwHighDateTime) << 32) | i64::from(time.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_music_tab_reads_as_a_player() {
        assert_eq!(site_trimmed("Lofi beat - SoundCloud"), "Lofi beat");
        assert_eq!(site_trimmed("Song | Bandcamp"), "Song");
        assert_eq!(site_trimmed("YouTube Music"), "YouTube Music");
        let hidden = HiddenTab {
            browser: "308046B0AF4A39CB".to_owned(),
            tab: "YouTube Music".to_owned(),
            title: "The Hills".to_owned(),
            artist: "Hozie".to_owned(),
            playing: true,
            heard_at: std::time::Instant::now(),
        };
        let info = hidden.info();
        assert_eq!(info.app_id, "308046B0AF4A39CB#YouTube Music");
        assert_eq!(info.source, "YouTube Music");
        assert!(info.playing && info.art.is_none() && info.duration == 0.0);
    }

    #[test]
    fn a_tab_the_browser_already_reports_is_not_shown_twice() {
        // The tab first looked like a video, so it was found as a hidden
        // tab; then Firefox's own session was accepted as music.
        let hidden = HiddenTab {
            browser: "308046B0AF4A39CB".to_owned(),
            tab: "Secondhand - Don Toliver - (feat. Rema) [Extended + Majestic Outro] | prod. codax - YouTube".to_owned(),
            title: "Secondhand - Don Toliver - (feat. Rema) [Extended + Majestic Outro] | prod. codax".to_owned(),
            artist: String::new(),
            playing: true,
            heard_at: std::time::Instant::now(),
        };
        let firefox = MediaInfo {
            app_id: "308046B0AF4A39CB".to_owned(),
            source: "Firefox".to_owned(),
            title: "Secondhand - Don Toliver - (feat. Rema) [Extended + Majestic Outro] | prod. codax".to_owned(),
            artist: "cactus nev".to_owned(),
            playing: true,
            position: 77.0,
            duration: 384.0,
            rate: 1.0,
            art: None,
            accent: None,
        };
        assert!(hidden.shown_by(std::slice::from_ref(&firefox)));
        // A different song in the browser's session leaves the tab its own.
        let other = MediaInfo { title: "Lucid Dreams".to_owned(), ..firefox };
        assert!(!hidden.shown_by(&[other]));
    }

    #[test]
    fn tick_conversion_round_trips() {
        assert_eq!(ticks_to_seconds(25_000_000), 2.5);
        assert_eq!(seconds_to_ticks(2.5), 25_000_000);
        assert_eq!(seconds_to_ticks(-1.0), 0);
    }

    #[test]
    fn hash_distinguishes_inputs() {
        assert_ne!(fnv1a(b"abc"), fnv1a(b"abd"));
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn refresh_flags_merge() {
        let mut flags = Refresh::default();
        assert!(!flags.any());
        flags.merge(Refresh {
            timeline: true,
            ..Refresh::default()
        });
        assert!(flags.any() && flags.timeline && !flags.properties);
    }
}
