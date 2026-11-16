//! The welcome tour's gesture practice: a pretend screen with the eyes pill
//! at its top edge, to drag left, right, down and up with the mouse. Each
//! swipe does what it does on the real island (off to a screen side, back
//! to the middle, to the other edge), and all four must be done to go on.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};

use crate::ui::{bold_font, CARD_LINE, MUTED, TEXT};

const PILL: Color32 = Color32::from_rgb(5, 5, 7);
const RIM: Color32 = Color32::from_rgb(56, 58, 64);
const SCREEN: Color32 = Color32::from_rgb(29, 30, 35);
const EYE: Color32 = Color32::from_rgb(244, 245, 250);
const HINT: Color32 = Color32::from_rgb(168, 178, 255);
const OK: Color32 = Color32::from_rgb(64, 210, 122);

/// The eyes pill, a little larger than on screen.
const PILL_W: f32 = 96.0;
const PILL_H: f32 = 36.0;
const GAP: f32 = 12.0;
const STAGE_H: f32 = 250.0;
/// How far a drag must go to count, as on the island.
const SWIPE: f32 = 44.0;
const PULL: f32 = 52.0;
/// A move across the stage.
const TRAVEL: f64 = 0.45;

/// The four gestures, in the order they are asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Down,
    Up,
}

pub const MOVES: [Move; 4] = [Move::Left, Move::Right, Move::Down, Move::Up];

impl Move {
    fn index(self) -> usize {
        MOVES.iter().position(|m| *m == self).unwrap_or(0)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Left => "Swipe left",
            Self::Right => "Swipe right",
            Self::Down => "Pull down",
            Self::Up => "Pull up",
        }
    }

    fn direction(self) -> Vec2 {
        match self {
            Self::Left => vec2(-1.0, 0.0),
            Self::Right => vec2(1.0, 0.0),
            Self::Down => vec2(0.0, 1.0),
            Self::Up => vec2(0.0, -1.0),
        }
    }
}

/// What a gesture did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Tucked into the left (-1) or right (1) side.
    ToSide(i8),
    ToMiddle,
    ToBottom,
    ToTop,
    /// Already against that side: nothing happens.
    AtSide,
    /// Already at that edge: nothing happens.
    AtEdge,
}

impl Outcome {
    fn caption(self) -> &'static str {
        match self {
            Self::ToSide(_) => {
                "With only the eyes showing, a swipe tucks the island into that side of your screen. It's not thrilled about it."
            }
            Self::ToMiddle => "Back in the middle, and happy again. Clicking a tucked island brings it back too.",
            Self::ToBottom => "Moved to the bottom edge. Onyx remembers where you leave it.",
            Self::ToTop => "Back at the top.",
            Self::AtSide => "It's already against that side. Swipe the other way.",
            Self::AtEdge => "It's already at that edge. Pull it the other way.",
        }
    }
}

pub struct Practice {
    pub started: bool,
    done: [bool; 4],
    /// At the top edge (else the bottom), and where along it: -1, 0 or 1.
    top: bool,
    side: i8,
    /// Where the last move began, and when.
    from: (f32, f32),
    moved_at: f64,
    /// When the eyes last cheered up (back in the middle).
    cheered_at: Option<f64>,
    last: Option<Outcome>,
    /// The drag under way, from where the press began.
    drag: Option<Vec2>,
    /// The pill's spring back after a drag: offset and when let go.
    released: Option<(Vec2, f64)>,
}

impl Default for Practice {
    fn default() -> Self {
        Self {
            started: false,
            done: [false; 4],
            top: true,
            side: 0,
            from: (0.0, 0.0),
            moved_at: f64::NEG_INFINITY,
            cheered_at: None,
            last: None,
            drag: None,
            released: None,
        }
    }
}

impl Practice {
    /// Every gesture has been tried.
    pub fn complete(&self) -> bool {
        self.done.iter().all(|done| *done)
    }

    /// The next gesture to try.
    fn current(&self) -> Option<Move> {
        MOVES.into_iter().find(|m| !self.done[m.index()])
    }

