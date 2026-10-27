//! The time and date for the always-on clock, in the user's own Windows
//! formats (12- or 24-hour, their date order and language).

use windows::core::PCWSTR;
use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, DATE_LONGDATE, TIME_NOSECONDS};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// ("9:41 PM", "Tuesday, 27 October 2026").
pub fn now() -> (String, String) {
    let mut time = [0u16; 64];
    let mut date = [0u16; 96];
    // SAFETY: null locale name means the user's default; the buffers are
    // sized above and the returned lengths include the terminator.
    unsafe {
        let written = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, None, PCWSTR::null(), Some(&mut time));
        let time = text(&time, written);
        let written = GetDateFormatEx(
            PCWSTR::null(),
            DATE_LONGDATE,
            None,
            PCWSTR::null(),
            Some(&mut date),
            PCWSTR::null(),
        );
        (time, text(&date, written))
    }
}

fn text(buffer: &[u16], written: i32) -> String {
    let length = usize::try_from(written).unwrap_or(0).saturating_sub(1);
    String::from_utf16_lossy(&buffer[..length.min(buffer.len())])
}

/// Milliseconds until the next minute begins (plus a little, so the timer
/// fires just after the change rather than just before it).
pub fn until_next_minute() -> u32 {
    // SAFETY: no arguments; returns the local time by value.
    let local = unsafe { GetLocalTime() };
    let into = u32::from(local.wSecond) * 1_000 + u32::from(local.wMilliseconds);
    60_000u32.saturating_sub(into) + 50
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_and_date_are_formatted() {
        let (time, date) = now();
        assert!(time.chars().any(|c| c.is_ascii_digit()), "{time}");
        assert!(date.len() > 6, "{date}");
        let wait = until_next_minute();
        assert!((50..=60_050).contains(&wait));
    }
}
