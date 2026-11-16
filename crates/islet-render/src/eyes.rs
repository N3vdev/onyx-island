//! The island's eyes: a resting page with two round eyes that blink now and
//! then and react to being pulled.
//!
//! While held, they look the way they are pulled. Lifted away from the edge
//! they turn happy (^ ^); pushed into it they squish and strain; pulled hard
//! sideways they squeeze shut (> <) and tremble. Let go, and each gesture
//! ends in its own reaction: a happy hop, a wide-eyed pop, or a dizzy shake.
//!
//! Now and then, instead of a blink, one eye winks: it closes into a little
//! smile while the other squints along.
//!
//! Between blinks and reactions nothing moves, so the render thread sleeps
//! until the next blink (see `Renderer::wake_in`) instead of drawing every
//! refresh.
use super::*;
use std::f32::consts::{PI, TAU};

const EYE_COLOR: [u8; 3] = [244, 245, 250];
/// The eyes' colour while the night light is on: a warm yellow.
const NIGHT_EYE_COLOR: [u8; 3] = [255, 214, 92];
/// Eye size and the distance from the middle to each eye, in eye units.
const EYE: f32 = 12.0;
/// The sad eyes' one tear: light blue, from this many seconds after they
/// turn sad, running for this long.
const TEAR_COLOR: [u8; 3] = [150, 205, 255];
const TEAR_AT: f32 = 1.0;
const TEAR_RUN: f32 = 1.6;
/// How long the sad face keeps moving after it starts.
pub(crate) const SULK_LENGTH: f64 = (TEAR_AT + TEAR_RUN) as f64 + 0.1;
const EYE_SPACING: f32 = 14.0;
/// One blink: the lids close quickly, rest a moment, and open more slowly.
const CLOSE: f64 = 0.085;
const SHUT: f64 = 0.035;
const OPEN: f64 = 0.13;
const BLINK: f64 = CLOSE + SHUT + OPEN;
/// The pause inside a double blink.
const BETWEEN: f64 = 0.09;
/// One wink: the eye closes into a smile, holds it, and opens again.
const WINK_CLOSE: f64 = 0.14;
const WINK_HOLD: f64 = 0.3;
const WINK_OPEN: f64 = 0.22;
const WINK: f64 = WINK_CLOSE + WINK_HOLD + WINK_OPEN;
/// How fast straining eyes tremble, per second.
const TREMBLE_HZ: f32 = 24.0;

/// What the eyes do after a pull is let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reaction {
    /// Lifted away from the edge: two happy hops, smiling.
    Hop,
    /// Pushed into the edge: a wide-eyed pop as the pressure goes.
    Pop,
    /// Pulled sideways: a dizzy shake as they settle.
    Shake,
    /// The night light came on: they look up at the dusk, grow heavy,
    /// yawn and stay drowsy.
    Sleepy,
    /// The night light went off: drowsy yellow eyes flutter, open wide as
    /// the yellow fades to white, and give a happy hop.
    Wake,
}

impl Reaction {
    /// The reaction a pull earns, from where it was when let go. `lift` is
    /// how far it was pulled away from the edge (negative: into it).
    pub fn for_pull(x: f32, lift: f32) -> Option<Self> {
        if lift > 12.0 && lift >= x.abs() * 0.8 {
            Some(Self::Hop)
        } else if x.abs() > 18.0 {
            Some(Self::Shake)
        } else if lift < -1.0 {
            Some(Self::Pop)
        } else {
            None
        }
    }

    pub fn length(self) -> f64 {
        match self {
            Self::Hop => 1.15,
            Self::Pop => 0.55,
            Self::Shake => 0.7,
            Self::Sleepy => 4.4,
            Self::Wake => 3.2,
        }
    }
}