    /// Does what `m` does on the island, and counts it when it did something.
    pub fn apply(&mut self, m: Move, now: f64) -> Outcome {
        let before = self.position(now);
        let outcome = match m {
            Move::Left | Move::Right => {
                let toward: i8 = if m == Move::Left { -1 } else { 1 };
                if self.side == toward {
                    Outcome::AtSide
                } else if self.side == 0 {
                    self.side = toward;
                    Outcome::ToSide(toward)
                } else {
                    self.side = 0;
                    Outcome::ToMiddle
                }
            }
            Move::Down if self.top => {
                self.top = false;
                Outcome::ToBottom
            }
            Move::Up if !self.top => {
                self.top = true;
                Outcome::ToTop
            }
            Move::Down | Move::Up => Outcome::AtEdge,
        };
        if !matches!(outcome, Outcome::AtSide | Outcome::AtEdge) {
            self.done[m.index()] = true;
            self.from = before;
            self.moved_at = now;
        }
        if outcome == Outcome::ToMiddle {
            self.cheered_at = Some(now + TRAVEL * 0.8);
        }
        self.last = Some(outcome);
        outcome
    }

    /// A click: a tucked island comes back to the middle.
    fn click(&mut self, now: f64) {
        if self.side != 0 {
            let before = self.position(now);
            self.side = 0;
            self.from = before;
            self.moved_at = now;
            self.cheered_at = Some(now + TRAVEL * 0.8);
            self.last = Some(Outcome::ToMiddle);
        }
    }

    /// (side, anchor) now, between where the last move began and its end:
    /// side -1..1 along the edge, anchor 0 at the top and 1 at the bottom.
    fn position(&self, now: f64) -> (f32, f32) {
        let to = (f32::from(self.side), if self.top { 0.0 } else { 1.0 });
        let t = (((now - self.moved_at) / TRAVEL) as f32).clamp(0.0, 1.0);
        let e = ease_in_out(t);
        (lerp(self.from.0, to.0, e), lerp(self.from.1, to.1, e))
    }

    fn moving(&self, now: f64) -> bool {
        now - self.moved_at < TRAVEL
    }

