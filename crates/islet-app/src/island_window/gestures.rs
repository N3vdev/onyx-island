//! Collapsed-island pointer gestures, using screen coordinates across captures.
use super::*;
use islet_core::config::IslandCorner;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis { Horizontal, Vertical }
/// `media`: the pill was showing the song when the press began, so sideways
/// swipes change track and a push into the screen edge plays or pauses.
/// `room`: logical pixels between the press and that edge, since the cursor
/// stops at the edge and a push can travel no further.
pub(super) struct Drag { start: POINT, dx: f32, dy: f32, axis: Option<Axis>, top: bool, media: bool, room: f32 }
fn axis(dx: f32, dy: f32) -> Option<Axis> {
    if dx.abs().max(dy.abs()) < 8.0 { None }
    else if dx.abs() > dy.abs() * 1.2 { Some(Axis::Horizontal) }
    else if dy.abs() > dx.abs() * 1.2 { Some(Axis::Vertical) }
    else { None }
}
fn rubber(distance: f32) -> f32 { distance / (1.0 + distance.abs() / 120.0) }
/// How far a push into the edge must go to count, given the room left.
fn push_threshold(room: f32) -> f32 { (room * 0.6).clamp(4.0, 24.0) }

impl IslandHost {
    pub(super) fn begin_gesture(&mut self) -> bool {
        if self.cursor_on_media_bubble() || self.state.expanded || self.traveling || self.relocating || self.state.dictation.is_some() || self.state.lens.is_some()
            || self.state.needs_input.is_some() || self.state.done.is_some()
            || self.state.converter.is_some() { return false; }
        let mut start = POINT::default();
        if unsafe { GetCursorPos(&mut start) }.is_err() { return false; }
        self.cancel_timer(TimerKind::HoverExpand);
        self.cancel_timer(TimerKind::HoverCollapse);
        self.press_armed = false;
        let top = self.effective_anchor_top();
        let media = matches!(self.state.primary_card(), Some(islet_core::ContentCard::Media(_)));
        let scale = 96.0 / self.dpi as f32;
        let room = monitor_info(unsafe { MonitorFromPoint(start, MONITOR_DEFAULTTONEAREST) })
            .map_or(0.0, |info| {
                let edge = if top { start.y - info.rcMonitor.top } else { info.rcMonitor.bottom - 1 - start.y };
                edge.max(0) as f32 * scale
            });
        self.gesture = Some(Drag { start, dx: 0.0, dy: 0.0, axis: None, top, media, room });
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
        // Resist a pull into the edge; allow generous inward travel. With a
        // song showing, the edge gives a little, since a push there plays or
        // pauses.
        let outward = (drag.top && drag.dy < 0.0) || (!drag.top && drag.dy > 0.0);
        let give = if !outward { 1.0 } else if drag.media { 0.45 } else { 0.04 };
        let (x, y) = match drag.axis {
            Some(Axis::Horizontal) => (rubber(drag.dx), 0.0),
            Some(Axis::Vertical) => (0.0, rubber(drag.dy) * give),
            // Feedback starts at the first pixel, before navigation locks an axis.
            None => (rubber(drag.dx), rubber(drag.dy) * give),
        };
        let target = self.last_target.unwrap_or(LayoutTarget::HIDDEN);
        self.morph_box = self.target_frame(target, self.effective_anchor_top()).bounds().map(|r| r.inflate(GESTURE_REGION_MARGIN_DP));
        self.update_region();
        let _ = self.tx.send(RenderCommand::Pull(x, y, true));
    }
    pub(super) fn finish_gesture(&mut self, commit: bool) {
        let Some(drag) = self.gesture.take() else { return; };
        unsafe { let _ = KillTimer(Some(self.hwnd), TIMER_GESTURE); let _ = ReleaseCapture(); }
        let _ = self.tx.send(RenderCommand::Pull(0.0, 0.0, false));
        if !commit { self.send_snapshot(); return; }
        let outward = (drag.top && drag.dy < 0.0) || (!drag.top && drag.dy > 0.0);
        match drag.axis {
            // A song on the pill: left for the next track, right for the last.
            Some(Axis::Horizontal) if drag.media && drag.dx.abs() >= 48.0 => {
                self.media_action(if drag.dx < 0.0 { MediaAction::Next } else { MediaAction::Previous });
            }
            // Pushed into the edge it already sits on: play or pause.
            Some(Axis::Vertical) if drag.media && outward && drag.dy.abs() >= push_threshold(drag.room) => {
                self.media_action(MediaAction::PlayPause);
            }
            // Only the eyes: off to a corner of this edge as a circle, or
            // back to the middle from one.
            Some(Axis::Horizontal) if drag.dx.abs() >= 48.0 && self.state.shows_only_eyes() && self.preview.is_none() => {
                if let Some(corner) = swiped_corner(self.state.corner, drag.dx < 0.0) {
                    self.set_corner(corner);
                }
            }
            Some(Axis::Horizontal) if drag.dx.abs() >= 48.0 => {
                let page: i32 = match self.state.compact_widget {
                    None => 0, Some(IdleGlance::Clock) => 1, Some(IdleGlance::Stats) => 2, Some(IdleGlance::Eyes) => 3,
                };
                let mut next = (page + if drag.dx < 0.0 { 1 } else { -1 }).rem_euclid(4);
                // Do not navigate to an invisible "normal" page on an idle PC.
                let mut normal = self.state.clone();
                normal.compact_widget = None;
                if next == 0 && normal.primary_card().is_none() {
                    next = if drag.dx < 0.0 { 1 } else { 3 };
                }
                self.state.compact_widget = match next {
                    1 => Some(IdleGlance::Clock), 2 => Some(IdleGlance::Stats), 3 => Some(IdleGlance::Eyes), _ => None,
                };
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
            None if drag.dx.abs().max(drag.dy.abs()) < 8.0 => self.click_island(),
            _ => {}
        }
        self.send_snapshot();
    }

    /// A click: the eyes attached to a screen side come back to the middle;
    /// anywhere else it is the island's usual click.
    pub(super) fn click_island(&mut self) {
        if self.state.cornered() && self.preview.is_none() {
            self.set_corner(IslandCorner::Middle);
            self.send_snapshot();
        } else {
            self.dispatch(IslandEvent::Click);
        }
    }

    /// Remembers the corner (kept across restarts); the next snapshot moves
    /// the island there.
    fn set_corner(&mut self, corner: IslandCorner) {
        self.state.corner = corner;
        let path = crate::paths::Paths::discover().config_file;
        if let Ok(mut config) = Config::load(&path) {
            config.general.corner = corner;
            if config.save_atomic(&path).is_ok() {
                *self.shared.write().unwrap_or_else(std::sync::PoisonError::into_inner) = config;
            }
        }
    }
}

/// Where a sideways swipe of the eyes takes them: from the middle to the
/// corner it points at, from a corner back to the middle. Swiping further
/// into a corner it is already in does nothing.
fn swiped_corner(from: IslandCorner, left: bool) -> Option<IslandCorner> {
    match (from, left) {
        (IslandCorner::Middle, true) => Some(IslandCorner::Left),
        (IslandCorner::Middle, false) => Some(IslandCorner::Right),
        (IslandCorner::Left, false) | (IslandCorner::Right, true) => Some(IslandCorner::Middle),
        _ => None,
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
    #[test] fn a_push_into_the_edge_fits_the_room_left() {
        assert_eq!(push_threshold(100.0), 24.0);
        assert_eq!(push_threshold(20.0), 12.0);
        assert_eq!(push_threshold(0.0), 4.0);
    }
    #[test] fn resistance_is_continuous_symmetric_and_bounded() {
        assert_eq!(rubber(0.0), 0.0);
        assert_eq!(rubber(-50.0), -rubber(50.0));
        assert!(rubber(200.0) > rubber(50.0));
        assert!(rubber(10000.0) < 120.0);
    }
    #[test] fn the_eyes_swipe_to_a_corner_and_back_to_the_middle() {
        assert_eq!(swiped_corner(IslandCorner::Middle, true), Some(IslandCorner::Left));
        assert_eq!(swiped_corner(IslandCorner::Middle, false), Some(IslandCorner::Right));
        assert_eq!(swiped_corner(IslandCorner::Left, false), Some(IslandCorner::Middle));
        assert_eq!(swiped_corner(IslandCorner::Right, true), Some(IslandCorner::Middle));
        assert_eq!(swiped_corner(IslandCorner::Left, true), None);
        assert_eq!(swiped_corner(IslandCorner::Right, false), None);
    }
}
