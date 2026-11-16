//! Hotkey entry point (captures + shows the overlay) and the
//! #[tauri::command] surface exposed to the overlay WebView.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size};
use windows::Win32::Foundation::RECT;

use crate::state::AppState;
use crate::{capture, layout, monitor, ocr, selection};

const OVERLAY_LABEL: &str = "overlay";

#[derive(Clone, serde::Serialize)]
struct FrameReadyPayload {
    #[serde(rename = "dataUrl")]
    data_url: String,
    width: u32,
    height: u32,
    /// The monitor's OS scale factor (`dpi_x / 96.0` — e.g. 1.5 at 150%
    /// Windows scaling), i.e. captured (physical) pixels per design pixel.
    /// Handed over so the designer toolkit's ruler can report a distance in
    /// design pixels (`framePixels / dpiScale`) alongside the raw captured
    /// pixel count, without a second round trip to ask Rust for monitor info.
    #[serde(rename = "dpiScale")]
    dpi_scale: f64,
    /// Onyx's island is on this screen and shows the tools instead of the
    /// overlay's own toolbar (see `island.rs`).
    #[serde(rename = "islandTools")]
    island_tools: bool,
    /// The island's look, which the overlay's bars match.
    #[serde(rename = "islandLook")]
    island_look: crate::island::Look,
}

#[derive(Clone, serde::Serialize)]
struct OcrWordPayload {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, serde::Serialize)]
struct OcrLinePayload {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    words: Vec<OcrWordPayload>,
}

impl From<ocr::OcrLine> for OcrLinePayload {
    fn from(line: ocr::OcrLine) -> Self {
        Self {
            text: line.text,
            x: line.x,
            y: line.y,
            width: line.width,
            height: line.height,
            words: line
                .words
                .into_iter()
                .map(|w| OcrWordPayload { text: w.text, x: w.x, y: w.y, width: w.width, height: w.height })
                .collect(),
        }
    }
}

pub fn on_hotkey(app: AppHandle) {
    let state = app.state::<AppState>();
    let Some(token) = state.try_arm() else {
        // Escape and right-click both live entirely in the overlay's own
        // JavaScript — if that page ever fails to load or crashes (the
        // overlay has no decorations, is always-on-top, and hidden from the
        // taskbar/Alt+Tab by design), neither one exists to get the user out.
        // The one thing a stuck user will always try is the hotkey they
        // already know, so a second press has to dismiss unconditionally
        // rather than being ignored — this is the only escape hatch that
        // cannot depend on the WebView actually working.
        tracing::warn!("hotkey pressed while already armed; dismissing instead of starting a new capture");
        dismiss_overlay_now(&app);
        return;
    };

    let span = tracing::info_span!("hotkey_to_overlay");
    tracing::info!(parent: &span, "hotkey pressed");

    std::thread::spawn(move || {
        let _enter = span.enter();
        let start = Instant::now();

        let mon = match monitor::monitor_under_cursor() {
            Ok(mon) => mon,
            Err(err) => {
                tracing::error!(%err, "monitor lookup failed");
                app.state::<AppState>().reset_if_active(token);
                return;
            }
        };

        // Freeze before show: capture completes before the overlay is shown.
        let hide_cursor = app.state::<AppState>().settings().behavior.hide_cursor;
        let frame = match capture::capture_monitor(mon.hmonitor as *mut std::ffi::c_void, hide_cursor) {
            Ok(frame) => frame,
            Err(err) => {
                tracing::error!(%err, "capture failed");
                app.state::<AppState>().reset_if_active(token);
                return;
            }
        };
        tracing::info!(
            width = frame.width,
            height = frame.height,
            bytes = frame.png_bytes.len(),
            "capture complete"
        );

        // WGC hands back an all-black frame for DRM-protected or exclusive-
        // fullscreen content instead of failing, so there is nothing to
        // select and nothing worth OCR'ing. Say so, rather than showing a
        // black overlay and leaving the user to guess why nothing works.
        if frame.is_blank {
            tracing::warn!("captured frame is blank; treating as protected or exclusive-fullscreen content");
            show_overlay(&app, token, mon.bounds, None, start);
            return;
        }

        if !app.state::<AppState>().is_active(token) {
            return;
        }
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&frame.png_bytes)
        );
        // Move the PNG into the session instead of making another full-frame
        // copy. A capture that finished after dismissal is discarded here.
        if !app.state::<AppState>().set_frame_cache(
            token,
            frame.png_bytes,
            mon.scale_factor,
            mon.bounds.left,
            mon.bounds.top,
        ) {
            return;
        }

        let island_tools = crate::island::begin(mon.hmonitor);
        let payload = FrameReadyPayload {
            data_url,
            width: frame.width,
            height: frame.height,
            dpi_scale: mon.scale_factor,
            island_tools,
            island_look: crate::island::look(),
        };
        show_overlay(&app, token, mon.bounds, Some(payload), start);
    });
}

