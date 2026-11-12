//! The night light: when it is on, and how warm the screen gets.
//!
//! The screen is warmed through the display's gamma ramp (a colour table the
//! graphics card already applies to every frame), so it costs nothing to
//! draw. This module only decides the table; the app applies it.

/// The warmest colour, at full intensity: a warm lamp. Windows refuses
/// warmer gamma ramps unless a registry limit is lifted, so this is the
/// warmest that works everywhere (f.lux's own night default).
pub const WARMEST_KELVIN: f32 = 3400.0;
/// Daylight: the screen as it is.
pub const DAYLIGHT_KELVIN: f32 = 6500.0;

/// "20:00" as minutes after midnight. Accepts "8:00", "08:00" and "8".
#[must_use]
pub fn parse_time(text: &str) -> Option<u32> {
    let text = text.trim();
    let (hour, minute) = text.split_once(':').unwrap_or((text, "0"));
    let (hour, minute) = (hour.trim().parse::<u32>().ok()?, minute.trim().parse::<u32>().ok()?);
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

/// Minutes after midnight as "20:00".
#[must_use]
pub fn format_time(minutes: u32) -> String {
    let minutes = minutes % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Minutes after midnight as the clock shows them: "8:00 PM".
#[must_use]
pub fn display_time(minutes: u32) -> String {
    let minutes = minutes % (24 * 60);
    let (hour, minute) = (minutes / 60, minutes % 60);
    let twelve = if hour % 12 == 0 { 12 } else { hour % 12 };
    format!("{twelve}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" })
}

/// Whether `now` (minutes after midnight) is inside the night from `start`
/// to `end`, which may run past midnight. Equal times mean always on.
#[must_use]
pub fn is_night(now: u32, start: u32, end: u32) -> bool {
    if start == end {
        return true;
    }
    if start < end {
        (start..end).contains(&now)
    } else {
        now >= start || now < end
    }
}

/// Minutes from `now` until the clock next reads `at` (0 when it does now).
#[must_use]
pub fn minutes_until(now: u32, at: u32) -> u32 {
    (at + 24 * 60 - now % (24 * 60)) % (24 * 60)
}

/// The colour temperature for an intensity from 0 (off) to 1 (warmest).
#[must_use]
pub fn kelvin(intensity: f32) -> f32 {
    DAYLIGHT_KELVIN - intensity.clamp(0.0, 1.0) * (DAYLIGHT_KELVIN - WARMEST_KELVIN)
}

/// How much of red, green and blue a white of `kelvin` keeps, compared with
/// daylight (each 0..=1). Fitted to the blackbody colours (after Tanner
/// Helland's approximation), so warmer means less blue, then less green.
#[must_use]
pub fn white_point(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40_000.0) / 100.0;
    let red = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let green = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_16 * (t - 60.0).powf(-0.075_514_85)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    let raw = [red, green, blue].map(|c| c.clamp(0.0, 255.0) / 255.0);
    // Relative to daylight, so 6500 K leaves the screen exactly as it is.
    let daylight = white_point_raw(DAYLIGHT_KELVIN);
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = (raw[i] / daylight[i]).clamp(0.0, 1.0);
    }
    out
}

fn white_point_raw(kelvin: f32) -> [f32; 3] {
    let t = kelvin / 100.0;
    let green = 99.470_8 * t.ln() - 161.119_57;
    let blue = 138.517_73 * (t - 10.0).ln() - 305.044_8;
    [1.0, (green / 255.0).clamp(0.01, 1.0), (blue / 255.0).clamp(0.01, 1.0)]
}

/// The gamma ramp (256 steps for red, green and blue) for `multipliers`.
#[must_use]
pub fn ramp(multipliers: [f32; 3]) -> [[u16; 256]; 3] {
    let mut table = [[0u16; 256]; 3];
    for (channel, row) in table.iter_mut().enumerate() {
        for (step, value) in row.iter_mut().enumerate() {
            *value = (step as f32 * 257.0 * multipliers[channel]).round().clamp(0.0, 65_535.0) as u16;
        }
    }
    table
}

/// Mixes daylight with the night colour: `amount` 0 is the screen as it
/// is, 1 the full night colour (used while fading).
#[must_use]
pub fn blend(night: [f32; 3], amount: f32) -> [f32; 3] {
    let amount = amount.clamp(0.0, 1.0);
    night.map(|c| 1.0 + (c - 1.0) * amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse_print_and_wrap_past_midnight() {
        assert_eq!(parse_time("20:00"), Some(1200));
        assert_eq!(parse_time("5:30"), Some(330));
        assert_eq!(parse_time("8"), Some(480));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("soon"), None);
        assert_eq!(format_time(300), "05:00");
        assert_eq!(display_time(1200), "8:00 PM");
        assert_eq!(display_time(0), "12:00 AM");
        assert_eq!(display_time(330), "5:30 AM");
        // 8 PM to 5 AM.
        let (start, end) = (1200, 300);
        assert!(is_night(1200, start, end));
        assert!(is_night(23 * 60, start, end));
        assert!(is_night(2 * 60, start, end));
        assert!(!is_night(300, start, end));
        assert!(!is_night(12 * 60, start, end));
        // A night that does not cross midnight.
        assert!(is_night(13 * 60, 12 * 60, 14 * 60));
        assert!(!is_night(15 * 60, 12 * 60, 14 * 60));
        assert_eq!(minutes_until(19 * 60, 1200), 60);
        assert_eq!(minutes_until(21 * 60, 1200), 23 * 60);
        assert_eq!(minutes_until(1200, 1200), 0);
    }

    #[test]
    fn warmer_means_less_blue_then_less_green_and_daylight_changes_nothing() {
        let day = white_point(kelvin(0.0));
        assert!(day.iter().all(|c| (c - 1.0).abs() < 1e-3), "{day:?}");
        let half = white_point(kelvin(0.5));
        let full = white_point(kelvin(1.0));
        assert_eq!(half[0], 1.0);
        assert!(half[2] < half[1] && half[1] < 1.0, "{half:?}");
        assert!(full[2] < half[2] && full[1] < half[1], "{full:?}");
        let table = ramp([1.0, 1.0, 1.0]);
        assert_eq!(table[0][0], 0);
        assert_eq!(table[0][255], 65_535);
        assert_eq!(table[2][128], 128 * 257);
        let warm = ramp(full);
        assert!(warm[2][255] < warm[1][255] && warm[1][255] < warm[0][255]);
        assert_eq!(blend(full, 0.0), [1.0, 1.0, 1.0]);
        assert_eq!(blend(full, 1.0), full);
    }
}
