//! Inserts text into a text box this program creates itself, using both
//! methods, and checks the result. It refuses to send anything unless its own
//! window really has the keyboard focus, so keys can never land elsewhere.
//!
//! Set the clipboard to a known value first to see it restored afterwards:
//!   Set-Clipboard KEEP-ME; cargo run -p islet-input --example inject_probe; Get-Clipboard

use std::time::{Duration, Instant};

use islet_input::{insert_text_with_clipboard, Method};
use windows::core::w;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, SetFocus, KEYEVENTF_KEYUP, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowTextLengthW,
    GetWindowTextW, PeekMessageW, PostMessageW, SetForegroundWindow, TranslateMessage,
    ES_MULTILINE, MSG, PM_REMOVE, WINDOW_EX_STYLE, WM_APP, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

const TEXT_A: &str = "Hello from Islet \u{2014} caf\u{e9} \u{4f60}\u{597d} \u{1f600}\nsecond line";
const TEXT_B: &str = " typed";

fn read_text(hwnd: HWND) -> String {
    // SAFETY: reads the text of our own window.
    unsafe {
        let length = GetWindowTextLengthW(hwnd) as usize;
        let mut buffer = vec![0u16; length + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer) as usize;
        String::from_utf16_lossy(&buffer[..copied])
    }
}

fn main() {
    let keep_clipboard = std::env::args().any(|arg| arg == "--keep-clipboard");
    let text_b = std::env::args()
        .skip_while(|arg| arg != "--history-marker")
        .nth(1)
        .map_or_else(|| TEXT_B.to_owned(), |marker| format!("{TEXT_B} {marker}"));
    let expected = format!("{TEXT_A}{text_b}");
    // SAFETY: creates and drives a window on this thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_OVERLAPPEDWINDOW
                | WS_VISIBLE
                | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_MULTILINE as u32),
            200,
            200,
            560,
            220,
            None,
            None,
            None,
            None,
        )
        .expect("create window")
    };

    // Windows only lets the foreground process hand focus over; a synthetic
    // Alt press is the usual way to be allowed to take it.
    unsafe {
        keybd_event(VK_MENU.0 as u8, 0, Default::default(), 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { GetForegroundWindow() } != hwnd {
        if Instant::now() > deadline {
            println!("SKIPPED: could not take keyboard focus, so nothing was sent");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let notify = hwnd.0 as isize;
    let worker = std::thread::spawn(move || {
        // Only send while our window is still the foreground one.
        let first = if unsafe { GetForegroundWindow() } == HWND(notify as *mut _) {
            insert_text_with_clipboard(TEXT_A, Method::Clipboard, keep_clipboard)
        } else {
            Err(islet_input::InjectError::Input("Probe lost focus".into()))
        };
        let copied_first = !keep_clipboard || clipboard_matches(TEXT_A);
        // Let the edit control consume the paste before a second dictation
        // intentionally replaces the retained clipboard contents.
        std::thread::sleep(Duration::from_millis(250));
        let second = if unsafe { GetForegroundWindow() } == HWND(notify as *mut _) {
            insert_text_with_clipboard(&text_b, Method::Unicode, keep_clipboard)
        } else {
            Err(islet_input::InjectError::Input("Probe lost focus".into()))
        };
        let copied_second = !keep_clipboard || clipboard_matches(&text_b);
        // SAFETY: posts to our own window.
        unsafe {
            let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_APP, WPARAM(0), LPARAM(0));
        }
        (first, second, copied_first, copied_second)
    });

    let mut message = MSG::default();
    // SAFETY: standard message loop on the window's thread.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.message == WM_APP {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // Posted messages overtake queued keystrokes, so keep pumping for a moment
    // to let the typed keys reach the box before reading it.
    let settle = Instant::now() + Duration::from_millis(500);
    // SAFETY: same thread as the window.
    unsafe {
        while Instant::now() < settle {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let (first, second, copied_first, copied_second) = worker.join().expect("worker");
    println!("clipboard method: {first:?}");
    println!("unicode method:   {second:?}");

    let got = read_text(hwnd).replace("\r\n", "\n");
    println!("text in box matches: {}", got == expected);
    assert!(first.is_ok() && second.is_ok());
    assert_eq!(first.unwrap().clipboard_saved, keep_clipboard);
    assert_eq!(second.unwrap().clipboard_saved, keep_clipboard);
    assert!(copied_first && copied_second);
    assert!(got == expected);
    println!("clipboard retention checks passed: keep={keep_clipboard}");
}

fn clipboard_matches(expected: &str) -> bool {
    // Read only for the synthetic retention probe; never print clipboard text.
    unsafe {
        let mut opened = false;
        for _ in 0..20 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !opened {
            return false;
        }
        let result = (|| {
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global) / 2;
            let pointer = GlobalLock(global).cast::<u16>();
            if pointer.is_null() {
                return None;
            }
            let units = std::slice::from_raw_parts(pointer, size);
            let end = units.iter().position(|u| *u == 0).unwrap_or(size);
            let matches = String::from_utf16_lossy(&units[..end]) == expected;
            let _ = GlobalUnlock(global);
            let exclude =
                RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
            let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
            let cloud = RegisterClipboardFormatW(w!("CanUploadToCloudClipboard"));
            Some(
                matches
                    && GetClipboardData(exclude).is_err()
                    && clipboard_flag(history) == Some(1)
                    && clipboard_flag(cloud) == Some(0),
            )
        })()
        .unwrap_or(false);
        let _ = CloseClipboard();
        result
    }
}

unsafe fn clipboard_flag(format: u32) -> Option<u32> {
    let handle = unsafe { GetClipboardData(format) }.ok()?;
    let global = HGLOBAL(handle.0);
    if unsafe { GlobalSize(global) } < 4 {
        return None;
    }
    let pointer = unsafe { GlobalLock(global) }.cast::<u32>();
    if pointer.is_null() {
        return None;
    }
    let value = unsafe { pointer.read_unaligned() };
    let _ = unsafe { GlobalUnlock(global) };
    Some(value)
}
