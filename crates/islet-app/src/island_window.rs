//! The island window, its UI-thread state machine, and the render thread.

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use islet_core::config::{IdleStyle, MonitorChoice, ScreenPosition};
use islet_core::island::{
    arrange, reduce, AgentCard, AgentState, AlertLevel, ContentSnapshot, DictationCard,
    DictationPhase, Effect, IslandEvent, IslandState, LayoutTarget, MediaCard, NotificationCard,
    TimerKind,
};
use islet_core::{Config, Rect};
use islet_render::{HitRegion, Renderer, RendererConfig};
use windows::core::{s, w, Result as WinResult};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromPoint,
    MonitorFromWindow, ScreenToClient, SetWindowRgn, HMONITOR, HRGN, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, RGN_OR,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, SHQueryUserNotificationState, ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP,
    APPBARDATA, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, KillTimer,
    PostMessageW, SetTimer, SetWindowPos, ShowWindow, GWLP_USERDATA, HTCLIENT, HTTRANSPARENT,
    HWND_TOPMOST, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE,
    SW_SHOWNOACTIVATE, WINDOWPOS, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_SETTINGCHANGE, WM_TIMER,
    WM_WINDOWPOSCHANGING,
};

pub const WINDOW_LOGICAL_WIDTH: f32 = 440.0;
pub const WINDOW_LOGICAL_HEIGHT: f32 = 200.0;
const EDGE_GAP_DP: f32 = 8.0;

const TIMER_HOVER_EXPAND: usize = 1;
const TIMER_HOVER_COLLAPSE: usize = 2;
const TIMER_DONE_EXPIRY: usize = 3;
const TIMER_POLL: usize = 4;
const TIMER_DEMO: usize = 5;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Shell appbar notifications (only `ABN_FULLSCREENAPP` is used).
const WM_APP_APPBAR: u32 = WM_APP + 20;
/// Posted by the render thread when it parks. `WPARAM` is the generation of
/// the last snapshot it applied.
const WM_APP_SETTLED: u32 = WM_APP + 21;
/// Extra room around a morphing island's window region for spring overshoot.
const REGION_MORPH_MARGIN_DP: f32 = 12.0;

const DEMO_INTERVAL_MS: u32 = 3_000;
/// Hover reconciliation while the pointer is over the pill. Catches the pill
/// shrinking out from under a stationary cursor, which produces no messages.
const POLL_HOVER_MS: u32 = 100;
/// Monitor re-check for the opt-in "follow cursor" mode.
const POLL_FOLLOW_MS: u32 = 500;
/// Content-only animation (media bars, dictation waveform) is capped at the
/// rate its data updates. Shape morphs and transitions run at full vblank.
const CONTENT_FRAME_INTERVAL: f64 = 1.0 / 30.0;
/// Accept a vblank this early relative to the content deadline, so 60 Hz
/// lands on every second vblank rather than every third.
const CONTENT_FRAME_SLACK: f64 = 0.004;

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowLayout {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    dpi: u32,
    anchor_top: bool,
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
    },
    Suspend(bool),
    Stop,
}

struct Demo {
    step: usize,
}

pub struct IslandHost {
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
    last_layout: Option<WindowLayout>,
    demo: Option<Demo>,
    poll_interval: Option<u32>,
    appbar: bool,
    generation: usize,
    /// Logical box the region must keep covering until the render thread
    /// confirms it has settled.
    morph_box: Option<Rect>,
    last_target: Option<LayoutTarget>,
    region: Option<WindowRegion>,
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
        let layout = compute_window_layout(hwnd, &config)?;
        apply_window_layout(hwnd, layout)?;

        let hit = Arc::new(Mutex::new(HitRegion::default()));
        let (tx, rx) = unbounded();
        let renderer_config = RendererConfig {
            hwnd,
            pixel_width: layout.width as u32,
            pixel_height: layout.height as u32,
            dpi: layout.dpi as f32,
            logical_width: WINDOW_LOGICAL_WIDTH,
            logical_height: WINDOW_LOGICAL_HEIGHT,
            anchor_top: layout.anchor_top,
        };
        let render_hit = Arc::clone(&hit);
        let notify = hwnd.0 as isize;
        let thread = std::thread::Builder::new()
            .name("islet-render".to_owned())
            .spawn(move || render_thread(renderer_config, render_hit, rx, notify))?;

