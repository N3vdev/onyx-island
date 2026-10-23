//! Platform-free now-playing logic: timeline interpolation, album-art colour,
//! session preference, expanded-view hit zones, and visualizer bar levels.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::geometry::Rect;

/// Decoded album art: premultiplied BGRA, row-major, `width * 4` stride.
///
/// Equality is by `id` so snapshots can be compared cheaply; a new track or
/// new thumbnail always gets a new id.
#[derive(Clone, Debug)]
pub struct ArtImage {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<[u8]>,
}

impl PartialEq for ArtImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// A playback position sample. GSMTC timeline updates are sparse, so the
/// position is extrapolated from the last sample while playing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    /// Seconds into the track at `sampled_at`.
    pub position: f64,
    /// Track length in seconds. Zero when unknown.
    pub duration: f64,
    /// Monotonic seconds (the island clock) when `position` was true.
    pub sampled_at: f64,
    pub rate: f64,
}

impl Timeline {
    #[must_use]
    pub fn position_at(&self, now: f64, playing: bool) -> f64 {
        let elapsed = if playing {
            (now - self.sampled_at).max(0.0) * self.rate
        } else {
            0.0
        };
        let position = self.position + elapsed;
        if self.duration > 0.0 {
            position.clamp(0.0, self.duration)
        } else {
            position.max(0.0)
        }
    }

    #[must_use]
    pub fn progress_at(&self, now: f64, playing: bool) -> f32 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.position_at(now, playing) / self.duration) as f32
    }
}

/// Formats seconds as `m:ss` (or `h:mm:ss` for an hour or more).
#[must_use]
pub fn format_clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (total / 3600, (total / 60) % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// One GSMTC session as seen by [`choose_session`].
#[derive(Clone, Copy, Debug)]
pub struct SessionCandidate<'a> {
    pub app_id: &'a str,
    pub playing: bool,
    /// This session was the last one seen playing.
    pub last_played: bool,
}

