//! Inserts text into a text box this program creates itself, using both
//! methods, and checks the result. It refuses to send anything unless its own
//! window really has the keyboard focus, so keys can never land elsewhere.
//!
//! Set the clipboard to a known value first to see it restored afterwards:
//!   Set-Clipboard KEEP-ME; cargo run -p islet-input --example inject_probe; Get-Clipboard

use std::time::{Duration, Instant};

use islet_input::{insert_text, Method};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
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
        let first = insert_text(TEXT_A, Method::Clipboard);
        let second = insert_text(TEXT_B, Method::Unicode);
        // SAFETY: posts to our own window.
        unsafe {
            let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_APP, WPARAM(0), LPARAM(0));
        }
        (first, second)
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
    let (first, second) = worker.join().expect("worker");
    println!("clipboard method: {first:?}");
    println!("unicode method:   {second:?}");

    let got = read_text(hwnd).replace("\r\n", "\n");
    let expected = format!("{TEXT_A}{TEXT_B}");
    println!("text in box matches: {}", got == expected);
    if got != expected {
        println!("expected: {expected:?}\n     got: {got:?}");
    }
}
