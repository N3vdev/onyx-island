//! Knowing when an app window sits behind the island, so it can dock away.
//!
//! Event driven: WinEvent hooks (foreground, move/size, minimize, show/hide,
//! cloak, location) post a message to the island window, which debounces and
//! then calls [`window_behind`]. Nothing runs while windows stay put: no
//! polling, no timers between changes.

use std::sync::atomic::{AtomicIsize, Ordering};

use islet_core::Rect;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetLayeredWindowAttributes, GetWindowLongW,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW,
    EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
    EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MOVESIZEEND, GA_ROOT, GWL_EXSTYLE,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

/// Window that receives `message` whenever a top-level window changes.
static NOTIFY: AtomicIsize = AtomicIsize::new(0);
static MESSAGE: AtomicIsize = AtomicIsize::new(0);

/// Installed hooks; dropping unhooks them.
pub struct WindowEvents(Vec<HWINEVENTHOOK>);

impl WindowEvents {
    /// Must be called on the thread that owns `notify` (out-of-context
    /// WinEvent callbacks run on the installing thread's message loop).
    pub fn install(notify: HWND, message: u32) -> Self {
        NOTIFY.store(notify.0 as isize, Ordering::Release);
        MESSAGE.store(message as isize, Ordering::Release);
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];
        let hooks = ranges
            .iter()
            .filter_map(|&(min, max)| {
                // SAFETY: out-of-context hook with a static callback.
                let hook = unsafe {
                    SetWinEventHook(
                        min,
                        max,
                        None,
                        Some(on_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };
                (!hook.is_invalid()).then_some(hook)
            })
            .collect::<Vec<_>>();
        if hooks.len() < ranges.len() {
            tracing::warn!("some window events are unavailable; slim mode may lag");
        }
        Self(hooks)
    }
}

impl Drop for WindowEvents {
    fn drop(&mut self) {
        for hook in self.0.drain(..) {
            // SAFETY: each hook was returned by SetWinEventHook.
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
        NOTIFY.store(0, Ordering::Release);
    }
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 || child != 0 || hwnd.is_invalid() {
        return;
    }
    // Only top-level windows matter. A destroyed window can no longer be
    // inspected, so let its event through.
    if event != EVENT_OBJECT_DESTROY && GetAncestor(hwnd, GA_ROOT) != hwnd {
        return;
    }
    let notify = NOTIFY.load(Ordering::Acquire);
    if notify != 0 {
        let message = MESSAGE.load(Ordering::Acquire) as u32;
        let _ = PostMessageW(Some(HWND(notify as *mut _)), message, WPARAM(0), LPARAM(0));
    }
}

/// Whether a real app window overlaps `zone` (physical screen pixels).
/// Ignores this process, the desktop and taskbar, minimised, cloaked
/// (other virtual desktop), click-through, and fully transparent overlay
/// windows.
pub fn window_behind(zone: Rect) -> bool {
    struct Search {
        zone: Rect,
        own_pid: u32,
        found: bool,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        if counts(hwnd, search.own_pid) && overlaps(hwnd, search.zone) {
            search.found = true;
            return false.into();
        }
        true.into()
    }

    let mut search = Search {
        zone,
        own_pid: std::process::id(),
        found: false,
    };
    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(std::ptr::addr_of_mut!(search) as isize));
    }
    search.found
}