/// Picks the session to show. Something playing always beats something
/// paused. When nothing plays, the session that played last wins (so
/// pausing keeps your song on screen). Then Spotify, then the system's
/// current session, then the first one.
#[must_use]
pub fn choose_session(sessions: &[SessionCandidate<'_>], current: Option<usize>) -> Option<usize> {
    let is_spotify = |index: usize| {
        sessions[index]
            .app_id
            .to_ascii_lowercase()
            .contains("spotify")
    };
    let current = current.filter(|&index| index < sessions.len());
    (0..sessions.len()).max_by_key(|&index| {
        (
            sessions[index].playing,
            !sessions[index].playing && sessions[index].last_played,
            is_spotify(index),
            current == Some(index),
            // max_by_key keeps the last maximum; prefer earlier sessions.
            std::cmp::Reverse(index),
        )
    })
}

/// A short human name for a GSMTC source app id (an AUMID or exe name).
#[must_use]
pub fn source_display_name(app_id: &str) -> String {
    let lower = app_id.to_ascii_lowercase();
    const KNOWN: [(&str, &str); 12] = [
        ("spotify", "Spotify"),
        ("msedge", "Edge"),
        ("chrome", "Chrome"),
        ("firefox", "Firefox"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("vivaldi", "Vivaldi"),
        ("applemusic", "Apple Music"),
        ("zunemusic", "Media Player"),
        ("vlc", "VLC"),
        ("foobar", "foobar2000"),
        ("tidal", "TIDAL"),
    ];
    if let Some((_, name)) = KNOWN.iter().find(|(needle, _)| lower.contains(needle)) {
        return (*name).to_owned();
    }
    // "Publisher.App_hash!Entry" -> "App"; "something.exe" -> "something".
    let base = app_id.rsplit(['\\', '/']).next().unwrap_or(app_id);
    let base = base.split('!').next().unwrap_or(base);
    let base = base.split('_').next().unwrap_or(base);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    let base = base.rsplit('.').next().unwrap_or(base);
    // Chromium-based browsers register under a hashed AUMID.
    if base.len() >= 12 && base.chars().all(|c| c.is_ascii_hexdigit()) {
        return "Browser".to_owned();
    }
    if base.is_empty() {
        "Media".to_owned()
    } else {
        base.to_owned()
    }
}

/// Dominant, readable-on-black colour of an image (premultiplied BGRA).
/// Returns `None` for greyscale or empty art so callers keep their default.
#[must_use]
pub fn dominant_color(pixels: &[u8], width: u32, height: u32) -> Option<[u8; 3]> {
    const GRID: usize = 16;
    let (width, height) = (width as usize, height as usize);
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return None;
    }

    // Box-downsample to a 16x16 grid of RGB samples in 0..1.
    let mut samples = Vec::with_capacity(GRID * GRID);
    for gy in 0..GRID {
        let (y0, y1) = (
            gy * height / GRID,
            ((gy + 1) * height / GRID).max(gy * height / GRID + 1),
        );
        for gx in 0..GRID {
            let (x0, x1) = (
                gx * width / GRID,
                ((gx + 1) * width / GRID).max(gx * width / GRID + 1),
            );
            let mut sum = [0.0f32; 3];
            let mut count = 0.0f32;
            for y in y0..y1.min(height) {
                for x in x0..x1.min(width) {
                    let i = (y * width + x) * 4;
                    sum[0] += f32::from(pixels[i + 2]);
                    sum[1] += f32::from(pixels[i + 1]);
                    sum[2] += f32::from(pixels[i]);
                    count += 1.0;
                }
            }
            if count > 0.0 {
                samples.push(sum.map(|channel| channel / count / 255.0));
            }
        }
    }

    // Deterministic k-means seeded across the luminance range.
    const K: usize = 5;
    samples.sort_by(|a, b| luminance(*a).total_cmp(&luminance(*b)));
    let mut centers: Vec<[f32; 3]> = (0..K)
        .map(|k| samples[(k * 2 + 1) * samples.len() / (K * 2)])
        .collect();
    let mut assignment = vec![0usize; samples.len()];
    for _ in 0..10 {
        for (sample, slot) in samples.iter().zip(assignment.iter_mut()) {
            *slot = (0..K)
                .min_by(|&a, &b| {
                    distance2(*sample, centers[a]).total_cmp(&distance2(*sample, centers[b]))
                })
                .unwrap_or(0);
        }
        let mut sums = [[0.0f32; 3]; K];
        let mut counts = [0usize; K];
        for (sample, &k) in samples.iter().zip(&assignment) {
            for c in 0..3 {
                sums[k][c] += sample[c];
            }
            counts[k] += 1;
        }
        for k in 0..K {
            if counts[k] > 0 {
                centers[k] = sums[k].map(|sum| sum / counts[k] as f32);
            }
        }
    }

    let mut counts = [0usize; K];
    for &k in &assignment {
        counts[k] += 1;
    }
    let (best, score) = (0..K)
        .map(|k| {
            let (_, saturation, lightness) = rgb_to_hsl(centers[k]);
            let usable = if (0.08..=0.94).contains(&lightness) {
                1.0
            } else {
                0.15
            };
            (k, counts[k] as f32 * (0.15 + saturation) * usable)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if score <= 0.0 {
        return None;
    }

    let (hue, saturation, lightness) = rgb_to_hsl(centers[best]);
    if saturation < 0.12 {
        return None;
    }
    let rgb = hsl_to_rgb(hue, saturation.max(0.45), lightness.clamp(0.52, 0.70));
    Some(rgb.map(|channel| (channel * 255.0).round().clamp(0.0, 255.0) as u8))
}

fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn rgb_to_hsl(rgb: [f32; 3]) -> (f32, f32, f32) {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let lightness = (max + min) * 0.5;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs()).max(f32::EPSILON);
    let hue = if (max - rgb[0]).abs() <= f32::EPSILON {
        ((rgb[1] - rgb[2]) / delta).rem_euclid(6.0)
    } else if (max - rgb[1]).abs() <= f32::EPSILON {
        (rgb[2] - rgb[0]) / delta + 2.0
    } else {
        (rgb[0] - rgb[1]) / delta + 4.0
    };
    (hue * 60.0, saturation.min(1.0), lightness)
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma * 0.5;
    [r + m, g + m, b + m]
}

/// Something the user can do in the expanded media view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaAction {
    Previous,
    PlayPause,
    Next,
    /// Seek to a fraction of the track, 0..=1.
    Seek(f32),
    /// Open or activate the source music application.
    OpenApp,
}

/// The expanded control under the pointer. Separate from track data so hover
/// feedback never starts a content transition or touches playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaHover {
    Artwork,
    Title,
    Source,
    Previous,
    PlayPause,
    Next,
    Seek,
    Volume,
}

