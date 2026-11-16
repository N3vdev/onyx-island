//! How Onyx starts a capture. Onyx owns the shortcut (it is set in Onyx's
//! settings) and signals this helper through named events, which costs
//! nothing while idle: a thread sleeps in `MsgWaitForMultipleObjects`.
//!
//! The same thread receives clicks on Onyx's island while it is this
//! overlay's toolbar (see `island.rs`), as thread messages.

use tauri::{AppHandle, Emitter};
use windows::core::w;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentThreadId, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjects, PeekMessageW, MSG, PM_NOREMOVE, PM_REMOVE, QS_POSTMESSAGE,
};

/// Set by Onyx when its screen-search shortcut is pressed.
const CAPTURE_EVENT: windows::core::PCWSTR = w!("Local\\Onyx.Lens.Capture");
/// Set by Onyx's settings to open this helper's own settings window.
const SETTINGS_EVENT: windows::core::PCWSTR = w!("Local\\Onyx.Lens.Settings");

/// Starts the listener thread. Auto-reset events: one press, one capture.
pub fn listen(app: &AppHandle) -> anyhow::Result<()> {
    // SAFETY: plain named-event creation; the handles live for the process.
    let (capture, settings) = unsafe {
        (
            CreateEventW(None, false, false, CAPTURE_EVENT)?,
            CreateEventW(None, false, false, SETTINGS_EVENT)?,
        )
    };
    let app = app.clone();
    // HANDLE is not Send; carry the raw values across.
    let raw = [capture.0 as isize, settings.0 as isize];
    std::thread::Builder::new()
        .name("onyx-lens-trigger".into())
        .spawn(move || {
            let handles = raw.map(|value| HANDLE(value as *mut core::ffi::c_void));
            let mut message = MSG::default();
            // SAFETY: creates this thread's message queue (a peek does), so
            // the island's clicks can be posted to it.
            unsafe {
                let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                crate::island::set_thread(GetCurrentThreadId());
            }
            loop {
                // SAFETY: waits on two valid event handles owned by this process.
                let result = unsafe { MsgWaitForMultipleObjects(Some(&handles), false, INFINITE, QS_POSTMESSAGE) };
                match result.0.wrapping_sub(WAIT_OBJECT_0.0) {
                    // On the main thread, as the global-shortcut callback was.
                    0 => {
                        let app = app.clone();
                        let _ = app.clone().run_on_main_thread(move || crate::commands::on_hotkey(app));
                    }
                    1 => {
                        let app = app.clone();
                        let _ = app.clone().run_on_main_thread(move || crate::show_settings_window(&app, None));
                    }
                    2 => {
                        // SAFETY: drains this thread's own queue.
                        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                            if message.message == crate::island::tool_message() {
                                let _ = app.emit_to("overlay", "island-tool", message.wParam.0);
                            }
                        }
                    }
                    _ => {
                        tracing::error!("lens trigger wait failed; captures stop until restart");
                        return;
                    }
                }
            }
        })?;
    Ok(())
}