/// Positions the overlay over `bounds`, hands the webview its content, and
/// shows it. `payload` is `None` when the capture came back blank, in which
/// case the overlay explains itself instead of painting a frame.
fn show_overlay(app: &AppHandle, token: u64, bounds: RECT, payload: Option<FrameReadyPayload>, start: Instant) {
    let app_for_main = app.clone();
    let result = app.run_on_main_thread(move || {
        let app = app_for_main;
        // Capture and main-thread dispatch may both finish after the user
        // has dismissed this session or already started another one.
        if !app.state::<AppState>().is_active(token) {
            return;
        }
        let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
            tracing::error!("overlay window not found");
            app.state::<AppState>().reset_if_active(token);
            return;
        };

        crate::overlay_memory::set_idle(&window, false);

        if let Err(err) =
            window.set_position(Position::Physical(PhysicalPosition { x: bounds.left, y: bounds.top }))
        {
            tracing::error!(%err, "failed to position overlay");
        }

        if let Err(err) = window.set_size(Size::Physical(PhysicalSize {
            width: (bounds.right - bounds.left) as u32,
            height: (bounds.bottom - bounds.top) as u32,
        })) {
            tracing::error!(%err, "failed to size overlay");
        }

        let emitted = match payload {
            Some(payload) => window.emit("frame-ready", payload),
            None => window.emit("capture-blocked", ()),
        };
        if let Err(err) = emitted {
            tracing::error!(%err, "failed to emit overlay content");
        }

        if let Err(err) = window.show() {
            tracing::error!(%err, "failed to show overlay");
        }
        let _ = window.set_focus();

        tracing::info!(elapsed_ms = start.elapsed().as_millis() as u64, "overlay visible");
    });

    if let Err(err) = result {
        tracing::error!(%err, "failed to dispatch to main thread");
        app.state::<AppState>().reset_if_active(token);
    }
}

#[tauri::command]
pub fn dismiss_overlay(app: AppHandle) {
    dismiss_overlay_now(&app);
}

/// Alt-Tab must never leave the always-on-top screenshot covering the new
/// app. Handle this natively, even when the page is busy or has lost focus.
pub(crate) fn dismiss_overlay_on_focus_loss(app: &AppHandle) {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOTOWNER};

    // Hiding also generates a focus-loss event. The session is reset before
    // hide(), so this prevents re-entering dismissal from that event.
    if !app.state::<AppState>().is_armed() {
        return;
    }
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else { return };
    if !window.is_visible().unwrap_or(false) || window.is_focused().unwrap_or(false) {
        return;
    }
    // Keep the selection while its own Save As dialog has focus. An owned
    // dialog's root owner is the overlay; an unrelated app's is not.
    if let Ok(hwnd) = window.hwnd() {
        let foreground = unsafe { GetForegroundWindow() };
        if !foreground.0.is_null() && unsafe { GetAncestor(foreground, GA_ROOTOWNER) }.0 == hwnd.0 {
            return;
        }
    }
    dismiss_overlay_now(app);
}

/// Shared by the `dismiss_overlay` command (normal Escape/right-click path)
/// and `on_hotkey`'s own native fallback for a stuck overlay — both need the
/// exact same hide-and-release sequence, and only one of them can go through
/// JavaScript.
fn dismiss_overlay_now(app: &AppHandle) {
    app.state::<AppState>().reset();
    crate::island::end();
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
        crate::overlay_memory::set_idle(&window, true);
        // The page resets itself on Escape, but not when Rust closes the
        // overlay (second hotkey press) — tell it, so its animations stop
        // and its full-screen canvases are released.
        let _ = window.emit("overlay-dismissed", ());
    }
    // Translator::release() blocks on the same mutex a translation holds for
    // its *entire* pass, including a first-time model download that can take
    // several seconds — release() on this command's own thread would make
    // Escape/dismiss hang for that long even though the window above is
    // already hidden. A background thread lets this return immediately
    // regardless; the model still gets dropped as soon as the in-flight
    // translation (whose result nothing will read now) finishes.
    let app_for_release = app.clone();
    std::thread::spawn(move || {
        // Here rather than on the hotkey path: it reads the license file,
        // and the next session is soon enough for a trial that just lapsed.
        crate::entitlements::refresh(&app_for_release);
        app_for_release.state::<crate::translate::Translator>().release();
    });
    tracing::info!("overlay dismissed");
}

