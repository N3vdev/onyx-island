//! Time-based eased motion for the island's entrances, exits and edge travel.
//!
//! Springs suit size changes, which can be retargeted at any moment. A spring
//! released from rest, though, accelerates hardest in its very first frame,
//! and settles with a long tail. For a pill welling out of the screen edge,
//! or slipping into one, the curve should be designed instead: a gentle start,
//! a soft landing, and a known duration.
//!
//! Like [`crate::Spring`], a [`Tween`] is sampled in closed form from the time
//! it is given, so motion is identical at 60 Hz and 180 Hz. Retargeting
//! mid-flight keeps the current speed (a cubic Hermite from the current value
//! and velocity), so an interrupted motion turns around without a jump.

/// A cubic-bezier timing curve from (0, 0) to (1, 1), as in CSS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ease {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl Ease {
    #[must_use]
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// Symmetric and strong: slow off the start, fastest in the middle, slow
    /// into the end. The island crosses between edges at its fastest while it
    /// is invisible inside the screen edge.
    pub const TRAVEL: Self = Self::new(0.62, 0.0, 0.38, 1.0);
    /// Welling out of the edge: eases in over the first frames (no snap from
    /// nothing), then a long, soft landing with no overshoot.
    pub const EMERGE: Self = Self::new(0.32, 0.0, 0.12, 1.0);
    /// Drawn back into the edge: a soft start that gathers speed, so the
    /// droplet slips through instead of creeping the last pixels.
    pub const RECEDE: Self = Self::new(0.45, 0.0, 0.85, 0.35);

    /// Progress `0..=1` at time fraction `u` (clamped).
    #[must_use]
    pub fn at(self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        if u <= 0.0 || u >= 1.0 {
            return u;
        }
        let s = self.solve_x(u);
        bezier(self.y1, self.y2, s)
    }

    /// The curve parameter whose x is `x` (Newton, then bisection fallback).
    fn solve_x(self, x: f64) -> f64 {
        let mut s = x;
        for _ in 0..8 {
            let error = bezier(self.x1, self.x2, s) - x;
            if error.abs() < 1e-7 {
                return s;
            }
            let slope = bezier_slope(self.x1, self.x2, s);
            if slope.abs() < 1e-6 {
                break;
            }
            s = (s - error / slope).clamp(0.0, 1.0);
        }
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..40 {
            s = (low + high) * 0.5;
            if bezier(self.x1, self.x2, s) < x {
                low = s;
            } else {
                high = s;
            }
        }
        s
    }
}

/// One coordinate of a cubic bezier with end points 0 and 1.
fn bezier(p1: f64, p2: f64, s: f64) -> f64 {
    let inv = 1.0 - s;
    3.0 * inv * inv * s * p1 + 3.0 * inv * s * s * p2 + s * s * s
}

