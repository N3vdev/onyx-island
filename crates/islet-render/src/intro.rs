//! The startup greeting: the pill draws itself into the Onyx logo, a black
//! pill with a silver rim, faint orbit lines and a glowing dot at its right
//! end. The rim traces itself in from the dot's end, both ways round. The
//! dot rides in along an orbit line and lights up where it lands, and one
//! soft light sweeps across the glass before the island moves on.
use super::*;
use std::f32::consts::{FRAC_PI_2, PI};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Factory, ID2D1LinearGradientBrush, ID2D1RenderTarget, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID,
    D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_ROUND,
    D2D1_STROKE_STYLE_PROPERTIES,
};

/// Each beat, in seconds from when the pill starts leaving the edge.
const RIM: (f32, f32) = (0.30, 1.05);
const TRAVEL: (f32, f32) = (0.62, 1.30);
const LAND: (f32, f32) = (1.30, 1.80);
const DETAIL: (f32, f32) = (1.05, 1.60);
const SHEEN: (f32, f32) = (1.55, 2.30);

const WHITE: [u8; 3] = [255, 255, 255];
const SILVER: [u8; 3] = [214, 218, 230];
/// Where the dot rests, as a fraction of the pill (as on the app icon).
const DOT_AT: (f32, f32) = (0.85, 0.5);

/// Progress through one beat, 0 before it and 1 after.
pub(super) fn phase(t: f32, (start, end): (f32, f32)) -> f32 {
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

/// Overshoots a touch and settles: the dot's pop.
pub(super) fn ease_out_back(x: f32) -> f32 {
    const OVERSHOOT: f32 = 1.7;
    let x = x.clamp(0.0, 1.0) - 1.0;
    1.0 + (OVERSHOOT + 1.0) * x.powi(3) + OVERSHOOT * x.powi(2)
}

fn ease_in_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) * 0.5
    }
}

