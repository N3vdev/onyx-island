//! Putting dictated text into the focused application.
//!
//! Paste can retain the dictated text or restore the previous clipboard.
//! Temporary paste text is excluded from history; retained text can appear in
//! local Windows clipboard history. Cloud sync remains disabled. Fallback: type the text
//! as Unicode key events, with the optional clipboard copy made after typing.
//!
//! Neither the text nor the clipboard contents are ever logged.

#![allow(unsafe_op_in_unsafe_fn)]

use std::fmt;
use std::time::{Duration, Instant};

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::EXTRA_INFO;

/// How long the target app gets to read the clipboard before it is restored.
const RESTORE_DELAY: Duration = Duration::from_millis(250);
/// How long to wait for the hotkey's modifiers to be physically released.
const MODIFIER_WAIT: Duration = Duration::from_millis(1_000);
/// Clipboard contents above this are not preserved (only text is put back).
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
const OPEN_ATTEMPTS: u32 = 20;
/// An unassigned virtual key; pressing it cancels the Start menu.
const VK_UNASSIGNED: u16 = 0xE8;
const VK_V: u16 = 0x56;
const UNICODE_BATCH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Clipboard + Ctrl+V.
    Clipboard,
    /// Type the text as Unicode key events.
    Unicode,
}

#[derive(Debug)]
pub enum InjectError {
    Clipboard(String),
    Input(String),
}

impl fmt::Display for InjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clipboard(message) => write!(f, "clipboard error: {message}"),
            Self::Input(message) => write!(f, "could not send keys: {message}"),
        }
    }
}

impl std::error::Error for InjectError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertReport {
    pub method: Method,
    /// The prior clipboard was restored or left unchanged by typing. False
    /// when dictation was retained or a newer user copy prevented restoration.
    pub clipboard_restored: bool,
    /// The final text was copied and retained for another paste.
    pub clipboard_saved: bool,
}

/// Inserts `text` into the focused window.
pub fn insert_text(text: &str, method: Method) -> Result<InsertReport, InjectError> {
    insert_text_with_clipboard(text, method, false)
}

/// Inserts text and optionally keeps it on the clipboard for both methods.
pub fn insert_text_with_clipboard(
    text: &str,
    method: Method,
    keep: bool,
) -> Result<InsertReport, InjectError> {
    if text.is_empty() {
        return Ok(InsertReport {
            method,
            clipboard_restored: true,
            clipboard_saved: false,
        });
    }
    // Ctrl+V while the hotkey's Ctrl/Win is still down would become a
    // different shortcut (Win+V is clipboard history), so wait for release.
    let released = wait_for_modifiers_released(MODIFIER_WAIT);
    let method = if released { method } else { Method::Unicode };
    match method {
        Method::Clipboard => paste_via_clipboard(text, keep),
        Method::Unicode => {
            type_unicode(text)?;
            let clipboard_saved = keep
                && match copy_to_clipboard(text) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(%error, "Text inserted, but clipboard copy failed");
                        false
                    }
                };
            Ok(InsertReport {
                method: Method::Unicode,
                clipboard_restored: !clipboard_saved,
                clipboard_saved,
            })
        }
    }
}

/// Sends a harmless key so releasing the Win key does not open Start.
pub fn suppress_start_menu() {
    let events = [
        key_event(VK_UNASSIGNED, false),
        key_event(VK_UNASSIGNED, true),
    ];
    // SAFETY: plain SendInput of two keyboard events.
    unsafe {
        SendInput(&events, std::mem::size_of::<INPUT>() as i32);
    }
}

fn modifier_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: reads global key state; no preconditions.
    unsafe { GetAsyncKeyState(i32::from(key.0)) as u16 & 0x8000 != 0 }
}

fn any_modifier_down() -> bool {
    [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(modifier_down)
}

fn wait_for_modifiers_released(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while any_modifier_down() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

fn key_event(vk: u16, up: bool) -> INPUT {
    // SAFETY: MapVirtualKeyW has no preconditions.
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: EXTRA_INFO,
            },
        },
    }
}

fn unicode_event(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: EXTRA_INFO,
            },
        },
    }
}

fn send(events: &[INPUT]) -> Result<(), InjectError> {
    // SAFETY: `events` is a valid slice of fully initialised INPUT structs.
    let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == events.len() {
        Ok(())
    } else {
        Err(InjectError::Input(format!(
            "only {sent} of {} key events were accepted (is the target window running as administrator?)",
            events.len()
        )))
    }
}

