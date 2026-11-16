//! The island as Onyx Lens's toolbar: the lens's tools as icon buttons, the
//! chosen one with its name beside it, and a close button.
//!
//! The icons are the lens's own (its `icons.ts`), drawn from the same SVG
//! path data so the island and the lens look like one thing.
use super::*;
use islet_core::lens::{LensCard, LensItem, LensTool};
use islet_core::widgets::WidgetHover;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN, D2D_SIZE_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
};

/// The icon's drawn size, in the pill's logical pixels (the paths are 24).
const ICON: f32 = 16.0;
const ICON_STROKE: f32 = 2.0;
const TEXT: [u8; 3] = [244, 245, 250];
const MUTED: [u8; 3] = [196, 200, 212];

fn icon_path(item: LensItem) -> &'static str {
    match item {
        LensItem::Title => "M4 9V6a2 2 0 0 1 2-2h3M15 4h3a2 2 0 0 1 2 2v3M20 15v3a2 2 0 0 1-2 2h-3M9 20H6a2 2 0 0 1-2-2v-3M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
        LensItem::Close => "M7 7l10 10M17 7 7 17",
        LensItem::Tool(tool) => match tool {
            LensTool::Lasso => "M12 4c4.4 0 8 2.5 8 5.5S16.4 15 12 15s-8-2.5-8-5.5S7.6 4 12 4ZM7.5 14c-.8 1.2-.6 2.8.6 3.6 1 .7 2.4.6 3.2-.3M11 17.5 10 21",
            LensTool::Rectangle => "M7 5h10a3 3 0 0 1 3 3v8a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3V8a3 3 0 0 1 3-3Z",
            LensTool::FullScreen => "M4 9V5a1 1 0 0 1 1-1h4M15 4h4a1 1 0 0 1 1 1v4M20 15v4a1 1 0 0 1-1 1h-4M9 20H5a1 1 0 0 1-1-1v-4",
            LensTool::Eyedropper => "m14 7 3 3M4.5 19.5l1-3L15 7l2-2a2.1 2.1 0 0 1 3 3l-2 2-9.5 9.5-3 1Z",
            LensTool::Measure => "M3 16 16 3l5 5L8 21l-5-5ZM7 12l2 2M10 9l2 2M13 6l2 2",
            // A magnet: the outline snaps to the edges.
            LensTool::Snap => "m6 15-4-4 6.75-6.77a7.79 7.79 0 0 1 11 11L13 22l-4-4 6.39-6.36a2.14 2.14 0 0 0-3-3L6 15ZM5 8l4 4M12 15l4 4",
            LensTool::Brush => "M9.5 14.5 18 6a1.9 1.9 0 0 1 2.7 2.7l-8.5 8.5M9.5 14.5c-2 0-3.5 1.5-3.5 3.5 0 1-.7 2-2 2 1 .7 2.3 1 3.5 1 2.5 0 4.2-1.8 4.2-4.3",
            LensTool::Erase => "M7 20h12M5.5 15.5 14 7a2 2 0 0 1 2.8 0l2.2 2.2a2 2 0 0 1 0 2.8L12 19H8.5l-3-3a1 1 0 0 1 0-1.5ZM10 11l5 5",
        },
    }
}

/// One drawing step of an SVG path, in absolute coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    /// Two control points, then the end point.
    Cubic([f32; 6]),
    Arc { x: f32, y: f32, rx: f32, ry: f32, rotation: f32, large: bool, sweep: bool },
    Close,
}