/// What the eyes do this frame. Offsets are in eye units (about a logical
/// pixel on the resting pill).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EyePose {
    /// -1..1: where they look (right and down are positive).
    pub look_x: f32,
    pub look_y: f32,
    /// 1 open, 0 shut (blinking).
    pub open: f32,
    /// 0..1: squeezed shut (> <) by a hard sideways pull.
    pub squeeze: f32,
    /// 0..1: a little wide-eyed.
    pub surprise: f32,
    /// 0..1: happy (^ ^).
    pub happy: f32,
    /// 0..1: squished flat, straining against the edge.
    pub strain: f32,
    /// -1..1: winking, with the left eye (negative) or the right (positive).
    pub wink: f32,
    /// Bounces, shakes and trembles.
    pub shift_x: f32,
    pub shift_y: f32,
    /// 0..1: tinted yellow, for the night light.
    pub warm: f32,
    /// 0..1: sad, the top of each eye cut by a lid sloping down toward the
    /// outside (/ \), like worried brows. Pushed off to the screen side,
    /// they sulk.
    pub sad: f32,
    /// 0..1: how far a single tear has run down (0: none).
    pub tear: f32,
}

impl EyePose {
    pub const REST: Self = Self {
        look_x: 0.0,
        look_y: 0.0,
        open: 1.0,
        squeeze: 0.0,
        surprise: 0.0,
        happy: 0.0,
        strain: 0.0,
        wink: 0.0,
        shift_x: 0.0,
        shift_y: 0.0,
        warm: 0.0,
        sad: 0.0,
        tear: 0.0,
    };

    /// The pose while pulled `x`, `y` logical pixels, `lift` of it away from
    /// the edge, with lids `open`. `now` drives the trembling.
    pub fn new(x: f32, y: f32, lift: f32, open: f32, now: f64) -> Self {
        let squeeze = smoothstep(48.0, 96.0, x.abs());
        let happy = smoothstep(8.0, 40.0, lift);
        // Into the edge the island barely moves, so a few pixels is effort.
        let strain = smoothstep(0.5, 4.0, -lift);
        let tremble = (squeeze * 0.8).max(strain) * (now as f32 * TAU * TREMBLE_HZ).sin();
        let mut pose = Self {
            look_x: (x / (x.abs() + 34.0) * 1.2).clamp(-1.0, 1.0),
            look_y: (y / (y.abs() + 22.0) * 1.2).clamp(-1.0, 1.0),
            open,
            squeeze,
            surprise: smoothstep(3.0, 26.0, x.hypot(y)) * (1.0 - squeeze) * (1.0 - happy) * (1.0 - strain),
            happy,
            strain,
            wink: 0.0,
            shift_x: tremble * 0.7,
            shift_y: 0.0,
            warm: 0.0,
            sad: 0.0,
            tear: 0.0,
        };
        pose.settle_lids();
        pose
    }

    /// Adds a wink (see `Blinker::wink`). The other eye squints a little, as
    /// a cheek would push it, and both lean a touch toward the wink.
    pub fn wink(mut self, wink: f32) -> Self {
        if wink == 0.0 {
            return self;
        }
        let amount = wink.abs();
        self.wink = wink;
        self.open = self.open.min(1.0 - 0.22 * amount);
        self.look_x = (self.look_x + 0.12 * wink).clamp(-1.0, 1.0);
        self.shift_y -= 0.8 * amount;
        self.settle_lids();
        self
    }

