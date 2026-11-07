//! Small looping illustrations for the welcome tour: the Onyx logo drawing
//! itself, and one scene for each thing your hands can do with the island.
//! Pure painting from a clock; nothing here keeps state.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use eframe::egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke,
    StrokeKind,
};

use crate::ui::{bold_font, MUTED, TEXT};

pub const LAVENDER: Color32 = Color32::from_rgb(168, 178, 255);
const SILVER: Color32 = Color32::from_rgb(214, 218, 230);
const PILL: Color32 = Color32::from_rgb(5, 5, 7);
const RIM: Color32 = Color32::from_rgb(56, 58, 64);
const SCREEN: Color32 = Color32::from_rgb(29, 30, 35);
const KEY: Color32 = Color32::from_rgb(44, 46, 52);
const TRACKS: [Color32; 2] = [
    Color32::from_rgb(255, 138, 101),
    Color32::from_rgb(122, 162, 255),
];

/// Where the logo's dot rests, as a fraction of the pill (as on the icon).
const DOT_AT: (f32, f32) = (0.85, 0.5);

fn cr(radius: f32) -> CornerRadius {
    CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8)
}

/// Progress through `[from, to]`: 0 before it, 1 after.
fn seg(t: f32, from: f32, to: f32) -> f32 {
    ((t - from) / (to - from)).clamp(0.0, 1.0)
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn ease_in_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) * 0.5
    }
}

