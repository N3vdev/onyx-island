//! The Windows "choose a folder" dialog, shown on its own thread so the
//! settings window keeps painting while it is open.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// A dialog in progress. [`FolderPick::take`] returns `Some` once it closes:
/// the chosen folder, or `None` if it was cancelled.
pub struct FolderPick {
    result: Arc<Mutex<Option<Option<PathBuf>>>>,
}

impl FolderPick {
    /// Opens the dialog over the active window, starting in `start`.
    pub fn open(title: &str, start: &Path) -> Self {
        let result = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&result);
        // Owned by the settings window (it is in front: its button was just
        // clicked), so the dialog is modal to it.
        let owner = unsafe { GetForegroundWindow() }.0 as isize;
        let (title, start) = (title.to_owned(), start.to_owned());
        std::thread::spawn(move || {
            let picked = show(&title, &start, HWND(owner as *mut _));
            if let Ok(mut slot) = slot.lock() {
                *slot = Some(picked);
            }
        });
        Self { result }
    }

    pub fn take(&self) -> Option<Option<PathBuf>> {
        self.result.lock().ok()?.take()
    }
}

fn show(title: &str, start: &Path, owner: HWND) -> Option<PathBuf> {
    unsafe {
        let initialized =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let picked = (|| -> windows::core::Result<PathBuf> {
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let options = dialog.GetOptions()?;
            dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
            dialog.SetTitle(&HSTRING::from(title))?;
            dialog.SetOkButtonLabel(w!("Use this folder"))?;
            if start.is_dir() {
                if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                    &HSTRING::from(start.as_os_str()),
                    None,
                ) {
                    let _ = dialog.SetFolder(&item);
                }
            }
            dialog.Show(Some(owner))?;
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string();
            CoTaskMemFree(Some(name.0 as *const _));
            Ok(PathBuf::from(path?))
        })()
        .ok();
        if initialized {
            CoUninitialize();
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowThreadProcessId, PostMessageW, IDOK, WM_COMMAND,
    };

    /// Opens the real dialog and presses its OK button: the folder it
    /// started in comes back. Shows a window, so it only runs on request.
    #[test]
    #[ignore = "shows the Windows folder dialog"]
    fn choosing_a_folder_returns_its_path() {
        let start = std::env::temp_dir().join(format!("onyx-pick-{}", std::process::id()));
        std::fs::create_dir_all(&start).unwrap();
        let pick = FolderPick::open("Onyx folder test", &start);
        let deadline = Instant::now() + Duration::from_secs(10);
        let picked = loop {
            assert!(Instant::now() < deadline, "the dialog never answered");
            if let Some(picked) = pick.take() {
                break picked;
            }
            let dialog = unsafe { FindWindowW(windows::core::w!("#32770"), &HSTRING::from("Onyx folder test")) };
            if let Ok(dialog) = dialog {
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(dialog, Some(&mut pid)) };
                if pid == unsafe { GetCurrentProcessId() } {
                    unsafe {
                        let _ = PostMessageW(Some(dialog), WM_COMMAND, WPARAM(IDOK.0 as usize), LPARAM(0));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let _ = std::fs::remove_dir_all(&start);
        assert_eq!(picked.map(|path| path.canonicalize().unwrap_or(path)), Some(start.canonicalize().unwrap_or(start)));
    }
}
