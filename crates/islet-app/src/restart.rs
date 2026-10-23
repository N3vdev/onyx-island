//! Start the replacement first, then let it wait for a clean shutdown.
use std::os::windows::process::CommandExt;

use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_SYNCHRONIZE,
};

const WAIT_ARGUMENT: &str = "--restart-after=";

pub fn launch() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let args = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with(WAIT_ARGUMENT));
    std::process::Command::new(exe)
        .args(args)
        .arg(format!("{WAIT_ARGUMENT}{}", std::process::id()))
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()?;
    Ok(())
}

pub fn wait_for_previous_instance() -> Result<(), Box<dyn std::error::Error>> {
    let Some(pid) = std::env::args().find_map(|arg| {
        arg.strip_prefix(WAIT_ARGUMENT)
            .and_then(|pid| pid.parse::<u32>().ok())
    }) else {
        return Ok(());
    };
    if pid == std::process::id() {
        return Err("cannot wait for this process".into());
    }
    // Open before shutdown; waiting on the process handle avoids both a fixed
    // sleep and a race with the singleton mutex or the old logging worker.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else {
        // It may already have exited between spawning this process and startup.
        return Ok(());
    };
    let result = unsafe { WaitForSingleObject(process, 30_000) };
    unsafe {
        let _ = CloseHandle(process);
    }
    if result != WAIT_OBJECT_0 {
        return Err("previous Islet instance did not stop".into());
    }
    Ok(())
}