fn ease_out(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

/// Overshoots a touch and settles.
fn ease_out_back(x: f32) -> f32 {
    const OVERSHOOT: f32 = 1.5;
    let x = x.clamp(0.0, 1.0) - 1.0;
    1.0 + (OVERSHOOT + 1.0) * x.powi(3) + OVERSHOOT * x.powi(2)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| lerp(f32::from(x), f32::from(y), t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

fn fade(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}

/// Pull with resistance: the further, the less it follows.
fn rubber(distance: f32, softness: f32) -> f32 {
    distance / (1.0 + distance.abs() / softness)
}

/// A spring settling from `offset` to 0, `elapsed` seconds after release.
fn settle(offset: f32, elapsed: f32) -> f32 {
    if elapsed <= 0.0 {
        return offset;
    }
    offset * (-7.5 * elapsed).exp() * (13.0 * elapsed).cos()
}

/// Things you can do with the island, each drawn as a looping scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Hover (or click, per settings) to open the full view.
    Open {
        click: bool,
    },
    SwipeSongs,
    PushPlay,
    PullMove,
    ScrollWidgets,
    DropConvert,
}

impl Gesture {
    pub fn title(self) -> &'static str {
        match self {
            Self::Open { click: false } => "Hover to open",
            Self::Open { click: true } => "Click to open",
            Self::SwipeSongs => "Swipe to skip",
            Self::PushPlay => "Push to play or pause",
            Self::PullMove => "Pull to move",
            Self::ScrollWidgets => "Scroll for widgets",
            Self::DropConvert => "Drop to convert",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Open { click: false } => {
                "Rest the pointer on the island for the full player and your widgets."
            }
            Self::Open { click: true } => "Click the island for the full player and your widgets.",
            Self::SwipeSongs => {
                "With a song playing, swipe left for the next track or right for the last one."
            }
            Self::PushPlay => "Push the island into the edge of the screen to play or pause.",
            Self::PullMove => "Drag it away from its edge to move the island to the top or bottom.",
            Self::ScrollWidgets => {
                "Open the island and scroll to switch between the player and widgets."
            }
            Self::DropConvert => {
                "Drag files onto the island and drop them on a format to convert them."
            }
        }
    }

    fn period(self) -> f32 {
        match self {
            Self::Open { .. } => 4.4,
            Self::SwipeSongs => 5.0,
            Self::PushPlay => 4.0,
            Self::PullMove => 5.4,
            Self::ScrollWidgets => 6.6,
            Self::DropConvert => 5.6,
        }
    }

    /// A moment that shows the gesture's result, for when animations are off.
    fn still(self) -> f32 {
        match self {
            Self::Open { .. } => 2.0,
            Self::SwipeSongs => 1.0,
            Self::PushPlay => 1.6,
            Self::PullMove => 1.2,
            Self::ScrollWidgets => 0.8,
            Self::DropConvert => 1.8,
        }
    }
}

/// Draws `gesture` into `rect` at `time` seconds (any clock), or a still
/// frame when `animate` is off.
pub fn scene(painter: &Painter, rect: Rect, gesture: Gesture, time: f32, animate: bool) {
    let painter = painter.with_clip_rect(rect);
    painter.rect_filled(rect, cr(12.0), SCREEN);
    // The screen's edge the island sits against.
    painter.line_segment(
        [
            pos2(rect.left() + 12.0, rect.top() + 0.5),
            pos2(rect.right() - 12.0, rect.top() + 0.5),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(10)),
    );
    let (t, cycle) = if animate {
        let period = gesture.period();
        (time.rem_euclid(period), (time / period).floor() as i64)
    } else {
        (gesture.still(), 0)
    };
    match gesture {
        Gesture::Open { click } => open_scene(&painter, rect, t, time, click),
        Gesture::SwipeSongs => swipe_scene(&painter, rect, t, time),
        Gesture::PushPlay => push_scene(&painter, rect, t, time, cycle),
        Gesture::PullMove => pull_scene(&painter, rect, t, time),
        Gesture::ScrollWidgets => scroll_scene(&painter, rect, t, time),
        Gesture::DropConvert => drop_scene(&painter, rect, t),
    }
}

const REST_W: f32 = 72.0;
const REST_H: f32 = 22.0;
const GAP: f32 = 9.0;

fn island(painter: &Painter, rect: Rect, corner: f32) {
    painter.rect_filled(rect, cr(corner), PILL);
    painter.rect_stroke(rect, cr(corner), Stroke::new(1.0, RIM), StrokeKind::Inside);
}

/// A resting pill with its top `top` and its middle at `x`.
fn rest_rect(x: f32, top: f32) -> Rect {
    Rect::from_min_size(pos2(x - REST_W / 2.0, top), vec2(REST_W, REST_H))
}

/// Album art and the moving bars, inside a resting pill.
fn compact_media(painter: &Painter, pill: Rect, art: Color32, time: f32, playing: f32, alpha: f32) {
    if alpha <= 0.01 {
        return;
    }
    let size = pill.height() - 9.0;
    let art_rect = Rect::from_min_size(
        pos2(pill.left() + 6.0, pill.center().y - size / 2.0),
        vec2(size, size),
    );
    painter.rect_filled(art_rect, cr(3.0), fade(art, alpha));
    bars(
        painter,
        pos2(pill.right() - 18.0, pill.center().y),
        size,
        mix(art, Color32::WHITE, 0.35),
        time,
        playing,
        alpha,
    );
}

fn bars(
    painter: &Painter,
    left_mid: Pos2,
    height: f32,
    color: Color32,
    time: f32,
    playing: f32,
    alpha: f32,
) {
    for i in 0..4 {
        let phase = i as f32;
        let wave = 0.5
            + 0.5
                * (0.6 * (time * 7.3 + phase * 1.7).sin() + 0.4 * (time * 4.1 + phase * 0.9).sin());
        let h = lerp(2.2, 2.2 + (height - 2.2) * wave, playing.clamp(0.0, 1.0));
        let bar = Rect::from_center_size(pos2(left_mid.x + phase * 3.6, left_mid.y), vec2(2.2, h));
        painter.rect_filled(bar, cr(1.1), fade(color, alpha));
    }
}

fn play_glyph(painter: &Painter, center: Pos2, size: f32, playing: bool, color: Color32) {
    if playing {
        // Showing what a press does: pause.
        for dx in [-0.22, 0.22] {
            painter.rect_filled(
                Rect::from_center_size(
                    center + vec2(dx * size, 0.0),
                    vec2(size * 0.24, size * 0.8),
                ),
                cr(size * 0.08),
                color,
            );
        }
    } else {
        let s = size * 0.42;
        painter.add(Shape::convex_polygon(
            vec![
                center + vec2(-s * 0.7, -s),
                center + vec2(s * 1.0, 0.0),
                center + vec2(-s * 0.7, s),
            ],
            color,
            Stroke::NONE,
        ));
    }
}

/// The full player inside an open island.
fn expanded_media(painter: &Painter, card: Rect, art: Color32, time: f32, alpha: f32) {
    if alpha <= 0.01 {
        return;
    }
    let art_size = (card.height() - 30.0).max(10.0);
    let art_rect = Rect::from_min_size(card.min + vec2(12.0, 11.0), vec2(art_size, art_size));
    painter.rect_filled(art_rect, cr(8.0), fade(art, alpha));
    painter.rect_filled(
        Rect::from_min_size(art_rect.min, vec2(art_size, art_size * 0.45)),
        CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        },
        fade(Color32::from_white_alpha(28), alpha),
    );
    let text_x = art_rect.right() + 10.0;
    painter.rect_filled(
        Rect::from_min_size(pos2(text_x, art_rect.top() + 4.0), vec2(74.0, 6.0)),
        cr(3.0),
        fade(TEXT, alpha * 0.9),
    );
    painter.rect_filled(
        Rect::from_min_size(pos2(text_x, art_rect.top() + 15.0), vec2(48.0, 5.0)),
        cr(2.5),
        fade(MUTED, alpha * 0.7),
    );
    // Previous, pause, next.
    let controls_y = art_rect.bottom() - 6.0;
    let gray = fade(Color32::from_gray(150), alpha);
    for (dx, flip) in [(0.0, true), (36.0, false)] {
        let c = pos2(text_x + 8.0 + dx, controls_y);
        let d = if flip { -1.0 } else { 1.0 };
        for step in [0.0, 4.0] {
            painter.add(Shape::convex_polygon(
                vec![
                    c + vec2(d * (step - 3.5), -3.5),
                    c + vec2(d * (step + 0.5), 0.0),
                    c + vec2(d * (step - 3.5), 3.5),
                ],
                gray,
                Stroke::NONE,
            ));
        }
    }
    play_glyph(
        painter,
        pos2(text_x + 26.0, controls_y),
        11.0,
        true,
        fade(TEXT, alpha),
    );
    // Progress.
    let track = Rect::from_min_max(
        pos2(card.left() + 12.0, card.bottom() - 11.0),
        pos2(card.right() - 12.0, card.bottom() - 8.0),
    );
    painter.rect_filled(track, cr(1.5), fade(Color32::from_gray(52), alpha));
    let done = 0.25 + 0.5 * ((time * 0.15).rem_euclid(1.0));
    painter.rect_filled(
        Rect::from_min_size(track.min, vec2(track.width() * done, track.height())),
        cr(1.5),
        fade(mix(art, Color32::WHITE, 0.2), alpha),
    );
}

