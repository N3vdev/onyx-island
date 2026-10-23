//! A text box that takes keyboard focus and prints what ended up in it. Used
//! to test dictation end to end without typing into any other window: it
//! waits until text has appeared and stopped changing (or a timeout), then
//! prints the text and exits.
//!
//!   cargo run -p islet-input --example focus_box -- 25

use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, SetFocus, KEYEVENTF_KEYUP, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    PeekMessageW, SetForegroundWindow, TranslateMessage, ES_MULTILINE, MSG, PM_REMOVE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

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
    let timeout: u64 = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    // SAFETY: creates and drives a window on this thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE as u32),
            300,
            300,
            640,
            240,
            None,
            None,
            None,
            None,
        )
        .expect("create window")
    };
    unsafe {
        keybd_event(VK_MENU.0 as u8, 0, Default::default(), 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { GetForegroundWindow() } != hwnd {
        if Instant::now() > deadline {
            println!("SKIPPED: could not take keyboard focus");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("READY");

    let end = Instant::now() + Duration::from_secs(timeout);
    let mut last_text = String::new();
    let mut last_change = Instant::now();
    let mut message = MSG::default();
    while Instant::now() < end {
        // SAFETY: standard message pump on the window's thread.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let text = read_text(hwnd);
        if text != last_text {
            last_text = text;
            last_change = Instant::now();
        }
        if !last_text.is_empty() && last_change.elapsed() > Duration::from_millis(1500) {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    println!("TEXT: {:?}", last_text.replace("\r\n", "\n"));
}