#[derive(Clone, serde::Serialize)]
pub struct SelectionPayload {
    kind: &'static str,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    /// One entry per OCR line in this selection, each carrying its own
    /// words — textlayer.ts materializes one span per word from this.
    lines: Vec<OcrLinePayload>,
}

impl From<selection::Selection> for SelectionPayload {
    fn from(sel: selection::Selection) -> Self {
        match sel {
            selection::Selection::Block(b) => Self {
                kind: "block",
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
                lines: b.lines.into_iter().map(OcrLinePayload::from).collect(),
            },
            selection::Selection::Line(l) => Self {
                kind: "line",
                x: l.x,
                y: l.y,
                width: l.width,
                height: l.height,
                lines: vec![OcrLinePayload::from(l)],
            },
        }
    }
}

#[derive(serde::Serialize)]
pub struct TextSelectionResult {
    selections: Vec<SelectionPayload>,
}

#[derive(serde::Serialize)]
pub struct PreparedRegion {
    /// A crop of the captured frame to the lasso's own bounding box, so
    /// image-based actions (reverse image search) work even when the
    /// circled region has no text at all. Present whenever the frame is
    /// cached and the lasso encloses a non-empty area — independent of
    /// whether any text got selected.
    #[serde(rename = "imageDataUrl")]
    image_data_url: Option<String>,
}

pub(crate) const PNG_DATA_URL_PREFIX: &str = "data:image/png;base64,";
const LENS_MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const SAVE_MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Shared with pin.rs, which decodes the same kind of selection crop.
pub(crate) const PIN_MAX_IMAGE_BYTES: usize = SAVE_MAX_IMAGE_BYTES;

fn decode_lens_image(data_url: &str) -> Result<Vec<u8>, String> {
    decode_selection_png(data_url, LENS_MAX_IMAGE_BYTES)
}

/// Reused by pin.rs — every command that accepts a data-URL crop from the
/// WebView validates it the same way (bounded size, real PNG header)
/// before trusting it.
pub(crate) fn decode_selection_png(data_url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let encoded = data_url
        .strip_prefix(PNG_DATA_URL_PREFIX)
        .ok_or_else(|| "Expected a PNG selection image".to_string())?;

    // Reject before decoding so an arbitrary IPC payload cannot allocate an
    // unbounded buffer. A lasso crop is normally far smaller.
    if encoded.len() > max_bytes.div_ceil(3) * 4 + 4 {
        return Err(format!("Selection image exceeds the {} MB limit", max_bytes / 1024 / 1024));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Selection image is not valid base64".to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!("Selection image exceeds the {} MB limit", max_bytes / 1024 / 1024));
    }
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return Err("Selection image is not a valid PNG".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 40_000_000 {
        return Err("Selection image dimensions are invalid or too large".into());
    }
    Ok(bytes)
}

/// What the save dialog can write. PNG keeps the selection's transparency
/// and is free; JPEG and WebP are part of the Pro visual pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Png,
    Jpeg,
    Webp,
}