fn v(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// A point `distance` along the pill's outline, starting at the middle of
/// its right end and going clockwise on screen (down first). `inset` pulls
/// the outline in, so a stroke sits inside the shell.
pub(super) fn outline_point(distance: f32, width: f32, height: f32, corner: f32, inset: f32) -> Vector2 {
    let (w, h) = (width - inset * 2.0, height - inset * 2.0);
    let r = (corner - inset).clamp(0.0, w.min(h) * 0.5);
    let (straight_x, straight_y) = (w - 2.0 * r, h - 2.0 * r);
    let quarter = FRAC_PI_2 * r;
    let mut d = distance.rem_euclid(outline_length(width, height, corner, inset));
    let arc = |cx: f32, cy: f32, from: f32, d: f32| {
        let angle = from + if r > 0.0 { d / r } else { 0.0 };
        v(inset + cx + r * angle.cos(), inset + cy + r * angle.sin())
    };
    // Right side, lower half; corner; bottom; corner; left; corner; top;
    // corner; right side, upper half.
    let half_y = straight_y * 0.5;
    if d <= half_y {
        return v(inset + w, inset + h * 0.5 + d);
    }
    d -= half_y;
    if d <= quarter {
        return arc(w - r, h - r, 0.0, d);
    }
    d -= quarter;
    if d <= straight_x {
        return v(inset + w - r - d, inset + h);
    }
    d -= straight_x;
    if d <= quarter {
        return arc(r, h - r, FRAC_PI_2, d);
    }
    d -= quarter;
    if d <= straight_y {
        return v(inset, inset + h - r - d);
    }
    d -= straight_y;
    if d <= quarter {
        return arc(r, r, PI, d);
    }
    d -= quarter;
    if d <= straight_x {
        return v(inset + r + d, inset);
    }
    d -= straight_x;
    if d <= quarter {
        return arc(w - r, r, PI + FRAC_PI_2, d);
    }
    d -= quarter;
    v(inset + w, inset + r + d)
}

pub(super) fn outline_length(width: f32, height: f32, corner: f32, inset: f32) -> f32 {
    let (w, h) = (width - inset * 2.0, height - inset * 2.0);
    let r = (corner - inset).clamp(0.0, w.min(h) * 0.5);
    2.0 * (w - 2.0 * r) + 2.0 * (h - 2.0 * r) + 2.0 * PI * r
}

/// A point along one of the faint orbit lines: `lift` 0 is the upper line,
/// which ends at the dot, and 1 the lower one.
pub(super) fn orbit_point(u: f32, width: f32, height: f32, lift: f32) -> Vector2 {
    let (start, control, end) = if lift < 0.5 {
        ((0.07, 0.76), (0.42, 0.14), DOT_AT)
    } else {
        ((0.06, 0.90), (0.50, 0.34), (0.95, 0.72))
    };
    let s = 1.0 - u;
    let x = s * s * start.0 + 2.0 * s * u * control.0 + u * u * end.0;
    let y = s * s * start.1 + 2.0 * s * u * control.1 + u * u * end.1;
    v(x * width, y * height)
}

impl Gfx {
    /// Draws the greeting in the pill's local space. `self.intro_t` is the
    /// time since the pill began to appear.
    pub(super) unsafe fn draw_intro(
        &mut self,
        width: f32,
        height: f32,
        corner: f32,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        // Nothing until the droplet has opened most of the way into a pill.
        let full = islet_core::island::INTRO.width;
        let alpha = alpha * smoothstep(full * 0.6, full * 0.92, width);
        if alpha <= 0.004 {
            return Ok(());
        }
        let t = self.intro_t;
        let corner = corner.min(width * 0.5).min(height * 0.5);

        // The silver rim traces itself in from the dot's end, both ways
        // round, meeting at the far end. A bright head leads each trace.
        let rim = ease_in_out(phase(t, RIM));
        if rim > 0.0 {
            let inset = 1.6;
            let half = outline_length(width, height, corner, inset) * 0.5;
            let reach = half * rim;
            let steps = ((reach / 2.0).ceil() as usize).max(2);
            for direction in [1.0_f32, -1.0] {
                // A negative distance runs counter-clockwise.
                let points: Vec<Vector2> = (0..=steps)
                    .map(|i| outline_point(direction * reach * i as f32 / steps as f32, width, height, corner, inset))
                    .collect();
                self.stroke_polyline(&points, 4.0, color(SILVER, 0.07 * alpha))?;
                self.stroke_polyline(&points, 1.4, color(SILVER, 0.80 * alpha))?;
                if rim < 1.0 {
                    let head = points[points.len() - 1];
                    let glow = alpha * (1.0 - rim).sqrt();
                    self.fill_circle(head.X, head.Y, 4.5, color(WHITE, 0.16 * glow))?;
                    self.fill_circle(head.X, head.Y, 1.6, color(WHITE, 0.95 * glow))?;
                }
            }
        }

        // The orbit lines, faint, as on the icon. The upper one is revealed
        // behind the dot as it rides in; the lower one fades in after.
        let travel = ease_in_out(phase(t, TRAVEL));
        let detail = ease_out(phase(t, DETAIL));
        if travel > 0.0 {
            let steps = 48;
            let points: Vec<Vector2> = (0..=steps)
                .map(|i| orbit_point(travel * i as f32 / steps as f32, width, height, 0.0))
                .collect();
            self.stroke_polyline(&points, 1.0, color(WHITE, 0.13 * alpha))?;
            // A brighter tail just behind the dot while it moves.
            if travel < 1.0 {
                let tail = 14;
                for i in 0..tail {
                    let from = (travel - 0.30 * (i + 1) as f32 / tail as f32).max(0.0);
                    let to = (travel - 0.30 * i as f32 / tail as f32).max(0.0);
                    let segment = [
                        orbit_point(from, width, height, 0.0),
                        orbit_point(to, width, height, 0.0),
                    ];
                    let strength = 1.0 - i as f32 / tail as f32;
                    self.stroke_polyline(&segment, 1.6, color(accent, 0.55 * strength * alpha))?;
                }
            }
        }
        if detail > 0.0 {
            let points: Vec<Vector2> = (0..=48)
                .map(|i| orbit_point(i as f32 / 48.0, width, height, 1.0))
                .collect();
            self.stroke_polyline(&points, 1.0, color(WHITE, 0.08 * detail * alpha))?;
            // The small gloss mark at the top left.
            self.fill_round_rect(
                width * 0.14,
                height * 0.17,
                width * 0.24 * detail,
                2.0,
                1.0,
                color(WHITE, 0.10 * detail * alpha),
            )?;
        }

        // The dot: small while it travels, then it lands, pops and blooms,
        // sending one ring out. It keeps a slow glow while it rests.
        if travel > 0.0 {
            let at = orbit_point(travel, width, height, 0.0);
            let land = phase(t, LAND);
            let radius = if land > 0.0 { 2.2 + 2.0 * ease_out_back(land) } else { 2.2 };
            let breathe = 0.5 + 0.5 * ((t - LAND.1).max(0.0) * 3.2).sin();
            let glow = if land > 0.0 { 0.20 + 0.10 * breathe * phase(t, (LAND.1, LAND.1 + 0.3)) } else { 0.16 };
            self.fill_circle(at.X, at.Y, radius * 3.2, color(accent, glow * 0.45 * alpha))?;
            self.fill_circle(at.X, at.Y, radius * 2.0, color(accent, glow * alpha))?;
            if land > 0.0 && land < 1.0 {
                let ring = ease_out(land);
                self.set_color(color(accent, 0.55 * (1.0 - ring) * alpha));
                self.dc.DrawEllipse(
                    &D2D1_ELLIPSE {
                        point: at,
                        radiusX: radius + 13.0 * ring,
                        radiusY: radius + 13.0 * ring,
                    },
                    &self.brush,
                    1.2,
                    None::<&ID2D1StrokeStyle>,
                );
            }
            self.fill_circle(at.X, at.Y, radius, color(accent, alpha))?;
            // A hot centre, like the icon's lit dot.
            self.fill_circle(at.X - radius * 0.2, at.Y - radius * 0.2, radius * 0.45, color(WHITE, 0.55 * alpha))?;
        }

        // One soft light sweeps across, and the rim brightens as it passes.
        let sweep = phase(t, SHEEN);
        if sweep > 0.0 && sweep < 1.0 {
            let outline = self.geometries.get(&self.factory, width, height, corner, 0.0, false)?;
            let band = width * 0.55;
            let center = -band + (width + band * 2.0) * ease_in_out(sweep);
            let sheen = self.sheen_brush()?;
            sheen.SetStartPoint(v(center - band * 0.5, 0.0));
            sheen.SetEndPoint(v(center + band * 0.5, height));
            sheen.SetOpacity(alpha);
            self.dc.FillGeometry(&outline, &sheen, None::<&ID2D1Brush>);
        }
        Ok(())
    }

    /// Strokes an open line through `points` with round ends.
    pub(super) unsafe fn stroke_polyline(&mut self, points: &[Vector2], width: f32, color: D2D1_COLOR_F) -> RenderResult<()> {
        if points.len() < 2 || color.a <= 0.003 {
            return Ok(());
        }
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(points[0], D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddLines(&points[1..]);
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        let style = self.round_stroke()?;
        self.set_color(color);
        self.dc.DrawGeometry(&geometry, &self.brush, width, &style);
        Ok(())
    }

    pub(super) unsafe fn round_stroke(&mut self) -> RenderResult<ID2D1StrokeStyle> {
        if let Some(style) = &self.intro_stroke {
            return Ok(style.clone());
        }
        // The plain factory overload (Factory1's own takes newer properties).
        let style = ID2D1Factory::CreateStrokeStyle(
            &self.factory,
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 1.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            },
            None,
        )?;
        self.intro_stroke = Some(style.clone());
        Ok(style)
    }

    /// A diagonal band of light, clear at both ends. Made once.
    unsafe fn sheen_brush(&mut self) -> RenderResult<ID2D1LinearGradientBrush> {
        if let Some(brush) = &self.intro_sheen {
            return Ok(brush.clone());
        }
        let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP {
            position,
            color: color(WHITE, a),
        };
        // The plain render-target overload (the device context's own takes
        // colour spaces).
        let stops = ID2D1RenderTarget::CreateGradientStopCollection(
            &self.dc,
            &[stop(0.0, 0.0), stop(0.5, 0.12), stop(1.0, 0.0)],
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        )?;
        let brush = self.dc.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: v(0.0, 0.0),
                endPoint: v(1.0, 0.0),
            },
            None,
            &stops,
        )?;
        self.intro_sheen = Some(brush.clone());
        Ok(brush)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Vector2, x: f32, y: f32) -> bool {
        (a.X - x).abs() < 0.01 && (a.Y - y).abs() < 0.01
    }

    #[test]
    fn beats_run_from_zero_to_one_and_hold() {
        assert_eq!(phase(0.0, RIM), 0.0);
        assert_eq!(phase(5.0, RIM), 1.0);
        assert!(ease_out_back(0.0).abs() < 1e-5);
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-5);
        assert!((0..100).any(|i| ease_out_back(i as f32 / 100.0) > 1.0));
        for beat in [RIM, TRAVEL, LAND, DETAIL, SHEEN] {
            assert!(beat.1 <= 2.4, "every beat ends before the app moves on");
        }
    }

    #[test]
    fn the_rim_starts_at_the_right_end_and_meets_itself_at_the_left() {
        let (w, h, r) = (112.0, 40.0, 20.0);
        let length = outline_length(w, h, r, 0.0);
        assert!((length - (2.0 * 72.0 + PI * 40.0)).abs() < 0.01);
        assert!(near(outline_point(0.0, w, h, r, 0.0), 112.0, 20.0));
        // Clockwise reaches the far end at half way, as does the other way.
        assert!(near(outline_point(length * 0.5, w, h, r, 0.0), 0.0, 20.0));
        assert!(near(outline_point(-length * 0.5, w, h, r, 0.0), 0.0, 20.0));
        // A quarter of the way round clockwise is on the bottom edge.
        assert!((outline_point(length * 0.25, w, h, r, 0.0).Y - 40.0).abs() < 0.01);
    }

    #[test]
    fn the_dot_lands_where_the_icon_has_it() {
        let end = orbit_point(1.0, 112.0, 40.0, 0.0);
        assert!(near(end, 112.0 * DOT_AT.0, 40.0 * DOT_AT.1));
    }
}
