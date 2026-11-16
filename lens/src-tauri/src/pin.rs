//! Pin to screen (free): a small always-on-top window showing exactly the
//! pixels of a selection. Multiple pins, not persisted — each is its own
//! window with its own frameless/transparent chrome (`pin.html`), closed
//! independently, and gone for good when the app exits. No state to save
//! or restore, on purpose (see the plan).
//!
//! Every pin window shares one Vite entry (`pin.html`) and is told which
//! pin it is only by its own Tauri window label (`pin-<uuid>`) — this
//! module reads that label back out of the calling `tauri::Window`, so
//! `get_pin_image` never needs the id passed explicitly and can't be pointed
//! at the wrong pin by a WebView bug.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Position, Size};

use crate::commands::{decode_selection_png, PIN_MAX_IMAGE_BYTES, PNG_DATA_URL_PREFIX};
use crate::state::AppState;

#[derive(Default)]
pub struct PinStore(Mutex<HashMap<String, Vec<u8>>>);

/// Height of pin.html's own drag handle strip, added on top of the
/// selection's own size so the image itself isn't squeezed to make room
/// for it.
const HANDLE_HEIGHT: u32 = 22;
const MIN_PIN_SIZE: u32 = 60;

fn pin_id_from_label(label: &str) -> Option<&str> {
    label.strip_prefix("pin-")
}

/// Creates a new pinned window showing `image_data_url`, positioned over
/// the exact screen location `region` — frame-pixel space, the same
/// convention `prepare_region`/`recognize_text` already use — was captured
/// from. The frame's own screen origin is never sent to the WebView (which
/// only ever deals in frame-relative coordinates); it comes from this
/// session's still-cached capture instead, keeping all screen geometry
/// decisions in Rust per CLAUDE.md. Grouped into one struct rather than
/// four more parameters — clippy (rightly) caps how many a command can take.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[tauri::command]
pub async fn create_pin(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    pins: tauri::State<'_, PinStore>,
    image_data_url: String,
    region: PinRegion,
) -> Result<(), String> {
    let bytes = decode_selection_png(&image_data_url, PIN_MAX_IMAGE_BYTES)?;
    let frame = state.frame_cache().ok_or_else(|| "No active capture to pin from".to_string())?;

    let id = uuid::Uuid::new_v4().to_string();
    let label = format!("pin-{id}");

    let screen_x = frame.origin_x + region.x.round() as i32;
    let screen_y = frame.origin_y + region.y.round() as i32;
    let window_width = (region.width.round() as u32).max(MIN_PIN_SIZE);
    let window_height = (region.height.round() as u32).max(MIN_PIN_SIZE) + HANDLE_HEIGHT;

    pins.0.lock().unwrap().insert(id.clone(), bytes);

    // WebviewWindowBuilder::build() and the set_position/set_size/show()
    // calls below are native Win32 window APIs, which on Windows must run
    // on the thread that owns the window message loop — doing this inline
    // on whatever thread ran create_pin took the whole process down
    // outright (a native-level violation, not a catchable Rust panic —
    // nothing reached the panic hook or the logs, the process just
    // vanished). show_overlay in commands.rs marshals its own equivalent
    // calls through run_on_main_thread for exactly this reason.
    //
    // The first fix used `std::sync::mpsc`'s *blocking* `recv()` to get the
    // result back, reasoning by analogy to show_overlay (which is fire-and-
    // forget and never needs to). That was wrong in a different, worse way:
    // if this command happens to already run ON the same thread
    // run_on_main_thread needs — which the earlier crash suggests it does
    // not, but a synchronous command's actual thread isn't something to
    // gamble on — blocking that thread on recv() deadlocks the entire app,
    // not just this command, because the very thread that would run the
    // queued closure and unblock it can never get back to its event loop.
    // `tokio::sync::oneshot` + `.await` instead: an async command genuinely
    // runs on the tokio runtime, never the UI thread, and `.await` yields
    // the task cooperatively rather than blocking an OS thread, so there is
    // no thread left to deadlock regardless of which one run_on_main_thread
    // itself ends up using.
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let app_for_main = app.clone();
    let id_for_main = id.clone();
    app.run_on_main_thread(move || {
        let outcome = (|| -> Result<(), String> {
            // Positioned/sized in *physical* pixels only after being built
            // hidden — `.position()`/`.inner_size()` on the builder are
            // logical pixels (DPI-scaled), which would misplace and
            // mis-size this on any monitor not at 100% scaling.
            let window =
                tauri::WebviewWindowBuilder::new(&app_for_main, &label, tauri::WebviewUrl::App("pin.html".into()))
                    .title("Onyx Lens Pin")
                    .decorations(false)
                    .transparent(true)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    .shadow(false)
                    .resizable(true)
                    .visible(false)
                    .build()
                    .map_err(|err| format!("Could not open the pin window: {err}"))?;

            // Frees the bytes however the window goes away — the close
            // button, Alt+F4, or app exit — rather than trusting the page
            // to report its own closing.
            let app_for_cleanup = app_for_main.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Destroyed = event {
                    app_for_cleanup.state::<PinStore>().0.lock().unwrap().remove(&id_for_main);
                }
            });

            window
                .set_position(Position::Physical(PhysicalPosition { x: screen_x, y: screen_y }))
                .map_err(|err| format!("Could not position the pin window: {err}"))?;
            window
                .set_size(Size::Physical(PhysicalSize { width: window_width, height: window_height }))
                .map_err(|err| format!("Could not size the pin window: {err}"))?;
            window.show().map_err(|err| format!("Could not show the pin window: {err}"))
        })();
        // The receiver only fails to exist if create_pin's future was
        // already dropped (e.g. the caller navigated away) — nothing
        // useful to do with that send failure here.
        let _ = result_tx.send(outcome);
    })
    .map_err(|err| format!("Could not schedule the pin window: {err}"))?;

    let outcome = result_rx.await.map_err(|_| "The pin window task never responded".to_string()).and_then(|r| r);
    if outcome.is_err() {
        pins.0.lock().unwrap().remove(&id);
    }
    outcome
}

/// Reads this pin's bytes back out using the *calling* window's own label —
/// pin.html never learns or sends its own id, it just asks "what am I".
#[tauri::command]
pub fn get_pin_image(window: tauri::Window, pins: tauri::State<PinStore>) -> Result<String, String> {
    let id = pin_id_from_label(window.label()).ok_or_else(|| "Not a pin window".to_string())?;
    let bytes = pins.0.lock().unwrap().get(id).cloned().ok_or_else(|| "This pin's image is gone".to_string())?;
    Ok(format!("{PNG_DATA_URL_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}