    /// Adds a let-go reaction, `t` seconds in.
    pub fn react(mut self, reaction: Reaction, t: f64) -> Self {
        let t = t as f32;
        let length = reaction.length() as f32;
        if !(0.0..length).contains(&t) {
            return self;
        }
        match reaction {
            Reaction::Hop => {
                // Two hops, the second smaller, smiling until they land.
                let hop = (PI * t / 0.3).sin().abs() * (-2.2 * t).exp() * (1.0 - smoothstep(0.8, length, t));
                self.shift_y -= 5.5 * hop;
                self.happy = self.happy.max(1.0 - smoothstep(0.75, length, t));
            }
            Reaction::Pop => {
                self.surprise = self.surprise.max((1.0 - t / length).powi(2) * smoothstep(0.0, 0.06, t));
            }
            Reaction::Sleepy => {
                // A glance up at the dusk, with a little surprise.
                let up = smoothstep(0.0, 0.35, t) * (1.0 - smoothstep(0.55, 1.2, t));
                self.look_y = lerp(self.look_y, -0.85, up);
                self.surprise = self.surprise.max(0.5 * up);
                // Then heavy lids, looking a little down.
                let heavy = smoothstep(0.7, 1.5, t);
                self.look_y = lerp(self.look_y, 0.35, heavy);
                let mut lids = 1.0 - 0.55 * heavy;
                // A slow yawn: the lids sink almost shut and lift again.
                let yawn = smoothstep(1.6, 2.2, t) * (1.0 - smoothstep(2.5, 3.2, t));
                lids = lids.min(1.0 - 0.9 * yawn);
                self.shift_y += 1.2 * yawn;
                // Settling in, a touch heavier still, swaying gently.
                lids -= 0.12 * smoothstep(3.2, 3.8, t);
                self.shift_x += 0.8 * (TAU * 0.6 * t).sin() * heavy;
                self.open = self.open.min(lids.max(0.08));
            }
            Reaction::Wake => {
                // Still drowsy, then two quick flutters...
                let drowsy = 1.0 - smoothstep(0.5, 0.75, t);
                let flutter = if (0.75..1.35).contains(&t) {
                    (0.5 - 0.5 * (TAU * (t - 0.75) / 0.3).cos()) * 0.85
                } else {
                    0.0
                };
                self.open = self.open.min(1.0 - 0.7 * drowsy - flutter);
                // ...wide awake as the yellow goes...
                let wide = smoothstep(1.3, 1.55, t) * (1.0 - smoothstep(1.9, 2.15, t));
                self.surprise = self.surprise.max(wide);
                self.look_y = lerp(self.look_y, -0.4, wide);
                self.warm = self.warm.min(1.0 - smoothstep(1.0, 2.0, t));
                // ...and a happy hop to start the day.
                let smile = smoothstep(2.0, 2.2, t) * (1.0 - smoothstep(2.8, 3.2, t));
                self.happy = self.happy.max(smile);
                self.shift_y -= 3.5 * (PI * ((t - 2.0) / 0.45).clamp(0.0, 1.0)).sin() * smile;
            }
            Reaction::Shake => {
                self.shift_x += 3.2 * (TAU * 6.5 * t).sin() * (-5.5 * t).exp();
                // A slightly dazed squint as they settle.
                self.open = self.open.min(1.0 - 0.35 * smoothstep(0.25, 0.4, t) * (1.0 - smoothstep(0.5, length, t)));
            }
        }
        self.settle_lids();
        self
    }

    /// Sulking at the screen side, `t` seconds after arriving there: a
    /// moment of their usual face, then the brows tilt up in the middle,
    /// the eyes go big and droop with a little sigh, and one small tear
    /// rolls down. Sad, but cute.
    pub fn sulk(mut self, t: f64) -> Self {
        let t = t as f32;
        self.sad = self.sad.max(smoothstep(0.3, 0.7, t));
        // Big, glossy puppy eyes, looking down.
        self.surprise = self.surprise.max(0.45 * self.sad);
        self.look_y = lerp(self.look_y, 0.5, self.sad);
        // A sigh: a slow sink and back, settling a touch low.
        let sigh = (PI * ((t - 0.35) / 0.9).clamp(0.0, 1.0)).sin();
        self.shift_y += 1.0 * self.sad + 1.3 * sigh;
        // Then one tear.
        if (TEAR_AT..TEAR_AT + TEAR_RUN).contains(&t) {
            self.tear = (t - TEAR_AT) / TEAR_RUN;
        }
        self
    }

    /// Happy or squeezed eyes are already closed in their own way, so they
    /// do not blink on top of it.
    fn settle_lids(&mut self) {
        let shut = self.squeeze.max(self.happy);
        self.open += (1.0 - self.open) * shut;
    }

    /// Settled-frame key at about half-pixel steps, so a frame that would
    /// look the same as the last one is skipped.
    pub fn key(self) -> [i16; 13] {
        [
            self.sad,
            self.tear,
            self.warm,
            self.wink,
            self.look_x,
            self.look_y,
            self.open,
            self.squeeze,
            self.surprise,
            self.happy,
            self.strain,
            self.shift_x / 8.0,
            self.shift_y / 8.0,
        ]
        .map(|value| (value * 40.0).round() as i16)
    }
}

/// What the next blink is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Blink {
    Single,
    Double,
    /// One eye: -1 the left, 1 the right.
    Wink(f32),
}

/// When the eyes next blink. Blinks come every two to six seconds, with the
/// odd double blink or wink, like someone idly watching.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Blinker {
    start: f64,
    kind: Blink,
    seed: u64,
}

impl Blinker {
    pub fn new(now: f64) -> Self {
        Self { start: now + 1.0, kind: Blink::Single, seed: now.to_bits() | 1 }
    }