/// The pointer, with its tip at `tip`. `pressed` (0..1) shrinks it a touch
/// and rings the tip.
fn cursor(painter: &Painter, tip: Pos2, pressed: f32) {
    let scale = 1.0 - 0.08 * pressed;
    if pressed > 0.01 {
        painter.circle_filled(
            tip,
            7.0 * pressed,
            Color32::from_white_alpha((40.0 * pressed) as u8),
        );
    }
    let points = [
        (0.0, 0.0),
        (0.0, 15.0),
        (4.0, 11.6),
        (6.6, 17.0),
        (9.0, 16.0),
        (6.5, 10.7),
        (11.0, 10.7),
    ]
    .iter()
    .map(|(x, y)| tip + vec2(x * scale, y * scale))
    .collect();
    painter.add(Shape::convex_polygon(
        points,
        Color32::WHITE,
        Stroke::new(1.0, Color32::from_gray(18)),
    ));
}

fn open_scene(painter: &Painter, rect: Rect, t: f32, time: f32, click: bool) {
    let x = rect.center().x;
    let start = pos2(x + 82.0, rect.bottom() - 26.0);
    let target = pos2(x + 16.0, rect.top() + GAP + 12.0);
    let go = ease_in_out(seg(t, 0.2, 1.0)) - ease_in_out(seg(t, 3.0, 3.6));
    let tip = start.lerp(target, go);
    let opens_at = if click { 1.1 } else { 1.25 };
    let open =
        ease_out_back(seg(t, opens_at, opens_at + 0.45)) * (1.0 - ease_in_out(seg(t, 3.0, 3.35)));
    let w = lerp(REST_W, 200.0, open);
    let h = lerp(REST_H, 88.0, open);
    let card = Rect::from_min_size(pos2(x - w / 2.0, rect.top() + GAP), vec2(w, h));
    island(painter, card, lerp(11.0, 22.0, open.clamp(0.0, 1.0)));
    compact_media(
        painter,
        card,
        TRACKS[0],
        time,
        1.0,
        1.0 - smooth(seg(open, 0.0, 0.3)),
    );
    expanded_media(painter, card, TRACKS[0], time, smooth(seg(open, 0.55, 1.0)));
    let mut pressed = 0.0;
    if click {
        let press = seg(t, 0.98, 1.3);
        pressed = (press * PI).sin();
        if press > 0.0 && press < 1.0 {
            painter.circle_stroke(
                tip,
                4.0 + 12.0 * press,
                Stroke::new(
                    1.5,
                    Color32::from_white_alpha((120.0 * (1.0 - press)) as u8),
                ),
            );
        }
    }
    cursor(painter, tip, pressed);
}