    /// The practice area, the gesture list and what the last one did.
    pub fn show(&mut self, ui: &mut egui::Ui, animate: bool) {
        let now = ui.input(|input| input.time);
        let width = ui.available_width();
        let (stage, _) = ui.allocate_exact_size(vec2(width, STAGE_H), Sense::hover());
        let painter = ui.painter().with_clip_rect(stage);
        painter.rect_filled(stage, CornerRadius::same(14), SCREEN);
        // The screen's top and bottom edges.
        for y in [stage.top() + 0.5, stage.bottom() - 0.5] {
            painter.line_segment(
                [pos2(stage.left() + 14.0, y), pos2(stage.right() - 14.0, y)],
                Stroke::new(1.0, Color32::from_white_alpha(12)),
            );
        }

        let (side, anchor) = if animate { self.position(now) } else { (f32::from(self.side), if self.top { 0.0 } else { 1.0 }) };
        let overhang = PILL_H / 2.0;
        let span = stage.width() / 2.0 - PILL_W / 2.0 + overhang;
        let rest = pos2(
            stage.center().x + side * span,
            lerp(stage.top() + GAP + PILL_H / 2.0, stage.bottom() - GAP - PILL_H / 2.0, anchor),
        );

        // Drag with the mouse, as on the island.
        let mut pill = Rect::from_center_size(rest, vec2(PILL_W, PILL_H));
        let response = ui.interact(pill, ui.id().with("practice-pill"), Sense::click_and_drag());
        let held = self.started && response.dragged();
        if held {
            let delta = response.drag_delta();
            *self.drag.get_or_insert(Vec2::ZERO) += delta;
        }
        let mut offset = Vec2::ZERO;
        if let Some(drag) = self.drag {
            offset = vec2(rubber(drag.x, 70.0), rubber(drag.y, 50.0));
            if !held {
                self.drag = None;
                self.released = Some((offset, now));
                if let Some(m) = swipe_of(drag) {
                    self.apply(m, now);
                }
                offset = Vec2::ZERO;
            }
        } else if let Some((from, at)) = self.released {
            let t = (now - at) as f32;
            if t > 0.8 {
                self.released = None;
            } else {
                offset = from * (-7.5 * t).exp() * (13.0 * t).cos();
            }
        }
        if self.started && response.clicked() {
            self.click(now);
        }
        if self.started {
            response.on_hover_cursor(egui::CursorIcon::Grab);
        }
        pill = pill.translate(offset);

        // Moods: sad while tucked into a side, a happy hop on coming back.
        let settled = !self.moving(now);
        let sad = if self.side != 0 && settled { ((now - self.moved_at - TRAVEL) as f32 / 0.25).clamp(0.0, 1.0) } else { 0.0 };
        let cheer = self.cheered_at.map_or(f32::MAX, |at| (now - at) as f32);
        let happy = (0.0..1.1).contains(&cheer);
        let hop = if happy { (std::f32::consts::PI * cheer / 0.3).sin().abs() * (-2.2 * cheer).exp() * 5.0 } else { 0.0 };

        // The pill, its outer end past the screen side when tucked.
        let corner = CornerRadius::same((PILL_H / 2.0) as u8);
        painter.rect_filled(pill, corner, PILL);
        painter.rect_stroke(pill, corner, Stroke::new(1.0, RIM), StrokeKind::Inside);
        let visible = Rect::from_x_y_ranges(
            pill.left().max(stage.left())..=pill.right().min(stage.right()),
            pill.y_range(),
        );
        // Sad eyes look down, a little low.
        let look = vec2((offset.x / 40.0).clamp(-1.0, 1.0), ((offset.y / 30.0) + 0.5 * sad).clamp(-1.0, 1.0));
        eyes(&painter, visible.center() - vec2(0.0, hop - sad), PILL_H / 32.0, look, sad, happy, blink(now));

        // Which way to go next: chevrons drifting that way.
        if let (true, Some(m), None) = (self.started, self.current(), self.drag) {
            if settled {
                hint(&painter, pill, m, now, stage);
            }
        }

        if !self.started {
            painter.rect_filled(stage, CornerRadius::same(14), Color32::from_black_alpha(120));
            let button = Rect::from_center_size(stage.center() + vec2(0.0, 30.0), vec2(220.0, 44.0));
            let start = ui.interact(button, ui.id().with("practice-start"), Sense::click());
            let hover = ui.ctx().animate_bool_responsive(start.id, start.hovered());
            painter.rect_filled(button, CornerRadius::same(22), lerp_color(TEXT, Color32::WHITE, hover));
            painter.text(button.center(), Align2::CENTER_CENTER, "Start gesture practice", bold_font(15.0), PILL);
            if start.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.started = true;
            }
        }

        // The prompt, then the four gestures, ticked off as they are done.
        ui.add_space(14.0);
        let prompt = match (self.started, self.current()) {
            (false, _) => "Press start, then drag the island with your mouse.".to_owned(),
            (true, None) => "That's all four. You're ready.".to_owned(),
            (true, Some(m)) => self.prompt(m).to_owned(),
        };
        ui.label(egui::RichText::new(prompt).font(bold_font(17.0)).color(TEXT));
        if let Some(last) = self.last {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(last.caption()).size(13.5).color(MUTED));
        }
        ui.add_space(12.0);
        let chip_w = ((width - 3.0 * 10.0) / 4.0).floor();
        let (row, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
        for m in MOVES {
            let rect = Rect::from_min_size(row.min + vec2(m.index() as f32 * (chip_w + 10.0), 0.0), vec2(chip_w, 40.0));
            let done = self.done[m.index()];
            let current = self.started && self.current() == Some(m);
            let line = if done { OK } else if current { HINT } else { CARD_LINE };
            ui.painter().rect_stroke(rect, CornerRadius::same(20), Stroke::new(1.5, line), StrokeKind::Inside);
            let icon = pos2(rect.left() + 22.0, rect.center().y);
            if done {
                tick(ui.painter(), icon, OK);
            } else {
                chevron(ui.painter(), icon, m.direction(), 5.0, Stroke::new(2.0, if current { HINT } else { MUTED }));
            }
            ui.painter().text(
                pos2(rect.left() + 40.0, rect.center().y),
                Align2::LEFT_CENTER,
                m.label(),
                FontId::proportional(14.0),
                if done || current { TEXT } else { MUTED },
            );
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("With a song playing, left and right skip tracks instead, and pushing the island into its edge plays or pauses.")
                .small()
                .color(MUTED),
        );

        // The eyes blink and the hints drift: this page always moves.
        ui.ctx().request_repaint();
    }

    fn prompt(&self, m: Move) -> &'static str {
        match m {
            Move::Left if self.side == 1 => "Drag the island left to bring it back",
            Move::Left => "Drag the island to the left and let go",
            Move::Right if self.side == -1 => "Now drag it right to bring it back",
            Move::Right => "Drag the island to the right and let go",
            Move::Down if self.top => "Pull the island down to the bottom edge",
            Move::Down => "Pull it up first, then down",
            Move::Up if !self.top => "Now pull it back up to the top",
            Move::Up => "Pull it down, then back up",
        }
    }
}

