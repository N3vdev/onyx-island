//! "Image to AI" opens the AI provider as a small chromeless window —
//! Chromium's own `--app=<url>` mode — rather than either a full browser tab
//! or an app-owned WebView2 window. The distinction that actually matters:
//! `--app=` still runs inside the *default browser's own real profile*, so
//! an already logged-in ChatGPT/Claude/Gemini session carries over exactly
//! as it would in a normal tab — unlike a Tauri-owned window, which would
//! start from a separate, empty WebView2 profile and force a fresh login
//! every time. Only Chromium-family browsers understand `--app=`. Firefox
//! has no real equivalent — its own `-app` flag launches a packaged Gecko
//! application from a manifest file, not an arbitrary URL without browser
//! chrome, so a true chromeless window isn't achievable there. Firefox
//! instead gets `-new-window` plus a best-effort resize down to a small
//! window afterward — full Firefox chrome (tabs, address bar) still shows,
//! since that part genuinely cannot be stripped from outside the browser,
//! but at least the window itself is small rather than full-size. Finding
//! *which* window to resize is inherently a little unreliable: Firefox is
//! single-instance, so if it was already running, `-new-window` is just
//! forwarded to that existing process over IPC and the process this app
//! actually spawned exits immediately — there's no PID to hold onto. The
//! best available signal is a before/after diff of top-level windows
//! belonging to any process named firefox.exe. Anything else falls back to
//! a normal tab in the default browser.
//!
//! Once that URL is handed off, this app has no handle to whatever window
//! opens, no way to know when its page finishes loading, and no way to run
//! script inside it — none of the tricks an app-owned window would allow.
//! All that's left for the paste is a timed guess, followed by a real
//! OS-level Ctrl+V once the now-foreground window's title looks like the
//! right one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
};
use windows::Win32::UI::Shell::{AssocQueryStringW, ShellExecuteW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetWindowPos,
    SWP_NOZORDER, SW_SHOWNORMAL,
};

/// However long the OS takes to bring the new window to the foreground and
/// load the provider's page — slower and far less predictable than the
/// app's own local pages would be, since this can involve a real process
/// launch plus a real network fetch. A guess, same trade as everything else
/// in this module.
const PASTE_DELAY_MS: u64 = 2200;

/// Size the Firefox window gets resized down to once found. Firefox's own
/// chrome (tabs, address bar, bookmarks) eats more of this than a chromeless
/// Chromium app-mode window would, so a bit roomier than that one.
const FIREFOX_WINDOW_SIZE: (i32, i32) = (760, 800);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens `url` as a small chromeless window when the default browser
/// supports it, a dedicated (but normally-chromed) window for Firefox, or a
/// normal tab otherwise — see the module doc for why this matters more than
/// it sounds like it should (the login).
#[tauri::command]
pub fn open_ai_provider(url: String) -> Result<(), String> {
    if let Some(exe) = default_browser_executable() {
        if is_chromium_browser(&exe) {
            match std::process::Command::new(&exe).arg(format!("--app={url}")).spawn() {
                Ok(_) => return Ok(()),
                Err(err) => {
                    tracing::warn!(%err, browser = %exe.display(), "could not launch app-mode window; opening a normal tab instead");
                }
            }
        } else if is_firefox_browser(&exe) {
            let before = firefox_window_handles();
            match std::process::Command::new(&exe).arg("-new-window").arg(&url).spawn() {
                Ok(_) => {
                    std::thread::spawn(move || shrink_new_firefox_window(before));
                    return Ok(());
                }
                Err(err) => {
                    tracing::warn!(%err, browser = %exe.display(), "could not launch a new Firefox window; opening a normal tab instead");
                }
            }
        }
    }
    shell_open(&url)
}