fn swipe_scene(painter: &Painter, rect: Rect, t: f32, time: f32) {
    let x = rect.center().x;
    let top = rect.top() + GAP;
    let home = pos2(x + 8.0, top + 12.0);
    let away = pos2(x + 84.0, rect.bottom() - 24.0);
    const REACH: f32 = 54.0;
    // Where the pointer has dragged to, sideways from the pill's middle.
    let dx = if t < 2.0 {
        -REACH * ease_in_out(seg(t, 0.75, 1.25))
    } else if t < 2.6 {
        -REACH * (1.0 - ease_in_out(seg(t, 2.0, 2.45)))
    } else if t < 3.8 {
        REACH * ease_in_out(seg(t, 2.6, 3.1))
    } else {
        REACH * (1.0 - ease_in_out(seg(t, 3.8, 4.25)))
    };
    let arrive = ease_in_out(seg(t, 0.1, 0.6)) - ease_in_out(seg(t, 4.35, 4.9));
    let tip = away.lerp(home + vec2(dx, 0.0), arrive);
    let first_press = (0.7..1.3).contains(&t);
    let second_press = (2.55..3.15).contains(&t);
    let pressed = if first_press || second_press {
        1.0
    } else {
        0.0
    };
    // The pill follows a held drag, then springs home.
    let offset = if first_press || second_press {
        rubber(dx, 60.0)
    } else if t >= 1.3 && t < 2.55 {
        settle(rubber(-REACH, 60.0), t - 1.3)
    } else if t >= 3.15 {
        settle(rubber(REACH, 60.0), t - 3.15)
    } else {
        0.0
    };
    // The track changes on each release.
    let (from, to, changed_at) = if t >= 3.15 {
        (TRACKS[1], TRACKS[0], 3.15)
    } else if t >= 1.3 {
        (TRACKS[0], TRACKS[1], 1.3)
    } else {
        (TRACKS[1], TRACKS[0], -1.0)
    };
    let art = mix(from, to, smooth(seg(t, changed_at, changed_at + 0.3)));
    let pill = rest_rect(x + offset, top);
    island(painter, pill, 11.0);
    compact_media(painter, pill, art, time, 1.0, 1.0);
    // A chevron shows which way the drag goes.
    if pressed > 0.0 && dx.abs() > 4.0 {
        let alpha = (dx.abs() / REACH).min(1.0);
        let side = dx.signum();
        let c = pos2(
            pill.center().x + side * (REST_W / 2.0 + 12.0),
            pill.center().y,
        );
        let s = 4.0;
        painter.add(Shape::line(
            vec![
                c + vec2(-side * s * 0.5, -s),
                c + vec2(side * s * 0.5, 0.0),
                c + vec2(-side * s * 0.5, s),
            ],
            Stroke::new(1.6, fade(TEXT, alpha)),
        ));
    }
    for (label, at) in [("Next", 1.3), ("Previous", 3.15)] {
        let alpha = smooth(seg(t, at, at + 0.2)) * (1.0 - smooth(seg(t, at + 0.75, at + 1.0)));
        if alpha > 0.01 {
            painter.text(
                pos2(x, top + REST_H + 14.0 + 4.0 * (1.0 - alpha)),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(12.0),
                fade(TEXT, alpha),
            );
        }
    }
    cursor(painter, tip, pressed);
}

fn push_scene(painter: &Painter, rect: Rect, t: f32, time: f32, cycle: i64) {
    let x = rect.center().x;
    let top = rect.top() + GAP;
    let home = pos2(x + 8.0, top + 12.0);
    let away = pos2(x + 80.0, rect.bottom() - 24.0);
    let arrive = ease_in_out(seg(t, 0.1, 0.6)) - ease_in_out(seg(t, 2.7, 3.3));
    let push = ease_in_out(seg(t, 0.85, 1.25)) * (1.0 - ease_out(seg(t, 1.45, 1.6)));
    let tip = away.lerp(home + vec2(0.0, -10.0 * push), arrive);
    let pressed = if (0.75..1.45).contains(&t) { 1.0 } else { 0.0 };
    let squash = smooth(seg(t, 0.85, 1.25)) * (1.0 - ease_out(seg(t, 1.45, 1.8)));
    let pill = Rect::from_min_size(
        pos2(x - (REST_W + 8.0 * squash) / 2.0, top - 4.0 * squash),
        vec2(REST_W + 8.0 * squash, REST_H - 3.0 * squash),
    );
    // The edge lights up under the push.
    if squash > 0.01 {
        painter.line_segment(
            [
                pos2(x - 60.0, rect.top() + 0.5),
                pos2(x + 60.0, rect.top() + 0.5),
            ],
            Stroke::new(2.0, fade(LAVENDER, 0.6 * squash)),
        );
    }
    let playing_before = cycle.rem_euclid(2) == 0;
    let toggled = t >= 1.45;
    let playing_now = playing_before != toggled;
    let level = if toggled {
        let k = smooth(seg(t, 1.45, 1.75));
        if playing_now {
            k
        } else {
            1.0 - k
        }
    } else if playing_before {
        1.0
    } else {
        0.0
    };
    island(painter, pill, 11.0);
    compact_media(painter, pill, TRACKS[0], time, level, 1.0);
    // What the push did, in a bubble under the island.
    let pop = ease_out_back(seg(t, 1.45, 1.75));
    let alpha = 1.0 - smooth(seg(t, 2.0, 2.4));
    if pop > 0.0 && alpha > 0.0 {
        let c = pos2(x, top + REST_H + 20.0);
        painter.circle_filled(c, 14.0 * pop, fade(KEY, alpha));
        // The glyph shows the new state: paused shows a play arrow.
        play_glyph(painter, c, 13.0 * pop, playing_now, fade(TEXT, alpha));
    }
    cursor(painter, tip, pressed);
}

