//! Single-frame grab of the monitor under the cursor.
//!
//! Fast path: a GDI copy of the monitor's pixels (~10-20 ms). Windows Graphics
//! Capture is the fallback — it handles what GDI can't see, but spends
//! ~180 ms starting a session for every frame, which alone blew the
//! hotkey-to-overlay budget.

use std::sync::mpsc;

use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::ImageEncoder;
use windows_capture::frame::{Frame, ImageFormat};
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

/// A captured frame, already PNG-encoded in memory.
pub struct CaptureResult {
    pub width: u32,
    pub height: u32,
    pub png_bytes: Vec<u8>,
    /// True when the frame came back entirely black — see `looks_blank`.
    pub is_blank: bool,
}

/// Windows Graphics Capture returns an all-black frame for DRM-protected or
/// exclusive-fullscreen content rather than failing, so this is the only
/// signal that a capture didn't really work. Sampled on a stride rather than
/// scanned in full: one non-black pixel anywhere rules it out, so real
/// content bails out almost immediately.
///
/// A genuinely all-black screen reads as blank too, but there'd be nothing
/// to select there either, so the message is still the right outcome.
fn looks_blank(rgba: &[u8], width: u32, height: u32) -> bool {
    // Leaves room for the near-black noise some encoders introduce without
    // letting real dark-theme UI (which still has text/chrome well above
    // this) register as blank.
    const CHANNEL_FLOOR: u8 = 8;
    const TARGET_SAMPLES: u32 = 4096;

    let pixels = width.saturating_mul(height);
    if pixels == 0 || rgba.len() < pixels as usize * 4 {
        return false;
    }

    let stride = (pixels / TARGET_SAMPLES).max(1) as usize;
    rgba
        .chunks_exact(4)
        .step_by(stride)
        .all(|px| px[0] <= CHANNEL_FLOOR && px[1] <= CHANNEL_FLOOR && px[2] <= CHANNEL_FLOOR)
}

/// A captured frame is a full screenshot of whatever the user was looking at
/// — credentials, messages, anything on screen. Release builds therefore
/// never put one on disk: the copy for eyeballing captures exists only in
/// debug builds, where the extra synchronous write also isn't competing with
/// the hotkey-to-overlay budget.
#[cfg(debug_assertions)]
fn debug_dump(png_bytes: &[u8]) {
    let dest = std::env::temp_dir().join("cs-debug.png");
    if let Err(err) = std::fs::write(&dest, png_bytes) {
        tracing::debug!(%err, "debug frame dump failed");
    }
}

#[cfg(not(debug_assertions))]
fn debug_dump(_png_bytes: &[u8]) {}

struct SingleFrameCapture {
    result_tx: mpsc::Sender<CaptureResult>,
}

impl GraphicsCaptureApiHandler for SingleFrameCapture {
    type Flags = mpsc::Sender<CaptureResult>;
    type Error = anyhow::Error;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { result_tx: ctx.flags })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let width = frame.width();
        let height = frame.height();

        let mut buffer = frame.buffer()?;
        let raw = buffer.as_nopadding_buffer()?;
        let is_blank = looks_blank(raw, width, height);

        // Encoding is timed separately because its cost scales with how
        // busy the screen is (deflate on a noisy desktop is far more work
        // than on a plain one), which makes it a variable chunk of the
        // hotkey-to-overlay budget rather than a fixed one.
        let encode_start = std::time::Instant::now();
        let png_bytes = ImageEncoder::new(ImageFormat::Png, ColorFormat::Rgba8)
            .encode(raw, width, height)
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        tracing::debug!(
            elapsed_ms = encode_start.elapsed().as_millis() as u64,
            bytes = png_bytes.len(),
            "frame png encoded"
        );

        debug_dump(&png_bytes);

        let _ = self.result_tx.send(CaptureResult { width, height, png_bytes, is_blank });

        // Release the capture session immediately after the first frame.
        capture_control.stop();
        Ok(())
    }
}