fn bezier_slope(p1: f64, p2: f64, s: f64) -> f64 {
    let inv = 1.0 - s;
    3.0 * inv * inv * p1 + 6.0 * inv * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Curve {
    Eased(Ease),
    /// Continues from a velocity (units per second) and lands at rest.
    Carry(f64),
}

/// A value moving from one number to another over a fixed duration.
#[derive(Clone, Debug)]
pub struct Tween {
    from: f64,
    to: f64,
    start: f64,
    duration: f64,
    curve: Curve,
}

impl Tween {
    /// At rest at `value`.
    #[must_use]
    pub fn new(value: f64, now: f64) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            duration: 0.0,
            curve: Curve::Eased(Ease::TRAVEL),
        }
    }

    #[must_use]
    pub fn target(&self) -> f64 {
        self.to
    }

    #[must_use]
    pub fn position(&self, now: f64) -> f64 {
        if self.duration <= 0.0 {
            return self.to;
        }
        let u = ((now - self.start) / self.duration).clamp(0.0, 1.0);
        match self.curve {
            Curve::Eased(ease) => self.from + (self.to - self.from) * ease.at(u),
            Curve::Carry(velocity) => {
                // Cubic Hermite: from (from, velocity) to (to, 0).
                let (u2, u3) = (u * u, u * u * u);
                let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
                let h10 = u3 - 2.0 * u2 + u;
                let h01 = -2.0 * u3 + 3.0 * u2;
                h00 * self.from + h10 * velocity * self.duration + h01 * self.to
            }
        }
    }

    /// Units per second, by a short difference (one-sided at the start, where
    /// earlier times are clamped).
    #[must_use]
    pub fn velocity(&self, now: f64) -> f64 {
        const H: f64 = 1e-4;
        if self.is_settled(now) {
            return 0.0;
        }
        if now - H < self.start {
            return (self.position(now + H) - self.position(now)) / H;
        }
        (self.position(now + H) - self.position(now - H)) / (2.0 * H)
    }

    #[must_use]
    pub fn is_settled(&self, now: f64) -> bool {
        now >= self.start + self.duration
    }

    /// Heads for `to` over `duration` seconds. From rest the move follows
    /// `ease`; mid-flight it carries the current speed into the new target.
    pub fn retarget(&mut self, to: f64, duration: f64, ease: Ease, now: f64) {
        let from = self.position(now);
        let velocity = self.velocity(now);
        self.from = from;
        self.to = to;
        self.start = now;
        self.duration = duration.max(0.0);
        // A speed that barely moves the value counts as rest.
        let moving = velocity.abs() * self.duration > (to - from).abs().max(1e-3) * 0.05;
        self.curve = if moving {
            Curve::Carry(velocity)
        } else {
            Curve::Eased(ease)
        };
    }

    /// Snaps to `value`, at rest.
    pub fn reset(&mut self, value: f64, now: f64) {
        *self = Self::new(value, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_start_and_end_on_their_endpoints() {
        for ease in [Ease::TRAVEL, Ease::EMERGE, Ease::RECEDE] {
            assert!(ease.at(0.0).abs() < 1e-9);
            assert!((ease.at(1.0) - 1.0).abs() < 1e-9);
            let mut previous = 0.0;
            for step in 1..=100 {
                let value = ease.at(f64::from(step) / 100.0);
                assert!(value >= previous - 1e-9, "monotonic: {ease:?}");
                assert!(value <= 1.0 + 1e-9, "no overshoot: {ease:?}");
                previous = value;
            }
        }
    }

    #[test]
    fn travel_is_symmetric_and_fastest_in_the_middle() {
        let ease = Ease::TRAVEL;
        assert!((ease.at(0.5) - 0.5).abs() < 1e-6);
        for u in [0.1, 0.2, 0.3, 0.4] {
            assert!((ease.at(u) + ease.at(1.0 - u) - 1.0).abs() < 1e-6);
        }
        let step = |u: f64| ease.at(u + 0.01) - ease.at(u);
        assert!(step(0.495) > step(0.2) && step(0.2) > step(0.0));
    }

    #[test]
    fn eases_in_gently_from_rest() {
        // No snap: the first frame at 180 Hz of a 0.5 s move barely moves.
        for ease in [Ease::TRAVEL, Ease::EMERGE, Ease::RECEDE] {
            assert!(ease.at(1.0 / 90.0) < 0.02, "{ease:?}");
        }
    }

    #[test]
    fn tween_lands_exactly_and_reports_settled() {
        let mut tween = Tween::new(0.0, 0.0);
        assert!(tween.is_settled(0.0));
        tween.retarget(1.0, 0.8, Ease::TRAVEL, 1.0);
        assert!(!tween.is_settled(1.4));
        assert!((tween.position(1.4) - 0.5).abs() < 1e-6);
        assert!(tween.is_settled(1.8));
        assert_eq!(tween.position(1.8), 1.0);
        assert_eq!(tween.position(9.0), 1.0);
    }

    #[test]
    fn an_interrupted_tween_turns_around_without_a_jump() {
        let mut tween = Tween::new(0.0, 0.0);
        tween.retarget(1.0, 0.8, Ease::TRAVEL, 0.0);
        let at = 0.3;
        let before = tween.position(at);
        let speed = tween.velocity(at);
        assert!(speed > 0.5);
        tween.retarget(0.0, 0.5, Ease::TRAVEL, at);
        assert!((tween.position(at) - before).abs() < 1e-9, "no jump");
        assert!(
            (tween.velocity(at) - speed).abs() < speed * 0.02,
            "keeps its speed"
        );
        assert!(tween.position(at + 0.5).abs() < 1e-9);
    }
}