        let mut host = Box::new(Self {
            hwnd,
            shared,
            state: IslandState {
                idle_style: config.general.idle_style,
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
            last_layout: Some(layout),
            demo: None,
            poll_interval: None,
            appbar: false,
            generation: 0,
            morph_box: None,
            last_target: None,
            region: None,
            warned_disconnected: false,
        });

        host.register_appbar();
        host.send_snapshot();
        host.update_poll_timer();
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
        Ok(host)
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn shutdown(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_HOVER_EXPAND);
            let _ = KillTimer(Some(self.hwnd), TIMER_HOVER_COLLAPSE);
            let _ = KillTimer(Some(self.hwnd), TIMER_DONE_EXPIRY);
            let _ = KillTimer(Some(self.hwnd), TIMER_POLL);
            let _ = KillTimer(Some(self.hwnd), TIMER_DEMO);
        }
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
        self.anchor_top = config.general.position == ScreenPosition::Top;
        self.hide_in_fullscreen = config.general.hide_in_fullscreen;
        self.follow_cursor = config.general.monitor == MonitorChoice::FollowCursor;
        self.idle_style = config.general.idle_style;
        self.dispatch(IslandEvent::SetIdleStyle(self.idle_style));
        if let Err(error) = self.reposition() {
            tracing::warn!(%error, "could not reposition island window");
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
            WM_NCHITTEST => Some(self.hit_test(lparam)),
            WM_MOUSEMOVE => {
                self.track_mouse();
                let inside = self.cursor_inside();
                self.dispatch(if inside {
                    IslandEvent::HoverEnter
                } else {
                    IslandEvent::HoverLeave
                });
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.tracking_mouse = false;
                self.dispatch(IslandEvent::HoverLeave);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.dispatch(IslandEvent::Click);
                Some(LRESULT(0))
            }
            WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_TIMER => {
                self.on_timer(wparam.0);
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_WINDOWPOSCHANGING => {
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
        let effects = reduce(&mut self.state, event, clock_now());
        let mut changed = false;
        for effect in effects {
            match effect {
                Effect::ArmTimer { kind, after_ms } => self.arm_timer(kind, after_ms),
                Effect::CancelTimer(kind) => self.cancel_timer(kind),
                Effect::LayoutChanged | Effect::ContentChanged => changed = true,
            }
        }
        if changed {
            self.send_snapshot();
        }
        self.update_poll_timer();
    }

    /// Runs the poll timer only while something needs it, so an idle island
    /// has no UI-thread wakeups.
    fn update_poll_timer(&mut self) {
        let wanted = if self.state.pointer_inside && !self.fullscreen_hidden {
            Some(POLL_HOVER_MS)
        } else if self.follow_cursor {
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
        if !self.hide_in_fullscreen || self.demo.is_some() {
            return;
        }
        // The shell also reports the desktop and some maximized windows as
        // full-screen, so confirm before hiding.
        let hide = opened && self.should_hide_for_fullscreen();
        self.set_fullscreen_hidden(hide);
    }

    fn send_snapshot(&mut self) {
        let target = self.state.layout();
        self.generation = self.generation.wrapping_add(1);
        // Grow the region before the render thread starts moving, so no
        // frame of the morph is clipped. It shrinks again on WM_APP_SETTLED.
        let previous = self.morph_box.or_else(|| {
            self.last_target
                .and_then(|last| self.target_frame(last).bounds())
        });
        self.morph_box = match (previous, self.target_frame(target).bounds()) {
            (Some(previous), Some(next)) => Some(previous.union(next)),
            (previous, next) => previous.or(next),
        };
        self.last_target = Some(target);
        self.update_region();
        let command = RenderCommand::Snapshot {
            target,
            content: Box::new(self.state.content()),
            anchor_top: self.anchor_top,
            generation: self.generation,
        };
        if self.tx.send(command).is_err() && !self.warned_disconnected {
            self.warned_disconnected = true;
            tracing::warn!("render thread is no longer accepting snapshots");
        }
    }

    fn target_frame(&self, target: LayoutTarget) -> islet_core::island::PillFrame {
        arrange(
            target.main,
            target.bubble,
            target.gap,
            WINDOW_LOGICAL_WIDTH,
            WINDOW_LOGICAL_HEIGHT,
            self.anchor_top,
        )
    }

    fn update_region(&mut self) {
        let scale = self.dpi as f32 / 96.0;
        let region = if let Some(morph) = self.morph_box {
            WindowRegion::Box(scale_rect(morph.inflate(REGION_MORPH_MARGIN_DP), scale))
        } else {
            let target = self.last_target.unwrap_or(LayoutTarget::HIDDEN);
            let frame = self.target_frame(target);
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
        if !self.fullscreen_hidden {
            let inside = self.cursor_inside();
            if inside != self.state.pointer_inside {
                self.dispatch(if inside {
                    IslandEvent::HoverEnter
                } else {
                    IslandEvent::HoverLeave
                });
            }
        }
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

    fn reposition(&mut self) -> WinResult<()> {
        let config = read_config(&self.shared);
        let layout = compute_window_layout(self.hwnd, &config)?;
        if self.last_layout == Some(layout) {
            return Ok(());
        }
        apply_window_layout(self.hwnd, layout)?;
        self.last_layout = Some(layout);
        self.dpi = layout.dpi;
        self.pixel_width = layout.width as u32;
        self.pixel_height = layout.height as u32;
        let _ = self.tx.send(RenderCommand::Resize {
            width: self.pixel_width,
            height: self.pixel_height,
            dpi: layout.dpi as f32,
        });
        self.region = None;
        self.update_region();
        Ok(())
    }

    fn set_fullscreen_hidden(&mut self, hidden: bool) {
        if hidden == self.fullscreen_hidden {
            return;
        }
        self.fullscreen_hidden = hidden;
        if hidden {
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
    }

    fn should_hide_for_fullscreen(&self) -> bool {
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
        let mut state = IslandState {
            idle_style: self.idle_style,
            ..IslandState::default()
        };
        match step % 8 {
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
            _ => unreachable!(),
        }
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
    rx: Receiver<RenderCommand>,
    notify: isize,
) {
    let notify = HWND(notify as *mut _);
    let mut renderer = match Renderer::new(config, Arc::clone(&hit), clock_now()) {
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
    let mut last_frame = f64::NEG_INFINITY;
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
            let morphing = dirty || renderer.is_morphing(now);
            if !dirty {
                wait_for_compositor_clock();
                // Content-only animation renders on the first vblank at or
                // after the 30 Hz deadline. Counting vblanks instead of
                // sleeping avoids the 15.6 ms system timer granularity.
                if !morphing
                    && clock_now() < last_frame + CONTENT_FRAME_INTERVAL - CONTENT_FRAME_SLACK
                {
                    continue;
                }
            }
            last_frame = clock_now();
            if let Err(error) = renderer.render(last_frame) {
                tracing::error!(%error, "island render failed");
                break;
            }
            if let Some((writer, last, count)) = &mut frame_log {
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
                // SAFETY: posting to a window owned by the UI thread is
                // non-blocking; a failure only delays the region shrink.
                unsafe {
                    let _ =
                        PostMessageW(Some(notify), WM_APP_SETTLED, WPARAM(generation), LPARAM(0));
                }
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
        RenderCommand::Snapshot {
            target,
            content,
            anchor_top,
            generation: snapshot_generation,
        } => {
            *generation = snapshot_generation;
            renderer.set_anchor(anchor_top);
            renderer.set_snapshot(target, *content, clock_now());
            *dirty = true;
            false
        }
        RenderCommand::Resize { width, height, dpi } => {
            if let Err(error) = renderer.resize(width, height, dpi) {
                tracing::error!(%error, "island renderer resize failed");
            }
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

fn read_config(shared: &Arc<RwLock<Config>>) -> Config {
    shared
        .read()
        .map(|config| config.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    POINT { x, y }
}

fn compute_window_layout(hwnd: HWND, config: &Config) -> WinResult<WindowLayout> {
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
    let height = (WINDOW_LOGICAL_HEIGHT * dpi as f32 / 96.0).round() as i32;
    let work = info.rcWork;
    let gap = (EDGE_GAP_DP * dpi as f32 / 96.0).round() as i32;
    let anchor_top = config.general.position == ScreenPosition::Top;
    let x = work.left + ((work.right - work.left) - width) / 2;
    let y = if anchor_top {
        work.top + gap
    } else {
        work.bottom - height - gap
    };

    let _ = hwnd;
    Ok(WindowLayout {
        x,
        y,
        width: width.max(1),
        height: height.max(1),
        dpi,
        anchor_top,
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
    MediaCard {
        title: "Midnight City".to_owned(),
        artist: "M83".to_owned(),
        progress: 0.42,
        playing: true,
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

fn demo_dictation() -> DictationCard {
    DictationCard {
        elapsed_ms: 7_400,
        level: 0.72,
        phase: DictationPhase::Recording,
    }
}
