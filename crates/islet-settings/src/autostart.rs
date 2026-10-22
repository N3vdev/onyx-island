//! "Start with Windows" via the per-user Run key. No admin rights needed.

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
};

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Adds or removes Islet from the Run key. `islet.exe` is expected next to
/// this executable.
pub fn set(enabled: bool) -> Result<(), String> {
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
                &HSTRING::from("Islet"),
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
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &HSTRING::from("Islet")) };
        if status.is_ok() || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(status.to_hresult().message())
        }
    }
}