/// Uses the same hit zones and precedence as mouse presses. Unavailable
/// sliders and invisible source shortcuts do not advertise an action.
#[must_use]
pub fn expanded_hover_at(
    x: f32,
    y: f32,
    pill_width: f32,
    seekable: bool,
    has_volume: bool,
    has_source: bool,
) -> Option<MediaHover> {
    if seekable && scrub_fraction(x, y, pill_width, false).is_some() {
        return Some(MediaHover::Seek);
    }
    if has_volume && volume_fraction(x, y, pill_width, false).is_some() {
        return Some(MediaHover::Volume);
    }
    match expanded_action_at(x, y, pill_width) {
        Some(MediaAction::Previous) => Some(MediaHover::Previous),
        Some(MediaAction::PlayPause) => Some(MediaHover::PlayPause),
        Some(MediaAction::Next) => Some(MediaHover::Next),
        Some(MediaAction::OpenApp) if expanded::ART_HIT.contains(x, y) => Some(MediaHover::Artwork),
        Some(MediaAction::OpenApp) if expanded::title_area(pill_width).contains(x, y) => {
            Some(MediaHover::Title)
        }
        Some(MediaAction::OpenApp) if has_source => Some(MediaHover::Source),
        _ => None,
    }
}

/// Layout of the expanded media view, relative to the pill's top-left, in
/// logical pixels. The renderer draws with these and the UI thread hit-tests
/// with them, so they cannot drift apart.
pub mod expanded {
    use super::Rect;

    /// The music pane stays fixed as features are added on its right.
    pub const PLAYER_WIDTH: f32 = 280.0;
    pub const ART: Rect = Rect::new(14.0, 14.0, 84.0, 84.0);
    pub const ART_RADIUS: f32 = 24.0;
    pub const TEXT_X: f32 = 112.0;
    pub const TEXT_WIDTH: f32 = 152.0;
    pub const TITLE_Y: f32 = 18.0;
    pub const TITLE_HEIGHT: f32 = 19.0;
    pub const ARTIST_Y: f32 = 38.0;
    pub const ARTIST_HEIGHT: f32 = 16.0;
    pub const CHIP_Y: f32 = 16.0;
    pub const CHIP_HEIGHT: f32 = 36.0;
    pub const SCRUBBER_Y: f32 = 90.0;
    pub const SCRUBBER_HEIGHT: f32 = 2.0;
    /// Vertical slack around the thin scrubber for comfortable clicking.
    pub const SCRUBBER_HIT_SLOP: f32 = 6.0;
    pub const TIMES_Y: f32 = 96.0;
    pub const TIMES_HEIGHT: f32 = 12.0;
    pub const CONTROLS_Y: f32 = 68.0;
    pub const CONTROL_SPACING: f32 = 42.0;
    pub const CONTROL_HIT_RADIUS: f32 = 16.0;
    pub const VOLUME_TRACK_WIDTH: f32 = 78.0;
    pub const VOLUME_TRACK_HEIGHT: f32 = 3.0;
    /// Gap between the speaker glyph and the start of the track.
    pub const VOLUME_ICON_GAP: f32 = 20.0;
    pub const VOLUME_HIT_SLOP: f32 = 11.0;

    #[must_use]
    pub fn scrubber(_pill_width: f32) -> Rect {
        Rect::new(TEXT_X, SCRUBBER_Y, TEXT_WIDTH, SCRUBBER_HEIGHT)
    }

    /// Centres of previous, play/pause, next.
    #[must_use]
    pub fn controls(_pill_width: f32) -> [(f32, f32); 3] {
        let center = TEXT_X + 56.0;
        [
            (center - CONTROL_SPACING, CONTROLS_Y),
            (center, CONTROLS_Y),
            (center + CONTROL_SPACING, CONTROLS_Y),
        ]
    }

    /// First feature column. Additional columns can grow to its right without
    /// moving playback controls or their hit zones.
    #[must_use]
    pub fn feature_area(pill_width: f32) -> Rect {
        Rect::new(
            PLAYER_WIDTH + 14.0,
            16.0,
            (pill_width - PLAYER_WIDTH - 28.0).clamp(64.0, 140.0),
            80.0,
        )
    }

