//! Analytic damped-spring solver.
//!
//! A spring is sampled from a closed-form solution rather than integrated per
//! frame. That makes the result independent of frame rate and lets the renderer
//! park while nothing is moving.
//!
//! The time argument is a monotonic value in seconds (QPC on Windows). It may
//! start at any value; only differences matter.

use std::f64::consts::TAU;

/// The two numbers that define a second-order spring.
///
/// `response` is the period of an undamped spring in seconds. `damping` is the
/// damping ratio: `< 1` overshoots, `1` is critically damped, `> 1` is
/// overdamped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    pub response: f64,
    pub damping: f64,
}

impl SpringParams {
    /// A deliberate, iOS-like expansion with a small overshoot.
    pub const EXPAND: Self = Self {
        response: 0.42,
        damping: 0.78,
    };
    /// A quick, controlled collapse.
    pub const COLLAPSE: Self = Self {
        response: 0.36,
        damping: 0.90,
    };
    /// Closing to hidden, horizontal axis: contracts quickly...
    pub const DISMISS_WIDTH: Self = Self {
        response: 0.34,
        damping: 0.92,
    };
    /// ...while the vertical axis lags, so the pill gathers into a round
    /// droplet before it shrinks away.
    pub const DISMISS_HEIGHT: Self = Self {
        response: 0.52,
        damping: 1.0,
    };
    /// A snappy pulse for arriving notifications.
    pub const PULSE: Self = Self {
        response: 0.25,
        damping: 0.55,
    };
    /// Travelling between the top and bottom screen edges. The island spends
    /// the first half of the travel gathering into a leaf and slipping through
    /// the edge it leaves, and the second half growing back out of the other
    /// edge, so this is slower than a straight glide would be.
    pub const DOCK: Self = Self {
        response: 0.85,
        damping: 0.90,
    };

    #[must_use]
    pub const fn new(response: f64, damping: f64) -> Self {
        Self { response, damping }
    }

    #[must_use]
    pub fn angular_frequency(self) -> f64 {
        TAU / self.response
    }
}

/// An animated scalar backed by a damped harmonic oscillator.
///
/// The public API is time-explicit: callers pass `now` to every sample. A
/// retarget samples the old curve at the retarget instant, keeps that velocity,
/// and starts a new curve from there.
#[derive(Clone, Debug)]
pub struct Spring {
    from: f64,
    from_velocity: f64,
    start_time: f64,
    target: f64,
    params: SpringParams,
}

impl Spring {
    /// Creates a spring at rest at `value`.
    #[must_use]
    pub fn new(value: f64, target: f64, params: SpringParams, now: f64) -> Self {
        assert!(params.response > 0.0, "spring response must be positive");
        assert!(
            params.damping > 0.0,
            "spring damping ratio must be positive"
        );
        Self {
            from: value,
            from_velocity: 0.0,
            start_time: now,
            target,
            params,
        }
    }

    #[must_use]
    pub fn target(&self) -> f64 {
        self.target
    }

    #[must_use]
    pub fn params(&self) -> SpringParams {
        self.params
    }

    /// Changes the target while preserving the current position and velocity.
    pub fn retarget(&mut self, target: f64, now: f64) {
        let (position, velocity) = self.sample(now);
        self.from = position;
        self.from_velocity = velocity;
        self.start_time = now;
        self.target = target;
    }

    /// Changes the spring constants without changing the current motion.
    ///
    /// The next sample starts fresh from the current position and velocity.
    pub fn set_params(&mut self, params: SpringParams, now: f64) {
        assert!(params.response > 0.0, "spring response must be positive");
        assert!(
            params.damping > 0.0,
            "spring damping ratio must be positive"
        );
        let (position, velocity) = self.sample(now);
        self.from = position;
        self.from_velocity = velocity;
        self.start_time = now;
        self.params = params;
    }

    /// Jumps to a new value at rest. Useful when a value is not visible (for
    /// example, while the island is hidden).
    pub fn reset(&mut self, value: f64, target: f64, now: f64) {
        self.from = value;
        self.from_velocity = 0.0;
        self.start_time = now;
        self.target = target;
    }