/// The gesture a finished drag of `drag` makes, as the island reads it.
pub fn swipe_of(drag: Vec2) -> Option<Move> {
    if drag.x.abs() > drag.y.abs() * 1.2 && drag.x.abs() >= SWIPE {
        Some(if drag.x < 0.0 { Move::Left } else { Move::Right })
    } else if drag.y.abs() > drag.x.abs() * 1.2 && drag.y.abs() >= PULL {
        Some(if drag.y < 0.0 { Move::Up } else { Move::Down })
    } else {
        None
    }
}

/// Two eyes around `center`. `sad` slants a lid down toward the outside of
/// each (and makes them a little bigger); `happy` turns them into ^ ^;
/// `open` is the blink.
fn eyes(painter: &Painter, center: Pos2, unit: f32, look: Vec2, sad: f32, happy: bool, open: f32) {
    let radius = 6.0 * unit * (1.0 + 0.06 * sad);
    let ink = EYE;
    for side in [-1.0f32, 1.0] {
        let at = center + vec2(side * 14.0 * unit + look.x * 5.0 * unit, look.y * 3.0 * unit);
        if happy {
            let half = 5.6 * unit;
            let sag = 4.0 * unit;
            let points = (0..=10)
                .map(|step| {
                    let u = step as f32 / 5.0 - 1.0;
                    pos2(at.x + u * half, at.y + sag * 0.5 - sag * (1.0 - u * u))
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(2.6 * unit, ink)));
            continue;
        }
        let ry = (radius * open).max(radius * 0.2);
        let outline: Vec<Pos2> = (0..28)
            .map(|step| {
                let a = step as f32 / 28.0 * std::f32::consts::TAU;
                pos2(at.x + radius * a.cos(), at.y + ry * a.sin())
            })
            .collect();
        let shape = if sad > 0.01 {
            let top = at.y - ry;
            let inner = pos2(at.x - side * radius, top - 0.5);
            let outer = pos2(at.x + side * radius, top + ry * 2.0 * 0.42 * sad);
            below_line(&outline, inner, outer)
        } else {
            outline
        };
        if shape.len() >= 3 {
            painter.add(Shape::convex_polygon(shape, ink, Stroke::NONE));
        }
    }
}

/// The part of the convex polygon on or below the line through `a` and `b`.
fn below_line(points: &[Pos2], a: Pos2, b: Pos2) -> Vec<Pos2> {
    let slope = if (b.x - a.x).abs() < 1e-4 { 0.0 } else { (b.y - a.y) / (b.x - a.x) };
    let depth = |p: Pos2| p.y - (a.y + (p.x - a.x) * slope);
    let mut kept = Vec::with_capacity(points.len() + 2);
    for (index, point) in points.iter().enumerate() {
        let next = points[(index + 1) % points.len()];
        let (here, there) = (depth(*point), depth(next));
        if here >= 0.0 {
            kept.push(*point);
        }
        if (here >= 0.0) != (there >= 0.0) {
            kept.push(point.lerp(next, here / (here - there)));
        }
    }
    kept
}