/// Reads the subset of SVG path data the lens's icons use: M L H V C S A Z,
/// absolute and relative, with SVG's compact number forms ("2.1-2.3",
/// "-.8", ".5.5") and implicit repeats.
pub(crate) fn parse_path(data: &str) -> Vec<Segment> {
    let bytes = data.as_bytes();
    let mut at = 0;
    let skip = |at: &mut usize| {
        while *at < bytes.len() && (bytes[*at].is_ascii_whitespace() || bytes[*at] == b',') {
            *at += 1;
        }
    };
    let number = |at: &mut usize| -> Option<f32> {
        skip(at);
        let start = *at;
        if *at < bytes.len() && matches!(bytes[*at], b'-' | b'+') {
            *at += 1;
        }
        let mut dot = false;
        let mut digits = false;
        while *at < bytes.len() {
            match bytes[*at] {
                b'0'..=b'9' => digits = true,
                b'.' if !dot => dot = true,
                _ => break,
            }
            *at += 1;
        }
        if !digits {
            *at = start;
            return None;
        }
        data[start..*at].parse().ok()
    };
    let mut segments = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut start_x, mut start_y) = (0.0, 0.0);
    // The second control point of the last cubic, for S.
    let mut last_control: Option<(f32, f32)> = None;
    let mut command = b'M';
    loop {
        skip(&mut at);
        if at >= bytes.len() {
            break;
        }
        if bytes[at].is_ascii_alphabetic() {
            command = bytes[at];
            at += 1;
        }
        let relative = command.is_ascii_lowercase();
        let (base_x, base_y) = if relative { (x, y) } else { (0.0, 0.0) };
        let before = at;
        match command.to_ascii_uppercase() {
            b'Z' => {
                segments.push(Segment::Close);
                (x, y) = (start_x, start_y);
                last_control = None;
                // Z takes no numbers; a following number is malformed.
                skip(&mut at);
                if at < bytes.len() && !bytes[at].is_ascii_alphabetic() {
                    break;
                }
                continue;
            }
            b'M' => {
                let (Some(a), Some(b)) = (number(&mut at), number(&mut at)) else { break };
                (x, y) = (base_x + a, base_y + b);
                (start_x, start_y) = (x, y);
                segments.push(Segment::Move(x, y));
                last_control = None;
                // Further pairs are lines.
                command = if relative { b'l' } else { b'L' };
            }
            b'L' => {
                let (Some(a), Some(b)) = (number(&mut at), number(&mut at)) else { break };
                (x, y) = (base_x + a, base_y + b);
                segments.push(Segment::Line(x, y));
                last_control = None;
            }
            b'H' => {
                let Some(a) = number(&mut at) else { break };
                x = base_x + a;
                segments.push(Segment::Line(x, y));
                last_control = None;
            }
            b'V' => {
                let Some(b) = number(&mut at) else { break };
                y = if relative { y + b } else { b };
                segments.push(Segment::Line(x, y));
                last_control = None;
            }
            b'C' | b'S' => {
                let first = if command.eq_ignore_ascii_case(&b'S') {
                    // The reflection of the last control point, or here.
                    last_control.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy))
                } else {
                    let (Some(a), Some(b)) = (number(&mut at), number(&mut at)) else { break };
                    (base_x + a, base_y + b)
                };
                let values: Option<Vec<f32>> = (0..4).map(|_| number(&mut at)).collect();
                let Some(values) = values else { break };
                let second = (base_x + values[0], base_y + values[1]);
                (x, y) = (base_x + values[2], base_y + values[3]);
                segments.push(Segment::Cubic([first.0, first.1, second.0, second.1, x, y]));
                last_control = Some(second);
            }
            b'A' => {
                let values: Option<Vec<f32>> = (0..7).map(|_| number(&mut at)).collect();
                let Some(values) = values else { break };
                (x, y) = (base_x + values[5], base_y + values[6]);
                segments.push(Segment::Arc {
                    x,
                    y,
                    rx: values[0].abs(),
                    ry: values[1].abs(),
                    rotation: values[2],
                    large: values[3] != 0.0,
                    sweep: values[4] != 0.0,
                });
                last_control = None;
            }
            _ => break,
        }
        if at == before {
            break;
        }
    }
    segments
}