impl ExportFormat {
    fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// Re-encodes the PNG selection into `format`. JPEG has no alpha channel, so
/// the transparent exterior of a freehand crop is flattened onto white
/// rather than turning black.
fn encode_export(png: &[u8], format: ExportFormat) -> Result<Vec<u8>, String> {
    // PNG is a pass-through (no re-encode needed) and, unlike the other two,
    // never decodes the image at all — genuinely a different shape from the
    // JPEG/WebP arms below, so it's its own arm rather than forced into the
    // same decode-then-encode structure just to avoid a second match.
    if format == ExportFormat::Png {
        return Ok(png.to_vec());
    }

    let rgba = image::load_from_memory(png)
        .map_err(|err| format!("Could not read the selection: {err}"))?
        .to_rgba8();
    let mut out = Vec::new();
    // Only Jpeg/Webp reach here (Png returned above), so this branches on
    // just the two live possibilities instead of a three-way match that
    // would need a dead Png arm to satisfy exhaustiveness.
    let written = if format == ExportFormat::Jpeg {
        let mut flat = image::RgbImage::new(rgba.width(), rgba.height());
        for (x, y, pixel) in rgba.enumerate_pixels() {
            let alpha = u16::from(pixel[3]);
            let over_white = |channel: u8| ((u16::from(channel) * alpha + 255 * (255 - alpha)) / 255) as u8;
            flat.put_pixel(x, y, image::Rgb([over_white(pixel[0]), over_white(pixel[1]), over_white(pixel[2])]));
        }
        image::DynamicImage::ImageRgb8(flat)
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92))
    } else {
        image::DynamicImage::ImageRgba8(rgba)
            .write_with_encoder(image::codecs::webp::WebPEncoder::new_lossless(&mut out))
    };
    written.map_err(|err| format!("Could not encode the image: {err}"))?;
    Ok(out)
}

/// Where the last image was saved, so the "Show in folder" button can open
/// it. Rust-side only: the WebView never supplies a path to open.
static LAST_SAVED: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Opens File Explorer with the last saved image selected.
#[tauri::command]
pub fn reveal_saved_image() -> Result<(), String> {
    let path = LAST_SAVED.lock().unwrap().clone().ok_or("Nothing has been saved yet")?;
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Could not open the folder: {err}"))
}

/// Save the alpha-bearing PNG generated by the selection canvas, or a JPEG/
/// WebP re-encode of it on a paid plan. The plan is read from Rust-side
/// entitlements, never from the WebView, and a typed .jpg/.webp name on the
/// Free plan is refused here even though the dialog only offers PNG.
#[tauri::command]
pub async fn save_selection_png(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    png_data_url: String,
) -> Result<bool, String> {
    let paid = state.entitlements().plan != crate::entitlements::Plan::Free;
    tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let bytes = decode_selection_png(&png_data_url, SAVE_MAX_IMAGE_BYTES)?;
        let mut dialog = rfd::FileDialog::new().set_parent(&window).add_filter("PNG image", &["png"]);
        if paid {
            dialog = dialog.add_filter("JPEG image", &["jpg", "jpeg"]).add_filter("WebP image", &["webp"]);
        }
        let Some(mut path) = dialog.set_file_name("Onyx Lens.png").save_file() else {
            return Ok(false);
        };
        let format = match path.extension().and_then(|extension| extension.to_str()) {
            None => {
                path.set_extension("png");
                ExportFormat::Png
            }
            Some(extension) => ExportFormat::from_extension(extension).ok_or_else(|| {
                if paid { "Choose a .png, .jpg or .webp filename" } else { "Choose a .png filename to save this image" }
                    .to_string()
            })?,
        };
        if format != ExportFormat::Png && !paid {
            return Err("JPEG and WebP export are part of Pro. Save as .png instead.".into());
        }
        let encoded = encode_export(&bytes, format)?;
        std::fs::write(&path, encoded).map_err(|err| format!("Could not save the image: {err}"))?;
        *LAST_SAVED.lock().unwrap() = Some(path);
        Ok(true)
    })
    .await
    .map_err(|err| format!("Save dialog failed: {err}"))?
}

