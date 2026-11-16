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

/// Seconds the local clock is ahead of UTC (negative behind), to the minute.
pub fn utc_offset() -> i64 {
    use windows::Win32::System::SystemInformation::GetSystemTime;
    // SAFETY: no arguments; both return times by value.
    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let offset = civil_seconds(&local) - civil_seconds(&utc);
    (offset as f64 / 60.0).round() as i64 * 60
}

/// Whether the user's time format is 12-hour ("1:00 PM", not "13:00").
pub fn twelve_hour() -> bool {
    use windows::Win32::Foundation::SYSTEMTIME;
    let afternoon = SYSTEMTIME { wYear: 2026, wMonth: 1, wDay: 1, wHour: 13, ..Default::default() };
    let mut buffer = [0u16; 64];
    // SAFETY: as in `now`.
    let written = unsafe {
        GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&afternoon), PCWSTR::null(), Some(&mut buffer))
    };
    !text(&buffer, written).contains("13")
}

fn civil_seconds(time: &windows::Win32::Foundation::SYSTEMTIME) -> i64 {
    islet_core::tasks::civil_seconds(
        time.wYear.into(),
        time.wMonth.into(),
        time.wDay.into(),
        time.wHour.into(),
        time.wMinute.into(),
        time.wSecond.into(),
    )
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

    #[test]
    fn the_utc_offset_is_whole_minutes() {
        assert_eq!(utc_offset() % 60, 0);
        assert!(utc_offset().abs() <= 14 * 3600);
    }
}