    /// The eyes just came on screen: blink soon, as if waking up.
    pub fn greet(&mut self, now: f64) {
        self.start = now + 0.7;
        self.kind = Blink::Double;
    }

    /// One blink at `at`, to round off a reaction.
    pub fn blink_at(&mut self, at: f64) {
        self.start = at;
        self.kind = Blink::Single;
    }

    fn random(&mut self) -> f64 {
        // xorshift64: plenty for spacing blinks.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn length(&self) -> f64 {
        match self.kind {
            Blink::Single => BLINK,
            Blink::Double => BLINK * 2.0 + BETWEEN,
            Blink::Wink(_) => WINK,
        }
    }

    /// A blink is under way or overdue: frames are needed until `advance`
    /// schedules the next one.
    pub fn due(&self, now: f64) -> bool {
        now >= self.start
    }

    /// Seconds until the next blink begins.
    pub fn wake_in(&self, now: f64) -> f64 {
        (self.start - now).max(0.0)
    }

    /// Moves on to the next blink once this one has finished.
    pub fn advance(&mut self, now: f64) {
        let end = self.start + self.length();
        if now >= end {
            self.start = now.max(end) + 2.2 + self.random() * 3.8;
            // About one in seven is a wink (every twenty seconds or so),
            // one in five a double blink.
            let pick = self.random();
            self.kind = if pick < 0.14 {
                Blink::Wink(if self.random() < 0.5 { -1.0 } else { 1.0 })
            } else if pick < 0.34 {
                Blink::Double
            } else {
                Blink::Single
            };
        }
    }

    /// How open the lids are at `now`, 0..1.
    pub fn open(&self, now: f64) -> f32 {
        let mut t = now - self.start;
        if t < 0.0 || t > self.length() || matches!(self.kind, Blink::Wink(_)) {
            return 1.0;
        }
        if self.kind == Blink::Double && t > BLINK {
            if t < BLINK + BETWEEN {
                return 1.0;
            }
            t -= BLINK + BETWEEN;
        }
        lids(t)
    }
}

impl Blinker {
    /// How far into a wink the eyes are at `now`: 0 none, toward -1 the left
    /// eye or 1 the right. Eased in and out, so it never snaps.
    pub fn wink(&self, now: f64) -> f32 {
        let Blink::Wink(side) = self.kind else { return 0.0 };
        let t = now - self.start;
        let amount = if !(0.0..WINK).contains(&t) {
            0.0
        } else if t < WINK_CLOSE {
            0.5 - 0.5 * (std::f64::consts::PI * t / WINK_CLOSE).cos()
        } else if t < WINK_CLOSE + WINK_HOLD {
            1.0
        } else {
            0.5 + 0.5 * (std::f64::consts::PI * (t - WINK_CLOSE - WINK_HOLD) / WINK_OPEN).cos()
        };
        side * amount as f32
    }
}

/// One blink, as how open the lids are `t` seconds in. Eased at both ends
/// so it never snaps.
fn lids(t: f64) -> f32 {
    let shut = if t < CLOSE {
        0.5 - 0.5 * (std::f64::consts::PI * t / CLOSE).cos()
    } else if t < CLOSE + SHUT {
        1.0
    } else if t < BLINK {
        0.5 + 0.5 * (std::f64::consts::PI * (t - CLOSE - SHUT) / OPEN).cos()
    } else {
        0.0
    };
    (1.0 - shut) as f32
}

