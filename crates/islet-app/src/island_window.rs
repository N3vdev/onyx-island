//! The island window, its UI-thread state machine, and the render thread.

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use islet_audio::{Loopback, VolumeHandle};
use islet_core::config::{IdleGlance, IdleStyle, MonitorChoice};
use islet_core::island::{
    arrange, reduce, AgentCard, AgentState, AlertLevel, ContentSnapshot, DictationCard,
    DictationPhase, Effect, IslandEvent, IslandState, LayoutTarget, MediaCard, NotificationCard,
    TimerKind,
};
use islet_core::media::{
    expanded_action_at, expanded_hover_at, scrub_fraction, volume_fraction, BarLevels, MediaAction,
    MediaHover, Timeline,
};
use islet_core::music::MusicFilter;
use islet_core::widgets::{self, WidgetHover};
use islet_core::{Config, Rect};
use islet_media::{MediaCommand, MediaInfo, MediaService};
use islet_render::{HitRegion, Renderer, RendererConfig};

use crate::dictation::{self, DictationHandle, UiUpdate};
use crate::occlusion;
use islet_core::handoff::{PREVIEW_ATTACH, PREVIEW_DEPARTED, PREVIEW_DETACH};
use windows::core::{s, w, Result as WinResult, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromPoint,
    MonitorFromWindow, ScreenToClient, SetWindowRgn, HMONITOR, HRGN, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, RGN_OR,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VK_CONTROL, VK_DELETE, VK_ESCAPE, VK_F9, VK_LWIN, VK_MENU, VK_RWIN,
};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, SHQueryUserNotificationState, ShellExecuteW, ABM_NEW, ABM_REMOVE,
    ABN_FULLSCREENAPP, APPBARDATA, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DefWindowProcW, EnumWindows, GetCursorPos, GetForegroundWindow,
    GetWindowLongPtrW, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, KillTimer, PostMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA,
    GWL_EXSTYLE, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, MA_ACTIVATE, MA_NOACTIVATE, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE,
    SW_SHOWNORMAL, WINDOWPOS, WM_APP, WM_APPCOMMAND, WM_CAPTURECHANGED, WM_CHAR, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST, WM_SETTINGCHANGE, WM_TIMER, WM_WINDOWPOSCHANGING,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, IsWindow, RemovePropW, SetPropW, GA_ROOT, HWND_NOTOPMOST,
};

/// Room for the expanded player plus the second player's bubble beside it.
pub const WINDOW_LOGICAL_WIDTH: f32 = 720.0;
pub const WINDOW_LOGICAL_HEIGHT: f32 = 200.0;

const TIMER_HOVER_EXPAND: usize = 1;
const TIMER_HOVER_COLLAPSE: usize = 2;
const TIMER_DONE_EXPIRY: usize = 3;
const TIMER_POLL: usize = 4;
const TIMER_DEMO: usize = 5;
const TIMER_MEDIA_PAUSED: usize = 6;
const TIMER_OCCLUSION: usize = 7;
const TIMER_PREVIEW_HANDOFF: usize = 8;
const TIMER_EMPTY_CHECK: usize = 9;
const TIMER_MEDIA_CONDENSE: usize = 10;
const TIMER_STATS: usize = 11;
const TIMER_CLOCK: usize = 12;
const TIMER_RESUME: usize = 13;
/// After this long without a media session, play is sent to the relaunched
/// player's window directly (Spotify reports no session until it plays).
const RESUME_NUDGE_AFTER: std::time::Duration = std::time::Duration::from_secs(4);
/// How long a relaunched player has to show up before resuming is dropped.
const RESUME_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// Full readings while the stats widget is on screen.
const STATS_VISIBLE_MS: u32 = 1_000;
/// CPU only, for the hot-CPU glow, while it is not.
const STATS_BACKGROUND_MS: u32 = 2_000;
/// While an app is behind the island, how often to look again at whether
/// the space under it is empty (tabs open, a title changes). Only runs then.
const EMPTY_CHECK_MS: u32 = 1_500;
/// The ring that must be empty: this much all round the small resting pill,
/// beyond its own edge, in DIPs. Never the opened size.
const EMPTY_MARGIN_DP: f32 = 2.0;
/// The island's own edge (its 1px outline) just outside the pill, skipped:
/// the ring starts beyond it.
const OWN_EDGE_DP: f32 = 1.0;
/// With the hot-CPU halo lit, the island's own glow reaches this far out.
const OWN_GLOW_DP: f32 = 5.0;
/// Window moves arrive in bursts (dragging); check what is behind after this.
const OCCLUSION_DEBOUNCE_MS: u32 = 120;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Shell appbar notifications (only `ABN_FULLSCREENAPP` is used).
const WM_APP_APPBAR: u32 = WM_APP + 20;
/// Posted by the render thread when it parks. `WPARAM` is the generation of
/// the last snapshot it applied.
const WM_APP_SETTLED: u32 = WM_APP + 21;
/// Posted by the media thread when now-playing changed.
const WM_APP_MEDIA: u32 = WM_APP + 22;
/// Posted by the WinEvent hooks when any top-level window changed.
const WM_APP_WINDOWS_CHANGED: u32 = WM_APP + 23;
/// Posted by the dictation controller when it queued island updates.
const WM_APP_DICTATION: u32 = WM_APP + 30;
/// Extra room around a morphing island's window region for spring overshoot.
const REGION_MORPH_MARGIN_DP: f32 = 12.0;

const DEMO_INTERVAL_MS: u32 = 3_000;
/// Hover reconciliation while the pointer is over the pill. Catches the pill
/// shrinking out from under a stationary cursor, which produces no messages.
const POLL_HOVER_MS: u32 = 100;
/// Monitor re-check for the opt-in "follow cursor" mode.
const POLL_FOLLOW_MS: u32 = 500;

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowLayout {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    dpi: u32,
    anchor_top: bool,
    /// Logical height of the window. At the top the window spans the whole
    /// work area so the renderer can spring the pill between both edges.
    logical_height: f32,
}

/// The window region, in physical pixels. `HTTRANSPARENT` only passes
/// clicks to windows on the same thread, so the region is what makes the
/// area around the pill click-through for other apps.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WindowRegion {
    Empty,
    /// While morphing: a box covering every shape the springs pass through.
    Box(Rect),
    /// Once settled: the pill and bubble shapes.
    Exact {
        main: Rect,
        main_corner: f32,
        bubble: Option<(Rect, f32)>,
    },
}

enum RenderCommand {
    Depart,
    Arrive,
    MediaHover(Option<MediaHover>),
    WidgetHover(Option<WidgetHover>),
    Snapshot {
        target: LayoutTarget,
        content: Box<ContentSnapshot>,
        anchor_top: bool,
        generation: usize,
    },
    Resize {
        width: u32,
        height: u32,
        dpi: f32,
        logical_width: f32,
        logical_height: f32,
    },
    Suspend(bool),
    Stop,
}

struct Demo {
    step: usize,
}

pub struct IslandHost {
    preview: Option<HWND>,
    preview_pending: Option<Option<HWND>>,
    appearance: [f32; 4],
    hwnd: HWND,
    shared: Arc<RwLock<Config>>,
    state: IslandState,
    hit: Arc<Mutex<HitRegion>>,
    tx: Sender<RenderCommand>,
    thread: Option<JoinHandle<()>>,
    anchor_top: bool,
    hide_in_fullscreen: bool,
    follow_cursor: bool,
    idle_style: IdleStyle,
    fullscreen_hidden: bool,
    tracking_mouse: bool,
    dpi: u32,
    pixel_width: u32,
    pixel_height: u32,
    logical_height: f32,
    last_layout: Option<WindowLayout>,
    /// Layout at the configured edge. The slim bar may sit elsewhere, but
    /// "is a window behind the island?" is always asked about this spot.
    home_layout: Option<WindowLayout>,
    demo: Option<Demo>,
    /// Debug only: with `--demo`, alternate between the top and bottom edges
    /// (set `ISLET_DEMO_DOCK=1`) so the dock animation can be watched
    /// without opening an app window.
    demo_dock: bool,
    poll_interval: Option<u32>,
    appbar: bool,
    generation: usize,
    /// Logical box the region must keep covering until the render thread
    /// confirms it has settled.
    morph_box: Option<Rect>,
    last_target: Option<LayoutTarget>,
    /// Effective edge of the last snapshot, kept separately because a dock
    /// flips edges before the target itself is recomputed.
    last_target_anchor_top: Option<bool>,
    last_sent_anchor_top: Option<bool>,
    /// The pill is between the top and bottom edges; the window is
    /// click-through until it arrives.
    traveling: bool,
    region: Option<WindowRegion>,
    bars: Arc<BarLevels>,
    media: Option<MediaService>,
    media_tx: Sender<Vec<MediaInfo>>,
    media_rx: Receiver<Vec<MediaInfo>>,
    /// Active players, first-started first.
    last_media: Vec<MediaInfo>,
    /// App id of the player the user last opened from the bubble. It keeps
    /// the pill after the island collapses (the two swap places) until the
    /// user switches back or that player goes away.
    media_focus: Option<String>,
    /// Last "pointer is on the island" value sent to the media service.
    media_held: bool,
    /// The current left-button press began on a settled island, so its
    /// release may act as a click.
    press_armed: bool,
    dictation: Option<DictationHandle>,
    dictation_tx: Sender<UiUpdate>,
    dictation_rx: Receiver<UiUpdate>,
    loopback: Option<Loopback>,
    volume: Option<VolumeHandle>,
    real_bars: bool,
    color_tint: bool,
    music_filter: MusicFilter,
    scrubbing: bool,
    volume_dragging: bool,
    media_hover: Option<MediaHover>,
    widget_hover: Option<WidgetHover>,
    /// A focus lease acquired only by a calculator click or wheel selection.
    calculator_return_focus: Option<HWND>,
    calculator_accepts_focus: bool,
    widget_scroll: widgets::ScrollGate,
    /// The last song played, shown paused when no player is open.
    remembered: Option<crate::last_music::Remembered>,
    /// The player was relaunched to resume the remembered song: press play
    /// once its session shows up.
    resume_pending: Option<(String, Instant)>,
    /// Present while the stats widget is enabled.
    stats_sampler: Option<crate::sysstats::Sampler>,
    stats_interval: Option<u32>,
    tuck_enabled: bool,
    /// Stay at the configured edge while the app behind shows nothing there.
    stay_over_empty: bool,
    /// The previous reading of that space while an app is behind. A move
    /// either way during re-checks takes two agreeing readings in a row, so
    /// a passing tooltip or a flicker never bounces the island.
    empty_reading: Option<bool>,
    window_events: Option<occlusion::WindowEvents>,
    occlusion_pending: bool,
    warned_disconnected: bool,
}