fn pull_scene(painter: &Painter, rect: Rect, t: f32, time: f32) {
    let x = rect.center().x;
    let top_y = rect.top() + GAP;
    let bottom_y = rect.bottom() - GAP - REST_H;
    let away = pos2(x + 84.0, rect.center().y + 10.0);
    const PULL: f32 = 62.0;
    let grab = vec2(8.0, 12.0);
    // First half: down from the top; second half: back up.
    let (from_y, to_y, local, sign) = if t < 2.4 {
        (top_y, bottom_y, t, 1.0)
    } else {
        (bottom_y, top_y, t - 2.4, -1.0)
    };
    let drag = PULL * ease_in_out(seg(local, 0.7, 1.3));
    let pressed = if (0.6..1.35).contains(&local) {
        1.0
    } else {
        0.0
    };
    let travel = ease_out_back(seg(local, 1.35, 1.9));
    let held = from_y + sign * rubber(drag, 90.0);
    let y = if local < 1.35 {
        held
    } else {
        lerp(held, to_y, travel)
    };
    let stretch = if local < 1.35 {
        drag / PULL
    } else {
        1.0 - seg(local, 1.35, 1.6)
    };
    let pill = Rect::from_min_size(
        pos2(x - REST_W / 2.0, y),
        vec2(REST_W - 4.0 * stretch, REST_H + 3.0 * stretch),
    )
    .translate(vec2(2.0 * stretch, 0.0));
    // Where it will land.
    let ghost = if local < 1.35 {
        drag / PULL
    } else {
        1.0 - seg(local, 1.35, 1.6)
    };
    if ghost > 0.01 {
        painter.rect_stroke(
            rest_rect(x, to_y),
            cr(11.0),
            Stroke::new(1.0, fade(Color32::from_gray(120), 0.7 * ghost)),
            StrokeKind::Inside,
        );
    }
    island(painter, pill, 11.0);
    compact_media(painter, pill, TRACKS[1], time, 1.0, 1.0);
    // The pointer: in, drag, ride along, out (and in again for the return).
    let start = pos2(x, from_y) + grab;
    let tip = if local < 0.6 {
        let from = if t < 2.4 {
            away
        } else {
            pos2(x, top_y + PULL * 0.6) + grab
        };
        from.lerp(start, ease_in_out(seg(local, 0.1, 0.55)))
    } else if local < 1.35 {
        start + vec2(0.0, sign * drag)
    } else if t < 2.4 {
        (start + vec2(0.0, sign * PULL)).lerp(
            pos2(x, to_y) + grab + vec2(0.0, -PULL * 0.6),
            ease_in_out(seg(local, 1.6, 2.3)),
        )
    } else {
        (start + vec2(0.0, sign * PULL)).lerp(away, ease_in_out(seg(local, 2.2, 2.9)))
    };
    cursor(painter, tip, pressed);
}

fn scroll_scene(painter: &Painter, rect: Rect, t: f32, time: f32) {
    let x = rect.center().x;
    let card = Rect::from_min_size(pos2(x - 98.0, rect.top() + GAP), vec2(196.0, 92.0));
    island(painter, card, 22.0);
    // Scroll position in pages: hold, glide to the next, hold...
    let glide = |from: f32| ease_in_out(seg(t, from, from + 0.4));
    let pages = glide(1.6) + glide(3.6) + glide(5.6);
    let inner = painter.with_clip_rect(card.shrink(1.0));
    for index in 0..4 {
        let offset = (index as f32 - pages) * card.height();
        if offset.abs() >= card.height() {
            continue;
        }
        let page = card.translate(vec2(0.0, offset));
        match index % 3 {
            0 => expanded_media(&inner, page, TRACKS[1], time, 1.0),
            1 => calculator_page(&inner, page),
            _ => stats_page(&inner, page, time),
        }
    }
    // Which page, as dots down the right side.
    let active = pages.round() as i32 % 3;
    for i in 0..3 {
        painter.circle_filled(
            pos2(card.right() - 8.0, card.center().y - 7.0 + i as f32 * 7.0),
            2.0,
            if i == active {
                TEXT
            } else {
                Color32::from_gray(70)
            },
        );
    }
    // A mouse beside the card, its wheel rolling on each change.
    let mouse =
        Rect::from_center_size(pos2(card.right() + 22.0, card.center().y), vec2(15.0, 22.0));
    painter.rect_stroke(
        mouse,
        cr(7.5),
        Stroke::new(1.3, Color32::from_gray(150)),
        StrokeKind::Middle,
    );
    let roll = (pages.fract() * TAU).sin().abs();
    painter.line_segment(
        [
            pos2(mouse.center().x, mouse.top() + 4.0 + 3.0 * roll),
            pos2(mouse.center().x, mouse.top() + 8.0 + 3.0 * roll),
        ],
        Stroke::new(1.6, fade(LAVENDER, 0.4 + 0.6 * roll)),
    );
    cursor(
        painter,
        pos2(card.right() - 54.0, card.bottom() - 18.0),
        0.0,
    );
}

fn calculator_page(painter: &Painter, page: Rect) {
    painter.text(
        pos2(page.left() + 14.0, page.top() + 26.0),
        Align2::LEFT_CENTER,
        "128 \u{d7} 4",
        FontId::proportional(11.0),
        MUTED,
    );
    painter.text(
        pos2(page.left() + 14.0, page.top() + 50.0),
        Align2::LEFT_CENTER,
        "512",
        bold_font(24.0),
        TEXT,
    );
    let origin = pos2(page.right() - 104.0, page.top() + 14.0);
    for row in 0..3 {
        for col in 0..4 {
            let key = Rect::from_min_size(
                origin + vec2(col as f32 * 21.0, row as f32 * 21.0),
                vec2(18.0, 17.0),
            );
            let fill = if col == 3 { fade(LAVENDER, 0.55) } else { KEY };
            painter.rect_filled(key, cr(5.0), fill);
        }
    }
}

