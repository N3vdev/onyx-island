//! Reads the titles of open browser tabs through UI Automation.
//!
//! This is how the music filter learns which site a browser session is on:
//! GSMTC exposes no URL, but every tab's accessible name is its page title,
//! including background tabs. Works for any Chromium- or Gecko-based browser
//! whose process name is in `BROWSER_PROCESSES`. Titles stay in memory and
//! are never logged.

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;

use islet_core::music::BROWSER_PROCESSES;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

/// Upper bound on tabs read per scan, so a pathological session cannot
/// stall the media thread.
const MAX_TABS: usize = 600;

pub struct TabReader {
    uia: IUIAutomation,
    condition: IUIAutomationCondition,
    /// pid -> is a browser. Process names never change for a pid's lifetime;
    /// recycled pids are rare enough that a stale entry only costs one scan.
    browsers: HashMap<u32, bool>,
}

impl TabReader {
    /// Must be called on a COM-initialised thread.
    pub fn new() -> windows::core::Result<Self> {
        // SAFETY: in-process COM activation on the caller's COM thread.
        unsafe {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let condition = uia.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_TabItemControlTypeId.0),
            )?;
            Ok(Self {
                uia,
                condition,
                browsers: HashMap::new(),
            })
        }
    }

    /// Titles of every tab in every visible browser window.
    pub fn titles(&mut self) -> Vec<String> {
        let mut windows: Vec<(HWND, u32)> = Vec::new();
        // SAFETY: the callback only pushes into `windows` through `lparam`,
        // which outlives the synchronous EnumWindows call.
        unsafe {
            let _ = EnumWindows(
                Some(collect_window),
                LPARAM(std::ptr::addr_of_mut!(windows) as isize),
            );
        }

        let mut titles = Vec::new();
        for (hwnd, pid) in windows {
            let is_browser = *self
                .browsers
                .entry(pid)
                .or_insert_with(|| process_is_browser(pid));
            if !is_browser {
                continue;
            }
            // SAFETY: plain UIA reads; failures (window closed mid-scan)
            // just skip the window.
            unsafe {
                let Ok(element) = self.uia.ElementFromHandle(hwnd) else {
                    continue;
                };
                let Ok(found) = element.FindAll(TreeScope_Descendants, &self.condition) else {
                    continue;
                };
                let count = found.Length().unwrap_or(0).max(0) as usize;
                for index in 0..count.min(MAX_TABS.saturating_sub(titles.len())) {
                    if let Ok(name) = found
                        .GetElement(index as i32)
                        .and_then(|tab| tab.CurrentName())
                    {
                        let name = name.to_string();
                        if !name.is_empty() {
                            titles.push(name);
                        }
                    }
                }
            }
            if titles.len() >= MAX_TABS {
                break;
            }
        }
        titles
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam.0 as *mut Vec<(HWND, u32)>);
    if IsWindowVisible(hwnd).as_bool() {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 {
            windows.push((hwnd, pid));
        }
    }
    true.into()
}

fn process_is_browser(pid: u32) -> bool {
    process_stem(pid).is_some_and(|stem| BROWSER_PROCESSES.contains(&stem.as_str()))
}

/// Lowercase executable name without extension, e.g. "firefox".
fn process_stem(pid: u32) -> Option<String> {
    // SAFETY: limited-information handle, closed before returning.
    unsafe {
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
        let file = path.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
        Some(file.strip_suffix(".exe").unwrap_or(&file).to_owned())
    }
}