/// Grabs a single frame of the monitor identified by `hmonitor` and returns
/// its PNG bytes for the overlay to use directly, without ever persisting
/// them (see `debug_dump`). Blocking — call from a background thread.
///
/// Tries the GDI copy first. A black result there can mean protected or
/// hardware-overlay content GDI can't read, so that case is retried through
/// Windows Graphics Capture, whose own blank result is then authoritative.
pub fn capture_monitor(hmonitor: *mut std::ffi::c_void, hide_cursor: bool) -> anyhow::Result<CaptureResult> {
    let started = std::time::Instant::now();
    match gdi::grab(hmonitor, hide_cursor) {
        Ok((width, height, rgba)) if !looks_blank(&rgba, width, height) => {
            let grabbed_ms = started.elapsed().as_millis() as u64;
            let png_bytes = fast_png(&rgba, width, height)?;
            tracing::debug!(grabbed_ms, total_ms = started.elapsed().as_millis() as u64, bytes = png_bytes.len(), "gdi capture");
            debug_dump(&png_bytes);
            return Ok(CaptureResult { width, height, png_bytes, is_blank: false });
        }
        Ok(_) => tracing::debug!("gdi capture came back black; retrying with Windows Graphics Capture"),
        Err(err) => tracing::debug!(%err, "gdi capture failed; using Windows Graphics Capture"),
    }
    capture_with_wgc(hmonitor, hide_cursor)
}

/// PNG with the fastest deflate setting: on a busy 1080p screen this is
/// several times quicker than the default encoder for a somewhat larger
/// file, and the file never leaves memory, so speed is what matters here.
fn fast_png(rgba: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder as _;
    let mut out = Vec::with_capacity(rgba.len() / 6);
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)?;
    Ok(out)
}

fn capture_with_wgc(hmonitor: *mut std::ffi::c_void, hide_cursor: bool) -> anyhow::Result<CaptureResult> {
    let monitor = Monitor::from_raw_hmonitor(hmonitor);
    let (result_tx, result_rx) = mpsc::channel();

    let settings = Settings::new(
        monitor,
        // Default includes the pointer; WithoutCursor asks the capture API
        // itself to leave it out, so it is absent from the pixels rather
        // than painted over afterwards (nothing to leak into OCR or a crop).
        if hide_cursor { CursorCaptureSettings::WithoutCursor } else { CursorCaptureSettings::Default },
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        result_tx,
    );

    SingleFrameCapture::start(settings).map_err(|err| anyhow::anyhow!(err.to_string()))?;

    result_rx.recv().map_err(|_| anyhow::anyhow!("capture handler exited without a frame"))
}

/// Fires and discards one capture at startup, so one-time costs (GDI/WIC
/// set-up, and WGC's D3D warm-up if it is ever needed) aren't paid on the
/// user's first press. Call on a background thread; failures are non-fatal.
pub fn warm_up() {
    let Ok(monitor) = Monitor::primary() else {
        tracing::debug!("capture warm-up skipped: no primary monitor");
        return;
    };

    let start = std::time::Instant::now();
    match capture_monitor(monitor.as_raw_hmonitor(), false) {
        Ok(_) => tracing::debug!(elapsed_ms = start.elapsed().as_millis() as u64, "capture warm-up complete"),
        Err(err) => tracing::debug!(%err, "capture warm-up failed, continuing anyway"),
    }
}