fn stats_page(painter: &Painter, page: Rect, time: f32) {
    for (i, label) in ["CPU", "GPU", "RAM", "NET"].iter().enumerate() {
        let column_x = page.left() + 26.0 + i as f32 * 40.0;
        let value =
            0.3 + 0.5 * (0.5 + 0.5 * (time * (0.9 + i as f32 * 0.35) + i as f32 * 1.3).sin());
        let track = Rect::from_min_size(pos2(column_x - 4.0, page.top() + 14.0), vec2(8.0, 46.0));
        painter.rect_filled(track, cr(4.0), KEY);
        let fill_h = track.height() * value;
        painter.rect_filled(
            Rect::from_min_max(pos2(track.left(), track.bottom() - fill_h), track.max),
            cr(4.0),
            if i == 0 {
                LAVENDER
            } else {
                Color32::from_gray(190)
            },
        );
        painter.text(
            pos2(column_x, page.top() + 72.0),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(10.0),
            MUTED,
        );
    }
}

fn file_icon(painter: &Painter, center: Pos2, scale: f32, alpha: f32) {
    if scale <= 0.01 || alpha <= 0.01 {
        return;
    }
    let (w, h, fold) = (22.0 * scale, 28.0 * scale, 7.0 * scale);
    let min = center - vec2(w, h) / 2.0;
    painter.add(Shape::convex_polygon(
        vec![
            min,
            min + vec2(w - fold, 0.0),
            min + vec2(w, fold),
            min + vec2(w, h),
            min + vec2(0.0, h),
        ],
        fade(Color32::from_rgb(228, 230, 237), alpha),
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            min + vec2(w - fold, 0.0),
            min + vec2(w - fold, fold),
            min + vec2(w, fold),
        ],
        fade(Color32::from_rgb(176, 180, 192), alpha),
        Stroke::NONE,
    ));
    let band = Rect::from_min_size(min + vec2(0.0, h - 10.0 * scale), vec2(w, 10.0 * scale));
    painter.rect_filled(band, CornerRadius::ZERO, fade(TRACKS[1], alpha));
    painter.text(
        band.center(),
        Align2::CENTER_CENTER,
        "PNG",
        bold_font(7.0 * scale),
        fade(Color32::WHITE, alpha),
    );
}

fn drop_scene(painter: &Painter, rect: Rect, t: f32) {
    let x = rect.center().x;
    let top = rect.top() + GAP;
    let open = ease_out_back(seg(t, 1.0, 1.4)) * (1.0 - ease_in_out(seg(t, 4.4, 4.8)));
    let w = lerp(REST_W, 204.0, open);
    let h = lerp(REST_H, 56.0, open);
    let card = Rect::from_min_size(pos2(x - w / 2.0, top), vec2(w, h));
    island(painter, card, lerp(11.0, 18.0, open.clamp(0.0, 1.0)));
    // At rest the island shows the time.
    let rest_alpha = 1.0 - smooth(seg(open, 0.0, 0.3));
    if rest_alpha > 0.01 {
        painter.text(
            card.center(),
            Align2::CENTER_CENTER,
            "9:41",
            bold_font(11.0),
            fade(TEXT, rest_alpha),
        );
    }
    // Formats to drop on, until the file lands; then the result.
    let chips_alpha = smooth(seg(open, 0.6, 1.0)) * (1.0 - smooth(seg(t, 2.3, 2.5)));
    let chip_centers = [-62.0, 0.0, 62.0].map(|dx| pos2(x + dx, card.center().y));
    let landed = t >= 2.0;
    for (i, (label, c)) in ["JPG", "WEBP", "PDF"].iter().zip(chip_centers).enumerate() {
        if chips_alpha <= 0.01 {
            break;
        }
        let chosen = i == 0 && landed;
        let hover = if i == 0 {
            smooth(seg(t, 1.6, 1.9))
        } else {
            0.0
        };
        let chip = Rect::from_center_size(c, vec2(52.0, 26.0));
        let fill = if chosen {
            LAVENDER
        } else {
            mix(KEY, Color32::from_gray(70), hover)
        };
        painter.rect_filled(chip, cr(9.0), fade(fill, chips_alpha));
        painter.text(
            c,
            Align2::CENTER_CENTER,
            *label,
            bold_font(11.0),
            fade(if chosen { PILL } else { TEXT }, chips_alpha),
        );
    }
    let done_alpha = smooth(seg(t, 2.45, 2.7)) * (1.0 - smooth(seg(t, 4.2, 4.45)));
    if done_alpha > 0.01 {
        let ring = pos2(card.left() + 30.0, card.center().y);
        let fill = ease_in_out(seg(t, 2.5, 3.2));
        painter.circle_stroke(ring, 10.0, Stroke::new(2.0, fade(KEY, done_alpha)));
        let steps = 40;
        let arc: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = -FRAC_PI_2 + TAU * fill * i as f32 / steps as f32;
                ring + vec2(a.cos(), a.sin()) * 10.0
            })
            .collect();
        painter.add(Shape::line(
            arc,
            Stroke::new(2.0, fade(LAVENDER, done_alpha)),
        ));
        let tick = smooth(seg(t, 3.2, 3.45));
        if tick > 0.0 {
            painter.add(Shape::line(
                vec![
                    ring + vec2(-4.5, 0.5),
                    ring + vec2(-1.2, 3.8),
                    ring + vec2(4.8, -3.4),
                ],
                Stroke::new(2.0, fade(TEXT, done_alpha * tick)),
            ));
        }
        painter.text(
            pos2(ring.x + 20.0, card.center().y),
            Align2::LEFT_CENTER,
            if t < 3.2 {
                "Converting\u{2026}"
            } else {
                "Saved as JPG"
            },
            FontId::proportional(12.0),
            fade(TEXT, done_alpha),
        );
    }
    // The file rides in on the pointer and drops on JPG.
    let start = pos2(rect.left() + 40.0, rect.bottom() - 30.0);
    let near = pos2(x - 20.0, top + 52.0);
    let carry = ease_in_out(seg(t, 0.3, 1.4));
    let control = pos2(rect.left() + 60.0, top + 40.0);
    let along = {
        let s = 1.0 - carry;
        pos2(
            s * s * start.x + 2.0 * s * carry * control.x + carry * carry * near.x,
            s * s * start.y + 2.0 * s * carry * control.y + carry * carry * near.y,
        )
    };
    let file_at = along.lerp(chip_centers[0], ease_in_out(seg(t, 1.5, 1.9)));
    let sink = seg(t, 2.0, 2.3);
    let appear = smooth(seg(t, 0.0, 0.25));
    file_icon(painter, file_at, 1.0 - 0.6 * sink, appear * (1.0 - sink));
    let leave = ease_in_out(seg(t, 2.5, 3.2));
    let tip =
        (file_at + vec2(5.0, 6.0)).lerp(pos2(rect.right() - 30.0, rect.bottom() - 26.0), leave);
    let pressed = if t < 2.0 {
        appear
    } else {
        1.0 - seg(t, 2.0, 2.15)
    };
    cursor(painter, tip, pressed);
}

