//! Onyx's island as this overlay's toolbar.
//!
//! When the overlay opens on the screen the island is on, the island moves
//! to the top and shows the overlay's tools instead of the overlay's own
//! toolbar (hidden then). The overlay reports its tools through
//! `island_tools`; this posts them to the island window. A click there comes
//! back to the trigger thread (`hotkey.rs`) as a thread message, and on to
//! the overlay as the `island-tool` event.
//!
//! The report is one message parameter, matching Onyx's
//! `islet_core::lens::pack`: bits 0-7 the visible tools, 8-15 the pressed
//! ones, bit 16 open. `lparam` is the trigger thread for clicks to come back.

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetPropW, IsWindowVisible, PostMessageW, RegisterWindowMessageW,
};

/// The island window for this session; 0 while the overlay keeps its own
/// toolbar (or is closed).
static ISLAND: AtomicIsize = AtomicIsize::new(0);
/// The trigger thread, which receives the island's clicks.
static THREAD: AtomicU32 = AtomicU32::new(0);

fn tools_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: registers a message name.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Onyx.Lens.Tools")) })
}

/// The island's click: `wparam` is the tool's index (0-7), or 8 for close.
pub fn tool_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: as above.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Onyx.Lens.Tool")) })
}

pub fn set_thread(id: u32) {
    THREAD.store(id, Ordering::Relaxed);
}

/// Called with the captured monitor before the overlay shows. True when the
/// island is showing on that screen and will be the toolbar. Onyx marks the
/// island (`OnyxLensToolbar`) when the shortcut is pressed if it can be: not
/// while it sits in Onyx Settings' live preview, where tools would land
/// under the frozen screen and the overlay would be left with none.
pub fn begin(monitor: isize) -> bool {
    ISLAND.store(0, Ordering::Relaxed);
    if THREAD.load(Ordering::Relaxed) == 0 {
        return false;
    }
    // SAFETY: plain window lookups.
    let island = unsafe {
        let Ok(island) = FindWindowW(w!("IsletIslandWindow"), None) else {
            return false;
        };
        let same_screen = MonitorFromWindow(island, MONITOR_DEFAULTTONEAREST)
            == HMONITOR(monitor as *mut core::ffi::c_void);
        let offered = !GetPropW(island, w!("OnyxLensToolbar")).0.is_null();
        if !IsWindowVisible(island).as_bool() || !same_screen || !offered {
            return false;
        }
        island
    };
    ISLAND.store(island.0 as isize, Ordering::Relaxed);
    true
}

fn post(open: bool, visible: u8, pressed: u8) {
    let island = ISLAND.load(Ordering::Relaxed);
    if island == 0 {
        return;
    }
    let value = usize::from(visible) | usize::from(pressed) << 8 | usize::from(open) << 16;
    let thread = THREAD.load(Ordering::Relaxed);
    // SAFETY: posts to a window that may have gone; that just fails.
    unsafe {
        let _ = PostMessageW(
            HWND(island as *mut core::ffi::c_void),
            tools_message(),
            WPARAM(value),
            LPARAM(thread as isize),
        );
    }
}

/// The overlay's tools changed (or it opened).
#[tauri::command]
pub fn island_tools(visible: u8, pressed: u8) {
    post(true, visible, pressed);
}

/// The overlay closed: the island goes home.
pub fn end() {
    post(false, 0, 0);
    ISLAND.store(0, Ordering::Relaxed);
}

/// How Onyx's island looks, so the overlay's bars match it: black, or
/// glass (a dark tint over a blur of what is behind).
#[derive(Clone, Copy, serde::Serialize)]
pub struct Look {
    pub glass: bool,
    /// The glass's tint, 0 (clear) to 1 (black).
    pub tint: f32,
    /// The glass is frosted: Windows' transparency effects are on (with
    /// them off, Onyx's glass is clear, and so is this).
    pub frost: bool,
}

/// Read from Onyx's config (`[general] look`, `glass_tint`) on each capture,
/// so a change there shows the next time.
pub fn look() -> Look {
    let mut look = Look { glass: false, tint: 0.45, frost: true };
    let Some(config) = std::env::var_os("APPDATA")
        .map(|dir| std::path::PathBuf::from(dir).join("Islet").join("config.toml"))
        .and_then(|path| std::fs::read_to_string(path).ok())
    else {
        return look;
    };
    let mut general = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            general = line == "[general]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| general) else { continue };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "look" => look.glass = value == "glass",
            "glass_tint" => look.tint = value.parse::<f32>().map_or(look.tint, |tint| tint.clamp(0.0, 1.0)),
            _ => {}
        }
    }
    look.frost = transparency_on();
    look
}

fn transparency_on() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into a local of that size.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    read.is_err() || value != 0
}