fn lens_handoff_html(data_url: &str, timestamp: u128) -> String {
    let encoded = data_url.strip_prefix(PNG_DATA_URL_PREFIX).unwrap();
    format!(r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Opening Google Lens</title>
<style>body{{font:16px system-ui,sans-serif;background:#171922;color:#f4f4f8;display:grid;place-items:center;min-height:90vh;text-align:center}}p{{color:#bfc1cd}}button{{padding:10px 18px;border:0;border-radius:8px;cursor:pointer}}</style>
</head><body><main><h1>Opening Google Lens…</h1><p id="status">Sending your selected image from this browser.</p>
<form id="lens-form" method="POST" enctype="multipart/form-data" action="https://lens.google.com/v3/upload"><input id="lens-file" type="file" name="encoded_image" hidden><button type="submit">Continue to Lens</button></form></main>
<script>
try {{
  const bytes = Uint8Array.from(atob("{encoded}"), c => c.charCodeAt(0));
  const file = new File([bytes], "selection.png", {{type: "image/png"}});
  const transfer = new DataTransfer();
  transfer.items.add(file);
  document.getElementById("lens-file").files = transfer.files;
  const url = new URL("https://lens.google.com/v3/upload");
  url.searchParams.set("ep", "cntpubb");
  url.searchParams.set("st", "{timestamp}");
  url.searchParams.set("hl", navigator.language || "en");
  url.searchParams.set("re", "df");
  url.searchParams.set("s", "4");
  const view = new DataView(bytes.buffer);
  url.searchParams.set("vpw", String(view.getUint32(16)));
  url.searchParams.set("vph", String(view.getUint32(20)));
  const form = document.getElementById("lens-form");
  form.action = url.toString();
  form.submit();
}} catch (error) {{
  document.getElementById("status").textContent = "Could not prepare the image. Return to Onyx Lens and try again.";
  document.getElementById("lens-form").hidden = true;
}}
</script></body></html>"#)
}

/// Serves the selected crop to the user's browser once, over loopback. The
/// browser then submits Chromium's Lens form itself, keeping Google's upload
/// and results in the same cookie/session context. A copied redirect URL from
/// an app-side HTTP client instead shows "Expired visual search" in browsers.
#[tauri::command]
pub fn search_with_lens(image_data_url: String) -> Result<String, String> {
    decode_lens_image(&image_data_url)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "System clock is not available".to_string())?
        .as_millis();
    let html = lens_handoff_html(&image_data_url, timestamp);
    let token = uuid::Uuid::new_v4();
    let path = format!("/lens/{token}");
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| format!("Could not open Lens handoff: {err}"))?;
    listener.set_nonblocking(true).map_err(|err| format!("Could not prepare Lens handoff: {err}"))?;
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}{path}");
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                Err(err) => {
                    tracing::warn!(%err, "Lens handoff connection failed");
                    return;
                }
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request_line = String::new();
            let read_ok = BufReader::new((&mut stream).take(2048)).read_line(&mut request_line).is_ok();
            if !read_ok || request_line.trim_end() != format!("GET {path} HTTP/1.1") {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
            }
            let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n", html.len());
            if stream.write_all(headers.as_bytes()).and_then(|_| stream.write_all(html.as_bytes())).is_err() {
                tracing::warn!("Lens handoff page could not be served");
            }
            return;
        }
        tracing::warn!("Lens handoff expired before the browser opened it");
    });
    Ok(url)
}

/// `None` for a lasso that encloses no area — a plain click rather than a
/// drag. Callers treat that as "nothing circled" instead of trying to crop
/// a zero-area region and reporting the failure as an error.
fn lasso_bounding_box(points: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    points
        .iter()
        .fold(None::<(f64, f64, f64, f64)>, |acc, &(x, y)| {
            Some(match acc {
                None => (x, y, x, y),
                Some((min_x, min_y, max_x, max_y)) => (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)),
            })
        })
        .map(|(min_x, min_y, max_x, max_y)| (min_x, min_y, max_x - min_x, max_y - min_y))
        .filter(|&(_, _, width, height)| width >= 1.0 && height >= 1.0)
}

fn crop_region_from_image(
    img: &image::DynamicImage,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> anyhow::Result<image::DynamicImage> {
    let (img_w, img_h) = (img.width() as f64, img.height() as f64);

    let x0 = x.max(0.0).min(img_w) as u32;
    let y0 = y.max(0.0).min(img_h) as u32;
    let x1 = (x + width).max(0.0).min(img_w) as u32;
    let y1 = (y + height).max(0.0).min(img_h) as u32;

    if x1 <= x0 || y1 <= y0 {
        anyhow::bail!("crop region is empty after clamping to frame bounds");
    }

    Ok(img.crop_imm(x0, y0, x1 - x0, y1 - y0))
}

fn encode_png_bytes(img: &image::DynamicImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
    Ok(buf)
}

fn encode_png_data_url(img: &image::DynamicImage) -> anyhow::Result<String> {
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(encode_png_bytes(img)?)))
}