    /// The source shortcut in the upper feature slot.
    #[must_use]
    pub fn chip(pill_width: f32, _source: &str) -> Rect {
        let area = feature_area(pill_width);
        Rect::new(area.x, CHIP_Y, area.width, CHIP_HEIGHT)
    }

    #[must_use]
    pub fn volume_area(pill_width: f32) -> Rect {
        let area = feature_area(pill_width);
        Rect::new(area.x, 60.0, area.width, 36.0)
    }

    /// The volume slider track, right-aligned with the card's margin.
    #[must_use]
    pub fn volume_track(pill_width: f32) -> Rect {
        let area = volume_area(pill_width);
        let width = VOLUME_TRACK_WIDTH.min(area.width - 54.0);
        Rect::new(
            area.right() - 18.0 - width,
            area.center().1 - VOLUME_TRACK_HEIGHT * 0.5,
            width,
            VOLUME_TRACK_HEIGHT,
        )
    }

    /// Centre of the speaker glyph left of the volume track.
    #[must_use]
    pub fn volume_icon_center(pill_width: f32) -> (f32, f32) {
        (
            volume_track(pill_width).x - VOLUME_ICON_GAP,
            volume_area(pill_width).center().1,
        )
    }

    /// Clickable region for the album art.
    pub const ART_HIT: Rect = ART;

    /// Clickable region for the title and artist, clear of transport controls.
    #[must_use]
    pub fn title_area(_pill_width: f32) -> Rect {
        Rect::new(
            TEXT_X,
            TITLE_Y,
            TEXT_WIDTH,
            ARTIST_Y + ARTIST_HEIGHT - TITLE_Y,
        )
    }
}

/// Maps a point inside the expanded media pill to an action.
#[must_use]
pub fn expanded_action_at(x: f32, y: f32, pill_width: f32) -> Option<MediaAction> {
    let [previous, play, next] = expanded::controls(pill_width);
    let hit = |(cx, cy): (f32, f32)| {
        (x - cx).powi(2) + (y - cy).powi(2) <= expanded::CONTROL_HIT_RADIUS.powi(2)
    };
    if hit(previous) {
        return Some(MediaAction::Previous);
    }
    if hit(play) {
        return Some(MediaAction::PlayPause);
    }
    if hit(next) {
        return Some(MediaAction::Next);
    }
    if let Some(fraction) = scrub_fraction(x, y, pill_width, false) {
        return Some(MediaAction::Seek(fraction));
    }
    if expanded::ART_HIT.contains(x, y)
        || expanded::title_area(pill_width).contains(x, y)
        || expanded::chip(pill_width, "").contains(x, y)
    {
        return Some(MediaAction::OpenApp);
    }
    None
}

/// Fraction along the scrubber for `x`. With `dragging`, any `y` is accepted
/// and `x` is clamped, so a drag can leave the bar without stopping.
#[must_use]
pub fn scrub_fraction(x: f32, y: f32, pill_width: f32, dragging: bool) -> Option<f32> {
    let bar = expanded::scrubber(pill_width);
    let slop = expanded::SCRUBBER_HIT_SLOP;
    let inside = x >= bar.x - slop
        && x <= bar.right() + slop
        && y >= bar.y - slop
        && y <= bar.bottom() + slop;
    (dragging || inside).then(|| ((x - bar.x) / bar.width).clamp(0.0, 1.0))
}

/// Fraction along the volume slider for `x`, or `None` when the point is
/// outside the slider and its speaker button. With `dragging`, any `y` is
/// accepted and `x` is clamped, so a drag can leave the row without stopping.
/// Clicking the speaker itself yields 0 (mute).
#[must_use]
pub fn volume_fraction(x: f32, y: f32, pill_width: f32, dragging: bool) -> Option<f32> {
    let track = expanded::volume_track(pill_width);
    let (icon_x, center_y) = expanded::volume_icon_center(pill_width);
    let slop = expanded::VOLUME_HIT_SLOP;
    let inside = x >= icon_x - 14.0
        && x <= track.right() + slop
        && (center_y - slop..=center_y + slop).contains(&y);
    (dragging || inside).then(|| ((x - track.x) / track.width).clamp(0.0, 1.0))
}

pub const BAR_COUNT: usize = 5;

