//! Collapsed-island pointer gestures, using screen coordinates across captures.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis { Horizontal, Vertical }
pub(super) struct Drag { start: POINT, dx: f32, dy: f32, axis: Option<Axis>, top: bool }
fn axis(dx: f32, dy: f32) -> Option<Axis> {
    if dx.abs().max(dy.abs()) < 8.0 { None }
    else if dx.abs() > dy.abs() * 1.2 { Some(Axis::Horizontal) }
    else if dy.abs() > dx.abs() * 1.2 { Some(Axis::Vertical) }
    else { None }
}
fn rubber(distance: f32) -> f32 { distance / (1.0 + distance.abs() / 120.0) }

impl IslandHost {
    pub(super) fn begin_gesture(&mut self) -> bool {
        if self.cursor_on_media_bubble() || self.state.expanded || self.traveling || self.state.dictation.is_some()
            || self.state.needs_input.is_some() || self.state.done.is_some()
            || self.state.converter.is_some() { return false; }
        let mut start = POINT::default();
        if unsafe { GetCursorPos(&mut start) }.is_err() { return false; }
        self.cancel_timer(TimerKind::HoverExpand);
        self.cancel_timer(TimerKind::HoverCollapse);
        self.press_armed = false;
        self.gesture = Some(Drag { start, dx: 0.0, dy: 0.0, axis: None, top: self.effective_anchor_top() });
        unsafe {
            SetCapture(self.hwnd);
            // A non-activating HWND cannot rely on capture outside its bounds.
            // This fallback lives only for the duration of the button press.
            SetTimer(Some(self.hwnd), TIMER_GESTURE, 8, None);
        }
        true
    }
    pub(super) fn move_gesture(&mut self) {
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() { return; }
        let Some(drag) = &mut self.gesture else { return; };
        let scale = 96.0 / self.dpi as f32;
        let dx = (point.x - drag.start.x) as f32 * scale;
        let dy = (point.y - drag.start.y) as f32 * scale;
        if dx == drag.dx && dy == drag.dy { return; }
        drag.dx = dx;
        drag.dy = dy;
        drag.axis = drag.axis.or_else(|| axis(drag.dx, drag.dy));
        let (x, y) = match drag.axis {
            Some(Axis::Horizontal) => (rubber(drag.dx), 0.0),
            Some(Axis::Vertical) => {
                // Resist a pull into the edge; allow generous inward travel.
                let outward = (drag.top && drag.dy < 0.0) || (!drag.top && drag.dy > 0.0);
                (0.0, if outward { rubber(drag.dy) * 0.04 } else { rubber(drag.dy) })
            }
            // Feedback starts at the first pixel, before navigation locks an axis.
            None => {
                let outward = (drag.top && drag.dy < 0.0) || (!drag.top && drag.dy > 0.0);
                (rubber(drag.dx), rubber(drag.dy) * if outward { 0.04 } else { 1.0 })
            },
        };
        let target = self.last_target.unwrap_or(LayoutTarget::HIDDEN);
        self.morph_box = self.target_frame(target, self.effective_anchor_top()).bounds().map(|r| r.inflate(140.0));
        self.update_region();
        let _ = self.tx.send(RenderCommand::Pull(x, y, true));
    }
    pub(super) fn finish_gesture(&mut self, commit: bool) {
        let Some(drag) = self.gesture.take() else { return; };
        unsafe { let _ = KillTimer(Some(self.hwnd), TIMER_GESTURE); let _ = ReleaseCapture(); }
        let _ = self.tx.send(RenderCommand::Pull(0.0, 0.0, false));
        if !commit { self.send_snapshot(); return; }
        match drag.axis {
            Some(Axis::Horizontal) if drag.dx.abs() >= 48.0 => {
                let page: i32 = match self.state.compact_widget {
                    None => 0, Some(IdleGlance::Clock) => 1, Some(IdleGlance::Stats) => 2,
                };
                let mut next = (page + if drag.dx < 0.0 { 1 } else { -1 }).rem_euclid(3);
                // Do not navigate to an invisible "normal" page on an idle PC.
                let mut normal = self.state.clone();
                normal.compact_widget = None;
                if next == 0 && normal.primary_card().is_none() {
                    next = if drag.dx < 0.0 { 1 } else { 2 };
                }
                self.state.compact_widget = match next { 1 => Some(IdleGlance::Clock), 2 => Some(IdleGlance::Stats), _ => None };
                if next == 2 && self.stats_sampler.is_none() { self.stats_sampler = Some(crate::sysstats::Sampler::new()); }
                if next != 2 && !needs_sampler(&read_config(&self.shared)) { self.stats_sampler = None; }
                self.update_clock();
                self.sample_stats();
                self.update_stats_timer();
            }
            Some(Axis::Vertical) if drag.dy.abs() >= 64.0 && self.preview.is_none() => {
                let top = drag.dy < 0.0;
                if top != drag.top {
                    let path = crate::paths::Paths::discover().config_file;
                    if let Ok(mut config) = Config::load(&path) {
                        config.general.position = if top { islet_core::config::ScreenPosition::Top } else { islet_core::config::ScreenPosition::Bottom };
                        if config.save_atomic(&path).is_ok() {
                            *self.shared.write().unwrap_or_else(std::sync::PoisonError::into_inner) = config;
                            self.apply_config();
                        }
                    }
                }
            }
            None if drag.dx.abs().max(drag.dy.abs()) < 8.0 => { self.dispatch(IslandEvent::Click); }
            _ => {}
        }
        self.send_snapshot();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn jitter_and_diagonals_do_not_choose_an_axis() {
        assert_eq!(axis(7.0, 2.0), None);
        assert_eq!(axis(30.0, 30.0), None);
        assert_eq!(axis(-40.0, 10.0), Some(Axis::Horizontal));
        assert_eq!(axis(4.0, -30.0), Some(Axis::Vertical));
    }
    #[test] fn resistance_is_continuous_symmetric_and_bounded() {
        assert_eq!(rubber(0.0), 0.0);
        assert_eq!(rubber(-50.0), -rubber(50.0));
        assert!(rubber(200.0) > rubber(50.0));
        assert!(rubber(10000.0) < 120.0);
    }
}
