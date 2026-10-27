//! "Start with Windows". Installed from the Microsoft Store (MSIX), Onyx uses
//! the package's startup task; Windows virtualizes a packaged app's registry,
//! so the Run key would never be read. Installed with the setup `.exe`, it
//! uses the per-user Run key. Neither needs admin rights.

use windows::core::{w, HSTRING};
use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_FILE_NOT_FOUND};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
};

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
/// The Run value's name, shown in Task Manager's Startup apps.
const RUN_NAME: &str = "Onyx";
/// The value written before the rename; removed whenever startup changes.
const OLD_RUN_NAME: &str = "Islet";
/// The startup task's id in the MSIX manifest (`packaging/AppxManifest.xml`).
const STARTUP_TASK_ID: &str = "OnyxStartup";

/// Turns starting with Windows on or off.
pub fn set(enabled: bool) -> Result<(), String> {
    if is_packaged() {
        // The async calls are waited on from a worker thread, never the UI's.
        std::thread::spawn(move || set_startup_task(enabled))
            .join()
            .map_err(|_| "the startup task could not be changed".to_owned())?
    } else {
        set_run_key(enabled)
    }
}

/// Whether Onyx runs from an MSIX package (the Microsoft Store install).
fn is_packaged() -> bool {
    let mut length = 0u32;
    // SAFETY: a length query with no buffer.
    let status = unsafe { GetCurrentPackageFullName(&mut length, None) };
    status != APPMODEL_ERROR_NO_PACKAGE
}

fn set_startup_task(enabled: bool) -> Result<(), String> {
    let task = StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK_ID))
        .and_then(|operation| operation.join())
        .map_err(|error| error.message().to_string())?;
    if !enabled {
        return task.Disable().map_err(|error| error.message().to_string());
    }
    let state = task
        .RequestEnableAsync()
        .and_then(|operation| operation.join())
        .map_err(|error| error.message().to_string())?;
    match state {
        StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy => Ok(()),
        StartupTaskState::DisabledByUser => Err(
            "Windows has Onyx turned off as a startup app. Turn it on in Settings > Apps > Startup."
                .to_owned(),
        ),
        _ => Err("Windows does not allow Onyx to start with Windows on this PC.".to_owned()),
    }
}

/// Adds or removes Onyx from the Run key. `islet.exe` is expected next to
/// this executable.
fn set_run_key(enabled: bool) -> Result<(), String> {
    // SAFETY: deleting a named value under HKCU; absent is fine.
    let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &HSTRING::from(OLD_RUN_NAME)) };
    if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let island = exe.with_file_name("islet.exe");
        let command: Vec<u16> = format!("\"{}\"", island.display())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: the value buffer is NUL-terminated and outlives the call.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                &HSTRING::from(RUN_NAME),
                REG_SZ.0,
                Some(command.as_ptr().cast()),
                (command.len() * 2) as u32,
            )
        }
        .ok()
        .map_err(|error| error.to_string())
    } else {
        // SAFETY: deleting a named value under HKCU.
        let status =
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &HSTRING::from(RUN_NAME)) };
        if status.is_ok() || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(status.to_hresult().message())
        }
    }
}
