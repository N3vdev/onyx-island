//! The overlay window is created at startup and stays hidden between
//! sessions (so the hotkey can show it in well under 100 ms). Its WebView2
//! renderer, though, has no reason to hold its full working set while hidden.
//! WebView2's memory target lets it page that out while idle and come back
//! on demand: `Low` on hide, `Normal` just before it is shown again.
//!
//! The controller's visibility is switched with it. A hidden *window* alone
//! doesn't tell WebView2 it is hidden, so animation frames kept being
//! rendered for nobody — about half a core, measured, after a session that
//! was closed from Rust rather than from the page.

use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
};
use windows_core_061::Interface;

pub fn set_idle(window: &WebviewWindow, idle: bool) {
    let result = window.with_webview(move |webview| {
        let level = if idle { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
        // Older WebView2 runtimes don't have ICoreWebView2_19; then this is
        // simply skipped — it is an optimisation, not a requirement.
        let controller = webview.controller();
        if let Err(err) = unsafe { controller.SetIsVisible(!idle) } {
            tracing::debug!(%err, idle, "could not set the overlay webview's visibility");
        }
        let applied = unsafe {
            controller
                .CoreWebView2()
                .and_then(|core| core.cast::<ICoreWebView2_19>())
                .and_then(|core| core.SetMemoryUsageTargetLevel(level))
        };
        if let Err(err) = applied {
            tracing::debug!(%err, idle, "could not set the overlay's memory target");
        }
    });
    if let Err(err) = result {
        tracing::debug!(%err, "overlay webview unavailable for memory target");
    }
}