unsafe fn counts(hwnd: HWND, own_pid: u32) -> bool {
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == own_pid {
        return false;
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        std::ptr::addr_of_mut!(cloaked).cast(),
        std::mem::size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
    {
        return false;
    }
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    let mut class = [0u16; 64];
    let length = GetClassNameW(hwnd, &mut class) as usize;
    let class = String::from_utf16_lossy(&class[..length]);
    is_drawn_window(ex_style, &class, layered_alpha(hwnd, ex_style))
}

/// Decides whether a window's styles and class make it a real, drawn app
/// window. Kept pure so the rules can be tested without a desktop.
///
/// Overlays such as the NVIDIA GeForce Overlay are reported as visible by the
/// OS while they are in fact fully transparent, which would otherwise pin the
/// island to the bottom edge even on a bare desktop.
fn is_drawn_window(ex_style: u32, class: &str, layered_alpha: Option<u8>) -> bool {
    if ex_style & WS_EX_TRANSPARENT.0 != 0 {
        return false;
    }
    if layered_alpha == Some(0) {
        return false;
    }
    !matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

/// The alpha of a layered window when it is set through
/// `SetLayeredWindowAttributes`. Per-pixel layered windows report `None` and
/// are treated as drawn, since their transparency cannot be read cheaply.
unsafe fn layered_alpha(hwnd: HWND, ex_style: u32) -> Option<u8> {
    if ex_style & WS_EX_LAYERED.0 == 0 {
        return None;
    }
    let mut key = COLORREF(0);
    let mut alpha = 0u8;
    let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
    // SAFETY: the three out-pointers are valid for the call.
    if GetLayeredWindowAttributes(hwnd, Some(&mut key), Some(&mut alpha), Some(&mut flags)).is_err()
    {
        return None;
    }
    (flags.0 & LWA_ALPHA.0 != 0).then_some(alpha)
}

unsafe fn overlaps(hwnd: HWND, zone: Rect) -> bool {
    // The extended frame excludes the invisible resize borders, which
    // would otherwise make a window ending just above the island count.
    let mut bounds = RECT::default();
    let measured = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        std::ptr::addr_of_mut!(bounds).cast(),
        std::mem::size_of::<RECT>() as u32,
    )
    .is_ok()
        || GetWindowRect(hwnd, &mut bounds).is_ok();
    if !measured {
        return false;
    }
    let window = Rect::new(
        bounds.left as f32,
        bounds.top as f32,
        (bounds.right - bounds.left) as f32,
        (bounds.bottom - bounds.top) as f32,
    );
    intersects(window, zone)
}

fn intersects(a: Rect, b: Rect) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && a.x < b.right()
        && b.x < a.right()
        && a.y < b.bottom()
        && b.y < a.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_is_strict() {
        let zone = Rect::new(100.0, 1000.0, 220.0, 44.0);
        assert!(intersects(Rect::new(0.0, 0.0, 1920.0, 1080.0), zone));
        // Ends exactly at the zone's top edge: no overlap.
        assert!(!intersects(Rect::new(0.0, 0.0, 1920.0, 1000.0), zone));
        assert!(!intersects(Rect::new(400.0, 900.0, 100.0, 200.0), zone));
        assert!(!intersects(Rect::new(0.0, 0.0, 0.0, 0.0), zone));
    }

    #[test]
    fn real_windows_count() {
        assert!(is_drawn_window(0, "MozillaWindowClass", None));
        assert!(is_drawn_window(
            WS_EX_LAYERED.0,
            "CEF-OSC-WIDGET",
            Some(255)
        ));
        // Per-pixel layered windows cannot be read; assume drawn.
        assert!(is_drawn_window(WS_EX_LAYERED.0, "SomeOverlay", None));
    }

    #[test]
    fn invisible_windows_do_not_count() {
        // The NVIDIA GeForce Overlay: "visible" to the OS, alpha 0.
        assert!(!is_drawn_window(WS_EX_LAYERED.0, "CEF-OSC-WIDGET", Some(0)));
        // Click-through overlays.
        assert!(!is_drawn_window(WS_EX_TRANSPARENT.0, "SomeOverlay", None));
        // Shell surfaces: desktop and taskbars.
        assert!(!is_drawn_window(0, "Progman", None));
        assert!(!is_drawn_window(0, "WorkerW", None));
        assert!(!is_drawn_window(0, "Shell_TrayWnd", None));
        assert!(!is_drawn_window(0, "Shell_SecondaryTrayWnd", None));
    }
}