/// Falls back to whatever `openUrl` (the frontend's usual opener) would have
/// done anyway — the same "ask the shell to open this" call, just made from
/// Rust since it's already here resolving the default browser for the
/// app-mode path above.
fn shell_open(url: &str) -> Result<(), String> {
    let operation = wide("open");
    let target = wide(url);
    let result = unsafe {
        ShellExecuteW(HWND(std::ptr::null_mut()), PCWSTR(operation.as_ptr()), PCWSTR(target.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    // A legacy HINSTANCE repurposed as a status code: > 32 means success.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("Could not open {url} (error {})", result.0 as isize))
    }
}

fn is_chromium_browser(exe: &Path) -> bool {
    const KNOWN: &[&str] = &["msedge.exe", "chrome.exe", "brave.exe", "vivaldi.exe", "opera.exe"];
    exe.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| KNOWN.iter().any(|known| name.eq_ignore_ascii_case(known)))
}

fn is_firefox_browser(exe: &Path) -> bool {
    const KNOWN: &[&str] = &["firefox.exe", "waterfox.exe", "librewolf.exe"];
    exe.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| KNOWN.iter().any(|known| name.eq_ignore_ascii_case(known)))
}

/// Polls for a visible, titled top-level window owned by a firefox.exe
/// process that wasn't in `before`, then shrinks it. Run on its own thread
/// (called via `std::thread::spawn`) since it blocks on retries — Firefox
/// itself can take a couple of seconds to actually present the new window,
/// longer still if it was a cold launch rather than an already-running
/// instance.
fn shrink_new_firefox_window(before: HashSet<isize>) {
    for _ in 0..25 {
        std::thread::sleep(Duration::from_millis(200));
        let after = firefox_window_handles();
        if let Some(&hwnd) = after.difference(&before).next() {
            let (width, height) = FIREFOX_WINDOW_SIZE;
            unsafe {
                let _ = SetWindowPos(HWND(hwnd as *mut _), HWND(std::ptr::null_mut()), 120, 90, width, height, SWP_NOZORDER);
            }
            return;
        }
    }
    tracing::warn!("no new Firefox window appeared to resize within the timeout");
}

fn firefox_window_handles() -> HashSet<isize> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let handles = &mut *(lparam.0 as *mut Vec<isize>);
        if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != 0 && process_exe_name(pid).is_some_and(|name| name.eq_ignore_ascii_case("firefox.exe")) {
                handles.push(hwnd.0 as isize);
            }
        }
        BOOL(1)
    }

    let mut handles: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut _ as isize));
    }
    handles.into_iter().collect()
}

fn process_exe_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        result.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&path).file_name().and_then(|name| name.to_str()).map(str::to_string)
    }
}

/// Resolves to the exact executable Windows itself would launch for an
/// `http`/`https` link — the default browser as the user actually
/// configured it, not a guess from a hardcoded list of install paths.
fn default_browser_executable() -> Option<PathBuf> {
    let protocol = wide("http");

    let mut len: u32 = 0;
    let hr = unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, PCWSTR(protocol.as_ptr()), PCWSTR::null(), PWSTR::null(), &mut len)
    };
    if hr.is_err() || len == 0 {
        return None;
    }

    let mut buf = vec![0u16; len as usize];
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_EXECUTABLE,
            PCWSTR(protocol.as_ptr()),
            PCWSTR::null(),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    if hr.is_err() {
        return None;
    }

    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
}

/// Called right after the frontend hands the provider's URL to
/// `open_ai_provider` — window creation happens there, entirely outside
/// this app's control from this point on, so pasting is a separate command
/// rather than something bundled into opening the window itself.
#[tauri::command]
pub async fn schedule_ai_paste(provider_label: String) {
    tokio::time::sleep(Duration::from_millis(PASTE_DELAY_MS)).await;

    // The one signal available for "is the window we just opened actually
    // focused" without a window handle of our own: both a chromeless
    // app-mode window and a normal tab title themselves after the active
    // page's title, and every provider this app points at titles its page
    // with its own name (ChatGPT, Claude, Gemini) — matched
    // case-insensitively since a normal tab also appends " - Google Chrome"
    // and similar suffixes to it.
    let title = foreground_window_title();
    if !title.to_lowercase().contains(&provider_label.to_lowercase()) {
        // Never the title itself: it can be any page or document name.
        tracing::warn!(provider_label, "foreground window doesn't look like the AI provider; skipping the paste");
        return;
    }
    send_ctrl_v();
}

fn foreground_window_title() -> String {
    unsafe {
        let hwnd = GetForegroundWindow();
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..copied.max(0) as usize])
    }
}

fn send_ctrl_v() {
    fn key(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, key_up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
    unsafe {
        SendInput(&inputs, core::mem::size_of::<INPUT>() as i32);
    }
}