impl Gfx {
    /// The icon's outline, built once per icon.
    unsafe fn lens_icon(&mut self, item: LensItem) -> RenderResult<ID2D1PathGeometry1> {
        let path = icon_path(item);
        if let Some(geometry) = self.lens_icons.get(path) {
            return Ok(geometry.clone());
        }
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        let point = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let mut open = false;
        let mut start = point(0.0, 0.0);
        for segment in parse_path(path) {
            if !open && !matches!(segment, Segment::Move(..) | Segment::Close) {
                sink.BeginFigure(start, D2D1_FIGURE_BEGIN_HOLLOW);
                open = true;
            }
            match segment {
                Segment::Move(x, y) => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    }
                    start = point(x, y);
                    sink.BeginFigure(start, D2D1_FIGURE_BEGIN_HOLLOW);
                    open = true;
                }
                Segment::Line(x, y) => sink.AddLine(point(x, y)),
                Segment::Cubic([x1, y1, x2, y2, x, y]) => {
                    sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                        point1: point(x1, y1),
                        point2: point(x2, y2),
                        point3: point(x, y),
                    });
                }
                Segment::Arc { x, y, rx, ry, rotation, large, sweep } => {
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: point(x, y),
                        size: D2D_SIZE_F { width: rx, height: ry },
                        rotationAngle: rotation,
                        // Both are y-down: SVG's positive sweep is clockwise.
                        sweepDirection: if sweep {
                            D2D1_SWEEP_DIRECTION_CLOCKWISE
                        } else {
                            D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                        },
                        arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                    });
                }
                Segment::Close => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                    }
                }
            }
        }
        if open {
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
        }
        sink.Close()?;
        self.lens_icons.insert(path, geometry.clone());
        Ok(geometry)
    }

    unsafe fn draw_lens_icon(&mut self, item: LensItem, x: f32, y: f32, rgb: [u8; 3], alpha: f32) -> RenderResult<()> {
        let geometry = self.lens_icon(item)?;
        let style = self.round_stroke()?;
        let mut base = Matrix3x2::default();
        self.dc.GetTransform(&mut base);
        let scale = ICON / 24.0;
        self.dc
            .SetTransform(&(Matrix3x2::scale(scale, scale) * Matrix3x2::translation(x, y) * base));
        self.set_color(color(rgb, alpha));
        self.dc.DrawGeometry(&geometry, &self.brush, ICON_STROKE, &style);
        self.dc.SetTransform(&base);
        Ok(())
    }

    /// The toolbar, laid out at its own size and centred in the pill, so it
    /// is revealed rather than squeezed while the pill grows into it.
    pub(super) unsafe fn draw_lens(
        &mut self,
        width: f32,
        height: f32,
        card: &LensCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        let size = card.size();
        let dx = (width - size.width) * 0.5;
        let dy = (height - size.height) * 0.5;
        for (x, y) in card.dividers() {
            self.fill_round_rect(dx + x - 0.5, dy + y - 9.0, 1.0, 18.0, 0.5, color([255, 255, 255], 0.16 * alpha))?;
        }
        for slot in card.slots() {
            let rect = Rect::new(slot.rect.x + dx, slot.rect.y + dy, slot.rect.width, slot.rect.height);
            let hover = self.widget_hover == Some(WidgetHover::Lens(slot.item));
            let icon_y = rect.y + (rect.height - ICON) * 0.5;
            let radius = rect.height * 0.5;
            if slot.pressed {
                self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, radius, color(accent, if hover { 0.34 } else { 0.26 } * alpha))?;
            } else if hover {
                self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, radius, color([255, 255, 255], 0.12 * alpha))?;
            }
            let ink = if slot.pressed || hover { TEXT } else { MUTED };
            // Every tool says what it is: icon, then its name (as the lens's
            // own toolbar does). Close is the one icon-only button.
            let label = match slot.item {
                LensItem::Title => Some("Onyx Lens"),
                LensItem::Tool(tool) => Some(tool.label()),
                LensItem::Close => None,
            };
            match label {
                Some(label) => {
                    let icon_ink = if slot.item == LensItem::Title { accent } else { ink };
                    self.draw_lens_icon(slot.item, rect.x + 10.0, icon_y, icon_ink, alpha)?;
                    let text_x = rect.x + 10.0 + ICON + 6.0;
                    self.draw_text(label, FormatId::CalculatorKey, text_x, rect.y, rect.right() - 10.0 - text_x, rect.height, color(ink, alpha))?;
                }
                None => {
                    self.draw_lens_icon(slot.item, rect.x + (rect.width - ICON) * 0.5, icon_y, ink, alpha)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_compact_numbers_and_relative_commands() {
        let segments = parse_path("M7 5h10a3 3 0 0 1 3 3v8l-.5 1.5.5.5Z");
        assert_eq!(segments[0], Segment::Move(7.0, 5.0));
        assert_eq!(segments[1], Segment::Line(17.0, 5.0));
        assert_eq!(
            segments[2],
            Segment::Arc { x: 20.0, y: 8.0, rx: 3.0, ry: 3.0, rotation: 0.0, large: false, sweep: true }
        );
        assert_eq!(segments[3], Segment::Line(20.0, 16.0));
        assert_eq!(segments[4], Segment::Line(19.5, 17.5));
        // An implicit repeat of `l`, with ".5.5" read as two numbers.
        assert_eq!(segments[5], Segment::Line(20.0, 18.0));
        assert_eq!(segments[6], Segment::Close);
    }

    #[test]
    fn smooth_curves_reflect_and_moves_repeat_as_lines() {
        let segments = parse_path("M12 4c4.4 0 8 2.5 8 5.5S16.4 15 12 15m1 1 2 2");
        assert_eq!(segments[1], Segment::Cubic([16.4, 4.0, 20.0, 6.5, 20.0, 9.5]));
        assert_eq!(segments[2], Segment::Cubic([20.0, 12.5, 16.4, 15.0, 12.0, 15.0]));
        assert_eq!(segments[3], Segment::Move(13.0, 16.0));
        assert_eq!(segments[4], Segment::Line(15.0, 18.0));
    }

    #[test]
    fn every_icon_parses_completely() {
        let items = LensTool::ALL.map(LensItem::Tool).into_iter().chain([LensItem::Title, LensItem::Close]);
        for item in items {
            let path = icon_path(item);
            let segments = parse_path(path);
            let commands = path.bytes().filter(|byte| byte.is_ascii_alphabetic()).count();
            assert!(segments.len() >= commands, "{item:?}: {segments:?}");
            assert!(matches!(segments[0], Segment::Move(..)));
        }
    }
}