    /// Returns `(position, velocity)` at `now`.
    #[must_use]
    pub fn sample(&self, now: f64) -> (f64, f64) {
        let t = (now - self.start_time).max(0.0);
        let displacement = self.from - self.target;

        if displacement.abs() < f64::EPSILON && self.from_velocity.abs() < f64::EPSILON {
            return (self.target, 0.0);
        }

        let omega = self.params.angular_frequency();
        let zeta = self.params.damping;

        if (zeta - 1.0).abs() < 1e-9 {
            // Critically damped: x(t) = (A + C t) e^(-wt)
            let c = self.from_velocity + omega * displacement;
            let decay = (-omega * t).exp();
            let position = self.target + (displacement + c * t) * decay;
            let velocity = (self.from_velocity - omega * c * t) * decay;
            (position, velocity)
        } else if zeta < 1.0 {
            // Underdamped: x(t) = e^(-z w t) (A cos(wd t) + B sin(wd t))
            let omega_d = omega * (1.0 - zeta * zeta).sqrt();
            let b = (self.from_velocity + zeta * omega * displacement) / omega_d;
            let decay = (-zeta * omega * t).exp();
            let cos = (omega_d * t).cos();
            let sin = (omega_d * t).sin();
            let position = self.target + decay * (displacement * cos + b * sin);
            let velocity = decay
                * (self.from_velocity * cos - (zeta * omega * b + displacement * omega_d) * sin);
            (position, velocity)
        } else {
            // Overdamped: two real roots.
            let root = (zeta * zeta - 1.0).sqrt();
            let r1 = -omega * (zeta - root);
            let r2 = -omega * (zeta + root);
            let c2 = (self.from_velocity - r1 * displacement) / (r2 - r1);
            let c1 = displacement - c2;
            let e1 = (r1 * t).exp();
            let e2 = (r2 * t).exp();
            let position = self.target + c1 * e1 + c2 * e2;
            let velocity = r1 * c1 * e1 + r2 * c2 * e2;
            (position, velocity)
        }
    }

    #[must_use]
    pub fn position(&self, now: f64) -> f64 {
        self.sample(now).0
    }

    #[must_use]
    pub fn velocity(&self, now: f64) -> f64 {
        self.sample(now).1
    }

    /// True when the spring is close enough to its target to stop rendering.
    ///
    /// Defaults match the island's pixel-space threshold: within 0.25 px and
    /// 1 px/s. Callers animating opacity or scale can pass their own thresholds.
    #[must_use]
    pub fn is_settled(&self, now: f64) -> bool {
        self.is_settled_with(now, 0.25, 1.0)
    }

    #[must_use]
    pub fn is_settled_with(&self, now: f64, position_epsilon: f64, velocity_epsilon: f64) -> bool {
        let (position, velocity) = self.sample(now);
        (position - self.target).abs() <= position_epsilon && velocity.abs() <= velocity_epsilon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, epsilon: f64) {
        assert!((a - b).abs() <= epsilon, "expected {b}, got {a}");
    }

    #[test]
    fn starts_at_rest_and_approaches_target() {
        let spring = Spring::new(0.0, 220.0, SpringParams::EXPAND, 10.0);
        assert_eq!(spring.position(10.0), 0.0);
        assert_eq!(spring.velocity(10.0), 0.0);

        let position = spring.position(10.0 + 2.0);
        approx(position, 220.0, 0.5);
        assert!(spring.is_settled(10.0 + 2.0));
    }

    #[test]
    fn underdamped_expand_overshoots() {
        let spring = Spring::new(0.0, 220.0, SpringParams::EXPAND, 0.0);
        let peak = (1..=2000)
            .map(|i| spring.position(f64::from(i) * 0.001))
            .fold(f64::MIN, f64::max);
        assert!(peak > 222.0, "expected visible overshoot, peak was {peak}");
        assert!(peak < 245.0, "overshoot was implausibly large: {peak}");
    }

