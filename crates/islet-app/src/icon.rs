use std::ffi::c_void;

use windows::core::BOOL;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};

const SIZE: usize = 32;

/// Builds the tray icon at runtime.
///
/// Keeping it procedural avoids a binary asset and produces a clean,
/// alpha-blended mark at every DPI: a white island pill with a black waveform.
pub fn create() -> windows::core::Result<HICON> {
    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    draw_island(&mut pixels);

    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE as i32,
            biHeight: -(SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut c_void = std::ptr::null_mut();
    let color =
        unsafe { CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    if bits.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(color.0));
        }
        return Err(windows::core::Error::from_thread());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
    }

    // A 32-bpp icon uses the alpha channel. The legacy AND mask is all zeroes
    // (fully opaque) so the alpha channel is authoritative.
    let mask_bits = [0u8; (SIZE * SIZE) / 8];
    let mask = unsafe {
        CreateBitmap(
            SIZE as i32,
            SIZE as i32,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        )
    };

    let info = ICONINFO {
        fIcon: BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let icon = unsafe { CreateIconIndirect(&info) };

    unsafe {
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteObject(HGDIOBJ(color.0));
    }

    icon
}

/// Deletes an icon created by [`create`].
pub fn destroy(icon: HICON) {
    if !icon.is_invalid() {
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }
}

fn draw_island(pixels: &mut [u8]) {
    const WHITE: [f64; 3] = [245.0, 245.0, 245.0];
    const BLACK: [f64; 3] = [5.0, 5.0, 5.0];

    let center = (SIZE as f64 - 1.0) / 2.0;
    let half_width = 11.0;
    let half_height = 6.5;
    let radius = 6.0;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = x as f64 - center;
            let py = y as f64 - center;

            let pill = rounded_rect_coverage(px, py, half_width, half_height, radius);
            if pill <= 0.0 {
                continue;
            }

            // Three bars of a tiny waveform, clipped to the pill.
            let bar = bar_coverage(px, py);
            let coverage = pill;
            let (rgb, alpha) = if bar > 0.0 {
                let mix = bar.min(pill);
                (BLACK, mix)
            } else {
                (WHITE, coverage)
            };

            let index = (y * SIZE + x) * 4;
            let alpha = alpha.clamp(0.0, 1.0);
            pixels[index] = (rgb[2] * alpha).round() as u8; // B
            pixels[index + 1] = (rgb[1] * alpha).round() as u8; // G
            pixels[index + 2] = (rgb[0] * alpha).round() as u8; // R
            pixels[index + 3] = (alpha * 255.0).round() as u8;
        }
    }
}

fn rounded_rect_coverage(px: f64, py: f64, half_width: f64, half_height: f64, radius: f64) -> f64 {
    let qx = px.abs() - half_width + radius;
    let qy = py.abs() - half_height + radius;
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    let distance = outside + inside - radius;
    (0.5 - distance).clamp(0.0, 1.0)
}

fn bar_coverage(px: f64, py: f64) -> f64 {
    let bars = [(-4.0, 2.2, 2.0), (0.0, 4.5, 1.6), (4.0, 3.2, 2.0)];
    let mut coverage: f64 = 0.0;
    for (x_center, half_height, half_width) in bars {
        let dx = (px - x_center).abs() - half_width;
        let dy = py.abs() - half_height;
        let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0);
        coverage = coverage.max((0.5 - distance).clamp(0.0, 1.0));
    }
    coverage
}