impl Gfx {
    /// Two eyes centred in a `width` x `height` pill.
    ///
    /// Every expression is one continuous shape: to squint (> <) or smile
    /// (^ ^), each round eye first narrows into a short rounded line, then
    /// that line folds into a chevron or bends into an arc. Nothing fades
    /// between two drawings, so a slow pull morphs smoothly both ways.
    pub(super) unsafe fn draw_eyes(&mut self, width: f32, height: f32, alpha: f32) -> RenderResult<()> {
        let pose = self.eyes;
        let unit = eye_unit(width, height);
        let (cx, cy) = (width * 0.5, height * 0.5);
        let grow = 1.0 + 0.14 * pose.surprise;
        // Round eyes: as wide as they are tall, unless squished.
        let eye_w = EYE * unit * grow * (1.0 + 0.18 * pose.strain);
        let eye_h = EYE * unit * grow * (1.0 - 0.45 * pose.strain);
        let look_x = (pose.look_x * 6.0 + pose.shift_x) * unit;
        let look_y = (pose.look_y * 3.0 + pose.shift_y) * unit;
        let tint = [0, 1, 2].map(|c| {
            lerp(f32::from(EYE_COLOR[c]), f32::from(NIGHT_EYE_COLOR[c]), pose.warm).round() as u8
        });
        let ink = color(tint, 0.96 * alpha);
        let stroke = 2.6 * unit;
        for side in [-1.0f32, 1.0] {
            // Whichever expression is stronger shapes the fold. A winking
            // eye folds into a smile on its own, the same way.
            let wink = if pose.wink * side > 0.0 { pose.wink.abs() } else { 0.0 };
            let smiling = pose.happy.max(wink) > pose.squeeze;
            let fold = pose.happy.max(pose.squeeze).max(wink);
            let narrow = (fold / 0.5).clamp(0.0, 1.0);
            let bend = ((fold - 0.5) / 0.5).clamp(0.0, 1.0);
            let half = if smiling { 5.6 } else { 3.8 } * unit;
            // The eye on the side they look toward comes a touch closer.
            let near = 1.0 + 0.08 * pose.look_x * side;
            let at_x = cx + side * EYE_SPACING * unit + look_x;
            let at_y = cy + look_y;
            if narrow < 1.0 {
                let w = eye_w * near;
                let full = eye_h * near;
                // Closing lids leave a short rounded line, a little low, as
                // if the upper lid came down.
                let lidded = (full * pose.open).max(w * 0.24);
                let lid_y = at_y + (full - lidded) * 0.22;
                let eye_w = w;
                let w = lerp(w, half * 2.0 + stroke, narrow);
                let h = lerp(lidded, stroke, narrow);
                let mid = lerp(lid_y, at_y, narrow);
                let sad = pose.sad * (1.0 - narrow);
                if sad > 0.01 {
                    // A lid sloping down toward the outside cuts the top of
                    // the eye: untouched at the inner edge, some way down
                    // at the outer.
                    let top = mid - h * 0.5;
                    let inner = (at_x - side * eye_w * 0.5, top - 0.5 * unit);
                    let outer = (at_x + side * eye_w * 0.5, top + h * 0.42 * sad);
                    let outline = round_rect_outline(at_x - w * 0.5, mid - h * 0.5, w, h, w.min(h) * 0.5);
                    self.fill_polygon(&below_line(&outline, inner, outer), ink)?;
                } else {
                    self.fill_round_rect(at_x - w * 0.5, mid - h * 0.5, w, h, w.min(h) * 0.5, ink)?;
                }
            } else if smiling {
                // The line bows up into a smile, staying centred.
                let sag = 4.0 * unit * bend;
                let points: Vec<Vector2> = (0..=10)
                    .map(|step| {
                        let u = step as f32 / 5.0 - 1.0;
                        Vector2 { X: at_x + u * half, Y: at_y + sag * 0.5 - sag * (1.0 - u * u) }
                    })
                    .collect();
                self.stroke_polyline(&points, stroke, ink)?;
            } else {
                // The line folds at its middle into a chevron pointing inward.
                let tip = -side * half;
                let spread = 4.6 * unit * bend;
                self.stroke_polyline(
                    &[
                        Vector2 { X: at_x - tip, Y: at_y - spread },
                        Vector2 { X: at_x + tip, Y: at_y },
                        Vector2 { X: at_x - tip, Y: at_y + spread },
                    ],
                    stroke,
                    ink,
                )?;
            }
        }
        if pose.tear > 0.0 && pose.tear < 1.0 {
            // A small drop from the outer corner of the right eye, sliding
            // down and fading.
            let t = pose.tear;
            let fade = smoothstep(0.0, 0.12, t) * (1.0 - smoothstep(0.7, 1.0, t));
            let r = 1.6 * unit;
            let x = cx + EYE_SPACING * unit + look_x + eye_w * 0.4;
            // Starts at the corner of the eye and stays inside the pill.
            let start = cy + look_y + eye_h * 0.42;
            let end = (cy + height * 0.5 - r * 1.6).max(start);
            let y = lerp(start, end, ease_in_out(t));
            let drop = color(TEAR_COLOR, 0.9 * fade * alpha);
            self.fill_circle(x, y, r, drop)?;
            self.fill_polygon(
                &[
                    Vector2 { X: x - r * 0.92, Y: y - r * 0.4 },
                    Vector2 { X: x, Y: y - r * 2.4 },
                    Vector2 { X: x + r * 0.92, Y: y - r * 0.4 },
                ],
                drop,
            )?;
        }
        Ok(())
    }