impl IslandHost {
    pub fn new(
        hwnd: HWND,
        shared: Arc<RwLock<Config>>,
        demo: bool,
        demo_state: Option<usize>,
    ) -> WinResult<Box<Self>> {
        let config = read_config(&shared);
        let layout = compute_window_layout(&config, config.general.position.home_is_top())?;
        apply_window_layout(hwnd, layout)?;

        let hit = Arc::new(Mutex::new(HitRegion::default()));
        let (tx, rx) = unbounded();
        let renderer_config = RendererConfig {
            hwnd,
            pixel_width: layout.width as u32,
            pixel_height: layout.height as u32,
            dpi: layout.dpi as f32,
            logical_width: WINDOW_LOGICAL_WIDTH,
            logical_height: layout.logical_height,
            anchor_top: layout.anchor_top,
        };
        let render_hit = Arc::clone(&hit);
        let bars = Arc::new(BarLevels::default());
        let render_bars = Arc::clone(&bars);
        let notify = hwnd.0 as isize;
        let thread = std::thread::Builder::new()
            .name("islet-render".to_owned())
            .spawn(move || render_thread(renderer_config, render_hit, render_bars, rx, notify))?;
        let (media_tx, media_rx) = unbounded();
        let (dictation_tx, dictation_rx) = unbounded();
        let volume = match VolumeHandle::start() {
            Ok(handle) => Some(handle),
            Err(error) => {
                tracing::warn!(%error, "could not start the volume thread");
                None
            }
        };

        let mut host = Box::new(Self {
            preview: None,
            preview_pending: None,
            appearance: [
                config.general.top_margin_percent,
                config.general.bottom_margin_percent,
                config.general.top_expanded_scale,
                config.general.bottom_expanded_scale,
            ],
            hwnd,
            shared,
            state: IslandState {
                calculator: config.widgets.calculator.then(widgets::Calculator::default),
                stats: config
                    .widgets
                    .system_stats
                    .then(widgets::SystemStats::default),
                idle_style: config.general.idle_style,
                expand_on: config.general.expand_on,
                glance: config.general.always_on.then_some(config.general.glance),
                ..IslandState::default()
            },
            hit,
            tx,
            thread: Some(thread),
            anchor_top: layout.anchor_top,
            hide_in_fullscreen: config.general.hide_in_fullscreen,
            follow_cursor: config.general.monitor == MonitorChoice::FollowCursor,
            idle_style: config.general.idle_style,
            fullscreen_hidden: false,
            tracking_mouse: false,
            dpi: layout.dpi,
            pixel_width: layout.width as u32,
            pixel_height: layout.height as u32,
            logical_height: layout.logical_height,
            last_layout: Some(layout),
            home_layout: Some(layout),
            demo: None,
            demo_dock: std::env::var_os("ISLET_DEMO_DOCK").is_some_and(|value| value != "0"),
            poll_interval: None,
            appbar: false,
            generation: 0,
            morph_box: None,
            last_target: None,
            last_target_anchor_top: None,
            last_sent_anchor_top: Some(layout.anchor_top),
            traveling: false,
            region: None,
            bars,
            media: None,
            media_tx,
            media_rx,
            last_media: Vec::new(),
            media_focus: None,
            media_held: false,
            press_armed: false,
            dictation: None,
            dictation_tx,
            dictation_rx,
            loopback: None,
            volume,
            real_bars: config.media.real_bars,
            color_tint: config.media.color_tint,
            music_filter: music_filter(&config),
            scrubbing: false,
            volume_dragging: false,
            media_hover: None,
            widget_hover: None,
            calculator_return_focus: None,
            calculator_accepts_focus: false,
            widget_scroll: widgets::ScrollGate::default(),
            stats_sampler: needs_sampler(&config).then(crate::sysstats::Sampler::new),
            remembered: crate::last_music::load(&crate::paths::Paths::discover().data_dir),
            resume_pending: None,
            stats_interval: None,
            tuck_enabled: config.general.position.docks_over_apps(),
            stay_over_empty: config.general.stay_over_empty_space,
            empty_reading: None,
            window_events: None,
            occlusion_pending: false,
            warned_disconnected: false,
        });

        host.state.media_paused_ms = config.media.pause_hide_seconds.saturating_mul(1_000);
        host.register_appbar();
        host.send_snapshot();
        host.update_poll_timer();
        host.update_stats_timer();
        host.update_clock();
        if config.media.enabled && demo_state.is_none() && !demo {
            host.start_media();
        }
        if demo_state.is_none() && !demo {
            host.send_remembered();
        }
        if let Some(step) = demo_state {
            host.demo = Some(Demo { step });
            host.demo_advance();
        } else if demo {
            host.demo = Some(Demo { step: 0 });
            unsafe {
                SetTimer(Some(hwnd), TIMER_DEMO, DEMO_INTERVAL_MS, None);
            }
            host.demo_advance();
        }
        if !demo && demo_state.is_none() {
            host.start_dictation(&config);
        }
        // After the demo is set up: the demo never tucks.
        host.update_window_events();
        Ok(host)
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn shutdown(&mut self) {
        self.release_calculator_focus();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_HOVER_EXPAND);
            let _ = KillTimer(Some(self.hwnd), TIMER_HOVER_COLLAPSE);
            let _ = KillTimer(Some(self.hwnd), TIMER_DONE_EXPIRY);
            let _ = KillTimer(Some(self.hwnd), TIMER_POLL);
            let _ = KillTimer(Some(self.hwnd), TIMER_DEMO);
            let _ = KillTimer(Some(self.hwnd), TIMER_MEDIA_PAUSED);
            let _ = KillTimer(Some(self.hwnd), TIMER_OCCLUSION);
            let _ = KillTimer(Some(self.hwnd), TIMER_EMPTY_CHECK);
            let _ = KillTimer(Some(self.hwnd), TIMER_MEDIA_CONDENSE);
            let _ = KillTimer(Some(self.hwnd), TIMER_STATS);
            let _ = KillTimer(Some(self.hwnd), TIMER_CLOCK);
        }
        self.stats_sampler = None;
        self.window_events = None;
        self.dictation = None;
        self.loopback = None;
        self.media = None;
        self.poll_interval = None;
        self.unregister_appbar();
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(RenderCommand::Stop);
            let _ = thread.join();
        }
        if let Ok(mut hit) = self.hit.lock() {
            *hit = HitRegion::default();
        }
    }

    pub fn apply_config(&mut self) {
        let config = read_config(&self.shared);
        self.appearance = [
            config.general.top_margin_percent,
            config.general.bottom_margin_percent,
            config.general.top_expanded_scale,
            config.general.bottom_expanded_scale,
        ];
        if let Some(dictation) = &self.dictation {
            dictation.configure(&config.dictation);
        }
        self.anchor_top = config.general.position.home_is_top();
        self.hide_in_fullscreen = config.general.hide_in_fullscreen;
        self.follow_cursor = config.general.monitor == MonitorChoice::FollowCursor;
        self.idle_style = config.general.idle_style;
        self.dispatch(IslandEvent::SetCalculatorEnabled(config.widgets.calculator));
        if needs_sampler(&config) != self.stats_sampler.is_some() {
            self.stats_sampler = needs_sampler(&config).then(crate::sysstats::Sampler::new);
        }
        self.dispatch(IslandEvent::SetStatsEnabled(config.widgets.system_stats));
        self.dispatch(IslandEvent::SetGlance(
            config.general.always_on.then_some(config.general.glance),
        ));
        self.update_clock();
        self.dispatch(IslandEvent::SetIdleStyle(self.idle_style));
        self.dispatch(IslandEvent::SetExpandTrigger(config.general.expand_on));
        self.dispatch(IslandEvent::SetMediaPauseDelay(
            config.media.pause_hide_seconds.saturating_mul(1_000),
        ));
        if self.demo.is_none() {
            match (config.media.enabled, self.media.is_some()) {
                (true, false) => self.start_media(),
                (false, true) => {
                    self.media = None;
                    self.last_media.clear();
                    self.media_focus = None;
                    self.dispatch_all([
                        IslandEvent::SetOtherMedia(None),
                        IslandEvent::SetMedia(None),
                    ]);
                }
                _ => {}
            }
        }
        if let Some(service) = &self.media {
            service.set_pause_hold(media_pause_hold(&config));
        }
        let filter = music_filter(&config);
        if filter != self.music_filter {
            if let Some(service) = &self.media {
                service.set_filter(filter.clone());
            }
            self.music_filter = filter;
        }
        self.tuck_enabled = config.general.position.docks_over_apps();
        self.stay_over_empty = config.general.stay_over_empty_space;
        if !self.tuck_enabled {
            // Drop the hooks before the window starts moving so the last
            // occlusion check cannot re-tuck it mid-reposition.
            self.window_events = None;
        }
        if let Err(error) = self.reposition() {
            tracing::warn!(%error, "could not reposition island window");
        }
        self.update_window_events();
        let tint_changed = self.color_tint != config.media.color_tint;
        self.real_bars = config.media.real_bars;
        self.color_tint = config.media.color_tint;
        if tint_changed {
            self.apply_media();
        }
        if self.hide_in_fullscreen && self.demo.is_none() {
            let hide = self.should_hide_for_fullscreen();
            self.set_fullscreen_hidden(hide);
        } else if self.fullscreen_hidden {
            self.set_fullscreen_hidden(false);
        }
        self.update_poll_timer();
        self.send_snapshot();
    }

    pub fn handle_message(
        &mut self,
        _hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match message {
            windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU => {
                unsafe {
                    if let Ok(tray) = windows::Win32::UI::WindowsAndMessaging::FindWindowW(
                        w!("IsletTrayWindow"),
                        None,
                    ) {
                        let _ = PostMessageW(
                            Some(tray),
                            windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU,
                            wparam,
                            lparam,
                        );
                    }
                }
                Some(LRESULT(0))
            }
            PREVIEW_ATTACH => {
                let anchor = HWND(wparam.0 as *mut _);
                if unsafe { IsWindow(Some(anchor)) }.as_bool() {
                    if self.preview == Some(anchor) {
                        self.refresh_preview();
                        self.send_snapshot();
                    } else if self.preview_pending != Some(Some(anchor)) {
                        unsafe {
                            let _ = SetPropW(
                                anchor,
                                w!("IsletPreviewAttached"),
                                Some(HANDLE(std::ptr::without_provenance_mut(1))),
                            );
                        }
                        self.preview_pending = Some(Some(anchor));
                        self.begin_departure();
                    }
                }
                Some(LRESULT(0))
            }
            PREVIEW_DETACH => {
                if self.preview.is_some_and(|h| h.0 as usize == wparam.0)
                    || self.preview_pending == Some(Some(HWND(wparam.0 as *mut _)))
                {
                    self.preview_pending = Some(None);
                    self.begin_departure();
                }
                Some(LRESULT(0))
            }
            PREVIEW_DEPARTED => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_PREVIEW_HANDOFF);
                }
                if let Some(next) = self.preview_pending.take() {
                    if let Some(old) = self.preview {
                        unsafe {
                            let _ = RemovePropW(old, w!("IsletPreviewAttached"));
                        }
                    }
                    self.preview = next.filter(|h| unsafe { IsWindow(Some(*h)) }.as_bool());
                    if self.preview.is_none() {
                        self.set_fullscreen_hidden(false);
                    }
                    self.last_layout = None;
                    self.last_target = None;
                    self.last_sent_anchor_top = None;
                    self.last_target_anchor_top = None;
                    self.morph_box = None;
                    self.state.tucked = false;
                    let _ = self.reposition_layout();
                    self.send_snapshot();
                    let _ = self.tx.send(RenderCommand::Arrive);
                    self.set_click_through(false);
                    self.update_window_events();
                    self.update_poll_timer();
                    if let Some(anchor) = self.preview {
                        unsafe {
                            let _ = SetPropW(
                                anchor,
                                w!("IsletPreviewAttached"),
                                Some(HANDLE(std::ptr::without_provenance_mut(2))),
                            );
                        }
                    }
                }
                Some(LRESULT(0))
            }
            WM_NCHITTEST => Some(self.hit_test(lparam)),
            WM_MOUSEMOVE if self.scrubbing => {
                if let Some((x, y, width)) = self.media_point(lparam) {
                    self.set_scrub(scrub_fraction(x, y, width, true));
                }
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE if self.volume_dragging => {
                if let Some((x, y, width)) = self.media_point(lparam) {
                    if let Some(fraction) = volume_fraction(x, y, width, true) {
                        self.set_volume(fraction);
                    }
                }
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                self.track_mouse();
                self.dispatch_hover(self.cursor_inside());
                self.update_media_hover();
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let mut point = point_from_lparam(lparam);
                if self.state.widget_navigation_available()
                    && self.shape_settled()
                    && !self.scrubbing
                    && !self.volume_dragging
                    && unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool()
                    && self.widget_point(point).is_some()
                {
                    let delta = ((wparam.0 >> 16) as u16 as i16) as i32;
                    if self.widget_scroll.push(delta, clock_now()) {
                        self.set_widget_hover(None);
                        self.set_media_hover(None);
                        self.dispatch(IslandEvent::CycleWidget);
                        self.focus_calculator();
                    }
                }
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.tracking_mouse = false;
                if !self.scrubbing && !self.volume_dragging {
                    self.set_media_hover(None);
                    self.set_widget_hover(None);
                    self.dispatch_hover(false);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                // While the island is still growing or shrinking, what is under
                // the pointer on screen is not the layout the controls use.
                // Ignore the whole press rather than guess.
                self.press_armed = self.shape_settled();
                if !self.press_armed {
                    return Some(LRESULT(0));
                }
                if let Some((x, y, width)) = self.media_point(lparam) {
                    let seekable = self.media_duration().is_some();
                    if let Some(fraction) = scrub_fraction(x, y, width, false).filter(|_| seekable)
                    {
                        self.scrubbing = true;
                        self.set_media_hover(Some(MediaHover::Seek));
                        unsafe {
                            SetCapture(self.hwnd);
                        }
                        self.set_scrub(Some(fraction));
                    } else if let Some(fraction) = self
                        .state
                        .media
                        .as_ref()
                        .and_then(|media| media.volume)
                        .and_then(|_| volume_fraction(x, y, width, false))
                    {
                        self.volume_dragging = true;
                        self.set_media_hover(Some(MediaHover::Volume));
                        unsafe {
                            SetCapture(self.hwnd);
                        }
                        self.set_volume(fraction);
                    }
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let armed = std::mem::take(&mut self.press_armed);
                if self.scrubbing {
                    self.finish_scrub(true);
                } else if self.volume_dragging {
                    self.finish_volume();
                } else if !armed || !self.shape_settled() {
                    // Pressed or released mid-animation: not a click.
                } else if self.handle_widget_click(lparam) {
                    // Widget controls never toggle or operate the music player.
                } else if self.media_bubble_hit(lparam) {
                    self.switch_player();

                } else if let Some(action) = self
                    .media_point(lparam)
                    .and_then(|(x, y, width)| expanded_action_at(x, y, width))
                {
                    self.media_action(action);
                } else {
                    self.dispatch(IslandEvent::Click);
                }
                self.focus_calculator();
                Some(LRESULT(0))
            }
            WM_CHAR if self.calculator_keyboard_active() => {
                if !calculator_shortcut_held() {
                    if let Some(key) = char::from_u32(wparam.0 as u32)
                        .and_then(widgets::CalculatorKey::from_character)
                    {
                        // Coalesced Windows repeats become one render snapshot.
                        let repeats = (lparam.0 as u16).clamp(1, 64) as usize;
                        self.dispatch_all((0..repeats).map(|_| IslandEvent::CalculatorKey(key)));
                    }
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN if self.calculator_keyboard_active() && !calculator_shortcut_held() => {
                let key = match wparam.0 as u16 {
                    value if value == VK_DELETE.0 || value == VK_ESCAPE.0 => {
                        Some(widgets::CalculatorKey::Clear)
                    }
                    value if value == VK_F9.0 => Some(widgets::CalculatorKey::Negate),
                    _ => None,
                };
                if let Some(key) = key {
                    self.dispatch(IslandEvent::CalculatorKey(key));
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            WM_KILLFOCUS => {
                // Another app was deliberately selected: never reclaim focus.
                self.calculator_return_focus = None;
                None
            }
            WM_CAPTURECHANGED => {
                if self.scrubbing {
                    self.finish_scrub(false);
                } else if self.volume_dragging {
                    self.finish_volume();
                }
                Some(LRESULT(0))
            }
            WM_APP_WINDOWS_CHANGED => {
                if self.preview.is_some() {
                    self.refresh_preview();
                    return Some(LRESULT(0));
                }
                if !self.occlusion_pending {
                    self.occlusion_pending = true;
                    unsafe {
                        SetTimer(
                            Some(self.hwnd),
                            TIMER_OCCLUSION,
                            OCCLUSION_DEBOUNCE_MS,
                            None,
                        );
                    }
                }
                Some(LRESULT(0))
            }
            WM_APP_DICTATION => {
                while let Ok(update) = self.dictation_rx.try_recv() {
                    match update {
                        UiUpdate::PauseMedia => islet_media::pause_playing_media(),
                        UiUpdate::Card(card) => self.dispatch(IslandEvent::SetDictation(card)),
                        UiUpdate::Notice(notice) => {
                            self.dispatch(IslandEvent::SetDone(Some(notice)));
                        }
                    }
                }
                Some(LRESULT(0))
            }
            WM_APP_MEDIA => {
                let mut latest = None;
                while let Ok(info) = self.media_rx.try_recv() {
                    latest = Some(info);
                }
                if let Some(info) = latest {
                    self.last_media = info;
                    self.finish_resume();
                    // The opened player went away: the pill goes back to the
                    // first-started one, and a later return starts fresh.
                    if self.media_focus.as_ref().is_some_and(|focus| {
                        !self.last_media.iter().any(|info| &info.app_id == focus)
                    }) {
                        self.media_focus = None;
                    }
                    self.apply_media();
                }
                Some(LRESULT(0))
            }
            WM_MOUSEACTIVATE => {
                if self.calculator_keyboard_available() {
                    let previous = unsafe { GetForegroundWindow() };
                    if previous != self.hwnd && unsafe { IsWindow(Some(previous)) }.as_bool() {
                        self.calculator_return_focus = Some(previous);
                    }
                    Some(LRESULT(MA_ACTIVATE as isize))
                } else {
                    Some(LRESULT(MA_NOACTIVATE as isize))
                }
            }
            WM_TIMER => {
                if wparam.0 == TIMER_PREVIEW_HANDOFF {
                    return self.handle_message(self.hwnd, PREVIEW_DEPARTED, WPARAM(0), LPARAM(0));
                }
                self.on_timer(wparam.0);
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_WINDOWPOSCHANGING => {
                if self.preview.is_some() {
                    return None;
                }
                let position = lparam.0 as *mut WINDOWPOS;
                if !position.is_null() {
                    let position = unsafe { &mut *position };
                    if position.flags.0 & SWP_NOZORDER.0 == 0
                        && position.hwndInsertAfter != HWND_TOPMOST
                    {
                        position.hwndInsertAfter = HWND_TOPMOST;
                    }
                }
                Some(LRESULT(0))
            }
            WM_APP_SETTLED => {
                if wparam.0 == self.generation {
                    self.morph_box = None;
                    if self.traveling {
                        self.traveling = false;
                        self.set_click_through(false);
                    }
                    self.update_calculator_activation();
                    self.update_region();
                }
                Some(LRESULT(0))
            }
            WM_APP_APPBAR => {
                if wparam.0 as u32 == ABN_FULLSCREENAPP {
                    self.on_fullscreen_notification(lparam.0 != 0);
                }
                Some(LRESULT(0))
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                if let Err(error) = self.reposition() {
                    tracing::warn!(%error, "could not reposition island window");
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn dispatch(&mut self, event: IslandEvent) {
        self.dispatch_all([event]);
    }

    /// Applies several events as one change, so the renderer gets a single
    /// snapshot (two players swapping places is one move, not two).
    fn dispatch_all(&mut self, events: impl IntoIterator<Item = IslandEvent>) {
        let mut changed = false;
        for event in events {
            changed |= self.reduce_one(event);
        }
        if changed {
            self.send_snapshot();
        }
        self.update_poll_timer();
        self.update_loopback();
        self.update_media_held();
        self.update_stats_timer();
    }

    /// Readings run only while the stats widget is enabled: every second
    /// while it is on screen, otherwise every two seconds and CPU only.
    fn update_stats_timer(&mut self) {
        let interval = self.stats_sampler.is_some().then(|| {
            if self.state.stats_visible() {
                STATS_VISIBLE_MS
            } else {
                STATS_BACKGROUND_MS
            }
        });
        if interval == self.stats_interval {
            return;
        }
        let became_visible = interval == Some(STATS_VISIBLE_MS);
        self.stats_interval = interval;
        unsafe {
            match interval {
                Some(ms) => {
                    SetTimer(Some(self.hwnd), TIMER_STATS, ms, None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_STATS);
                }
            }
        }
        if became_visible {
            // Fill the card now rather than a second after it opens.
            self.sample_stats();
        }
    }

    fn sample_stats(&mut self) {
        let visible = self.state.stats_visible();
        let Some(sampler) = &mut self.stats_sampler else {
            return;
        };
        if visible {
            let reading = sampler.full();
            let hot = sampler.hot();
            self.dispatch_all([IslandEvent::SetStats(reading), IslandEvent::SetCpuHot(hot)]);
        } else if self.state.glance == Some(IdleGlance::Stats) {
            let (cpu, memory) = sampler.light();
            let hot = sampler.hot();
            self.dispatch_all([
                IslandEvent::SetGlanceStats { cpu, memory },
                IslandEvent::SetCpuHot(hot),
            ]);
        } else {
            sampler.cpu();
            let hot = sampler.hot();
            self.dispatch(IslandEvent::SetCpuHot(hot));
        }
    }

    /// Keeps the always-on clock current: one wakeup a minute, on the
    /// minute, and none when the clock is not in use.
    fn update_clock(&mut self) {
        let wanted = matches!(
            self.state.glance,
            Some(IdleGlance::Clock)
        );
        unsafe {
            if wanted {
                SetTimer(Some(self.hwnd), TIMER_CLOCK, crate::clock::until_next_minute(), None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_CLOCK);
                return;
            }
        }
        let (time, long_date) = crate::clock::now();
        self.dispatch(IslandEvent::SetGlanceTime { time, long_date });
    }

    /// Runs one event through the reducer; true if the island changed.
    fn reduce_one(&mut self, event: IslandEvent) -> bool {
        let mut changed = false;
        for effect in reduce(&mut self.state, event, clock_now()) {
            match effect {
                Effect::ArmTimer { kind, after_ms } => self.arm_timer(kind, after_ms),
                Effect::CancelTimer(kind) => self.cancel_timer(kind),
                Effect::LayoutChanged | Effect::ContentChanged => changed = true,
            }
        }
        changed
    }

    /// Paused players keep their place while the pointer is on the island.
    fn update_media_held(&mut self) {
        let held = self.state.pointer_inside;
        if held != self.media_held {
            self.media_held = held;
            if let Some(service) = &self.media {
                service.set_held(held);
            }
        }
    }

    /// Hooks window events only while slim mode is enabled.
    fn update_window_events(&mut self) {
        let wanted = self.preview.is_some() || (self.tuck_enabled && self.demo.is_none());
        match (wanted, self.window_events.is_some()) {
            (true, false) => {
                self.window_events = Some(occlusion::WindowEvents::install(
                    self.hwnd,
                    WM_APP_WINDOWS_CHANGED,
                ));
            }
            (false, true) => self.window_events = None,
            _ => {}
        }
        self.update_tucked();
    }

    /// Slims the island while an app window overlaps where it normally sits.
    /// The window already spans the work area, so the renderer simply springs
    /// the same pill to the other edge.
    fn update_tucked(&mut self) {
        if self.preview.is_some() || self.preview_pending.is_some() {
            return;
        }
        // The demo drives docking itself; real occlusion is not monitored.
        if self.demo.is_some() {
            return;
        }
        let behind = self.window_events.is_some() && self.window_behind_island();
        let tucked = behind && !self.home_space_is_empty();
        // Content under the island can change without any window event (a
        // new tab, a title), so look again now and then, but only while an
        // app is actually behind it.
        let recheck = behind && self.stay_over_empty;
        unsafe {
            if recheck {
                SetTimer(Some(self.hwnd), TIMER_EMPTY_CHECK, EMPTY_CHECK_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_EMPTY_CHECK);
            }
        }
        if !behind {
            self.empty_reading = None;
        }
        if tucked == self.state.tucked {
            return;
        }
        self.dispatch(IslandEvent::SetTucked(tucked));
    }

    /// The app behind the island shows one flat colour where the island sits
    /// (and a margin around it), so the island can stay there.
    fn home_space_is_empty(&mut self) -> bool {
        if !self.stay_over_empty {
            return false;
        }
        // In use, or still moving (its own shape would be in the sample):
        // keep the verdict.
        if self.state.expanded
            || self.state.pointer_inside
            || self.traveling
            || self.morph_box.is_some()
        {
            return !self.state.tucked;
        }
        let Some(home) = self.home_layout else {
            return false;
        };
        let scale = home.dpi as f32 / 96.0;
        let to_screen = |rect: Rect| {
            Rect::new(
                home.x as f32 + rect.x * scale,
                home.y as f32 + rect.y * scale,
                rect.width * scale,
                rect.height * scale,
            )
        };
        // The small pill where it rests, its own edge (or glow), and a 2px
        // ring beyond that: only the ring is judged while the island sits
        // there; from the other edge, the whole spot is.
        let footprint = self.home_footprint(home);
        let own = if self.state.cpu_hot {
            OWN_GLOW_DP
        } else {
            OWN_EDGE_DP
        };
        let zone = to_screen(footprint.inflate(own + EMPTY_MARGIN_DP));
        let exclude = (!self.state.tucked).then(|| to_screen(footprint.inflate(own)));
        let empty = occlusion::zone_is_blank(zone, exclude).unwrap_or(false);
        let previous = self.empty_reading.replace(empty);
        match previous {
            // A changed reading must repeat before the island moves.
            Some(previous) if previous != empty => !self.state.tucked,
            _ => empty,
        }
    }

    /// Where the resting pill sits at the configured edge (logical pixels),
    /// at the size it has now (shrunk while music has played a while).
    fn home_footprint(&self, home: WindowLayout) -> Rect {
        let margin = if home.anchor_top {
            self.appearance[0]
        } else {
            self.appearance[1]
        };
        arrange(
            self.state.resting_size(),
            None,
            0.0,
            islet_core::island::EDGE_GAP_DP + home.logical_height * margin / 100.0,
            WINDOW_LOGICAL_WIDTH,
            home.logical_height,
            home.anchor_top,
        )
        .main
    }

    fn window_behind_island(&self) -> bool {
        let Some(home) = self.home_layout else {
            return false;
        };
        // The island's own footprint at the configured edge, in screen
        // pixels: exactly where the resting pill is drawn, nothing more. A
        // window may be anywhere else on the screen without disturbing it.
        let scale = home.dpi as f32 / 96.0;
        let compact = self.home_footprint(home);
        let zone = Rect::new(
            home.x as f32 + compact.x * scale,
            home.y as f32 + compact.y * scale,
            compact.width * scale,
            compact.height * scale,
        );
        occlusion::window_behind(zone)
    }

    fn start_dictation(&mut self, config: &Config) {
        let hwnd = self.hwnd.0 as isize;
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            // SAFETY: a non-blocking post to the island window.
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    WM_APP_DICTATION,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
        let models = crate::paths::Paths::discover().model_dir;
        match dictation::spawn(&config.dictation, models, self.dictation_tx.clone(), notify) {
            Ok(handle) => self.dictation = Some(handle),
            Err(error) => tracing::error!(%error, "could not start dictation"),
        }
    }

    /// Tray "Pause dictation": hotkeys pass through while paused.
    pub fn set_dictation_paused(&mut self, paused: bool) {
        if let Some(dictation) = &self.dictation {
            dictation.set_paused(paused);
        }
    }

    fn start_media(&mut self) {
        let tx = self.media_tx.clone();
        let hwnd = self.hwnd.0 as isize;
        let service = MediaService::start(self.music_filter.clone(), move |info| {
            if tx.send(info).is_ok() {
                // SAFETY: non-blocking post to the island window.
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(hwnd as *mut _)),
                        WM_APP_MEDIA,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        });
        match service {
            Ok(service) => {
                service.set_pause_hold(media_pause_hold(&read_config(&self.shared)));
                self.media_held = false;
                self.media = Some(service);
            }
            Err(error) => tracing::warn!(%error, "could not start the media thread"),
        }
    }

    /// Converts the latest media report into the island's cards.
    fn apply_media(&mut self) {
        self.remember_playing();
        let events = self.media_events();
        self.dispatch_all(events);
    }

    /// Keeps the song now playing on disk, once per track and cover.
    fn remember_playing(&mut self) {
        let Some(info) = self
            .focused_media()
            .filter(|info| {
                info.playing
                    && !islet_core::music::is_chat_app(&format!("{} {}", info.app_id, info.source))
            })
        else {
            return;
        };
        let card = MediaCard {
            title: info.title.clone(),
            artist: info.artist.clone(),
            source: info.source.clone(),
            playing: true,
            timeline: None,
            art: info.art.clone(),
            accent: info.accent,
            scrub: None,
            volume: None,
        };
        let next = crate::last_music::Remembered::from_live(&info.app_id, &card);
        if self.remembered.as_ref().is_some_and(|known| known.same_as(&next)) {
            return;
        }
        crate::last_music::save(&crate::paths::Paths::discover().data_dir, &next);
        self.remembered = Some(next);
        self.send_remembered();
    }

    /// Hands the remembered song to the island for its opened view.
    fn send_remembered(&mut self) {
        let card = self.remembered.as_ref().map(|remembered| MediaCard {
            accent: remembered.card.accent.filter(|_| self.color_tint),
            ..remembered.card.clone()
        });
        self.dispatch(IslandEvent::SetRemembered(card));
    }

    /// Presses play on a player relaunched for the remembered song: on its
    /// media session once that appears, or, if the app reports none (Spotify
    /// until something plays), on its own window. Only that app is touched.
    fn finish_resume(&mut self) {
        let Some((app_id, asked)) = self.resume_pending.clone() else {
            return;
        };
        let done = if asked.elapsed() > RESUME_WAIT {
            true
        } else if let Some(info) = self.last_media.iter().find(|info| info.app_id == app_id) {
            if !info.playing {
                if let Some(service) = &self.media {
                    service.send(&app_id, MediaCommand::TogglePlayPause);
                }
            }
            true
        } else if asked.elapsed() >= RESUME_NUDGE_AFTER {
            let source = self
                .remembered
                .as_ref()
                .map(|remembered| remembered.card.source.clone())
                .unwrap_or_default();
            match find_app_window(&app_id, &source) {
                Some(window) => {
                    const APPCOMMAND_MEDIA_PLAY_PAUSE: isize = 14;
                    unsafe {
                        let _ = PostMessageW(
                            Some(window),
                            WM_APPCOMMAND,
                            WPARAM(self.hwnd.0 as usize),
                            LPARAM(APPCOMMAND_MEDIA_PLAY_PAUSE << 16),
                        );
                    }
                    true
                }
                None => false,
            }
        } else {
            false
        };
        if done {
            self.resume_pending = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_RESUME);
            }
        }
    }

    /// The player in the main pill: the one the user switched to, else the
    /// first-started one.
    fn focused_media(&self) -> Option<&MediaInfo> {
        self.media_focus
            .as_ref()
            .and_then(|focus| self.last_media.iter().find(|info| &info.app_id == focus))
            .or_else(|| self.last_media.first())
    }

    /// Main-pill and bubble cards for the current players.
    fn media_events(&self) -> Vec<IslandEvent> {
        if self.demo.is_some() {
            return Vec::new();
        }
        let main = self.focused_media();
        let other = self
            .last_media
            .iter()
            .find(|info| Some(info.app_id.as_str()) != main.map(|main| main.app_id.as_str()));
        let scrub = self
            .state
            .media
            .as_ref()
            .and_then(|media| media.scrub)
            .filter(|_| self.scrubbing);
        let volume = self.volume.as_ref().and_then(VolumeHandle::read);
        let card = |info: &MediaInfo| MediaCard {
            title: info.title.clone(),
            artist: info.artist.clone(),
            source: info.source.clone(),
            playing: info.playing,
            timeline: (info.duration > 0.0).then(|| Timeline {
                position: info.position,
                duration: info.duration,
                sampled_at: clock_now(),
                rate: info.rate,
            }),
            art: info.art.clone(),
            accent: info.accent.filter(|_| self.color_tint),
            scrub: None,
            volume: None,
        };
        vec![
            IslandEvent::SetOtherMedia(other.map(card)),
            IslandEvent::SetMedia(main.map(|info| MediaCard {
                scrub,
                volume,
                ..card(info)
            })),
        ]
    }

    /// The island has finished moving: the render thread confirmed the last
    /// shape change settled, so what is on screen matches the hit zones.
    fn shape_settled(&self) -> bool {
        self.morph_box.is_none() && !self.traveling
    }

    /// True if a click at `lparam` landed on a second player's bubble.
    fn media_bubble_hit(&self, lparam: LPARAM) -> bool {
        let point = point_from_lparam(lparam);
        self.media_bubble_at(point.x, point.y)
    }

    /// True if the client pixel is on a second player's bubble.
    fn media_bubble_at(&self, x: i32, y: i32) -> bool {
        if !matches!(
            self.state.content().secondary,
            Some(islet_core::ContentCard::Media(_))
        ) {
            return false;
        }
        let Some(bubble) = self
            .target_frame(
                self.customized_layout(self.state.layout(), self.effective_anchor_top()),
                self.effective_anchor_top(),
            )
            .bubble
        else {
            return false;
        };
        let scale = self.dpi as f32 / 96.0;
        bubble.contains(x as f32 / scale, y as f32 / scale)
    }

    fn cursor_on_media_bubble(&self) -> bool {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }.is_ok()
            && unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool()
            && self.media_bubble_at(point.x, point.y)
    }

    /// Hover in or out of the island. The second player's bubble is a
    /// button: resting on it keeps the island as it is instead of expanding.
    fn dispatch_hover(&mut self, inside: bool) {
        if inside {
            let on_switcher = self.cursor_on_media_bubble();
            self.dispatch_all([
                IslandEvent::HoverSwitcher(on_switcher),
                IslandEvent::HoverEnter,
            ]);
        } else {
            self.release_calculator_focus();
            self.dispatch_all([IslandEvent::HoverLeave, IslandEvent::HoverSwitcher(false)]);
        }
    }

    /// Opens the bubble's player in the expanded island; while expanded,
    /// the bubble then holds the other one, so clicking it swaps back.
    fn switch_player(&mut self) {
        if self.demo.is_some() {
            std::mem::swap(&mut self.state.media, &mut self.state.other_media);
            self.state.expanded = true;
            self.send_snapshot();
            return;
        }
        let shown = self.focused_media().map(|info| info.app_id.clone());
        let Some(next) = self
            .last_media
            .iter()
            .find(|info| Some(&info.app_id) != shown.as_ref())
            .map(|info| info.app_id.clone())
        else {
            return;
        };
        self.media_focus = Some(next);
        let mut events = self.media_events();
        events.push(IslandEvent::SetExpanded(true));
        self.dispatch_all(events);
    }

    /// Runs the loopback visualizer only while playing media is on screen.
    fn update_loopback(&mut self) {
        let wanted = self.real_bars
            && self.media.is_some()
            && !self.fullscreen_hidden
            && self.state.dictation.is_none()
            && !matches!(
                self.state.primary_card(),
                Some(islet_core::ContentCard::Calculator(_))
            )
            && self.state.media.as_ref().is_some_and(|media| media.playing);
        match (wanted, self.loopback.is_some()) {
            (true, false) => match Loopback::start(Arc::clone(&self.bars), clock_now_ms) {
                Ok(loopback) => self.loopback = Some(loopback),
                Err(error) => {
                    tracing::warn!(%error, "could not start the visualizer");
                    self.real_bars = false;
                }
            },
            (false, true) => self.loopback = None,
            _ => {}
        }
    }

    /// Local logical coordinates and pill width, if `lparam` (client pixels)
    /// is inside the expanded media pill.
    fn media_point(&self, lparam: LPARAM) -> Option<(f32, f32, f32)> {
        let layout = self.state.layout();
        let showing_media = self.state.expanded
            && layout.main == islet_core::island::MEDIA
            && matches!(
                self.state.primary_card(),
                Some(islet_core::ContentCard::Media(_))
            );
        if !showing_media {
            return None;
        }
        let scale = self.dpi as f32 / 96.0;
        // Client-area mouse messages carry client coordinates.
        let point = point_from_lparam(lparam);
        let layout = self.customized_layout(layout, self.effective_anchor_top());
        let main = self.target_frame(layout, self.effective_anchor_top()).main;
        Some((
            (point.x as f32 / scale - main.x) / layout.content_scale,
            (point.y as f32 / scale - main.y) / layout.content_scale,
            main.width / layout.content_scale,
        ))
    }

    fn widget_point(&self, point: POINT) -> Option<(f32, f32)> {
        if !self.state.expanded || self.traveling || self.fullscreen_hidden {
            return None;
        }
        let layout = self.customized_layout(self.state.layout(), self.effective_anchor_top());
        let main = self.target_frame(layout, self.effective_anchor_top()).main;
        let dpi = self.dpi as f32 / 96.0;
        let (x, y) = (point.x as f32 / dpi, point.y as f32 / dpi);
        main.contains(x, y).then_some((
            (x - main.x) / layout.content_scale,
            (y - main.y) / layout.content_scale,
        ))
    }

    fn widget_hover_at(&self, point: POINT) -> Option<WidgetHover> {
        let (x, y) = self.widget_point(point)?;
        let calculator = matches!(
            self.state.primary_card(),
            Some(islet_core::ContentCard::Calculator(_))
        );
        if calculator {
            widgets::key_at(x, y).map(WidgetHover::Key)
        } else {
            None
        }
    }

    fn calculator_keyboard_available(&self) -> bool {
        self.state.expanded
            && !self.traveling
            && !self.fullscreen_hidden
            && self.preview_pending.is_none()
            && matches!(
                self.state.primary_card(),
                Some(islet_core::ContentCard::Calculator(_))
            )
    }

    fn calculator_keyboard_active(&self) -> bool {
        self.calculator_keyboard_available()
            && unsafe { GetForegroundWindow() == self.hwnd && GetFocus() == self.hwnd }
    }

    fn update_calculator_activation(&mut self) {
        let accepts_focus = self.calculator_keyboard_available();
        if accepts_focus != self.calculator_accepts_focus {
            unsafe {
                let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
                let no_activate = WS_EX_NOACTIVATE.0 as isize;
                SetWindowLongPtrW(
                    self.hwnd,
                    GWL_EXSTYLE,
                    if accepts_focus {
                        style & !no_activate
                    } else {
                        style | no_activate
                    },
                );
            }
            self.calculator_accepts_focus = accepts_focus;
        }
    }

    fn focus_calculator(&mut self) {
        if !self.calculator_keyboard_available() {
            return;
        }
        unsafe {
            let previous = GetForegroundWindow();
            let activated = previous != self.hwnd && SetForegroundWindow(self.hwnd).as_bool();
            tracing::debug!(activated, foreground = ?GetForegroundWindow(), "calculator keyboard focus requested");
            if activated {
                self.calculator_return_focus =
                    IsWindow(Some(previous)).as_bool().then_some(previous);
                let _ = SetFocus(Some(self.hwnd));
            }
        }
    }

    fn release_calculator_focus(&mut self) {
        if let Some(previous) = self.calculator_return_focus.take() {
            tracing::debug!("calculator keyboard focus released");
            unsafe {
                if GetForegroundWindow() == self.hwnd && IsWindow(Some(previous)).as_bool() {
                    let _ = SetForegroundWindow(previous);
                }
            }
        }
    }

    fn set_widget_hover(&mut self, hover: Option<WidgetHover>) {
        if self.widget_hover != hover {
            self.widget_hover = hover;
            let _ = self.tx.send(RenderCommand::WidgetHover(hover));
        }
    }

    fn update_widget_hover(&mut self) {
        let mut point = POINT::default();
        let hover = if unsafe { GetCursorPos(&mut point) }.is_ok()
            && unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool()
        {
            self.widget_hover_at(point)
        } else {
            None
        };
        self.set_widget_hover(hover);
    }

    fn handle_widget_click(&mut self, lparam: LPARAM) -> bool {
        let point = point_from_lparam(lparam);
        match self.widget_hover_at(point) {
            Some(WidgetHover::Key(key)) => {
                self.dispatch(IslandEvent::CalculatorKey(key));
                true
            }
            None => {
                self.widget_point(point).is_some()
                    && matches!(
                        self.state.primary_card(),
                        Some(islet_core::ContentCard::Calculator(_))
                    )
            }
        }
    }

    fn media_duration(&self) -> Option<f64> {
        self.state
            .media
            .as_ref()
            .and_then(|media| media.timeline)
            .map(|timeline| timeline.duration)
            .filter(|duration| *duration > 0.0)
    }

    fn set_media_hover(&mut self, hover: Option<MediaHover>) {
        if self.media_hover != hover {
            self.media_hover = hover;
            let _ = self.tx.send(RenderCommand::MediaHover(hover));
        }
    }

    /// Reconcile on mouse movement, expansion, and the existing hover poll,
    /// including when the island moves under a stationary pointer.
    fn update_media_hover(&mut self) {
        self.update_widget_hover();
        let mut point = POINT::default();
        let hover = if !self.state.expanded || self.fullscreen_hidden || self.traveling {
            None
        } else if self.scrubbing {
            Some(MediaHover::Seek)
        } else if self.volume_dragging {
            Some(MediaHover::Volume)
        } else if unsafe { GetCursorPos(&mut point) }.is_ok()
            && unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool()
        {
            match self.state.primary_card() {
                Some(islet_core::ContentCard::Media(media)) => {
                    let layout = self.state.layout();
                    let layout = self.customized_layout(layout, self.effective_anchor_top());
                    let main = self.target_frame(layout, self.effective_anchor_top()).main;
                    let scale = self.dpi as f32 / 96.0;
                    expanded_hover_at(
                        (point.x as f32 / scale - main.x) / layout.content_scale,
                        (point.y as f32 / scale - main.y) / layout.content_scale,
                        main.width / layout.content_scale,
                        media
                            .timeline
                            .is_some_and(|timeline| timeline.duration > 0.0),
                        media.volume.is_some(),
                        !media.source.is_empty(),
                    )
                }
                _ => None,
            }
        } else {
            None
        };
        self.set_media_hover(hover);
    }

    fn set_scrub(&mut self, scrub: Option<f32>) {
        if let Some(mut media) = self.state.media.clone() {
            media.scrub = scrub;
            self.dispatch(IslandEvent::SetMedia(Some(media)));
        }
    }

    fn finish_scrub(&mut self, commit: bool) {
        self.scrubbing = false;
        unsafe {
            let _ = ReleaseCapture();
        }
        let fraction = self.state.media.as_ref().and_then(|media| media.scrub);
        match fraction {
            Some(fraction) if commit && self.media_duration().is_some() => {
                self.media_action(MediaAction::Seek(fraction));
            }
            _ => self.set_scrub(None),
        }
        self.update_media_hover();
    }

    /// Moves the slider and the system volume together; the worker writes the
    /// device on its own thread.
    fn set_volume(&mut self, value: f32) {
        let value = value.clamp(0.0, 1.0);
        if self.demo.is_none() {
            if let Some(handle) = &self.volume {
                handle.set(value);
            }
        }
        if let Some(mut media) = self.state.media.clone() {
            if media.volume != Some(value) {
                media.volume = Some(value);
                self.dispatch(IslandEvent::SetMedia(Some(media)));
            }
        }
    }

    fn finish_volume(&mut self) {
        self.volume_dragging = false;
        unsafe {
            let _ = ReleaseCapture();
        }
        self.update_media_hover();
    }

    /// Follows volume changes made elsewhere (media keys, the Windows mixer)
    /// while the player is on screen. Nothing is polled while it is hidden.
    fn sync_volume(&mut self) {
        if self.volume_dragging || self.demo.is_some() {
            return;
        }
        let Some(handle) = &self.volume else {
            return;
        };
        if !self.state.expanded
            || !matches!(
                self.state.primary_card(),
                Some(islet_core::ContentCard::Media(_))
            )
        {
            return;
        }
        handle.refresh();
        let Some(value) = handle.read() else {
            return;
        };
        if let Some(mut media) = self.state.media.clone() {
            if media.volume != Some(value) {
                media.volume = Some(value);
                self.dispatch(IslandEvent::SetMedia(Some(media)));
            }
        }
    }

    /// Sends a transport command and updates the card optimistically, so
    /// the island reacts before the app reports back.
    fn media_action(&mut self, action: MediaAction) {
        if action == MediaAction::OpenApp {
            self.open_current_media_app();
            return;
        }
        let Some(mut media) = self.state.media.clone() else {
            // The opened island is showing the remembered song.
            self.remembered_action(action);
            return;
        };
        let now = clock_now();
        let command = match action {
            MediaAction::Previous => MediaCommand::Previous,
            MediaAction::Next => MediaCommand::Next,
            MediaAction::PlayPause => {
                if let Some(timeline) = &mut media.timeline {
                    timeline.position = timeline.position_at(now, media.playing);
                    timeline.sampled_at = now;
                }
                media.playing = !media.playing;
                MediaCommand::TogglePlayPause
            }
            MediaAction::Seek(fraction) => {
                let Some(timeline) = &mut media.timeline else {
                    return;
                };
                timeline.position = f64::from(fraction) * timeline.duration;
                timeline.sampled_at = now;
                MediaCommand::Seek(timeline.position)
            }
            MediaAction::OpenApp => unreachable!(),
        };
        media.scrub = None;
        if let (Some(service), Some(info)) = (&self.media, self.focused_media()) {
            service.send(&info.app_id, command);
        }
        self.dispatch(IslandEvent::SetMedia(Some(media)));
    }

    /// A control on the remembered song. A player still open in the
    /// background (paused, so no longer on the island) takes the command
    /// directly; a closed app is opened and play is pressed once its session
    /// appears; a closed browser has no tab to return to, so the song is
    /// looked up on YouTube Music instead.
    fn remembered_action(&mut self, action: MediaAction) {
        let remembered_app = self.remembered.as_ref().map(|r| r.app_id.clone());
        // Only the remembered song's own player: another app with a media
        // session (WhatsApp voice notes, say) must never be started instead.
        let live = remembered_app
            .as_deref()
            .and_then(|app| self.last_media.iter().find(|info| info.app_id == app))
            .map(|info| info.app_id.clone());
        let command = match action {
            MediaAction::PlayPause => Some(MediaCommand::TogglePlayPause),
            MediaAction::Next => Some(MediaCommand::Next),
            MediaAction::Previous => Some(MediaCommand::Previous),
            MediaAction::Seek(_) | MediaAction::OpenApp => None,
        };
        if let Some(app) = &live {
            match (command, &self.media) {
                (Some(command), Some(service)) => service.send(app, command),
                _ if action == MediaAction::OpenApp => self.open_current_media_app(),
                _ => {}
            }
            return;
        }
        if !matches!(action, MediaAction::PlayPause | MediaAction::OpenApp) {
            return;
        }
        let Some(remembered) = &self.remembered else {
            return;
        };
        let card = &remembered.card;
        if action == MediaAction::PlayPause && is_browser(&remembered.app_id, &card.source) {
            let query = format!("{} {}", card.title, card.artist);
            shell_execute(&format!(
                "https://music.youtube.com/search?q={}",
                url_encode(query.trim())
            ));
            return;
        }
        open_media_source(&remembered.app_id, &card.source);
        if action == MediaAction::PlayPause {
            self.resume_pending = Some((remembered.app_id.clone(), Instant::now()));
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_RESUME, 1_000, None);
            }
        }
    }

    fn open_current_media_app(&self) {
        if let Some(info) = self.focused_media() {
            open_media_source(&info.app_id, &info.source);
        } else if let Some(remembered) = &self.remembered {
            open_media_source(&remembered.app_id, &remembered.card.source);
        }
    }

    /// Runs the poll timer only while something needs it, so an idle island
    /// has no UI-thread wakeups.
    fn update_poll_timer(&mut self) {
        let wanted = if self.state.pointer_inside && !self.fullscreen_hidden {
            Some(POLL_HOVER_MS)
        } else if self.follow_cursor && self.preview.is_none() {
            Some(POLL_FOLLOW_MS)
        } else {
            None
        };
        if wanted == self.poll_interval {
            return;
        }
        unsafe {
            match wanted {
                Some(interval) => {
                    SetTimer(Some(self.hwnd), TIMER_POLL, interval, None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_POLL);
                }
            }
        }
        self.poll_interval = wanted;
    }

    /// Registers as a shell appbar purely to receive `ABN_FULLSCREENAPP`.
    /// No `ABM_SETPOS` is sent, so no screen space is reserved.
    fn register_appbar(&mut self) {
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: self.hwnd,
            uCallbackMessage: WM_APP_APPBAR,
            ..APPBARDATA::default()
        };
        self.appbar = unsafe { SHAppBarMessage(ABM_NEW, &mut data) } != 0;
        if !self.appbar {
            tracing::warn!("could not register for full-screen notifications");
        }
    }

    fn unregister_appbar(&mut self) {
        if !self.appbar {
            return;
        }
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: self.hwnd,
            ..APPBARDATA::default()
        };
        unsafe {
            SHAppBarMessage(ABM_REMOVE, &mut data);
        }
        self.appbar = false;
    }

    fn on_fullscreen_notification(&mut self, opened: bool) {
        if self.preview.is_some() || !self.hide_in_fullscreen || self.demo.is_some() {
            return;
        }
        // The shell also reports the desktop and some maximized windows as
        // full-screen, so confirm before hiding.
        let hide = opened && self.should_hide_for_fullscreen();
        self.set_fullscreen_hidden(hide);
    }

    fn send_snapshot(&mut self) {
        self.update_calculator_activation();
        if !self.calculator_keyboard_available() {
            self.release_calculator_focus();
        }
        let customizing = self.preview_property(w!("IsletCustomize")) != 0;
        let preview_state = self.preview.map(|_| {
            let mut state = self.state.clone();
            state.tucked = false;
            if customizing {
                state.dictation = None;
                state.done = None;
                state.needs_input = None;
                state.running = None;
                state.expanded = true;
                if state.media.is_none() {
                    let mut media = demo_media();
                    media.title = "Your island".into();
                    media.artist = "Expanded preview".into();
                    media.source = String::new();
                    media.playing = false;
                    media.timeline = None;
                    media.volume = None;
                    state.media = Some(media);
                }
            }
            if state.content().primary.is_none() {
                state.running = Some(AgentCard {
                    project: "Onyx".into(),
                    activity: "Ready when you are".into(),
                    state: AgentState::Done,
                });
            }
            state
        });
        let display_state = preview_state.as_ref().unwrap_or(&self.state);
        let anchor_top = self.effective_anchor_top();
        let target = self.customized_layout(display_state.layout(), anchor_top);
        let content = display_state.content();
        if self
            .last_sent_anchor_top
            .is_some_and(|last| last != anchor_top)
        {
            // The region will grow to cover the whole path between the two
            // edges. Make the moving pill click-through until it arrives, so
            // that a tall transparent window never swallows clicks.
            self.set_click_through(true);
            self.traveling = true;
            self.release_calculator_focus();
        }
        self.last_sent_anchor_top = Some(anchor_top);
        self.generation = self.generation.wrapping_add(1);
        // Grow the region before the render thread starts moving, so no
        // frame of the morph is clipped. It shrinks again on WM_APP_SETTLED.
        if self.last_target != Some(target)
            || self.morph_box.is_some()
            || self.last_target_anchor_top != Some(anchor_top)
        {
            let previous_anchor = self.last_target_anchor_top.unwrap_or(anchor_top);
            let previous = self.morph_box.or_else(|| {
                self.last_target
                    .and_then(|last| self.target_frame(last, previous_anchor).bounds())
            });
            self.morph_box = match (previous, self.target_frame(target, anchor_top).bounds()) {
                (Some(previous), Some(next)) => Some(previous.union(next)),
                (previous, next) => previous.or(next),
            };
            self.last_target = Some(target);
            self.last_target_anchor_top = Some(anchor_top);
            self.update_region();
        }
        let command = RenderCommand::Snapshot {
            target,
            content: Box::new(content),
            anchor_top,
            generation: self.generation,
        };
        if self.tx.send(command).is_err() && !self.warned_disconnected {
            self.warned_disconnected = true;
            tracing::warn!("render thread is no longer accepting snapshots");
        }
        self.update_media_hover();
    }

    fn target_frame(
        &self,
        target: LayoutTarget,
        anchor_top: bool,
    ) -> islet_core::island::PillFrame {
        arrange(
            target.main,
            target.bubble,
            target.gap,
            target.inset,
            self.pixel_width as f32 * 96.0 / self.dpi as f32,
            self.logical_height,
            anchor_top,
        )
    }

    fn preview_property(&self, key: windows::core::PCWSTR) -> usize {
        self.preview.map_or(0, |anchor| unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetPropW(anchor, key).0 as usize
        })
    }

    fn customized_layout(&self, target: LayoutTarget, anchor_top: bool) -> LayoutTarget {
        let mode = self.preview_property(w!("IsletCustomize"));
        let top = if self.preview.is_some() && mode == 0 {
            self.anchor_top
        } else {
            anchor_top
        };
        let (margin, scale) = if mode != 0 {
            (
                self.preview_property(w!("IsletMargin")).saturating_sub(1) as f32 / 100.0,
                self.preview_property(w!("IsletScale")) as f32 / 1000.0,
            )
        } else {
            (
                if top {
                    self.appearance[0]
                } else {
                    self.appearance[1]
                },
                if top {
                    self.appearance[2]
                } else {
                    self.appearance[3]
                },
            )
        };
        target.customized(
            scale,
            if self.preview.is_some() && mode == 0 {
                0.0
            } else {
                margin
            },
            self.logical_height,
        )
    }

    /// The slim bar always docks at the bottom edge: at the top it would
    /// cover browser tabs and title bars. Dictation always shows at the
    /// bottom too, near where you type; a top island travels down as a
    /// droplet when it starts and back up when it ends.
    fn effective_anchor_top(&self) -> bool {
        if self.preview.is_some() {
            self.preview_property(w!("IsletCustomize")) != 2
        } else {
            self.anchor_top && !self.state.tucked && self.state.dictation.is_none()
        }
    }

    fn update_region(&mut self) {
        let scale = self.dpi as f32 / 96.0;
        let region = if let Some(morph) = self.morph_box {
            WindowRegion::Box(scale_rect(morph.inflate(REGION_MORPH_MARGIN_DP), scale))
        } else {
            let target = self.last_target.unwrap_or(LayoutTarget::HIDDEN);
            let anchor_top = self
                .last_target_anchor_top
                .unwrap_or_else(|| self.effective_anchor_top());
            let frame = self.target_frame(target, anchor_top);
            match frame.bounds() {
                None => WindowRegion::Empty,
                Some(_) => WindowRegion::Exact {
                    main: scale_rect(frame.main, scale),
                    main_corner: frame.main_corner * scale,
                    bubble: frame
                        .bubble
                        .map(|bubble| (scale_rect(bubble, scale), frame.bubble_corner * scale)),
                },
            }
        };
        if self.region == Some(region) {
            return;
        }
        // SAFETY: plain GDI region calls on the UI thread that owns the
        // window. On success the system owns the region handle.
        unsafe {
            let handle = build_region(region);
            if SetWindowRgn(self.hwnd, Some(handle), true) == 0 {
                let _ = DeleteObject(handle.into());
                tracing::warn!(?region, "could not set island window region");
                return;
            }
        }
        self.region = Some(region);
    }

    fn arm_timer(&self, kind: TimerKind, after_ms: u32) {
        let id = match kind {
            TimerKind::HoverExpand => TIMER_HOVER_EXPAND,
            TimerKind::HoverCollapse => TIMER_HOVER_COLLAPSE,
            TimerKind::DoneExpiry => TIMER_DONE_EXPIRY,
            TimerKind::MediaPaused => TIMER_MEDIA_PAUSED,
            TimerKind::MediaCondense => TIMER_MEDIA_CONDENSE,
        };
        let result = unsafe { SetTimer(Some(self.hwnd), id, after_ms, None) };
        if result == 0 {
            tracing::warn!(?kind, "could not arm island timer");
        }
    }

    fn cancel_timer(&self, kind: TimerKind) {
        let id = match kind {
            TimerKind::HoverExpand => TIMER_HOVER_EXPAND,
            TimerKind::HoverCollapse => TIMER_HOVER_COLLAPSE,
            TimerKind::DoneExpiry => TIMER_DONE_EXPIRY,
            TimerKind::MediaPaused => TIMER_MEDIA_PAUSED,
            TimerKind::MediaCondense => TIMER_MEDIA_CONDENSE,
        };
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_HOVER_EXPAND => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_HOVER_EXPAND);
                }
                self.dispatch(IslandEvent::HoverExpandElapsed);
            }
            TIMER_HOVER_COLLAPSE => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_HOVER_COLLAPSE);
                }
                self.dispatch(IslandEvent::HoverCollapseElapsed);
            }
            TIMER_DONE_EXPIRY => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_DONE_EXPIRY);
                }
                self.dispatch(IslandEvent::Tick { now: clock_now() });
            }
            TIMER_OCCLUSION => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_OCCLUSION);
                }
                self.occlusion_pending = false;
                self.update_tucked();
            }
            TIMER_EMPTY_CHECK => self.update_tucked(),
            TIMER_STATS => self.sample_stats(),
            TIMER_CLOCK => self.update_clock(),
            TIMER_RESUME => self.finish_resume(),
            TIMER_MEDIA_PAUSED => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_MEDIA_PAUSED);
                }
                self.dispatch(IslandEvent::MediaPausedElapsed);
            }
            TIMER_MEDIA_CONDENSE => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_MEDIA_CONDENSE);
                }
                self.dispatch(IslandEvent::MediaCondenseElapsed);
            }
            TIMER_POLL => self.poll(),
            TIMER_DEMO => self.demo_advance(),
            _ => {}
        }
    }

    fn poll(&mut self) {
        if self.follow_cursor {
            if let Err(error) = self.reposition() {
                tracing::warn!(%error, "could not follow monitor");
            }
        }
        if !self.fullscreen_hidden && !self.scrubbing {
            let inside = self.cursor_inside();
            let on_switcher = inside && self.cursor_on_media_bubble();
            if inside != self.state.pointer_inside || on_switcher != self.state.pointer_on_switcher
            {
                self.dispatch_hover(inside);
            }
        }
        self.sync_volume();
        self.update_media_hover();
        self.update_poll_timer();
    }

    fn track_mouse(&mut self) {
        if self.tracking_mouse {
            return;
        }
        let mut track = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        if unsafe { TrackMouseEvent(&mut track) }.is_ok() {
            self.tracking_mouse = true;
        }
    }

    fn cursor_inside(&self) -> bool {
        if self.fullscreen_hidden {
            return false;
        }
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            return false;
        }
        if !unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            return false;
        }
        self.hit
            .lock()
            .map(|hit| hit.contains(point.x as f32, point.y as f32))
            .unwrap_or(false)
    }

    fn hit_test(&self, lparam: LPARAM) -> LRESULT {
        if self.fullscreen_hidden {
            return LRESULT(HTTRANSPARENT as isize);
        }
        let mut point = point_from_lparam(lparam);
        if !unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            return LRESULT(HTTRANSPARENT as isize);
        }
        let inside = self
            .hit
            .lock()
            .map(|hit| hit.contains(point.x as f32, point.y as f32))
            .unwrap_or(false);
        if inside {
            LRESULT(HTCLIENT as isize)
        } else {
            LRESULT(HTTRANSPARENT as isize)
        }
    }

    fn begin_departure(&mut self) {
        self.release_calculator_focus();
        self.set_click_through(true);
        if let Some(target) = self.last_target {
            self.morph_box = self
                .target_frame(target, self.effective_anchor_top())
                .bounds();
            self.update_region();
        }
        let _ = self.tx.send(RenderCommand::Suspend(false));
        let _ = self.tx.send(RenderCommand::Depart);
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_PREVIEW_HANDOFF, 900, None);
        }
    }

    fn refresh_preview(&mut self) {
        let Some(anchor) = self.preview else {
            return;
        };
        if !unsafe { IsWindow(Some(anchor)) }.as_bool() {
            self.preview = None;
            self.preview_pending = None;
            self.last_layout = None;
            let _ = self.reposition_layout();
            self.set_fullscreen_hidden(false);
            self.send_snapshot();
            let _ = self.tx.send(RenderCommand::Arrive);
            self.update_window_events();
            return;
        }
        let root = unsafe { GetAncestor(anchor, GA_ROOT) };
        // Keep an independent window: owning it across processes would destroy
        // the island if settings crashed. Match the settings z-order explicitly.
        let hidden =
            unsafe { IsIconic(root) }.as_bool() || !unsafe { IsWindowVisible(root) }.as_bool();
        self.set_fullscreen_hidden(hidden);
        let mut rect = windows::Win32::Foundation::RECT::default();
        if unsafe { GetWindowRect(anchor, &mut rect) }.is_err() {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(root) }.max(96);
        let layout = WindowLayout {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            dpi,
            anchor_top: true,
            logical_height: (rect.bottom - rect.top) as f32 * 96.0 / dpi as f32,
        };
        if self.last_layout == Some(layout) {
            self.align_preview_z_order(root);
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_NOTOPMOST),
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                SWP_NOACTIVATE,
            );
        }
        self.align_preview_z_order(root);
        self.last_layout = Some(layout);
        self.dpi = dpi;
        self.pixel_width = layout.width as u32;
        self.pixel_height = layout.height as u32;
        self.logical_height = layout.logical_height;
        let _ = self.tx.send(RenderCommand::Resize {
            width: self.pixel_width,
            height: self.pixel_height,
            dpi: dpi as f32,
            logical_width: layout.width as f32 * 96.0 / dpi as f32,
            logical_height: self.logical_height,
        });
        self.region = None;
        self.update_region();
    }

    fn align_preview_z_order(&self, root: HWND) {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindow, GW_HWNDPREV, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE,
        };
        let previous = unsafe { GetWindow(root, GW_HWNDPREV) }.unwrap_or(HWND_TOP);
        if previous != self.hwnd {
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(previous),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                );
            }
        }
    }

    fn reposition(&mut self) -> WinResult<()> {
        self.reposition_layout()?;
        self.update_tucked();
        Ok(())
    }

    /// Places and sizes the window for the configured edge. At the top the
    /// window spans the whole work area, giving the renderer room to spring
    /// the pill to the bottom and back.
    fn reposition_layout(&mut self) -> WinResult<()> {
        if self.preview.is_some() {
            self.refresh_preview();
            return Ok(());
        }
        let config = read_config(&self.shared);
        let layout = compute_window_layout(&config, self.anchor_top)?;
        self.home_layout = Some(layout);
        if self.last_layout == Some(layout) {
            return Ok(());
        }
        apply_window_layout(self.hwnd, layout)?;
        self.last_layout = Some(layout);
        self.dpi = layout.dpi;
        self.pixel_width = layout.width as u32;
        self.pixel_height = layout.height as u32;
        self.logical_height = layout.logical_height;
        let _ = self.tx.send(RenderCommand::Resize {
            width: self.pixel_width,
            height: self.pixel_height,
            dpi: layout.dpi as f32,
            logical_width: WINDOW_LOGICAL_WIDTH,
            logical_height: self.logical_height,
        });
        self.region = None;
        self.update_region();
        Ok(())
    }

    /// Extended style bit that makes the window invisible to the mouse.
    fn set_click_through(&self, click_through: bool) {
        // SAFETY: reading and writing this window's own extended style from
        // the thread that owns it.
        unsafe {
            let current = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let flag = WS_EX_TRANSPARENT.0 as isize;
            let updated = if click_through {
                current | flag
            } else {
                current & !flag
            };
            if updated != current {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, updated);
            }
        }
    }

    fn set_fullscreen_hidden(&mut self, hidden: bool) {
        if hidden == self.fullscreen_hidden {
            return;
        }
        self.fullscreen_hidden = hidden;
        if hidden {
            self.release_calculator_focus();
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            let _ = self.tx.send(RenderCommand::Suspend(true));
            if let Ok(mut hit) = self.hit.lock() {
                *hit = HitRegion::default();
            }
        } else {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            let _ = self.tx.send(RenderCommand::Suspend(false));
            self.send_snapshot();
        }
        tracing::debug!(hidden, "full-screen visibility changed");
        self.update_poll_timer();
        self.update_loopback();
    }

    fn should_hide_for_fullscreen(&self) -> bool {
        if self.preview.is_some() {
            return self.fullscreen_hidden;
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() || foreground == self.hwnd {
            return false;
        }
        let mut rect = windows::Win32::Foundation::RECT::default();
        if unsafe { GetWindowRect(foreground, &mut rect) }.is_err() {
            return false;
        }
        let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
        let Some(info) = monitor_info(monitor) else {
            return false;
        };
        let monitor_rect = info.rcMonitor;
        let covers = rect.left <= monitor_rect.left + 2
            && rect.top <= monitor_rect.top + 2
            && rect.right >= monitor_rect.right - 2
            && rect.bottom >= monitor_rect.bottom - 2;
        if !covers {
            return false;
        }
        matches!(
            unsafe { SHQueryUserNotificationState() }.ok(),
            Some(QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_BUSY)
        )
    }

    fn demo_advance(&mut self) {
        let step = self.demo.as_ref().map_or(0, |demo| demo.step);
        // Debug: `ISLET_DEMO_STEPS=0,1` cycles only those looks (here: hidden
        // and music, to watch the island appear and disappear).
        let look = std::env::var("ISLET_DEMO_STEPS")
            .ok()
            .map(|list| {
                list.split(',')
                    .filter_map(|item| item.trim().parse::<usize>().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|looks| !looks.is_empty())
            .map_or(step, |looks| looks[step % looks.len()]);
        let mut state = IslandState {
            calculator: self.state.calculator.clone(),
            stats: self.state.stats.clone(),
            widget: self.state.widget,
            idle_style: self.idle_style,
            expand_on: self.state.expand_on,
            ..IslandState::default()
        };
        match look % DEMO_STATES {
            0 => {}
            1 => state.media = Some(demo_media()),
            2 => {
                state.media = Some(demo_media());
                state.expanded = true;
            }
            3 => {
                state.media = Some(demo_media());
                state.running = Some(demo_agent());
            }
            4 => state.needs_input = Some(demo_needs_input()),
            5 => {
                state.needs_input = Some(demo_needs_input());
                state.expanded = true;
            }
            6 => state.dictation = Some(demo_dictation()),
            7 => state.done = Some(demo_done()),
            // The dictation pill's other looks. With no music behind them
            // the bars are white; 11 and 12 borrow a music colour.
            8 => state.dictation = Some(demo_dictation_as(DictationPhase::Loading, false)),
            9 => state.dictation = Some(demo_dictation_as(DictationPhase::Transcribing, false)),
            10 => state.dictation = Some(demo_dictation_as(DictationPhase::Recording, true)),
            11 => {
                state.last_music_accent = Some([224, 149, 94]);
                state.dictation = Some(demo_dictation_as(DictationPhase::Recording, true));
            }
            12 => {
                state.media = Some(demo_media());
                state.last_music_accent = Some([224, 149, 94]);
                state.dictation = Some(demo_dictation());
            }
            // Two players: the second waits in the bubble, and stays beside
            // the expanded one.
            13 => {
                state.media = Some(demo_media());
                state.other_media = Some(demo_browser_media());
            }
            14 => {
                state.media = Some(demo_media());
                state.other_media = Some(demo_browser_media());
                state.expanded = true;
            }
            // Music that has played a while: just art and bars.
            15 => {
                state.media = Some(demo_media());
                state.media_condensed = true;
            }
            16 => {
                state.running = Some(demo_agent());
                state.expanded = true;
            }
            _ => unreachable!(),
        }
        // Debug: every other step, glides down to the bottom edge.
        state.tucked = self.demo_dock && step % 2 == 1;
        self.state = state;
        if let Some(demo) = &mut self.demo {
            demo.step = demo.step.wrapping_add(1);
        }
        self.send_snapshot();
    }
}

impl Drop for IslandHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut IslandHost;
    if !pointer.is_null() {
        let host = unsafe { &mut *pointer };
        if let Some(result) = host.handle_message(hwnd, message, wparam, lparam) {
            return result;
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn render_thread(
    config: RendererConfig,
    hit: Arc<Mutex<HitRegion>>,
    bars: Arc<BarLevels>,
    rx: Receiver<RenderCommand>,
    notify: isize,
) {
    let notify = HWND(notify as *mut _);
    // Tells the UI thread the shape stopped moving, so it can shrink the
    // window region. Content-only animation does not count as moving.
    let report_settled = |generation: usize| {
        // SAFETY: posting to a window owned by the UI thread is
        // non-blocking; a failure only delays the region shrink.
        unsafe {
            let _ = PostMessageW(Some(notify), WM_APP_SETTLED, WPARAM(generation), LPARAM(0));
        }
    };
    let mut renderer = match Renderer::new(config, Arc::clone(&hit), bars, clock_now()) {
        Ok(renderer) => renderer,
        Err(error) => {
            tracing::error!(%error, "could not create island renderer");
            if let Ok(mut hit) = hit.lock() {
                *hit = HitRegion::default();
            }
            return;
        }
    };

    let mut frame_log = std::env::var_os("ISLET_FRAME_LOG").and_then(|path| {
        std::fs::File::create(path)
            .ok()
            .map(|file| (std::io::BufWriter::new(file), Instant::now(), 0u64))
    });

    let mut dirty = true;
    let mut suspended = false;
    let mut generation = 0;
    let mut reported_settled = None;
    loop {
        let mut stop = false;
        while let Ok(command) = rx.try_recv() {
            if apply_command(
                &mut renderer,
                command,
                &mut dirty,
                &mut suspended,
                &mut generation,
            ) {
                stop = true;
                break;
            }
        }
        if stop {
            break;
        }

        if suspended {
            if let Some((_, last, _)) = &mut frame_log {
                *last = Instant::now();
            }
            match rx.recv() {
                Ok(command) => {
                    if apply_command(
                        &mut renderer,
                        command,
                        &mut dirty,
                        &mut suspended,
                        &mut generation,
                    ) {
                        break;
                    }
                }
                Err(_) => break,
            }
            continue;
        }

        let now = clock_now();
        if dirty || renderer.needs_frame(now) {
            let morphing = renderer.is_morphing(now);
            if !dirty {
                // Every visible animation follows the compositor's actual
                // refresh cadence, including bars and the interpolated seekbar.
                // The renderer still parks completely when nothing animates.
                wait_for_compositor_clock();
            }
            let last_frame = clock_now();
            let presented = match renderer.render(last_frame, dirty) {
                Ok(presented) => presented,
                Err(error) => {
                    tracing::error!(%error, "island render failed");
                    break;
                }
            };
            if renderer.take_departure_completed(last_frame) {
                unsafe {
                    let _ = PostMessageW(Some(notify), PREVIEW_DEPARTED, WPARAM(0), LPARAM(0));
                }
            }
            if reported_settled != Some(generation) && !renderer.is_morphing(clock_now()) {
                reported_settled = Some(generation);
                report_settled(generation);
            }
            if let Some((writer, last, count)) = frame_log.as_mut().filter(|_| presented) {
                use std::io::Write;
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(*last).as_secs_f64() * 1000.0;
                if *count > 0 {
                    let kind = if morphing { 'm' } else { 'c' };
                    let _ = writeln!(writer, "{elapsed:.3},{kind}");
                }
                *last = now;
                *count += 1;
                if *count % 120 == 0 {
                    let _ = writer.flush();
                }
            }
            dirty = false;
        } else {
            if reported_settled != Some(generation) {
                reported_settled = Some(generation);
                report_settled(generation);
            }
            if let Some((writer, _, _)) = &mut frame_log {
                use std::io::Write;
                let _ = writer.flush();
            }
            if let Some((_, last, count)) = &mut frame_log {
                *last = Instant::now();
                *count = 0;
            }
            // Parked. The only timed wakeup is the one that releases
            // graphics after the island has been hidden for a while.
            let received = match renderer.graphics_release_in(clock_now()) {
                Some(delay) => match rx.recv_timeout(std::time::Duration::from_secs_f64(delay)) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => {
                        renderer.release_graphics();
                        tracing::debug!("released island graphics after idle");
                        None
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                },
            };
            if let Some(command) = received {
                if apply_command(
                    &mut renderer,
                    command,
                    &mut dirty,
                    &mut suspended,
                    &mut generation,
                ) {
                    break;
                }
            }
        }
    }

    if let Some((mut writer, _, _)) = frame_log {
        use std::io::Write;
        let _ = writer.flush();
    }

    if let Ok(mut hit) = hit.lock() {
        *hit = HitRegion::default();
    }
}

fn apply_command(
    renderer: &mut Renderer,
    command: RenderCommand,
    dirty: &mut bool,
    suspended: &mut bool,
    generation: &mut usize,
) -> bool {
    match command {
        RenderCommand::Depart => {
            renderer.depart_surface(clock_now(), surface_animations_enabled());
            *dirty = true;
            false
        }
        RenderCommand::Arrive => {
            renderer.arrive_surface(clock_now(), surface_animations_enabled());
            *dirty = true;
            false
        }
        RenderCommand::MediaHover(hover) => {
            renderer.set_media_hover(hover);
            *dirty = true;
            false
        }
        RenderCommand::WidgetHover(hover) => {
            renderer.set_widget_hover(hover);
            *dirty = true;
            false
        }
        RenderCommand::Snapshot {
            target,
            content,
            anchor_top,
            generation: snapshot_generation,
        } => {
            *generation = snapshot_generation;
            let now = clock_now();
            renderer.set_anchor(anchor_top, now);
            *dirty |= renderer.set_snapshot(target, *content, now);
            false
        }
        RenderCommand::Resize {
            width,
            height,
            dpi,
            logical_width,
            logical_height,
        } => {
            if let Err(error) = renderer.resize(width, height, dpi) {
                tracing::error!(%error, "island renderer resize failed");
            }
            renderer.set_logical_size(logical_width, logical_height);
            *dirty = true;
            false
        }
        RenderCommand::Suspend(value) => {
            *suspended = value;
            if !value {
                *dirty = true;
            }
            false
        }
        RenderCommand::Stop => true,
    }
}

type WaitForCompositorClock = unsafe extern "system" fn(u32, *const HANDLE, u32) -> u32;

/// Blocks until the next compositor frame. Windows 11 exposes the compositor
/// clock directly; Windows 10 falls back to `DwmFlush`. The Windows 11 export
/// is resolved at runtime so the binary still loads on Windows 10.
fn wait_for_compositor_clock() {
    static WAIT: OnceLock<Option<WaitForCompositorClock>> = OnceLock::new();
    let wait = WAIT.get_or_init(|| unsafe {
        let module = LoadLibraryW(w!("dcomp.dll")).ok()?;
        let proc = GetProcAddress(module, s!("DCompositionWaitForCompositorClock"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            WaitForCompositorClock,
        >(proc))
    });
    unsafe {
        match wait {
            // A bounded timeout keeps the loop alive if the clock stalls
            // (display off, remote session).
            Some(wait) => {
                wait(0, std::ptr::null(), 100);
            }
            None => {
                let _ = DwmFlush();
            }
        }
    }
}

fn clock_now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// The island clock in milliseconds, for [`BarLevels`] freshness.
fn clock_now_ms() -> u64 {
    (clock_now() * 1000.0) as u64
}

fn music_filter(config: &Config) -> MusicFilter {
    MusicFilter::new(config.media.music_only, &config.media.music_sites)
}

/// A paused player keeps its place beside another for the same delay a
/// lone paused player stays on the island.
fn media_pause_hold(config: &Config) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(config.media.pause_hide_seconds.max(1)))
}

/// The stats sampler runs for the stats widget or the stats glance.
fn needs_sampler(config: &Config) -> bool {
    config.widgets.system_stats
        || (config.general.always_on && config.general.glance == IdleGlance::Stats)
}

fn read_config(shared: &Arc<RwLock<Config>>) -> Config {
    shared
        .read()
        .map(|config| config.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

fn calculator_shortcut_held() -> bool {
    [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(|key| unsafe { GetKeyState(i32::from(key.0)) < 0 })
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    POINT { x, y }
}

fn compute_window_layout(config: &Config, anchor_top: bool) -> WinResult<WindowLayout> {
    let monitor = if config.general.monitor == MonitorChoice::FollowCursor {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }?;
        unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) }
    } else {
        unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
    };
    let info = monitor_info(monitor).ok_or_else(windows::core::Error::from_thread)?;

    let mut dpi_x = 96;
    let mut dpi_y = 96;
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let dpi = dpi_x.max(96);

    let width = (WINDOW_LOGICAL_WIDTH * dpi as f32 / 96.0).round() as i32;
    let work = info.rcWork;
    // At the top the window covers the full work area: the island can then
    // glide between the top and bottom edges inside one window instead of
    // teleporting a small window between them. At the bottom there is nowhere
    // to glide to, so keep the window compact.
    let (height, y, logical_height) = if anchor_top {
        let height = (work.bottom - work.top).max(1);
        (height, work.top, height as f32 * 96.0 / dpi as f32)
    } else {
        let logical_height = if config.widgets.calculator {
            240.0
        } else {
            WINDOW_LOGICAL_HEIGHT
        };
        let height = (logical_height * dpi as f32 / 96.0).round() as i32;
        (height.max(1), work.bottom - height, logical_height)
    };
    let x = work.left + ((work.right - work.left) - width) / 2;

    Ok(WindowLayout {
        x,
        y,
        width: width.max(1),
        height: height.max(1),
        dpi,
        anchor_top,
        logical_height,
    })
}

fn apply_window_layout(hwnd: HWND, layout: WindowLayout) -> WinResult<()> {
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            layout.x,
            layout.y,
            layout.width,
            layout.height,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    }
}

fn scale_rect(rect: Rect, scale: f32) -> Rect {
    Rect::new(
        rect.x * scale,
        rect.y * scale,
        rect.width * scale,
        rect.height * scale,
    )
}

/// Builds a GDI region. Rounded shapes are grown by a pixel so the region
/// never trims the pill's anti-aliased edge; the squircle lies inside a
/// circular corner of the same radius.
unsafe fn build_region(region: WindowRegion) -> HRGN {
    let rounded = |rect: Rect, corner: f32| {
        let diameter = (corner * 2.0).round() as i32 + 2;
        CreateRoundRectRgn(
            rect.x.floor() as i32 - 1,
            rect.y.floor() as i32 - 1,
            rect.right().ceil() as i32 + 2,
            rect.bottom().ceil() as i32 + 2,
            diameter,
            diameter,
        )
    };
    match region {
        WindowRegion::Empty => CreateRectRgn(0, 0, 0, 0),
        WindowRegion::Box(rect) => CreateRectRgn(
            rect.x.floor() as i32,
            rect.y.floor() as i32,
            rect.right().ceil() as i32,
            rect.bottom().ceil() as i32,
        ),
        WindowRegion::Exact {
            main,
            main_corner,
            bubble,
        } => {
            let combined = rounded(main, main_corner);
            if let Some((bubble, corner)) = bubble {
                let extra = rounded(bubble, corner);
                CombineRgn(Some(combined), Some(combined), Some(extra), RGN_OR);
                let _ = DeleteObject(extra.into());
            }
            combined
        }
    }
}

fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFO> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then_some(info)
}

fn demo_media() -> MediaCard {
    let art = demo_art();
    let accent = art
        .as_ref()
        .and_then(|art| islet_core::media::dominant_color(&art.pixels, art.width, art.height));
    MediaCard {
        title: "Midnight City".to_owned(),
        artist: "M83".to_owned(),
        source: "Spotify".to_owned(),
        playing: true,
        timeline: Some(Timeline {
            position: 91.5,
            duration: 243.0,
            sampled_at: clock_now(),
            rate: 1.0,
        }),
        art,
        accent,
        scrub: None,
        volume: Some(0.66),
    }
}

/// Debug: `ISLET_DEMO_ART=<file>` gives the demo song a cover (raw
/// premultiplied BGRA, square), for Store screenshots.
fn demo_art() -> Option<islet_core::media::ArtImage> {
    let pixels = std::fs::read(std::env::var_os("ISLET_DEMO_ART")?).ok()?;
    let side = ((pixels.len() / 4) as f64).sqrt() as u32;
    (side > 0 && (side * side * 4) as usize == pixels.len()).then(|| islet_core::media::ArtImage {
        id: 0xDE70_0000_0000_0001,
        width: side,
        height: side,
        pixels: pixels.into(),
    })
}

fn demo_browser_media() -> MediaCard {
    MediaCard {
        title: "Lo-fi beats to study to".to_owned(),
        artist: "Lofi Girl".to_owned(),
        source: "Chrome".to_owned(),
        timeline: Some(Timeline {
            position: 600.0,
            duration: 1_800.0,
            sampled_at: clock_now(),
            rate: 1.0,
        }),
        accent: Some([236, 120, 140]),
        ..demo_media()
    }
}

fn demo_agent() -> AgentCard {
    AgentCard {
        project: "islet".to_owned(),
        activity: "Running cargo test".to_owned(),
        state: AgentState::Running,
    }
}

fn demo_needs_input() -> NotificationCard {
    NotificationCard {
        source: "Claude Code".to_owned(),
        title: "Permission needed".to_owned(),
        body: "Allow cargo clippy to run?".to_owned(),
        level: AlertLevel::NeedsInput,
    }
}

fn demo_done() -> NotificationCard {
    NotificationCard {
        source: "Claude Code".to_owned(),
        title: "Claude finished".to_owned(),
        body: "Refactored auth.rs and updated tests".to_owned(),
        level: AlertLevel::Success,
    }
}

/// How many looks `--demo` cycles through.
pub const DEMO_STATES: usize = 17;

fn demo_dictation() -> DictationCard {
    demo_dictation_as(DictationPhase::Recording, false)
}

fn demo_dictation_as(phase: DictationPhase, hands_free: bool) -> DictationCard {
    // A plausible speech waveform: syllable bursts with small pauses.
    let mut bars = [0u8; islet_core::island::DICTATION_BARS];
    for (index, bar) in bars.iter_mut().enumerate() {
        let burst =
            ((index as f32 * 0.9).sin() * 0.5 + 0.5) * ((index as f32 * 0.37).cos() * 0.4 + 0.6);
        *bar = (burst * 230.0) as u8;
    }
    DictationCard {
        elapsed_ms: 7_400,
        level: 0.72,
        phase,
        bars,
        hands_free,
    }
}

fn open_media_source(app_id: &str, source: &str) {
    if app_id.is_empty() && source.is_empty() {
        return;
    }

    if try_activate_window(app_id, source) {
        return;
    }

    let lower_app = app_id.to_ascii_lowercase();
    let lower_source = source.to_ascii_lowercase();

    if (lower_app.contains("spotify") || lower_source.contains("spotify"))
        && shell_execute("spotify:")
    {
        return;
    }

    if app_id.contains('!') {
        let uri = format!("shell:AppsFolder\\{app_id}");
        if shell_execute(&uri) {
            return;
        }
    }

    if (app_id.ends_with(".exe") || app_id.contains('\\') || app_id.contains('/'))
        && shell_execute(app_id)
    {
        return;
    }

    if !app_id.is_empty() {
        let uri = format!("shell:AppsFolder\\{app_id}");
        if shell_execute(&uri) {
            return;
        }
        let _ = shell_execute(app_id);
    }
}

/// Whether a media session belongs to a web browser.
fn is_browser(app_id: &str, source: &str) -> bool {
    // A music tab found inside a browser carries "browser#tab" as its id.
    if app_id.contains('#') {
        return true;
    }
    let name = format!("{app_id} {source}").to_ascii_lowercase();
    ["chrome", "msedge", "edge", "firefox", "brave", "opera", "vivaldi", "arc"]
        .iter()
        .any(|browser| name.contains(browser))
}

/// Percent-encodes text for a URL query.
fn url_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn shell_execute(target: &str) -> bool {
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    result.0 as usize > 32
}

struct WindowFinder {
    needles: Vec<String>,
    found: Option<HWND>,
}

fn try_activate_window(app_id: &str, source: &str) -> bool {
    let Some(hwnd) = find_app_window(app_id, source) else {
        return false;
    };
    activate_window(hwnd)
}

/// The top-level window of the app behind a media session, if it is open.
fn find_app_window(app_id: &str, source: &str) -> Option<HWND> {
    let mut needles = Vec::new();
    let lower_source = source.to_ascii_lowercase();
    let lower_app = app_id.to_ascii_lowercase();

    if !lower_source.is_empty() {
        needles.push(lower_source.clone());
    }

    let base = lower_app.rsplit(['\\', '/']).next().unwrap_or(&lower_app);
    let base = base.split('!').next().unwrap_or(base);
    let base = base.split('_').next().unwrap_or(base);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    if !base.is_empty() && !needles.iter().any(|n| n == base) {
        needles.push(base.to_owned());
    }

    if (lower_app.contains("spotify") || lower_source.contains("spotify"))
        && !needles.iter().any(|n| n == "spotify")
    {
        needles.push("spotify".to_owned());
    }

    if needles.is_empty() {
        return None;
    }

    let mut finder = WindowFinder {
        needles,
        found: None,
    };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let finder = &mut *(lparam.0 as *mut WindowFinder);
        if is_candidate_window(hwnd, &finder.needles) {
            finder.found = Some(hwnd);
            return false.into();
        }
        true.into()
    }

    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(std::ptr::addr_of_mut!(finder) as isize),
        );
    }
    finder.found
}