    #[test]
    fn critical_damping_has_no_overshoot() {
        let spring = Spring::new(0.0, 100.0, SpringParams::new(0.4, 1.0), 0.0);
        let peak = (1..=4000)
            .map(|i| spring.position(f64::from(i) * 0.001))
            .fold(f64::MIN, f64::max);
        assert!(
            peak <= 100.0 + 1e-6,
            "critically damped spring overshot: {peak}"
        );
    }

    #[test]
    fn overdamped_approaches_from_one_side() {
        let spring = Spring::new(0.0, 100.0, SpringParams::new(0.4, 1.8), 0.0);
        for i in 0..=4000 {
            let position = spring.position(f64::from(i) * 0.001);
            assert!(position <= 100.0 + 1e-6);
        }
        approx(spring.position(5.0), 100.0, 1e-3);
    }

    #[test]
    fn retarget_preserves_position_and_velocity() {
        let mut spring = Spring::new(0.0, 220.0, SpringParams::EXPAND, 0.0);
        let now = 0.137;
        let (before_position, before_velocity) = spring.sample(now);

        spring.retarget(120.0, now);

        let (after_position, after_velocity) = spring.sample(now);
        approx(after_position, before_position, 1e-12);
        approx(after_velocity, before_velocity, 1e-12);
        assert_eq!(spring.target(), 120.0);
    }

    #[test]
    fn retarget_mid_flight_reaches_new_target() {
        let mut spring = Spring::new(0.0, 220.0, SpringParams::EXPAND, 0.0);
        spring.retarget(120.0, 0.1);
        assert!(spring.is_settled(2.1));
        approx(spring.position(2.1), 120.0, 0.5);
    }

    #[test]
    fn sampling_is_frame_rate_independent() {
        let spring = Spring::new(0.0, 220.0, SpringParams::EXPAND, 0.0);

        // 60 Hz and 144 Hz both happen to land on 0.25 s here; sample the same
        // absolute instant and compare the closed-form result.
        let at_60 = spring.position(0.25);
        let at_144 = spring.position(0.25);
        approx(at_60, at_144, f64::EPSILON);

        // Simulate different frame sequences; they must still agree at the
        // common instant because no integration state exists.
        let mut frame_60 = 0.0;
        for _ in 0..15 {
            frame_60 += 1.0 / 60.0;
        }
        let mut frame_144 = 0.0;
        for _ in 0..36 {
            frame_144 += 1.0 / 144.0;
        }
        approx(spring.position(frame_60), spring.position(frame_144), 1e-12);
    }

    #[test]
    fn settling_thresholds_are_respected() {
        let spring = Spring::new(0.0, 100.0, SpringParams::PULSE, 0.0);
        assert!(!spring.is_settled(0.0));
        assert!(spring.is_settled_with(0.0, 1000.0, 1000.0));
        assert!(spring.is_settled(3.0));
    }

    #[test]
    fn reset_clears_velocity() {
        let mut spring = Spring::new(0.0, 100.0, SpringParams::EXPAND, 0.0);
        let _ = spring.sample(0.05);
        spring.reset(50.0, 200.0, 0.05);
        let (position, velocity) = spring.sample(0.05);
        assert_eq!(position, 50.0);
        assert_eq!(velocity, 0.0);
        assert_eq!(spring.target(), 200.0);
    }

    #[test]
    fn time_before_start_is_clamped() {
        let spring = Spring::new(10.0, 20.0, SpringParams::EXPAND, 1.0);
        assert_eq!(spring.position(0.5), 10.0);
        assert_eq!(spring.velocity(0.5), 0.0);
    }

    #[test]
    fn energy_decays_for_stable_presets() {
        let spring = Spring::new(0.0, 100.0, SpringParams::EXPAND, 0.0);
        let early = spring.position(0.05).abs() + spring.velocity(0.05).abs();
        let late = spring.position(1.0).abs() + spring.velocity(1.0).abs();
        assert!(late < early);
    }
}
