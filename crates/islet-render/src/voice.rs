//! The dictation pill's six bars. Each bar is a small, slightly bouncy
//! spring chasing its own reading of the mic, so the bars move smoothly at
//! the display's frame rate, wobble a little as they settle, and never all
//! move as one.

use islet_core::island::{DictationCard, DictationPhase};

pub(super) const BARS: usize = 6;
/// Bar spacing and width on the pill, in logical pixels.
pub(super) const BAR_PITCH: f32 = 9.0;
pub(super) const BAR_WIDTH: f32 = 3.0;

/// How many meter readings (33 ms each) behind the newest each bar looks,
/// and how tall it can get: the middle leads and stands tallest, the edges
/// trail and stay shorter, so speech rolls outward across the bars.
const DELAY: [usize; BARS] = [5, 3, 0, 1, 2, 4];
const REACH: [f32; BARS] = [0.58, 0.84, 1.0, 0.95, 0.80, 0.55];
/// Bars wake from the middle out when the pill opens, this far apart.
const STAGGER: [f32; BARS] = [0.10, 0.05, 0.0, 0.0, 0.05, 0.10];
/// Spring stiffness and damping: under-damped, so a bar overshoots a
/// touch and settles.
const STIFFNESS: f32 = 520.0;
const DAMPING_RATIO: f32 = 0.52;
/// When the pill opens, the bars ripple up from the middle and settle
/// into the voice: this tall at the crest, over this long.
const WAKE_HEIGHT: f32 = 0.62;
const WAKE_SECONDS: f32 = 0.30;
/// A pause this long means a new dictation: start again from flat.
const RESTART_AFTER: f64 = 0.4;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VoiceBars {
    height: [f32; BARS],
    speed: [f32; BARS],
    started: f64,
    last: Option<f64>,
}

/// Where each bar is heading, from the recent levels (oldest first,
/// 0..=255) and a faint shimmer that keeps a quiet pill alive.
pub(super) fn targets(levels: &[u8], now: f64) -> [f32; BARS] {
    let newest = levels.len().saturating_sub(1);
    let mut out = [0.0; BARS];
    for (index, target) in out.iter_mut().enumerate() {
        let level = levels
            .get(newest.saturating_sub(DELAY[index]))
            .map_or(0.0, |&value| f32::from(value) / 255.0);
        let shimmer = 0.05 * (1.0 + (now as f32 * (5.1 + index as f32 * 0.9) + index as f32 * 1.7).sin());
        *target = (level * REACH[index] + shimmer).min(1.0);
    }
    out
}

/// Where the bars are heading for this dictation: the live mic, or a slow
/// wave while the speech is being turned into text.
pub(super) fn card_targets(dictation: &DictationCard, now: f64) -> [f32; BARS] {
    match dictation.phase {
        DictationPhase::Transcribing => {
            let mut wave = [0.0; BARS];
            for (index, value) in wave.iter_mut().enumerate() {
                let phase = (now * 6.0 - index as f64 * 0.8).sin() * 0.5 + 0.5;
                *value = 0.12 + 0.34 * phase as f32;
            }
            wave
        }
        _ => targets(&dictation.bars, now),
    }
}

/// How strongly each bar is drawn: the middle brightest while listening.
pub(super) fn bar_alpha(dictation: &DictationCard, index: usize) -> f32 {
    match dictation.phase {
        DictationPhase::Transcribing => 0.9,
        _ => 0.7 + 0.25 * (1.0 - (index as f32 - 2.5).abs() / 2.5),
    }
}

impl VoiceBars {
    /// Advances every bar towards its target and returns the heights,
    /// 0..=1 (a little over while overshooting).
    pub(super) fn step(&mut self, now: f64, targets: [f32; BARS]) -> [f32; BARS] {
        let restart = self.last.is_none_or(|last| now - last > RESTART_AFTER || now < last);
        if restart {
            *self = Self { started: now, last: Some(now), ..Self::default() };
        }
        let dt = (now - self.last.unwrap_or(now)).clamp(0.0, 1.0 / 30.0) as f32;
        self.last = Some(now);
        if dt <= 0.0 {
            return self.height;
        }
        let damping = 2.0 * DAMPING_RATIO * STIFFNESS.sqrt();
        let awake = (now - self.started) as f32;
        // Small substeps keep the spring stable on slow frames.
        let steps = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for index in 0..BARS {
            let since = awake - STAGGER[index];
            let target = if since < 0.0 {
                0.0
            } else {
                let wake = (since / WAKE_SECONDS).min(1.0);
                let ripple = if wake < 1.0 { WAKE_HEIGHT * (wake * std::f32::consts::PI).sin() } else { 0.0 };
                targets[index].max(ripple)
            };
            for _ in 0..steps {
                let force = STIFFNESS * (target - self.height[index]) - damping * self.speed[index];
                self.speed[index] += force * h;
                self.height[index] += self.speed[index] * h;
            }
            self.height[index] = self.height[index].clamp(-0.05, 1.12);
        }
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(bars: &mut VoiceBars, from: f64, seconds: f64, target: [f32; BARS]) -> (f64, [f32; BARS]) {
        let mut now = from;
        let mut out = [0.0; BARS];
        while now < from + seconds {
            now += 1.0 / 120.0;
            out = bars.step(now, target);
        }
        (now, out)
    }

    #[test]
    fn bars_glide_overshoot_a_touch_and_settle() {
        let mut bars = VoiceBars::default();
        bars.step(0.0, [0.0; BARS]);
        let mut peak: f32 = 0.0;
        let mut now = 0.0;
        while now < 0.6 {
            now += 1.0 / 120.0;
            peak = peak.max(bars.step(now, [0.6; BARS])[2]);
        }
        assert!(peak > 0.62 && peak < 0.8, "a small wobble past the target: {peak}");
        let (_, settled) = run(&mut bars, now, 1.0, [0.6; BARS]);
        assert!((settled[2] - 0.6).abs() < 0.01);
    }

    #[test]
    fn the_middle_wakes_first() {
        let mut bars = VoiceBars::default();
        let (_, early) = run(&mut bars, 0.0, 0.04, [0.8; BARS]);
        assert!(early[2] > 0.0 && early[0] == 0.0 && early[5] == 0.0);
    }

    #[test]
    fn a_loud_moment_is_not_six_equal_bars() {
        let mut levels = [0u8; 24];
        levels[23] = 255;
        levels[22] = 200;
        let heights = targets(&levels, 0.0);
        let (low, high) = heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &h| (lo.min(h), hi.max(h)));
        assert!(high - low > 0.5, "bars differ: {heights:?}");
    }

    #[test]
    fn opening_ripples_the_bars_up_then_they_settle() {
        let mut bars = VoiceBars::default();
        bars.step(0.0, [0.0; BARS]);
        let (_, crest) = run(&mut bars, 0.0, 0.2, [0.0; BARS]);
        assert!(crest[2] > 0.4, "the middle bar rises: {crest:?}");
        let (_, rest) = run(&mut bars, 0.2, 1.0, [0.0; BARS]);
        assert!(rest.iter().all(|h| h.abs() < 0.02), "then settles: {rest:?}");
    }

    #[test]
    fn a_new_dictation_starts_flat() {
        let mut bars = VoiceBars::default();
        let (end, _) = run(&mut bars, 0.0, 1.0, [0.9; BARS]);
        let restarted = bars.step(end + 2.0, [0.9; BARS]);
        assert_eq!(restarted, [0.0; BARS]);
    }
}
