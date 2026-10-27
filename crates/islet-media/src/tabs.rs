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
use windows::core::{BOOL, BSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationInvokePattern, TreeScope_Descendants, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_ImageControlTypeId,
    UIA_InvokePatternId, UIA_NamePropertyId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

/// Upper bound on tabs read per scan, so a pathological session cannot
/// stall the media thread.
const MAX_TABS: usize = 600;
/// YouTube's label for the music-note badge next to an artist's channel.
const OFFICIAL_ARTIST_BADGE: &str = "Official Artist Channel";

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

    /// Visible top-level windows that belong to a browser.
    fn browser_windows(&mut self) -> Vec<HWND> {
        let mut windows: Vec<(HWND, u32)> = Vec::new();
        // SAFETY: the callback only pushes into `windows` through `lparam`,
        // which outlives the synchronous EnumWindows call.
        unsafe {
            let _ = EnumWindows(
                Some(collect_window),
                LPARAM(std::ptr::addr_of_mut!(windows) as isize),
            );
        }
        windows
            .into_iter()
            .filter(|&(_, pid)| {
                *self
                    .browsers
                    .entry(pid)
                    .or_insert_with(|| process_is_browser(pid))
            })
            .map(|(hwnd, _)| hwnd)
            .collect()
    }

    /// Whether the YouTube page in the tab titled `tab_title` shows the
    /// "Official Artist Channel" badge beside the video's channel name.
    /// `None` when that page is not readable (a background tab in a
    /// browser that only exposes the active one). Sidebar recommendations
    /// carry the same label on a different element, so only the owner's
    /// badge (an image styled `ytBadgeShape`) counts.
    pub fn official_artist(&mut self, tab_title: &str) -> Option<bool> {
        // SAFETY: plain UIA reads; failures skip the window.
        unsafe {
            let page = self
                .uia
                .CreateAndCondition(
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_ControlTypePropertyId,
                            &VARIANT::from(UIA_DocumentControlTypeId.0),
                        )
                        .ok()?,
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_NamePropertyId,
                            &VARIANT::from(BSTR::from(tab_title)),
                        )
                        .ok()?,
                )
                .ok()?;
            let badge = self
                .uia
                .CreateAndCondition(
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_ControlTypePropertyId,
                            &VARIANT::from(UIA_ImageControlTypeId.0),
                        )
                        .ok()?,
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_NamePropertyId,
                            &VARIANT::from(BSTR::from(OFFICIAL_ARTIST_BADGE)),
                        )
                        .ok()?,
                )
                .ok()?;
            for hwnd in self.browser_windows() {
                let Ok(window) = self.uia.ElementFromHandle(hwnd) else {
                    continue;
                };
                let Ok(document) = window.FindFirst(TreeScope_Descendants, &page) else {
                    continue;
                };
                let found = document
                    .FindFirst(TreeScope_Descendants, &badge)
                    .ok()
                    .and_then(|element| element.CurrentClassName().ok())
                    .is_some_and(|class| class.to_string().contains("ytBadgeShape"));
                return Some(found);
            }
            None
        }
    }

    /// Titles of tabs that are playing sound: Firefox gives such a tab a
    /// "Mute tab" button (its speaker icon), Chromium adds "playing audio"
    /// to the tab's name.
    pub fn audible_titles(&mut self) -> Vec<String> {
        let mut audible = Vec::new();
        for hwnd in self.browser_windows() {
            // SAFETY: plain UIA reads; failures skip the window.
            unsafe {
                let Ok(element) = self.uia.ElementFromHandle(hwnd) else {
                    continue;
                };
                let Ok(found) = element.FindAll(TreeScope_Descendants, &self.condition) else {
                    continue;
                };
                let count = found.Length().unwrap_or(0).max(0) as usize;
                for index in 0..count.min(MAX_TABS) {
                    let Ok(tab) = found.GetElement(index as i32) else {
                        continue;
                    };
                    let name = tab.CurrentName().map(|n| n.to_string()).unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    let lower = name.to_lowercase();
                    let chromium = lower.contains("playing audio") || lower.contains("audio playing");
                    let firefox = self
                        .uia
                        .CreateTrueCondition()
                        .ok()
                        .and_then(|all| tab.FindAll(TreeScope_Descendants, &all).ok())
                        .is_some_and(|children| {
                            (0..children.Length().unwrap_or(0)).any(|child| {
                                children
                                    .GetElement(child)
                                    .and_then(|c| c.CurrentName())
                                    .is_ok_and(|n| n.to_string().to_lowercase().starts_with("mute tab"))
                            })
                        });
                    if chromium || firefox {
                        audible.push(name);
                    }
                }
            }
        }
        audible
    }

    /// The page (document) shown in the tab titled `tab_title`.
    fn page(&mut self, tab_title: &str) -> Option<IUIAutomationElement> {
        // SAFETY: plain UIA reads; failures skip the window.
        unsafe {
            let condition = self
                .uia
                .CreateAndCondition(
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_ControlTypePropertyId,
                            &VARIANT::from(UIA_DocumentControlTypeId.0),
                        )
                        .ok()?,
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_NamePropertyId,
                            &VARIANT::from(BSTR::from(tab_title)),
                        )
                        .ok()?,
                )
                .ok()?;
            self.browser_windows().into_iter().find_map(|hwnd| {
                self.uia
                    .ElementFromHandle(hwnd)
                    .and_then(|window| window.FindFirst(TreeScope_Descendants, &condition))
                    .ok()
            })
        }
    }

    /// Presses a button of YouTube Music's player bar ("Play", "Pause",
    /// "Next", "Previous") in the tab titled `tab_title`, without switching
    /// to it. Returns whether one was pressed.
    pub fn press_player_button(&mut self, tab_title: &str, names: &[&str]) -> bool {
        let Some(document) = self.page(tab_title) else {
            return false;
        };
        // SAFETY: plain UIA reads and one Invoke on the found button.
        unsafe {
            let Ok(condition) = self.uia.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_ButtonControlTypeId.0),
            ) else {
                return false;
            };
            let Ok(buttons) = document.FindAll(TreeScope_Descendants, &condition) else {
                return false;
            };
            for index in 0..buttons.Length().unwrap_or(0) {
                let Ok(button) = buttons.GetElement(index) else {
                    continue;
                };
                let name = button.CurrentName().map(|n| n.to_string()).unwrap_or_default();
                let class = button
                    .CurrentClassName()
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                if !class.contains("ytmusic-player-bar")
                    || !names.iter().any(|wanted| name.eq_ignore_ascii_case(wanted))
                {
                    continue;
                }
                if let Ok(invoke) =
                    button.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                {
                    return invoke.Invoke().is_ok();
                }
            }
            false
        }
    }

    /// What YouTube Music's player bar shows: (title, artist line), read
    /// from the page in the tab titled `tab_title`. `None` when the page is
    /// not readable or the bar is empty.
    pub fn youtube_music_now_playing(&mut self, tab_title: &str) -> Option<(String, String)> {
        // SAFETY: plain UIA reads; failures skip the window.
        unsafe {
            let page = self
                .uia
                .CreateAndCondition(
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_ControlTypePropertyId,
                            &VARIANT::from(UIA_DocumentControlTypeId.0),
                        )
                        .ok()?,
                    &self
                        .uia
                        .CreatePropertyCondition(
                            UIA_NamePropertyId,
                            &VARIANT::from(BSTR::from(tab_title)),
                        )
                        .ok()?,
                )
                .ok()?;
            let all = self.uia.CreateTrueCondition().ok()?;
            for hwnd in self.browser_windows() {
                let Ok(window) = self.uia.ElementFromHandle(hwnd) else {
                    continue;
                };
                let Ok(document) = window.FindFirst(TreeScope_Descendants, &page) else {
                    continue;
                };
                let Ok(found) = document.FindAll(TreeScope_Descendants, &all) else {
                    continue;
                };
                let mut title = None;
                let mut byline = None;
                for index in 0..found.Length().unwrap_or(0) {
                    let Ok(element) = found.GetElement(index) else {
                        continue;
                    };
                    let class = element
                        .CurrentClassName()
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    if !class.contains("ytmusic-player-bar") {
                        continue;
                    }
                    let text = element
                        .CurrentName()
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }
                    if title.is_none() && class.split_whitespace().any(|c| c == "title") {
                        title = Some(text);
                    } else if byline.is_none() && class.split_whitespace().any(|c| c == "byline") {
                        byline = Some(text);
                    }
                }
                return title.map(|title| (title, byline.unwrap_or_default()));
            }
            None
        }
    }

    /// Titles of every tab in every visible browser window.
    pub fn titles(&mut self) -> Vec<String> {
        let mut titles = Vec::new();
        for hwnd in self.browser_windows() {
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