// Kept for tests, which exercise this at the raw-bytes level for convenience.
#[cfg(test)]
fn crop_to_data_url(png_bytes: &[u8], x: f64, y: f64, width: f64, height: f64) -> anyhow::Result<String> {
    let img = image::load_from_memory(png_bytes)?;
    let cropped = crop_region_from_image(&img, x, y, width, height)?;
    encode_png_data_url(&cropped)
}

/// Prepares an image-only crop for the non-text lasso actions. This runs once
/// after a completed gesture; no OCR, QR decode, or background worker starts.
#[tauri::command]
pub async fn prepare_region(
    state: tauri::State<'_, AppState>,
    points: Vec<(f64, f64)>,
) -> Result<PreparedRegion, String> {
    let Some((frame, bounds)) = state.frame_cache().zip(lasso_bounding_box(&points)) else {
        return Ok(PreparedRegion { image_data_url: None });
    };
    let image_data_url = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let image = image::load_from_memory(&frame.png_bytes)?;
        let (x, y, width, height) = bounds;
        let crop = crop_region_from_image(&image, x, y, width, height)?;
        encode_png_data_url(&crop)
    })
    .await
    .map_err(|err| format!("selection worker failed: {err}"))?
    .map_err(|err| format!("could not prepare selection: {err}"))?;

    Ok(PreparedRegion { image_data_url: Some(image_data_url) })
}