/// Visualizer levels shared between the audio thread (writer) and the render
/// thread (reader) without locks.
#[derive(Debug, Default)]
pub struct BarLevels {
    levels: [AtomicU32; BAR_COUNT],
    /// Monotonic milliseconds of the last write; 0 = never.
    updated_ms: AtomicU64,
}

impl BarLevels {
    pub fn store(&self, levels: [f32; BAR_COUNT], now_ms: u64) {
        for (slot, level) in self.levels.iter().zip(levels) {
            slot.store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        self.updated_ms.store(now_ms.max(1), Ordering::Release);
    }

    /// Levels if they were written within `max_age_ms` of `now_ms`.
    #[must_use]
    pub fn load_fresh(&self, now_ms: u64, max_age_ms: u64) -> Option<[f32; BAR_COUNT]> {
        let updated = self.updated_ms.load(Ordering::Acquire);
        if updated == 0 || now_ms.saturating_sub(updated) > max_age_ms {
            return None;
        }
        Some(std::array::from_fn(|i| {
            f32::from_bits(self.levels[i].load(Ordering::Relaxed))
        }))
    }
}

/// Turns mono PCM windows into five smoothed band levels (0..1).
///
/// A 512-point Hann-windowed FFT; bands are roughly log-spaced from 60 Hz to
/// 8 kHz. Fast attack, slow release, like a hardware meter.
pub struct Spectrum {
    sample_rate: f32,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    smoothed: [f32; BAR_COUNT],
}

pub const SPECTRUM_SIZE: usize = 512;
const BAND_EDGES_HZ: [f32; BAR_COUNT + 1] = [60.0, 180.0, 500.0, 1_400.0, 3_500.0, 8_000.0];

impl Spectrum {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let n = SPECTRUM_SIZE as f32;
        Self {
            sample_rate: sample_rate as f32,
            window: (0..SPECTRUM_SIZE)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n).cos())
                .collect(),
            re: vec![0.0; SPECTRUM_SIZE],
            im: vec![0.0; SPECTRUM_SIZE],
            smoothed: [0.0; BAR_COUNT],
        }
    }

    /// Processes the most recent `SPECTRUM_SIZE` samples (fewer are
    /// zero-padded). Does not allocate.
    pub fn process(&mut self, samples: &[f32]) -> [f32; BAR_COUNT] {
        let take = samples.len().min(SPECTRUM_SIZE);
        let recent = &samples[samples.len() - take..];
        for i in 0..SPECTRUM_SIZE {
            self.re[i] = recent.get(i).copied().unwrap_or(0.0) * self.window[i];
            self.im[i] = 0.0;
        }
        fft_in_place(&mut self.re, &mut self.im);

        // Disjoint bin ranges; each band gets at least one bin.
        let bin_hz = self.sample_rate / SPECTRUM_SIZE as f32;
        let mut edges = [0usize; BAR_COUNT + 1];
        for (i, edge) in edges.iter_mut().enumerate() {
            let floor = if i == 0 { 1 } else { 0 };
            *edge = ((BAND_EDGES_HZ[i] / bin_hz).round() as usize).max(floor);
        }
        for i in 1..edges.len() {
            edges[i] = edges[i].max(edges[i - 1] + 1).min(SPECTRUM_SIZE / 2);
        }
        let mut levels = [0.0f32; BAR_COUNT];
        for (band, level) in levels.iter_mut().enumerate() {
            let mut peak = 0.0f32;
            for bin in edges[band]..edges[band + 1] {
                peak = peak.max(self.re[bin].hypot(self.im[bin]));
            }
            // Normalise by the window's coherent gain (N/4 for Hann), then
            // map -54..0 dBFS to 0..1.
            let amplitude = peak / (SPECTRUM_SIZE as f32 * 0.25);
            let db = 20.0 * amplitude.max(1e-6).log10();
            *level = ((db + 54.0) / 54.0).clamp(0.0, 1.0);
        }

        for (smoothed, level) in self.smoothed.iter_mut().zip(levels) {
            let rate = if level > *smoothed { 0.65 } else { 0.18 };
            *smoothed += (level - *smoothed) * rate;
        }
        self.smoothed
    }
}