fn activate_window(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }

        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let target_thread = GetWindowThreadProcessId(hwnd, None);
        let current_thread = GetCurrentThreadId();

        if foreground_thread != 0 && foreground_thread != target_thread {
            let _ = AttachThreadInput(current_thread, foreground_thread, true);
            let _ = AttachThreadInput(current_thread, target_thread, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            let _ = AttachThreadInput(current_thread, target_thread, false);
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        } else {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    true
}

unsafe fn is_candidate_window(hwnd: HWND, needles: &[String]) -> bool {
    if !IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() {
        return false;
    }

    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 || pid == std::process::id() {
        return false;
    }

    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return false;
    }

    // 1. Process image name.
    if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        let mut path_buf = [0u16; 1024];
        let mut size = path_buf.len() as u32;
        let success = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(path_buf.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(process);

        if success && size > 0 {
            let path = String::from_utf16_lossy(&path_buf[..size as usize]).to_ascii_lowercase();
            let file_name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            for needle in needles {
                if file_name.contains(needle) {
                    return true;
                }
            }
        }
    }

    // 2. Window title.
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len > 0 {
        let mut title_buf = vec![0u16; (title_len + 1) as usize];
        let copied = GetWindowTextW(hwnd, &mut title_buf);
        if copied > 0 {
            let title =
                String::from_utf16_lossy(&title_buf[..copied as usize]).to_ascii_lowercase();
            for needle in needles {
                if title.contains(needle) {
                    return true;
                }
            }
        }
    }

    false
}

fn surface_animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut enabled = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut windows::core::BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    enabled.as_bool()
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    #[test]
    fn browsers_are_recognised_and_queries_are_encoded() {
        assert!(is_browser("Chrome", ""));
        assert!(is_browser("MSEdge", "Microsoft Edge"));
        assert!(!is_browser("Spotify.exe", "Spotify"));
        assert_eq!(url_encode("Hey Jude & co"), "Hey+Jude+%26+co");
    }
}