/// The key events that type `text`: Unicode packets, with line breaks sent
/// as Enter (a Unicode packet for `\n` is ignored by many editors).
fn typing_events(text: &str) -> Vec<INPUT> {
    let mut events = Vec::with_capacity(text.len() * 2);
    let mut units = [0u16; 2];
    for character in text.chars() {
        match character {
            '\r' => {}
            '\n' => {
                events.push(key_event(VK_RETURN.0, false));
                events.push(key_event(VK_RETURN.0, true));
            }
            other => {
                for unit in other.encode_utf16(&mut units) {
                    events.push(unicode_event(*unit, false));
                    events.push(unicode_event(*unit, true));
                }
            }
        }
    }
    events
}

fn type_unicode(text: &str) -> Result<(), InjectError> {
    let events = typing_events(text);
    for batch in events.chunks(UNICODE_BATCH * 2) {
        send(batch)?;
        // Give slow editors a moment between batches.
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

struct Clipboard;

impl Clipboard {
    /// Opens the clipboard, retrying while another program holds it.
    fn open() -> Result<Self, InjectError> {
        let mut last = None;
        for _ in 0..OPEN_ATTEMPTS {
            // SAFETY: associates the clipboard with the current task.
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(Self),
                Err(error) => {
                    last = Some(error);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        Err(InjectError::Clipboard(last.map_or_else(
            || "could not open".to_owned(),
            |error| error.message(),
        )))
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: closes the clipboard this guard opened.
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// One clipboard format's bytes.
struct SavedFormat {
    format: u32,
    bytes: Vec<u8>,
}

/// Formats whose handle is not global memory, or that Windows synthesises.
fn skip_format(format: u32) -> bool {
    matches!(format, 2 | 3 | 9 | 14 | 0x80 | 0x81..=0x8F | 0x300..=0x3FF)
}

/// Copies every global-memory clipboard format. The clipboard must be open.
unsafe fn snapshot() -> Vec<SavedFormat> {
    let mut saved = Vec::new();
    let mut total = 0usize;
    let mut format = EnumClipboardFormats(0);
    while format != 0 {
        if !skip_format(format) {
            if let Ok(handle) = GetClipboardData(format) {
                let global = HGLOBAL(handle.0);
                let size = GlobalSize(global);
                if size > 0 && total + size <= MAX_SNAPSHOT_BYTES {
                    let pointer = GlobalLock(global);
                    if !pointer.is_null() {
                        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
                        let _ = GlobalUnlock(global);
                        total += size;
                        saved.push(SavedFormat { format, bytes });
                    }
                }
            }
        }
        format = EnumClipboardFormats(format);
    }
    saved
}

/// Puts bytes on the clipboard as `format`. The clipboard must be open.
unsafe fn put(format: u32, bytes: &[u8]) -> Result<(), InjectError> {
    let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
        .map_err(|error| InjectError::Clipboard(error.message()))?;
    let pointer = GlobalLock(global);
    if pointer.is_null() {
        return Err(InjectError::Clipboard("could not lock memory".to_owned()));
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
    let _ = GlobalUnlock(global);
    // On success the clipboard owns the memory.
    SetClipboardData(format, Some(HANDLE(global.0)))
        .map(|_| ())
        .map_err(|error| InjectError::Clipboard(error.message()))
}

fn utf16_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Clipboard must be open. Retained copies follow local history preferences;
/// temporary paste data is excluded from monitoring entirely.
unsafe fn write_dictation_clipboard(text: &str, keep: bool) -> Result<(), InjectError> {
    EmptyClipboard().map_err(|error| InjectError::Clipboard(error.message()))?;
    put(u32::from(CF_UNICODETEXT.0), &utf16_bytes(text))?;
    if !keep {
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let _ = put(exclude, &1u32.to_le_bytes());
    }
    let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
    let cloud = RegisterClipboardFormatW(w!("CanUploadToCloudClipboard"));
    let _ = put(history, &u32::from(keep).to_le_bytes());
    let _ = put(cloud, &0u32.to_le_bytes());
    Ok(())
}

fn copy_to_clipboard(text: &str) -> Result<(), InjectError> {
    let _clipboard = Clipboard::open()?;
    // SAFETY: the clipboard guard holds it open for all writes.
    unsafe { write_dictation_clipboard(text, true) }
}

fn paste_via_clipboard(text: &str, keep: bool) -> Result<InsertReport, InjectError> {
    // SAFETY: every clipboard call below happens while the guard holds the
    // clipboard open; memory handed to the clipboard is not touched again.
    unsafe {
        let saved = {
            let _clipboard = Clipboard::open()?;
            let saved = if keep { Vec::new() } else { snapshot() };
            write_dictation_clipboard(text, keep)?;
            saved
        };
        let ours = GetClipboardSequenceNumber();

        send(&[
            key_event(VK_CONTROL.0, false),
            key_event(VK_V, false),
            key_event(VK_V, true),
            key_event(VK_CONTROL.0, true),
        ])?;
        if keep {
            return Ok(InsertReport {
                method: Method::Clipboard,
                clipboard_restored: false,
                clipboard_saved: true,
            });
        }
        std::thread::sleep(RESTORE_DELAY);

        // If the user copied something in the meantime, leave it alone.
        if GetClipboardSequenceNumber() != ours {
            return Ok(InsertReport {
                method: Method::Clipboard,
                clipboard_restored: false,
                clipboard_saved: false,
            });
        }
        let _clipboard = Clipboard::open()?;
        EmptyClipboard().map_err(|error| InjectError::Clipboard(error.message()))?;
        for item in &saved {
            // A format that cannot be put back is skipped; the rest still are.
            let _ = put(item.format, &item.bytes);
        }
        Ok(InsertReport {
            method: Method::Clipboard,
            clipboard_restored: true,
            clipboard_saved: false,
        })
    }
}

/// Whether the focused window runs elevated while Islet does not: Windows
/// silently drops input sent to such windows.
#[must_use]
pub fn foreground_is_blocked_by_elevation() -> bool {
    // SAFETY: read-only process queries; handles are closed.
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let ours = is_elevated(GetCurrentProcess());
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // Cannot even query it: almost certainly a higher-privilege process.
            return !ours;
        };
        let theirs = is_elevated(process);
        let _ = CloseHandle(process);
        theirs && !ours
    }
}

unsafe fn is_elevated(process: HANDLE) -> bool {
    let mut token = HANDLE::default();
    if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0u32;
    let result = GetTokenInformation(
        token,
        TokenElevation,
        Some(std::ptr::addr_of_mut!(elevation).cast()),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
        &mut length,
    );
    let _ = CloseHandle(token);
    result.is_ok() && elevation.TokenIsElevated != 0
}

/// Lowercase executable name of the focused window ("notepad.exe"), for
/// per-app insertion overrides.
#[must_use]
pub fn foreground_exe() -> Option<String> {
    // SAFETY: read-only process query; the handle is closed.
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_ascii_lowercase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_of(event: &INPUT) -> (u16, u16, bool) {
        // SAFETY: every event built here is a keyboard event.
        let key = unsafe { event.Anonymous.ki };
        (key.wVk.0, key.wScan, key.dwFlags.0 & KEYEVENTF_KEYUP.0 != 0)
    }

    #[test]
    fn typing_events_are_down_up_pairs_with_our_marker() {
        let events = typing_events("Hi");
        assert_eq!(events.len(), 4);
        assert_eq!(unit_of(&events[0]), (0, u16::from(b'H'), false));
        assert_eq!(unit_of(&events[1]), (0, u16::from(b'H'), true));
        assert_eq!(unit_of(&events[2]), (0, u16::from(b'i'), false));
        for event in &events {
            // SAFETY: keyboard events.
            assert_eq!(unsafe { event.Anonymous.ki.dwExtraInfo }, EXTRA_INFO);
        }
    }

    #[test]
    fn newlines_become_enter_and_carriage_returns_vanish() {
        let events = typing_events("a\r\nb");
        let vks: Vec<u16> = events.iter().map(|e| unit_of(e).0).collect();
        assert_eq!(vks, [0, 0, VK_RETURN.0, VK_RETURN.0, 0, 0]);
    }

    #[test]
    fn characters_outside_the_bmp_use_surrogate_pairs() {
        let events = typing_events("😀");
        assert_eq!(events.len(), 4);
        let scans: Vec<u16> = events.iter().map(|e| unit_of(e).1).collect();
        assert_eq!(scans, [0xD83D, 0xD83D, 0xDE00, 0xDE00]);
    }

    #[test]
    fn utf16_clipboard_bytes_are_nul_terminated() {
        assert_eq!(utf16_bytes("a"), [b'a', 0, 0, 0]);
        assert_eq!(utf16_bytes(""), [0, 0]);
    }

    #[test]
    fn gdi_and_synthesised_formats_are_skipped() {
        for format in [2, 3, 9, 14, 0x80, 0x82, 0x300, 0x3FF] {
            assert!(skip_format(format), "{format:#x}");
        }
        for format in [1, 13, 15, 49_000, 0x200] {
            assert!(!skip_format(format), "{format:#x}");
        }
    }
}
