//! Onyx Lens: press a shortcut, then circle anything on screen to copy its
//! text, translate it, search it or ask AI about it.
//!
//! The lens itself is a helper process (`onyx-lens.exe`, built from `lens/`
//! in this repository) with a WebView overlay that is created hidden at
//! start, so a press shows it in well under 100 ms. Onyx owns the shortcut
//! (`RegisterHotKey` on the island window) and signals the helper through a
//! named event; the helper sleeps on it and costs no CPU while idle. The
//! helper is in Onyx's kill-on-close job, so it ends with Onyx however Onyx
//! ends.
//!
//! Onyx creates the events itself, so a press made while the helper is still
//! starting stays signalled and is picked up as soon as it is ready.
//!
//! While the overlay is open the island is the lens's toolbar: the lens posts
//! [`tools_message`] to the island window with its tools (see
//! `islet_core::lens`), and a click on the island goes back to the lens's
//! listener thread as `tool_message`. A press hides the island from screen
//! capture for a moment, so the lens's frozen frame does not show the pill
//! where it was before it moves.

use std::process::{Child, Command};
use std::sync::OnceLock;

use islet_core::config::LensConfig;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    CreateEventW, GetProcessIdOfThread, OpenThread, ResetEvent, SetEvent, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, PostThreadMessageW, RegisterWindowMessageW,
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

/// The island window's `WM_HOTKEY` id for the lens shortcut.
pub const HOTKEY_ID: i32 = 0x4C45;

/// Lens to island: `wparam` is `islet_core::lens::pack`, `lparam` the
/// lens's listener thread.
pub fn tools_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: registers a message name.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Onyx.Lens.Tools")) })
}

/// Island to lens: `wparam` is the clicked item's index.
fn tool_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: as above.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Onyx.Lens.Tool")) })
}

const CAPTURE_EVENT: PCWSTR = w!("Local\\Onyx.Lens.Capture");
const SETTINGS_EVENT: PCWSTR = w!("Local\\Onyx.Lens.Settings");

pub struct Lens {
    hwnd: HWND,
    child: Option<Child>,
    /// The shortcut registered now, as written in the config.
    registered: Option<String>,
    capture: Option<HANDLE>,
    settings: Option<HANDLE>,
    /// Set once the helper could not be found, so it is not searched for on
    /// every press.
    missing: bool,
    /// The lens's listener thread, from its last report.
    thread: u32,
    /// The island is hidden from screen capture for a press.
    hidden_from_capture: bool,
}

impl Lens {
    pub fn new(hwnd: HWND) -> Self {
        // SAFETY: plain named-event creation (auto-reset, not signalled).
        let event = |name| unsafe { CreateEventW(None, false, false, name) }.ok();
        Self {
            hwnd,
            child: None,
            registered: None,
            capture: event(CAPTURE_EVENT),
            settings: event(SETTINGS_EVENT),
            missing: false,
            thread: 0,
            hidden_from_capture: false,
        }
    }

    /// Starts or stops the helper and (re)registers the shortcut.
    pub fn configure(&mut self, config: &LensConfig) {
        if !config.enabled {
            self.unregister();
            self.stop();
            return;
        }
        self.ensure_running();
        if self.registered.as_deref() != Some(config.hotkey.as_str()) {
            self.unregister();
            self.register(&config.hotkey);
        }
    }

    /// The shortcut was pressed.
    pub fn on_hotkey(&mut self) {
        let Some(pid) = self.ensure_running() else {
            return;
        };
        // The press gave Onyx the right to bring a window forward; pass it
        // on so the overlay can take focus.
        // SAFETY: plain Win32 calls with a live process id and event, on
        // the island's own window from its thread.
        unsafe {
            let _ = AllowSetForegroundWindow(pid);
            // Before the lens captures: its frozen frame shows what is
            // behind the pill, which is about to move.
            self.hidden_from_capture =
                SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok();
            if let Some(event) = self.capture {
                let _ = SetEvent(event);
            }
        }
    }

