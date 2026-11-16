//! The playing app's icon, for its round button on the music card.
//!
//! Loaded once per app, on a short-lived thread (the shell takes a few
//! milliseconds), then kept for the session.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use islet_core::media::ArtImage;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the island when an icon is ready.
pub const WM_APP_SOURCE_ICON: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 35;

pub struct SourceIcons {
    island: isize,
    asked: HashSet<String>,
    ready: Arc<Mutex<Vec<(String, ArtImage)>>>,
}

impl SourceIcons {
    pub fn new(island: HWND) -> Self {
        Self { island: island.0 as isize, asked: HashSet::new(), ready: Arc::default() }
    }

    /// Starts loading the icon of `source` (its short name) from its
    /// session's `app_id`, unless it was already asked for.
    pub fn want(&mut self, source: &str, app_id: &str) {
        if source.is_empty() || !self.asked.insert(source.to_owned()) {
            return;
        }
        let (source, app_id) = (source.to_owned(), app_id.to_owned());
        let ready = self.ready.clone();
        let island = self.island;
        let _ = std::thread::Builder::new().name("app-icon".to_owned()).spawn(move || {
            let Some(icon) = load(&app_id) else { return };
            if let Ok(mut ready) = ready.lock() {
                ready.push((source, icon));
            }
            // SAFETY: a non-blocking post to the island window.
            unsafe {
                let _ = PostMessageW(Some(HWND(island as *mut _)), WM_APP_SOURCE_ICON, WPARAM(0), LPARAM(0));
            }
        });
    }

    /// Icons loaded since last asked.
    pub fn take(&self) -> Vec<(String, ArtImage)> {
        self.ready.lock().map(|mut ready| std::mem::take(&mut *ready)).unwrap_or_default()
    }
}

/// A Store app (or one with a Start menu ID, like Firefox's) by its ID; a
/// desktop program (Spotify's "Spotify.exe") from where it is running.
fn load(app_id: &str) -> Option<ArtImage> {
    // A music tab in a browser: "firefox.exe#tab".
    let id = app_id.split('#').next().unwrap_or(app_id);
    if id.to_ascii_lowercase().ends_with(".exe") {
        if let Some(path) = running_path(id) {
            if let Some(icon) = crate::assistant::shell_icon(&path) {
                return Some(icon);
            }
        }
    }
    crate::assistant::shell_icon(&format!("shell:AppsFolder\\{id}"))
}

/// The full path of a running program by its file name ("Spotify.exe").
fn running_path(exe: &str) -> Option<String> {
    let exe = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    // SAFETY: the snapshot and process handles are closed here.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase() == exe {
                if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID) {
                    let mut buffer = [0u16; 1024];
                    let mut size = buffer.len() as u32;
                    if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut size).is_ok() {
                        found = Some(String::from_utf16_lossy(&buffer[..size as usize]));
                    }
                    let _ = CloseHandle(process);
                }
                if found.is_some() {
                    break;
                }
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}