/// Recognizes text only when the user enters text-selection mode. All image
/// decode and Windows OCR work happens off the Tauri command thread, against
/// the lasso crop plus a small edge margin.
#[tauri::command]
pub async fn recognize_text(
    state: tauri::State<'_, AppState>,
    points: Vec<(f64, f64)>,
) -> Result<TextSelectionResult, String> {
    let Some((frame, bounds)) = state.frame_cache().zip(lasso_bounding_box(&points)) else {
        return Ok(TextSelectionResult { selections: Vec::new() });
    };
    let start = Instant::now();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<selection::Selection>> {
        let image = image::load_from_memory(&frame.png_bytes)?;
        let (x, y, width, height) = bounds;
        let lines = ocr::recognize_detail_from_image(&image, x, y, width, height)?;
        let blocks = layout::build_blocks(lines, frame.dpi_scale);
        Ok(selection::hit_test_contained_words(&points, &blocks))
    })
    .await;

    let selections = match result {
        Ok(Ok(selections)) => selections,
        Ok(Err(err)) => {
            tracing::warn!(%err, "selected-region OCR failed");
            Vec::new()
        }
        Err(err) => {
            tracing::warn!(%err, "selected-region OCR worker failed");
            Vec::new()
        }
    };
    tracing::info!(
        elapsed_ms = start.elapsed().as_millis() as u64,
        count = selections.len(),
        "selected-region OCR complete"
    );
    Ok(TextSelectionResult { selections: selections.into_iter().map(SelectionPayload::from).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        // A 10x10 red square, PNG-encoded — just enough to exercise real
        // decode/crop/encode round-tripping without a fixture file.
        let img = image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn lens_image_accepts_a_generated_crop() {
        let png = tiny_png();
        let data_url = format!("{PNG_DATA_URL_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(&png));
        assert_eq!(decode_lens_image(&data_url).unwrap(), png);
    }

    #[test]
    fn selection_png_keeps_transparent_pixels() {
        let mut image = image::RgbaImage::from_pixel(2, 2, image::Rgba([24, 42, 60, 255]));
        image.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let data_url = format!("{PNG_DATA_URL_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(&png));
        let decoded = decode_selection_png(&data_url, SAVE_MAX_IMAGE_BYTES).unwrap();
        assert_eq!(decoded, png);
        let restored = image::load_from_memory(&decoded).unwrap().to_rgba8();
        assert_eq!(restored.get_pixel(0, 0)[3], 0);
        assert_eq!(restored.get_pixel(1, 1)[3], 255);
    }

    #[test]
    fn export_formats_are_read_from_the_extension_case_insensitively() {
        assert_eq!(ExportFormat::from_extension("PNG"), Some(ExportFormat::Png));
        assert_eq!(ExportFormat::from_extension("jpeg"), Some(ExportFormat::Jpeg));
        assert_eq!(ExportFormat::from_extension("Jpg"), Some(ExportFormat::Jpeg));
        assert_eq!(ExportFormat::from_extension("webp"), Some(ExportFormat::Webp));
        assert_eq!(ExportFormat::from_extension("gif"), None);
    }

    #[test]
    fn jpeg_export_flattens_transparency_onto_white() {
        let mut image = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 255]));
        image.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let jpeg = encode_export(&png, ExportFormat::Jpeg).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "JPEG magic");
        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        let white = decoded.get_pixel(0, 0);
        assert!(white.0.iter().all(|&channel| channel > 240), "transparent pixel became {white:?}");
    }

    #[test]
    fn webp_export_keeps_transparency_and_png_passes_through() {
        let png = tiny_png();
        assert_eq!(encode_export(&png, ExportFormat::Png).unwrap(), png);

        let mut transparent = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 0])))
            .write_to(&mut std::io::Cursor::new(&mut transparent), image::ImageFormat::Png)
            .unwrap();
        let webp = encode_export(&transparent, ExportFormat::Webp).unwrap();
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
        let decoded = image::load_from_memory(&webp).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0)[3], 0, "lossless WebP keeps alpha");
    }

    #[test]
    fn lens_image_rejects_invalid_data() {
        assert!(decode_lens_image("data:text/plain;base64,abc").is_err());
        assert!(decode_lens_image("data:image/png;base64,!!!").is_err());
        assert!(decode_lens_image("data:image/png;base64,QUJD").is_err());
    }

    #[test]
    fn lens_handoff_contains_browser_form() {
        let png = tiny_png();
        let data_url = format!("{PNG_DATA_URL_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(&png));
        let html = lens_handoff_html(&data_url, 123);
        assert!(html.contains("method=\"POST\" enctype=\"multipart/form-data\""));
        assert!(html.contains("name=\"encoded_image\""));
        assert!(html.contains("https://lens.google.com/v3/upload"));
        assert!(html.contains("Uint8Array.from(atob("));
    }

    #[test]
    fn lens_handoff_serves_the_selection_over_loopback() {
        let data_url = format!("{PNG_DATA_URL_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(tiny_png()));
        let url = search_with_lens(data_url).unwrap();
        let address_and_path = url.strip_prefix("http://127.0.0.1:").unwrap();
        let (port, path) = address_and_path.split_once('/').unwrap();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
        stream.write_all(format!("GET /{path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("name=\"encoded_image\""));
    }


    #[test]
    fn lasso_bounding_box_covers_all_points() {
        let points = vec![(50.0, 100.0), (200.0, 80.0), (120.0, 250.0)];

        let (x, y, width, height) = lasso_bounding_box(&points).unwrap();

        assert_eq!((x, y), (50.0, 80.0));
        assert_eq!((width, height), (150.0, 170.0));
    }

    #[test]
    fn lasso_bounding_box_is_none_for_no_points() {
        assert!(lasso_bounding_box(&[]).is_none());
    }

    #[test]
    fn lasso_bounding_box_is_none_for_a_click_rather_than_a_drag() {
        // Clicking without dragging is normal user behaviour, not an error
        // worth attempting a zero-area crop for.
        assert!(lasso_bounding_box(&[(500.0, 400.0)]).is_none());
        assert!(lasso_bounding_box(&[(500.0, 400.0), (500.4, 400.2)]).is_none());
    }

    #[test]
    fn crop_to_data_url_produces_a_decodable_png() {
        let png = tiny_png();

        let data_url = crop_to_data_url(&png, 2.0, 2.0, 4.0, 4.0).unwrap();

        assert!(data_url.starts_with("data:image/png;base64,"));
        let b64 = data_url.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let cropped = image::load_from_memory(&bytes).unwrap();
        assert_eq!((cropped.width(), cropped.height()), (4, 4));
    }


    #[test]
    fn crop_to_data_url_clamps_a_region_extending_past_the_frame_edge() {
        // The lasso can extend slightly beyond the captured frame (drawn
        // right at the screen edge); the crop should clamp, not error.
        let png = tiny_png();

        let data_url = crop_to_data_url(&png, 8.0, 8.0, 20.0, 20.0).unwrap();

        let b64 = data_url.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        let cropped = image::load_from_memory(&bytes).unwrap();
        assert_eq!((cropped.width(), cropped.height()), (2, 2));
    }

    #[test]
    fn crop_to_data_url_rejects_a_region_entirely_outside_the_frame() {
        let png = tiny_png();

        let result = crop_to_data_url(&png, 100.0, 100.0, 10.0, 10.0);

        assert!(result.is_err());
    }

}