/// The GDI copy. Everything here is plain Win32 on handles this function
/// owns, released on every path.
mod gdi {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HMONITOR, MONITORINFO,
        SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, ICONINFO,
    };

    /// Returns (width, height, RGBA pixels) for the monitor.
    pub fn grab(hmonitor: *mut std::ffi::c_void, hide_cursor: bool) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        unsafe {
            let monitor = HMONITOR(hmonitor);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                anyhow::bail!("GetMonitorInfoW failed");
            }
            let rect = info.rcMonitor;
            let (width, height) = ((rect.right - rect.left) as i32, (rect.bottom - rect.top) as i32);
            if width <= 0 || height <= 0 {
                anyhow::bail!("monitor has no area");
            }

            let screen = GetDC(HWND::default());
            if screen.is_invalid() {
                anyhow::bail!("GetDC failed");
            }
            let memory = CreateCompatibleDC(screen);
            let header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let bitmap_info = BITMAPINFO { bmiHeader: header, ..Default::default() };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(memory, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0);
            let result = (|| -> anyhow::Result<Vec<u8>> {
                let bitmap = bitmap.map_err(|err| anyhow::anyhow!("CreateDIBSection: {err}"))?;
                if bits.is_null() {
                    anyhow::bail!("no DIB memory");
                }
                let previous = SelectObject(memory, bitmap);
                // CAPTUREBLT includes layered windows (menus, tooltips, the
                // taskbar's flyouts), which a plain SRCCOPY would miss.
                let copied = BitBlt(memory, 0, 0, width, height, screen, rect.left, rect.top, SRCCOPY | CAPTUREBLT);
                if copied.is_ok() && !hide_cursor {
                    draw_cursor(memory, rect.left, rect.top);
                }
                SelectObject(memory, previous);
                let pixels_result = copied.map(|_| {
                    let source = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
                    // BGRA (with an undefined alpha byte) → opaque RGBA.
                    let mut rgba = Vec::with_capacity(source.len());
                    for px in source.chunks_exact(4) {
                        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                    rgba
                });
                let _ = DeleteObject(bitmap);
                pixels_result.map_err(|err| anyhow::anyhow!("BitBlt: {err}"))
            })();
            let _ = DeleteDC(memory);
            ReleaseDC(HWND::default(), screen);
            Ok((width as u32, height as u32, result?))
        }
    }

    /// GDI copies don't include the pointer; draw it in, as WGC would have.
    unsafe fn draw_cursor(target: windows::Win32::Graphics::Gdi::HDC, origin_x: i32, origin_y: i32) {
        let mut cursor = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut cursor).is_err() || cursor.flags.0 & CURSOR_SHOWING.0 == 0 || cursor.hCursor.is_invalid() {
            return;
        }
        let mut icon = ICONINFO::default();
        if GetIconInfo(cursor.hCursor, &mut icon).is_err() {
            return;
        }
        let POINT { x, y } = cursor.ptScreenPos;
        let _ = DrawIconEx(
            target,
            x - origin_x - icon.xHotspot as i32,
            y - origin_y - icon.yHotspot as i32,
            cursor.hCursor,
            0,
            0,
            0,
            None,
            DI_NORMAL,
        );
        if !icon.hbmMask.is_invalid() {
            let _ = DeleteObject(icon.hbmMask);
        }
        if !icon.hbmColor.is_invalid() {
            let _ = DeleteObject(icon.hbmColor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grabs the real primary monitor through the GDI path and writes it to
    /// `CAPTURE_OUT` for a look. Needs a desktop session, so not run by default:
    /// `CAPTURE_OUT=out.png cargo test --lib -- --ignored gdi_grab`.
    #[test]
    #[ignore = "needs a desktop session"]
    fn gdi_grab_of_the_primary_monitor() {
        let Ok(out) = std::env::var("CAPTURE_OUT") else { return };
        let monitor = Monitor::primary().expect("a primary monitor");
        let start = std::time::Instant::now();
        let (w, h, rgba) = gdi::grab(monitor.as_raw_hmonitor(), false).expect("gdi grab");
        let grabbed = start.elapsed();
        assert!(!looks_blank(&rgba, w, h));
        image::RgbaImage::from_raw(w, h, rgba).unwrap().save(&out).unwrap();
        println!("grabbed {w}x{h} in {grabbed:?}");
    }

    fn rgba(width: u32, height: u32, fill: [u8; 4]) -> Vec<u8> {
        fill.iter().copied().cycle().take((width * height * 4) as usize).collect()
    }

    #[test]
    fn all_black_frame_reads_as_blank() {
        assert!(looks_blank(&rgba(10, 10, [0, 0, 0, 255]), 10, 10));
    }

    #[test]
    fn near_black_frame_still_reads_as_blank() {
        assert!(looks_blank(&rgba(10, 10, [4, 4, 4, 255]), 10, 10));
    }

    #[test]
    fn a_single_lit_pixel_rules_out_blank() {
        // 10x10 is small enough that every pixel is sampled, so this covers
        // a lit pixel anywhere rather than just the first one.
        let mut buf = rgba(10, 10, [0, 0, 0, 255]);
        let last = buf.len() - 4;
        buf[last] = 200;
        assert!(!looks_blank(&buf, 10, 10));
    }

    #[test]
    fn dark_theme_ui_is_not_blank() {
        // Near-black background with text/chrome well above the floor.
        let mut buf = rgba(10, 10, [12, 12, 14, 255]);
        buf[40] = 240;
        buf[41] = 240;
        buf[42] = 240;
        assert!(!looks_blank(&buf, 10, 10));
    }

    #[test]
    fn a_buffer_smaller_than_its_dimensions_is_not_treated_as_blank() {
        assert!(!looks_blank(&rgba(2, 2, [0, 0, 0, 255]), 10, 10));
    }
}