    /// The eyes becoming the dictation bars (`amount` 1) or the bars
    /// becoming the eyes (0). Each eye splits into three bars, which narrow,
    /// stretch to the voice and slide apart, taking the bars' colour.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_eyes_to_bars(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        amount: f32,
        dictation: Option<&DictationCard>,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        use voice::{BARS, BAR_PITCH, BAR_WIDTH};
        let unit = eye_unit(width, height);
        let (cx, cy) = (width * 0.5, height * 0.5);
        let targets = dictation.map_or([0.0; BARS], |card| voice::card_targets(card, now));
        let heights = self.voice_bars.step(now, targets);
        let left = (width - ((BARS - 1) as f32 * BAR_PITCH + BAR_WIDTH)) * 0.5;
        let max_h = (height - 12.0).max(6.0);
        let eye = EYE * unit;
        for (index, level) in heights.iter().enumerate() {
            // The middle bars leave first and the outer ones follow.
            let delay = [0.14, 0.07, 0.0, 0.0, 0.07, 0.14][index];
            let t = ease_in_out(((amount - delay) / (1.0 - 0.14)).clamp(0.0, 1.0));
            let side = if index < BARS / 2 { -1.0 } else { 1.0 };
            let eye_x = cx + side * EYE_SPACING * unit;
            let bar_x = left + index as f32 * BAR_PITCH + BAR_WIDTH * 0.5;
            let bar_h = 3.0 + level.max(0.0) * (max_h - 3.0);
            let bar_alpha = dictation.map_or(0.85, |card| voice::bar_alpha(card, index));
            let w = lerp(eye, BAR_WIDTH, t);
            let h = lerp(eye, bar_h, t);
            let x = lerp(eye_x, bar_x, t);
            let tint = [0, 1, 2].map(|c| lerp(f32::from(EYE_COLOR[c]), f32::from(accent[c]), t).round() as u8);
            // Three bars drawn over each other are one eye: share the ink.
            let share = lerp(0.96, bar_alpha, t);
            self.fill_round_rect(x - w * 0.5, cy - h * 0.5, w, h, w.min(h) * 0.5, color(tint, share * alpha))?;
        }
        Ok(())
    }
}

