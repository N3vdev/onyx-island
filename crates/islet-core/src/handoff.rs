//! Finite edge-to-surface handoff; no timers or work after it settles.

pub const PREVIEW_ATTACH: u32 = 0x8000 + 40;
pub const PREVIEW_DETACH: u32 = 0x8000 + 41;
pub const PREVIEW_DEPARTED: u32 = 0x8000 + 42;
pub const DEPART_SECONDS: f64 = 0.28;
pub const ARRIVE_SECONDS: f64 = 0.50;

#[derive(Clone, Copy, Debug)]
pub struct Handoff {
    pub arriving: bool,
    started: f64,
    duration: f64,
}

impl Handoff {
    pub fn new(arriving: bool, now: f64, animate: bool) -> Self {
        Self {
            arriving,
            started: now,
            duration: if !animate {
                0.0
            } else if arriving {
                ARRIVE_SECONDS
            } else {
                DEPART_SECONDS
            },
        }
    }

    pub fn settled(self, now: f64) -> bool {
        now >= self.started + self.duration
    }

    pub fn fullness(self, now: f64) -> f32 {
        let t = if self.duration == 0.0 {
            1.0
        } else {
            ((now - self.started) / self.duration).clamp(0.0, 1.0) as f32
        };
        if self.arriving {
            1.0 - (1.0 - t).powi(3)
        } else {
            1.0 - t * t * (3.0 - 2.0 * t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_has_a_fully_hidden_relocation_and_a_finite_end() {
        let depart = Handoff::new(false, 2.0, true);
        assert_eq!(depart.fullness(2.0), 1.0);
        assert_eq!(depart.fullness(3.0), 0.0);
        let arrive = Handoff::new(true, 3.0, true);
        assert_eq!(arrive.fullness(3.0), 0.0);
        assert_eq!(arrive.fullness(4.0), 1.0);
        for i in 0..100 {
            let t = i as f64 / 100.0;
            assert!(depart.fullness(2.0 + t) >= depart.fullness(2.01 + t));
            assert!(arrive.fullness(3.0 + t) <= arrive.fullness(3.01 + t));
        }
        assert!(depart.settled(3.0) && arrive.settled(4.0));
    }

    #[test]
    fn reduced_motion_relocates_without_spatial_animation() {
        for arriving in [false, true] {
            let transfer = Handoff::new(arriving, 0.0, false);
            assert!(transfer.settled(0.0));
            assert_eq!(transfer.fullness(0.0), if arriving { 1.0 } else { 0.0 });
        }
    }
}