/// Iterative radix-2 FFT. `re.len()` must be a power of two.
fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let (w_im, w_re) = angle.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cur_re, mut cur_im) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let t_re = re[b] * cur_re - im[b] * cur_im;
                let t_im = re[b] * cur_im + im[b] * cur_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let next_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next_re;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_extrapolates_only_while_playing() {
        let timeline = Timeline {
            position: 30.0,
            duration: 200.0,
            sampled_at: 10.0,
            rate: 1.0,
        };
        assert_eq!(timeline.position_at(15.0, true), 35.0);
        assert_eq!(timeline.position_at(15.0, false), 30.0);
        assert_eq!(timeline.position_at(1_000.0, true), 200.0);
        assert!((timeline.progress_at(15.0, true) - 0.175).abs() < 1e-6);
    }

    #[test]
    fn unknown_duration_has_zero_progress() {
        let timeline = Timeline {
            position: 30.0,
            duration: 0.0,
            sampled_at: 0.0,
            rate: 1.0,
        };
        assert_eq!(timeline.progress_at(5.0, true), 0.0);
        assert_eq!(timeline.position_at(5.0, true), 35.0);
    }

    #[test]
    fn clock_formatting() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(218.4), "3:38");
        assert_eq!(format_clock(3_725.0), "1:02:05");
        assert_eq!(format_clock(-4.0), "0:00");
    }

    fn sessions<'a>(list: &[(&'a str, bool)]) -> Vec<SessionCandidate<'a>> {
        list.iter()
            .map(|&(app_id, playing)| SessionCandidate {
                app_id,
                playing,
                last_played: false,
            })
            .collect()
    }

    #[test]
    fn spotify_is_preferred_among_equally_playing_sessions() {
        let all = sessions(&[("MSEdge", true), ("Spotify.exe", true), ("Chrome", true)]);
        assert_eq!(choose_session(&all, Some(0)), Some(1));
        let paused = sessions(&[("MSEdge", false), ("Chrome", false)]);
        assert_eq!(choose_session(&paused, Some(1)), Some(1));
        assert_eq!(choose_session(&paused, None), Some(0));
        assert_eq!(
            choose_session(&sessions(&[("MSEdge", false)]), Some(4)),
            Some(0)
        );
        assert_eq!(choose_session(&[], None), None);
    }

    #[test]
    fn pausing_keeps_the_last_played_session() {
        // YouTube Music was playing, then paused; Spotify was paused earlier.
        let mut all = sessions(&[("Spotify.exe", false), ("Firefox", false)]);
        all[1].last_played = true;
        assert_eq!(choose_session(&all, Some(0)), Some(1));
        // Anything actually playing still wins.
        all[0].playing = true;
        assert_eq!(choose_session(&all, None), Some(0));
    }

    #[test]
    fn a_playing_browser_beats_paused_spotify() {
        // The real report: Spotify paused, YouTube in Chrome playing.
        let all = sessions(&[
            ("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", false),
            ("308046B0AF4A39CB", true),
        ]);
        assert_eq!(choose_session(&all, Some(1)), Some(1));
        assert_eq!(choose_session(&all, None), Some(1));
    }

    #[test]
    fn source_names() {
        assert_eq!(source_display_name("Spotify.exe"), "Spotify");
        assert_eq!(
            source_display_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(source_display_name("MSEdge"), "Edge");
        assert_eq!(source_display_name("308046B0AF4A39CB"), "Browser");
        assert_eq!(
            source_display_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Media Player"
        );
        assert_eq!(source_display_name("C:\\Apps\\Plexamp.exe"), "Plexamp");
        assert_eq!(source_display_name("Contoso.Player_abc!App"), "Player");
    }

    fn solid(width: u32, height: u32, bgra: [u8; 4]) -> Vec<u8> {
        bgra.iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect()
    }

    #[test]
    fn dominant_color_finds_the_main_hue() {
        // Mostly red with a dark border.
        let (w, h) = (64u32, 64u32);
        let mut pixels = solid(w, h, [10, 10, 10, 255]);
        for y in 8..56 {
            for x in 8..56 {
                let i = ((y * w + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[30, 40, 220, 255]);
            }
        }
        let [r, g, b] = dominant_color(&pixels, w, h).expect("colour");
        assert!(r > 150 && g < 110 && b < 110, "got {r},{g},{b}");
    }

    #[test]
    fn grey_art_has_no_dominant_color() {
        let pixels = solid(32, 32, [128, 128, 128, 255]);
        assert_eq!(dominant_color(&pixels, 32, 32), None);
        assert_eq!(dominant_color(&[], 0, 0), None);
    }

    #[test]
    fn dark_colours_are_lifted_for_contrast() {
        let pixels = solid(32, 32, [60, 10, 10, 255]); // dark navy
        let [r, g, b] = dominant_color(&pixels, 32, 32).expect("colour");
        let (_, _, lightness) = rgb_to_hsl([r, g, b].map(|c| f32::from(c) / 255.0));
        assert!(lightness >= 0.5, "lightness {lightness}");
        assert!(b > r && b > g);
    }

    #[test]
    fn expanded_controls_and_scrubber_hit_testing() {
        let width = crate::island::MEDIA.width;
        let [previous, play, next] = expanded::controls(width);
        assert_eq!(
            expanded_action_at(previous.0, previous.1, width),
            Some(MediaAction::Previous)
        );
        assert_eq!(
            expanded_action_at(play.0 + 5.0, play.1 - 5.0, width),
            Some(MediaAction::PlayPause)
        );
        assert_eq!(
            expanded_action_at(next.0, next.1, width),
            Some(MediaAction::Next)
        );

        let bar = expanded::scrubber(width);
        match expanded_action_at(bar.x + bar.width * 0.25, bar.y + 1.0, width) {
            Some(MediaAction::Seek(fraction)) => assert!((fraction - 0.25).abs() < 1e-4),
            other => panic!("expected seek, got {other:?}"),
        }
        // The art, title and source chip open the app. Empty space does nothing.
        assert_eq!(
            expanded_action_at(40.0, 40.0, width),
            Some(MediaAction::OpenApp)
        );
        assert_eq!(
            expanded_action_at(150.0, 30.0, width),
            Some(MediaAction::OpenApp)
        );
        let chip = expanded::chip(width, "Spotify");
        assert_eq!(
            expanded_action_at(chip.x + 6.0, chip.y + 8.0, width),
            Some(MediaAction::OpenApp)
        );
        assert_eq!(expanded_action_at(380.0, 160.0, width), None);
    }

    #[test]
    fn volume_slider_hit_testing() {
        let width = crate::island::MEDIA.width;
        let track = expanded::volume_track(width);
        let somewhere = track.y + track.height * 0.5;

        match volume_fraction(track.x + track.width * 0.3, somewhere, width, false) {
            Some(fraction) => assert!((fraction - 0.3).abs() < 1e-4),
            None => panic!("expected a volume fraction"),
        }
        // The speaker button mutes.
        let (icon_x, _) = expanded::volume_icon_center(width);
        assert_eq!(volume_fraction(icon_x, somewhere, width, false), Some(0.0));
        // Away from the row, a press is not captured.
        assert_eq!(
            volume_fraction(track.x, somewhere - 40.0, width, false),
            None
        );
        // A drag keeps steering and clamps at both ends.
        assert_eq!(
            volume_fraction(track.x - 80.0, -500.0, width, true),
            Some(0.0)
        );
        assert_eq!(
            volume_fraction(track.right() + 80.0, -500.0, width, true),
            Some(1.0)
        );
    }

    #[test]
    fn dragging_clamps_outside_the_bar() {
        let width = crate::island::MEDIA.width;
        let bar = expanded::scrubber(width);
        assert_eq!(scrub_fraction(bar.x - 50.0, 0.0, width, true), Some(0.0));
        assert_eq!(
            scrub_fraction(bar.right() + 50.0, 0.0, width, true),
            Some(1.0)
        );
        assert_eq!(scrub_fraction(bar.x, 0.0, width, false), None);
    }

    #[test]
    fn extending_the_feature_lane_preserves_playback_and_separate_hit_zones() {
        let width = crate::island::MEDIA.width;
        let wider = width + 160.0;
        assert_eq!(expanded::controls(width), expanded::controls(wider));
        assert_eq!(expanded::scrubber(width), expanded::scrubber(wider));
        assert_eq!(
            expanded::chip(width, "Spotify"),
            expanded::chip(wider, "Spotify")
        );
        for test_width in [width, wider] {
            for (x, y) in expanded::controls(test_width) {
                assert_eq!(volume_fraction(x, y, test_width, false), None);
                assert_eq!(scrub_fraction(x, y, test_width, false), None);
            }
            let track = expanded::volume_track(test_width);
            let (x, y) = track.center();
            assert_eq!(expanded_action_at(x, y, test_width), None);
            assert_eq!(scrub_fraction(x, y, test_width, false), None);
            // The divider and future feature space must not open the source app.
            assert_eq!(
                expanded_action_at(expanded::PLAYER_WIDTH, 56.0, test_width),
                None
            );
            assert_eq!(expanded_action_at(wider - 24.0, 32.0, test_width), None);
        }
    }

    #[test]
    fn unavailable_controls_and_empty_space_do_not_show_hover_feedback() {
        let width = crate::island::MEDIA.width;
        let seek = expanded::scrubber(width).center();
        let volume = expanded::volume_track(width).center();
        let source = expanded::chip(width, "").center();
        for (point, expected) in [
            (seek, MediaHover::Seek),
            (volume, MediaHover::Volume),
            (source, MediaHover::Source),
        ] {
            assert_eq!(
                expanded_hover_at(point.0, point.1, width, true, true, true),
                Some(expected)
            );
            assert_eq!(
                expanded_hover_at(point.0, point.1, width, false, false, false),
                None
            );
        }
        for (x, y) in [(280.0, 56.0), (-20.0, 40.0), (460.0, 40.0), (250.0, 70.0)] {
            assert_eq!(expanded_hover_at(x, y, width, true, true, true), None);
        }
    }

    #[test]
    fn hover_at_the_transport_seek_boundary_follows_mouse_down_precedence() {
        let width = crate::island::MEDIA.width;
        let (x, y) = expanded::controls(width)[1];
        let boundary = y + expanded::CONTROL_HIT_RADIUS;
        assert!(scrub_fraction(x, boundary, width, false).is_some());
        assert_eq!(
            expanded_hover_at(x, boundary, width, true, true, true),
            Some(MediaHover::Seek)
        );
        assert_eq!(
            expanded_hover_at(x, boundary, width, false, true, true),
            Some(MediaHover::PlayPause)
        );
    }

    #[test]
    fn bar_levels_expire() {
        let levels = BarLevels::default();
        assert_eq!(levels.load_fresh(100, 250), None);
        levels.store([0.1, 0.2, 0.3, 0.4, 2.0], 1_000);
        assert_eq!(
            levels.load_fresh(1_100, 250),
            Some([0.1, 0.2, 0.3, 0.4, 1.0])
        );
        assert_eq!(levels.load_fresh(1_300, 250), None);
    }

    fn sine(hz: f32, rate: u32, amplitude: f32) -> Vec<f32> {
        (0..SPECTRUM_SIZE)
            .map(|i| amplitude * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn settle(spectrum: &mut Spectrum, samples: &[f32]) -> [f32; BAR_COUNT] {
        let mut levels = [0.0; BAR_COUNT];
        for _ in 0..30 {
            levels = spectrum.process(samples);
        }
        levels
    }

    #[test]
    fn spectrum_puts_tones_in_the_right_band() {
        let rate = 48_000;
        for (hz, band) in [
            (100.0, 0),
            (300.0, 1),
            (900.0, 2),
            (2_000.0, 3),
            (5_000.0, 4),
        ] {
            let mut spectrum = Spectrum::new(rate);
            let levels = settle(&mut spectrum, &sine(hz, rate, 0.5));
            let loudest = (0..BAR_COUNT)
                .max_by(|&a, &b| levels[a].total_cmp(&levels[b]))
                .unwrap();
            assert_eq!(loudest, band, "{hz} Hz -> {levels:?}");
            assert!(levels[band] > 0.8, "{hz} Hz -> {levels:?}");
        }
    }

    #[test]
    fn silence_decays_to_zero() {
        let rate = 44_100;
        let mut spectrum = Spectrum::new(rate);
        settle(&mut spectrum, &sine(1_000.0, rate, 0.5));
        let levels = settle(&mut spectrum, &[0.0; SPECTRUM_SIZE]);
        assert!(levels.iter().all(|&level| level < 0.01), "{levels:?}");
    }

    #[test]
    fn fft_of_impulse_is_flat() {
        let mut re = vec![0.0f32; 8];
        let mut im = vec![0.0f32; 8];
        re[0] = 1.0;
        fft_in_place(&mut re, &mut im);
        for (r, i) in re.iter().zip(&im) {
            assert!((r - 1.0).abs() < 1e-6 && i.abs() < 1e-6);
        }
    }
}
