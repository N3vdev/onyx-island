//! Cursor position -> HMONITOR -> bounds, scale factor, DPI.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

const BASE_DPI: f64 = 96.0;

/// Bounds, scale factor, and raw `HMONITOR` of the monitor under the cursor.
/// `hmonitor` is the raw pointer value; reconstruct the handle with
/// `hmonitor as *mut std::ffi::c_void` where a Win32 `HMONITOR` is needed.
#[derive(Debug, Clone, Copy)]
pub struct MonitorInfo {
    pub hmonitor: isize,
    pub bounds: RECT,
    pub dpi_x: u32,
    pub dpi_y: u32,
    pub scale_factor: f64,
}

pub fn monitor_under_cursor() -> anyhow::Result<MonitorInfo> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor)? };

    let hmonitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if hmonitor.is_invalid() {
        anyhow::bail!("no monitor found under cursor ({}, {})", cursor.x, cursor.y);
    }

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { !GetMonitorInfoW(hmonitor, &mut info).as_bool() } {
        anyhow::bail!("GetMonitorInfoW failed for monitor at ({}, {})", cursor.x, cursor.y);
    }

    let mut dpi_x = BASE_DPI as u32;
    let mut dpi_y = BASE_DPI as u32;
    unsafe { GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)? };

    let result = MonitorInfo {
        hmonitor: hmonitor.0 as isize,
        bounds: info.rcMonitor,
        dpi_x,
        dpi_y,
        scale_factor: dpi_x as f64 / BASE_DPI,
    };

    tracing::info!(
        cursor_x = cursor.x,
        cursor_y = cursor.y,
        hmonitor = result.hmonitor,
        left = result.bounds.left,
        top = result.bounds.top,
        right = result.bounds.right,
        bottom = result.bounds.bottom,
        dpi_x = result.dpi_x,
        dpi_y = result.dpi_y,
        scale_factor = result.scale_factor,
        "monitor under cursor"
    );

    Ok(result)
}