/// Chevrons beside the pill, drifting the way to drag it.
fn hint(painter: &Painter, pill: Rect, m: Move, now: f64, stage: Rect) {
    let dir = m.direction();
    let start = pill.center() + vec2(dir.x * (pill.width() / 2.0 + 16.0), dir.y * (pill.height() / 2.0 + 14.0));
    let phase = (now as f32 * 0.9).fract();
    for i in 0..3 {
        let t = (phase + i as f32 / 3.0).fract();
        let at = start + dir * (t * 34.0);
        let alpha = (t * 3.0).min(1.0) * (1.0 - t);
        if stage.shrink(4.0).contains(at) {
            chevron(painter, at, dir, 6.0, Stroke::new(2.2, HINT.gamma_multiply(alpha)));
        }
    }
}

fn chevron(painter: &Painter, at: Pos2, dir: Vec2, size: f32, stroke: Stroke) {
    let across = vec2(-dir.y, dir.x);
    painter.add(Shape::line(
        vec![at - dir * size * 0.5 + across * size, at + dir * size * 0.5, at - dir * size * 0.5 - across * size],
        stroke,
    ));
}

fn tick(painter: &Painter, at: Pos2, color: Color32) {
    painter.add(Shape::line(
        vec![at + vec2(-5.0, 0.0), at + vec2(-1.5, 3.5), at + vec2(5.5, -4.0)],
        Stroke::new(2.2, color),
    ));
}

/// A blink every few seconds.
fn blink(now: f64) -> f32 {
    let t = (now % 3.7) as f32;
    if t < 0.2 {
        1.0 - (t / 0.2 * std::f32::consts::PI).sin()
    } else {
        1.0
    }
}

fn rubber(distance: f32, softness: f32) -> f32 {
    distance / (1.0 + distance.abs() / softness) * 1.6
}

fn ease_in_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) * 0.5
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let c = |x: u8, y: u8| lerp(f32::from(x), f32::from(y), t).round() as u8;
    Color32::from_rgb(c(a.r(), b.r()), c(a.g(), b.g()), c(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags_read_as_the_island_reads_them() {
        assert_eq!(swipe_of(vec2(-60.0, 5.0)), Some(Move::Left));
        assert_eq!(swipe_of(vec2(60.0, -8.0)), Some(Move::Right));
        assert_eq!(swipe_of(vec2(4.0, 70.0)), Some(Move::Down));
        assert_eq!(swipe_of(vec2(0.0, -70.0)), Some(Move::Up));
        // Too short, or diagonal: nothing.
        assert_eq!(swipe_of(vec2(-20.0, 0.0)), None);
        assert_eq!(swipe_of(vec2(60.0, 60.0)), None);
    }

    #[test]
    fn all_four_gestures_must_be_done_and_only_count_when_they_move_it() {
        let mut practice = Practice::default();
        assert_eq!(practice.current(), Some(Move::Left));
        // Up at the top does nothing, and does not count.
        assert_eq!(practice.apply(Move::Up, 0.0), Outcome::AtEdge);
        assert!(!practice.done[Move::Up.index()]);
        assert_eq!(practice.apply(Move::Left, 1.0), Outcome::ToSide(-1));
        assert_eq!(practice.apply(Move::Left, 2.0), Outcome::AtSide);
        assert_eq!(practice.current(), Some(Move::Right));
        assert_eq!(practice.apply(Move::Right, 3.0), Outcome::ToMiddle);
        assert_eq!(practice.apply(Move::Down, 4.0), Outcome::ToBottom);
        assert!(!practice.complete());
        assert_eq!(practice.apply(Move::Up, 5.0), Outcome::ToTop);
        assert!(practice.complete());
        assert_eq!(practice.current(), None);
    }

    #[test]
    fn a_click_brings_a_tucked_island_back() {
        let mut practice = Practice::default();
        practice.apply(Move::Right, 0.0);
        assert_eq!(practice.side, 1);
        practice.click(1.0);
        assert_eq!(practice.side, 0);
        assert_eq!(practice.last, Some(Outcome::ToMiddle));
        // The move eases from where it was.
        assert_eq!(practice.position(1.0), (1.0, 0.0));
        assert_eq!(practice.position(2.0), (0.0, 0.0));
    }
}