// ---------------------------------------------------------------------------
// The logo, drawing itself as the island does at startup.

/// A point `distance` along the outline of `rect` (corner `r`), starting at
/// the middle of its right end and going clockwise on screen.
fn outline_point(rect: Rect, r: f32, distance: f32) -> Pos2 {
    let (w, h) = (rect.width(), rect.height());
    let r = r.clamp(0.0, w.min(h) * 0.5);
    let (straight_x, straight_y) = (w - 2.0 * r, h - 2.0 * r);
    let quarter = FRAC_PI_2 * r;
    let mut d = distance.rem_euclid(outline_length(rect, r));
    let o = rect.min;
    let arc = |cx: f32, cy: f32, from: f32, d: f32| {
        let angle = from + if r > 0.0 { d / r } else { 0.0 };
        o + vec2(cx + r * angle.cos(), cy + r * angle.sin())
    };
    let half_y = straight_y * 0.5;
    if d <= half_y {
        return o + vec2(w, h * 0.5 + d);
    }
    d -= half_y;
    if d <= quarter {
        return arc(w - r, h - r, 0.0, d);
    }
    d -= quarter;
    if d <= straight_x {
        return o + vec2(w - r - d, h);
    }
    d -= straight_x;
    if d <= quarter {
        return arc(r, h - r, FRAC_PI_2, d);
    }
    d -= quarter;
    if d <= straight_y {
        return o + vec2(0.0, h - r - d);
    }
    d -= straight_y;
    if d <= quarter {
        return arc(r, r, PI, d);
    }
    d -= quarter;
    if d <= straight_x {
        return o + vec2(r + d, 0.0);
    }
    d -= straight_x;
    if d <= quarter {
        return arc(w - r, r, PI + FRAC_PI_2, d);
    }
    d -= quarter;
    o + vec2(w, r + d)
}

fn outline_length(rect: Rect, r: f32) -> f32 {
    let (w, h) = (rect.width(), rect.height());
    let r = r.clamp(0.0, w.min(h) * 0.5);
    2.0 * (w - 2.0 * r) + 2.0 * (h - 2.0 * r) + 2.0 * PI * r
}

/// A point along a faint orbit line: `upper` ends at the dot.
fn orbit_point(rect: Rect, u: f32, upper: bool) -> Pos2 {
    let (start, control, end) = if upper {
        ((0.07, 0.76), (0.42, 0.14), DOT_AT)
    } else {
        ((0.06, 0.90), (0.50, 0.34), (0.95, 0.72))
    };
    let s = 1.0 - u;
    let fx = s * s * start.0 + 2.0 * s * u * control.0 + u * u * end.0;
    let fy = s * s * start.1 + 2.0 * s * u * control.1 + u * u * end.1;
    rect.min + vec2(fx * rect.width(), fy * rect.height())
}