    /// Whether the helper is running now.
    pub fn running(&mut self) -> bool {
        self.child.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    /// Whether `thread` is one of the helper's own: reports from any other
    /// process are not the lens's.
    pub fn owns_thread(&self, thread: u32) -> bool {
        let Some(child) = &self.child else {
            return false;
        };
        // SAFETY: a query-only handle, closed before returning.
        unsafe {
            let Ok(handle) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread) else {
                return false;
            };
            let pid = GetProcessIdOfThread(handle);
            let _ = CloseHandle(handle);
            pid != 0 && pid == child.id()
        }
    }

    /// The lens reported its tools (or closed). The capture is done by now.
    pub fn on_report(&mut self, thread: u32) {
        self.thread = thread;
        self.show_to_capture();
    }

    /// Ends the press's capture hiding; also run on a timer, in case the
    /// lens never reports (its overlay opened on another screen).
    pub fn show_to_capture(&mut self) {
        if std::mem::take(&mut self.hidden_from_capture) {
            // SAFETY: the island's own window, from its thread.
            unsafe {
                let _ = SetWindowDisplayAffinity(self.hwnd, WDA_NONE);
            }
        }
    }

    /// A button on the island's lens toolbar was clicked.
    pub fn send_tool(&self, index: u8) {
        if self.thread == 0 {
            return;
        }
        // SAFETY: posts to a thread id the lens gave us; a stale one fails.
        unsafe {
            let _ = PostThreadMessageW(self.thread, tool_message(), WPARAM(usize::from(index)), LPARAM(0));
        }
    }

    fn register(&mut self, hotkey: &str) {
        if hotkey.is_empty() {
            return;
        }
        let (modifiers, key) = match islet_core::hotkey::system_shortcut(hotkey) {
            Ok(shortcut) => shortcut,
            Err(error) => {
                tracing::warn!(%error, "the lens shortcut is not usable");
                return;
            }
        };
        // SAFETY: registers on the island window, on the thread that owns it.
        let result = unsafe {
            RegisterHotKey(
                Some(self.hwnd),
                HOTKEY_ID,
                HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT,
                u32::from(key),
            )
        };
        match result {
            Ok(()) => self.registered = Some(hotkey.to_owned()),
            Err(error) => {
                tracing::warn!(%error, "the lens shortcut is taken by another app")
            }
        }
    }

    fn unregister(&mut self) {
        if self.registered.take().is_some() {
            // SAFETY: as above.
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_ID);
            }
        }
    }

    /// The helper's process id, starting it if it is not running.
    fn ensure_running(&mut self) -> Option<u32> {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => return Some(child.id()),
                _ => self.child = None,
            }
        }
        if self.missing {
            return None;
        }
        let exe = std::env::current_exe()
            .ok()?
            .with_file_name("onyx-lens.exe");
        if !exe.exists() {
            tracing::warn!("onyx-lens.exe is not installed next to Onyx");
            self.missing = true;
            return None;
        }
        match Command::new(&exe).spawn() {
            Ok(child) => {
                islet_asr::tie_to_app(&child);
                let pid = child.id();
                self.child = Some(child);
                Some(pid)
            }
            Err(error) => {
                tracing::warn!(%error, "could not start Onyx Lens");
                None
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // A press made just before it stopped must not fire on the next start.
        // SAFETY: valid event handles owned by this struct.
        unsafe {
            for event in [self.capture, self.settings].into_iter().flatten() {
                let _ = ResetEvent(event);
            }
        }
    }
}

impl Drop for Lens {
    fn drop(&mut self) {
        self.unregister();
        self.stop();
        // SAFETY: closes handles this struct owns, once.
        unsafe {
            for event in [self.capture.take(), self.settings.take()].into_iter().flatten() {
                let _ = CloseHandle(event);
            }
        }
    }
}