/// The outline of a rounded rectangle as points, clockwise from the top
/// left, each corner an arc of `radius`.
fn round_rect_outline(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Vec<Vector2> {
    let radius = radius.min(width * 0.5).min(height * 0.5).max(0.0);
    let corners = [
        (x + width - radius, y + radius, -0.5 * PI),
        (x + width - radius, y + height - radius, 0.0),
        (x + radius, y + height - radius, 0.5 * PI),
        (x + radius, y + radius, PI),
    ];
    let mut points = Vec::with_capacity(4 * 9);
    for (cx, cy, start) in corners {
        for step in 0..=8 {
            let angle = start + 0.5 * PI * step as f32 / 8.0;
            points.push(Vector2 { X: cx + radius * angle.cos(), Y: cy + radius * angle.sin() });
        }
    }
    points
}

/// The part of the convex polygon `points` on or below the line through
/// `a` and `b` (larger y is lower).
fn below_line(points: &[Vector2], a: (f32, f32), b: (f32, f32)) -> Vec<Vector2> {
    let slope = if (b.0 - a.0).abs() < 1e-4 { 0.0 } else { (b.1 - a.1) / (b.0 - a.0) };
    let depth = |p: &Vector2| p.Y - (a.1 + (p.X - a.0) * slope);
    let mut kept = Vec::with_capacity(points.len() + 2);
    for (index, point) in points.iter().enumerate() {
        let next = &points[(index + 1) % points.len()];
        let (here, there) = (depth(point), depth(next));
        if here >= 0.0 {
            kept.push(*point);
        }
        if (here >= 0.0) != (there >= 0.0) {
            let t = here / (here - there);
            kept.push(Vector2 { X: point.X + (next.X - point.X) * t, Y: point.Y + (next.Y - point.Y) * t });
        }
    }
    kept
}

fn eye_unit(width: f32, height: f32) -> f32 {
    (height / 32.0).min(width / 80.0).clamp(0.6, 2.4)
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

fn ease_in_out(x: f32) -> f32 {
    if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) * 0.5 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blink_closes_and_opens_smoothly() {
        assert_eq!(lids(0.0), 1.0);
        assert!(lids(CLOSE * 0.5) < 1.0 && lids(CLOSE * 0.5) > 0.0);
        assert_eq!(lids(CLOSE + SHUT * 0.5), 0.0);
        assert!((lids(BLINK) - 1.0).abs() < 1e-6);
        // No step between frames at 120 Hz is larger than half the travel.
        let mut last = 1.0;
        for frame in 0..30 {
            let open = lids(f64::from(frame) / 120.0);
            assert!((open - last).abs() <= 0.5, "frame {frame}: {last} -> {open}");
            last = open;
        }
    }

    #[test]
    fn blinks_come_every_few_seconds_and_the_next_waits_its_turn() {
        let mut blinker = Blinker::new(10.0);
        assert!(!blinker.due(10.5));
        assert!((blinker.wake_in(10.0) - 1.0).abs() < 1e-9);
        let start = blinker.start;
        assert!(blinker.open(start + CLOSE + 0.01) < 0.1);
        blinker.advance(start + blinker.length() + 0.01);
        let gap = blinker.start - start;
        assert!((2.2..=6.6).contains(&gap), "gap {gap}");
        assert!(!blinker.due(start + 1.0));
    }

    #[test]
    fn a_double_blink_reopens_between_its_two_closes() {
        let mut blinker = Blinker::new(0.0);
        blinker.greet(0.0);
        let start = blinker.start;
        assert!(blinker.open(start + CLOSE + 0.01) < 0.1);
        assert_eq!(blinker.open(start + BLINK + BETWEEN * 0.5), 1.0);
        assert!(blinker.open(start + BLINK + BETWEEN + CLOSE + 0.01) < 0.1);
        assert_eq!(blinker.open(start + blinker.length() + 0.01), 1.0);
    }

    #[test]
    fn a_wink_closes_one_eye_into_a_smile_and_opens_it_again() {
        let mut blinker = Blinker::new(0.0);
        blinker.kind = Blink::Wink(1.0);
        let start = blinker.start;
        assert_eq!(blinker.wink(start - 0.01), 0.0);
        assert_eq!(blinker.wink(start + WINK_CLOSE + 0.01), 1.0);
        assert_eq!(blinker.open(start + WINK_CLOSE + 0.01), 1.0, "the other eye stays open");
        assert!(blinker.wink(start + WINK - 0.001) < 0.01);
        assert_eq!(blinker.wink(start + WINK + 0.01), 0.0);
        // Smooth at 120 Hz.
        let mut last = 0.0;
        for frame in 0..90 {
            let wink = blinker.wink(start + f64::from(frame) / 120.0);
            assert!((wink - last).abs() <= 0.25, "frame {frame}: {last} -> {wink}");
            last = wink;
        }
        let pose = EyePose::REST.wink(-1.0);
        assert_eq!(pose.wink, -1.0);
        assert!(pose.open < 1.0 && pose.open > 0.7, "the open eye squints along");
        assert_eq!(EyePose::REST.wink(0.0), EyePose::REST);
    }

    #[test]
    fn the_night_light_makes_the_eyes_look_up_then_grow_heavy_and_yawn() {
        let at = |t: f64| EyePose::REST.react(Reaction::Sleepy, t);
        assert!(at(0.4).look_y < -0.5, "they look up first");
        assert!(at(1.5).open < 0.6 && at(1.5).look_y > 0.2, "then grow heavy");
        assert!(at(2.4).open < 0.2, "and yawn");
        let settled = at(4.0);
        assert!(settled.open > 0.2 && settled.open < 0.5, "drowsy, not asleep: {}", settled.open);
        assert_eq!(at(5.0), EyePose::REST, "the reaction ends");
    }

    #[test]
    fn the_morning_eyes_flutter_awake_lose_the_amber_and_smile() {
        let warm = EyePose { warm: 1.0, ..EyePose::REST };
        let at = |t: f64| warm.react(Reaction::Wake, t);
        assert!(at(0.2).open < 0.4 && at(0.2).warm == 1.0, "drowsy and yellow at first");
        assert!(at(0.9).open < 0.5, "a flutter");
        assert!(at(1.7).surprise > 0.8 && at(1.7).warm < 0.5, "wide awake as the yellow goes");
        assert!(at(2.4).happy > 0.9 && at(2.4).warm == 0.0, "a happy hop, all white");
    }

    #[test]
    fn winks_come_now_and_then() {
        let mut blinker = Blinker::new(1.234);
        let mut winks = 0;
        for _ in 0..700 {
            let now = blinker.start + blinker.length() + 0.01;
            blinker.advance(now);
            winks += usize::from(matches!(blinker.kind, Blink::Wink(_)));
        }
        assert!((50..=160).contains(&winks), "{winks} winks in 700 blinks");
    }

    #[test]
    fn eyes_look_where_they_are_pulled_and_squeeze_when_pulled_hard() {
        assert_eq!(EyePose::new(0.0, 0.0, 0.0, 1.0, 0.0), EyePose::REST);
        let right = EyePose::new(30.0, 0.0, 0.0, 1.0, 0.0);
        assert!(right.look_x > 0.4 && right.squeeze == 0.0 && right.surprise > 0.9);
        let left = EyePose::new(-30.0, 0.0, 0.0, 1.0, 0.0);
        assert!((left.look_x + right.look_x).abs() < 1e-6);
        let hard = EyePose::new(110.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(hard.squeeze, 1.0);
        assert_eq!(hard.open, 1.0, "a squeeze overrides a blink");
        assert!(EyePose::new(1000.0, 1000.0, 0.0, 1.0, 0.0).look_x <= 1.0);
    }

    #[test]
    fn lifted_away_from_the_edge_they_smile_and_into_it_they_strain() {
        let lifted = EyePose::new(0.0, -50.0, 50.0, 1.0, 0.0);
        assert_eq!(lifted.happy, 1.0);
        assert!(lifted.look_y < 0.0, "they look up as they rise");
        let pushed = EyePose::new(0.0, 4.0, -4.0, 1.0, 0.0);
        assert_eq!(pushed.strain, 1.0);
        assert_eq!(pushed.happy, 0.0);
        // Straining eyes tremble: the shake changes from moment to moment.
        let shakes: Vec<f32> = (0..8).map(|i| EyePose::new(0.0, 4.0, -4.0, 1.0, f64::from(i) * 0.013).shift_x).collect();
        assert!(shakes.iter().any(|s| *s > 0.2) && shakes.iter().any(|s| *s < -0.2));
    }

    #[test]
    fn each_gesture_ends_in_its_own_reaction() {
        assert_eq!(Reaction::for_pull(0.0, 40.0), Some(Reaction::Hop));
        assert_eq!(Reaction::for_pull(40.0, 0.0), Some(Reaction::Shake));
        assert_eq!(Reaction::for_pull(-40.0, 5.0), Some(Reaction::Shake));
        assert_eq!(Reaction::for_pull(0.0, -3.0), Some(Reaction::Pop));
        assert_eq!(Reaction::for_pull(2.0, 1.0), None);

        let rest = EyePose::REST;
        let hop = rest.react(Reaction::Hop, 0.15);
        assert!(hop.shift_y < -3.0 && hop.happy == 1.0);
        assert!(rest.react(Reaction::Pop, 0.1).surprise > 0.5);
        assert!(rest.react(Reaction::Shake, 0.04).shift_x.abs() > 1.0);
        // Every reaction ends exactly at rest, so nothing jumps.
        for reaction in [Reaction::Hop, Reaction::Pop, Reaction::Shake] {
            let end = rest.react(reaction, reaction.length() - 0.001);
            assert!(end.shift_x.abs() < 0.2 && end.shift_y.abs() < 0.6, "{reaction:?}: {end:?}");
            assert!(end.happy < 0.05 && end.surprise < 0.05 && end.open > 0.95, "{reaction:?}: {end:?}");
            assert_eq!(rest.react(reaction, reaction.length() + 0.1), rest);
        }
    }
}