/// The logo in `rect`, `t` seconds into drawing itself (complete from about
/// 1.8 s; `time` keeps the dot breathing afterwards).
pub fn logo(painter: &Painter, rect: Rect, t: f32, time: f32) {
    draw_logo(painter, rect, t, time, true);
}

fn draw_logo(painter: &Painter, rect: Rect, t: f32, time: f32, halo: bool) {
    let corner = rect.height() * 0.5;
    let shell = smooth(seg(t, 0.0, 0.45));
    // A soft lavender glow behind it once the dot lands.
    let land = seg(t, 1.30, 1.80);
    let glow = if halo {
        smooth(land) * (0.8 + 0.2 * (time * 1.6).sin())
    } else {
        0.0
    };
    // Many faint layers, so the glow falls off without visible rings.
    for layer in (1..=16).rev() {
        let grow = layer as f32 * 3.2;
        painter.rect_filled(
            rect.expand(grow),
            cr(corner + grow),
            fade(LAVENDER, 0.016 * glow),
        );
    }
    painter.rect_filled(rect, cr(corner), fade(PILL, shell));
    // Orbit lines draw in from the left.
    let detail = ease_in_out(seg(t, 1.05, 1.60));
    for (upper, alpha) in [(true, 0.16), (false, 0.10)] {
        let end = if upper { detail } else { detail * 0.92 };
        if end > 0.0 {
            let points: Vec<Pos2> = (0..=48)
                .map(|i| orbit_point(rect, end * i as f32 / 48.0, upper))
                .collect();
            painter.add(Shape::line(
                points,
                Stroke::new(1.2, fade(Color32::WHITE, alpha)),
            ));
        }
    }
    // The rim traces itself from the dot's end, both ways round.
    let rim = ease_in_out(seg(t, 0.30, 1.05));
    if rim > 0.0 {
        let inset = rect.shrink(1.0);
        let length = outline_length(inset, corner - 1.0);
        let reach = rim * length * 0.5;
        let steps = (reach / 2.0).ceil().max(2.0) as usize;
        for sign in [1.0, -1.0] {
            let points: Vec<Pos2> = (0..=steps)
                .map(|i| outline_point(inset, corner - 1.0, sign * reach * i as f32 / steps as f32))
                .collect();
            painter.add(Shape::line(points, Stroke::new(1.6, fade(SILVER, 0.85))));
        }
    }
    // The dot rides in along the upper orbit and lands with a pulse.
    let travel = seg(t, 0.62, 1.30);
    if travel > 0.0 {
        let at = orbit_point(rect, ease_in_out(travel), true);
        let size = rect.height() * 0.085 * lerp(0.55, 1.0, ease_out_back(land));
        let breathe = if land >= 1.0 {
            0.85 + 0.15 * (time * 1.6).sin()
        } else {
            1.0
        };
        painter.circle_filled(at, size * 2.6, fade(LAVENDER, 0.10 * breathe));
        painter.circle_filled(at, size * 1.7, fade(LAVENDER, 0.22 * breathe));
        painter.circle_filled(at, size, LAVENDER);
        if land > 0.0 && land < 1.0 {
            painter.circle_stroke(
                at,
                size + rect.height() * 0.4 * ease_out(land),
                Stroke::new(1.4, fade(LAVENDER, 0.7 * (1.0 - land))),
            );
        }
    }
}

/// A finished logo, small: for headers.
pub fn mark(painter: &Painter, center: Pos2, width: f32) {
    let rect = Rect::from_center_size(center, vec2(width, width * 0.38));
    draw_logo(painter, rect, 3.0, 0.0, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outline_starts_at_the_right_middle_and_closes() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 80.0));
        let start = outline_point(rect, 40.0, 0.0);
        assert!((start.x - 200.0).abs() < 0.01 && (start.y - 40.0).abs() < 0.01);
        let end = outline_point(rect, 40.0, outline_length(rect, 40.0) - 0.001);
        assert!(start.distance(end) < 0.1);
    }

    #[test]
    fn the_upper_orbit_ends_at_the_dot() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 80.0));
        let end = orbit_point(rect, 1.0, true);
        assert!((end.x - (10.0 + 170.0)).abs() < 0.01 && (end.y - 60.0).abs() < 0.01);
    }

    #[test]
    fn eases_stay_in_range_at_their_ends() {
        for f in [smooth, ease_in_out, ease_out, ease_out_back] {
            assert!(f(0.0).abs() < 1e-5);
            assert!((f(1.0) - 1.0).abs() < 1e-5);
        }
        assert_eq!(seg(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(seg(2.0, 0.0, 1.0), 1.0);
    }

    #[test]
    fn a_released_pull_settles_home() {
        assert!(settle(30.0, 1.5).abs() < 0.1);
        assert_eq!(settle(30.0, 0.0), 30.0);
    }
}
