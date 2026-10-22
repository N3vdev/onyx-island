//! Now-playing data from Windows' Global System Media Transport Controls.
//!
//! [`MediaService`] owns one MTA thread. GSMTC events (which arrive on WinRT
//! thread-pool threads) only post a "refresh" flag to that thread; it then
//! re-reads the session and reports a [`MediaInfo`] through the callback. No
//! polling: an idle system with nothing playing costs no wakeups.

#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use islet_core::media::{choose_session, dominant_color, source_display_name, ArtImage};
use windows::core::{Ref, Result as WinResult, HSTRING};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    MediaPropertiesChangedEventArgs, PlaybackInfoChangedEventArgs, SessionsChangedEventArgs,
    TimelinePropertiesChangedEventArgs,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeHighQualityCubic,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

/// Album art edge length in pixels. Drawn at up to 78 dp, so this covers
/// 150% scaling crisply; larger displays get a mild upscale.
pub const ART_SIZE: u32 = 128;
/// GSMTC fires several events per change; coalesce them.
const COALESCE: Duration = Duration::from_millis(40);

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

enum Message {
    Refresh(Refresh),
    Command(MediaCommand),
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
    /// Starts the media thread. `on_update` runs on that thread whenever the
    /// shown track changes; `None` means nothing is playing.
    pub fn start(
        on_update: impl FnMut(Option<MediaInfo>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = unbounded();
        let thread_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("islet-media".to_owned())
            .spawn(move || media_thread(thread_tx, rx, on_update))?;
        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    pub fn send(&self, command: MediaCommand) {
        let _ = self.tx.send(Message::Command(command));
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
    mut on_update: impl FnMut(Option<MediaInfo>),
) {
    // SAFETY: balanced with RoUninitialize below on this same thread.
    if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        tracing::error!(%error, "media thread could not initialise WinRT");
        return;
    }
    if let Err(error) = run(&tx, &rx, &mut on_update) {
        tracing::warn!(%error, "now playing is unavailable");
    }
    // SAFETY: matches the successful RoInitialize above.
    unsafe { RoUninitialize() };
}

fn run(
    tx: &Sender<Message>,
    rx: &Receiver<Message>,
    on_update: &mut impl FnMut(Option<MediaInfo>),
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

    let mut state = State {
        subscription: None,
        cache: TrackCache::default(),
        playback: None,
        timeline: None,
        last_sent: None,
    };
    let mut pending = Refresh::ALL;

    loop {
        if pending.any() {
            // Coalesce bursts, but keep commands responsive.
            let deadline = std::time::Instant::now() + COALESCE;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                match rx.recv_timeout(remaining) {
                    Ok(Message::Refresh(flags)) => pending.merge(flags),
                    Ok(Message::Command(command)) => state.command(command),
                    Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        return cleanup(&manager, sessions_token, current_token);
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            let flags = std::mem::take(&mut pending);
            if let Err(error) = state.refresh(&manager, flags, tx, wic.as_ref()) {
                tracing::debug!(%error, "media refresh failed");
                // The session probably went away; re-resolve next time.
                state.subscription = None;
            }
            let info = state.info();
            if info != state.last_sent {
                state.last_sent.clone_from(&info);
                on_update(info);
            }
            continue;
        }

        match rx.recv() {
            Ok(Message::Refresh(flags)) => pending.merge(flags),
            Ok(Message::Command(command)) => state.command(command),
            Ok(Message::Stop) | Err(_) => return cleanup(&manager, sessions_token, current_token),
        }
    }
}

fn cleanup(manager: &Manager, sessions: i64, current: i64) -> WinResult<()> {
    let _ = manager.RemoveSessionsChanged(sessions);
    let _ = manager.RemoveCurrentSessionChanged(current);
    Ok(())
}

struct State {
    subscription: Option<Subscription>,
    cache: TrackCache,
    playback: Option<Playback>,
    timeline: Option<TimelineSample>,
    last_sent: Option<MediaInfo>,
}

impl State {
    fn session(&self) -> Option<&Session> {
        self.subscription
            .as_ref()
            .map(|subscription| &subscription.session)
    }

    fn refresh(
        &mut self,
        manager: &Manager,
        mut flags: Refresh,
        tx: &Sender<Message>,
        wic: Option<&IWICImagingFactory>,
    ) -> WinResult<()> {
        if flags.sessions || self.subscription.is_none() {
            let chosen = pick_session(manager)?;
            let current_id = self
                .session()
                .and_then(|session| session.SourceAppUserModelId().ok());
            let chosen_id = chosen
                .as_ref()
                .and_then(|session| session.SourceAppUserModelId().ok());
            if chosen_id != current_id || self.subscription.is_none() {
                self.subscription = None;
                self.cache = TrackCache::default();
                self.playback = None;
                self.timeline = None;
                if let Some(session) = chosen {
                    self.subscription = Some(Subscription::new(session, tx)?);
                    flags = Refresh::ALL;
                }
            }
        }
        let Some(session) = self.session().cloned() else {
            return Ok(());
        };

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
        let session = self.session()?;
        if self.cache.title.is_empty() && self.cache.artist.is_empty() {
            return None;
        }
        let app_id = session
            .SourceAppUserModelId()
            .map(|id| id.to_string_lossy())
            .unwrap_or_default();
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
            source: source_display_name(&app_id),
            app_id,
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

    fn command(&mut self, command: MediaCommand) {
        let Some(session) = self.session() else {
            return;
        };
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

fn pick_session(manager: &Manager) -> WinResult<Option<Session>> {
    let sessions = manager.GetSessions()?;
    let count = sessions.Size()?;
    let mut all = Vec::with_capacity(count as usize);
    let mut ids = Vec::with_capacity(count as usize);
    for index in 0..count {
        let session = sessions.GetAt(index)?;
        ids.push(
            session
                .SourceAppUserModelId()
                .map(|id| id.to_string_lossy())
                .unwrap_or_default(),
        );
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
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    Ok(choose_session(&id_refs, current).map(|index| all.swap_remove(index)))
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
