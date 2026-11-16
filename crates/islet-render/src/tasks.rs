//! The to-do list page and the focus timer's pill.

use islet_core::geometry::Rect;
use islet_core::tasks::{self, FocusPhase, FocusView, TaskItem, TasksCard};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN};
use windows::Win32::Graphics::Direct2D::{
    ID2D1StrokeStyle, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_SWEEP_DIRECTION_CLOCKWISE,
};
use windows_numerics::Vector2;

use crate::{color, FormatId, Gfx, RenderResult};

const TEXT: [u8; 3] = [240, 241, 244];
const MUTED: [u8; 3] = [150, 155, 166];
const WHITE: [u8; 3] = [255, 255, 255];
const FOCUS_COLOR: [u8; 3] = [255, 176, 92];
const BREAK_COLOR: [u8; 3] = [110, 214, 160];
const OVERDUE: [u8; 3] = [255, 120, 100];

fn phase_color(focus: Option<FocusView>) -> [u8; 3] {
    match focus.map(|focus| focus.phase) {
        Some(FocusPhase::Break) => BREAK_COLOR,
        _ => FOCUS_COLOR,
    }
}

impl Gfx {
    /// Ink for strokes: on glass, grey becomes see-through white like the
    /// text does (see `glass_ink`).
    fn ink(&self, rgb: [u8; 3], alpha: f32) -> D2D1_COLOR_F {
        let ink = color(rgb, alpha);
        if self.glass.is_some() {
            crate::glass_ink(ink)
        } else {
            ink
        }
    }

    /// On glass a faint white fill is lost over a light backdrop, so fields
    /// and chips get a hairline edge too.
    unsafe fn glass_edge(&mut self, rect: Rect, radius: f32, alpha: f32) -> RenderResult<()> {
        if self.glass.is_none() {
            return Ok(());
        }
        self.stroke_round_rect(rect, radius, color(WHITE, 0.3 * alpha))
    }

    /// The page when opened; the focus countdown (or a task count) when small.
    pub(crate) unsafe fn draw_tasks(
        &mut self,
        width: f32,
        height: f32,
        card: &TasksCard,
        alpha: f32,
    ) -> RenderResult<()> {
        if height < 60.0 {
            return self.draw_tasks_compact(width, height, card, alpha);
        }
        // Laid out at its own size, centred in the shape.
        let x0 = ((width - tasks::WIDTH) * 0.5).max(0.0);
        let y0 = ((height - tasks::HEIGHT) * 0.5).max(0.0);
        let at = |rect: Rect| Rect::new(rect.x + x0, rect.y + y0, rect.width, rect.height);
        let hover = match self.widget_hover {
            Some(islet_core::widgets::WidgetHover::Task(item)) => Some(item),
            _ => None,
        };

        // Header: the title and what is left.
        self.draw_text("To-do", FormatId::Title, x0 + tasks::PAD, y0 + tasks::HEADER_Y, 80.0, 24.0, color(TEXT, alpha))?;
        let left = match card.open {
            0 => "All done".to_owned(),
            open => format!("{open} left"),
        };
        self.draw_text(&left, FormatId::Sub, x0 + tasks::PAD + 62.0, y0 + tasks::HEADER_Y + 4.0, 90.0, 18.0, color(MUTED, alpha))?;

        if let Some(clear) = card.clear_button() {
            let rect = at(clear);
            let lit = hover == Some(TaskItem::ClearDone);
            self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, 15.0, color(WHITE, if lit { 0.14 } else { 0.07 } * alpha))?;
            self.glass_edge(rect, 15.0, alpha)?;
            self.draw_text("Clear done", FormatId::Button, rect.x, rect.y, rect.width, rect.height, color(MUTED, alpha))?;
        }
        self.draw_focus_button(at(card.focus_button()), card, hover == Some(TaskItem::Focus), alpha)?;

        // The tasks.
        if card.lines.is_empty() {
            let message = if card.done > 0 { "Everything's done. Nice." } else { "Nothing to do yet." };
            let list = at(Rect::new(0.0, tasks::LIST_Y, tasks::WIDTH, tasks::LINE_HEIGHT * 2.0));
            self.draw_text(message, FormatId::Button, list.x, list.y + 18.0, list.width, 24.0, color(MUTED, alpha))?;
            self.draw_text(
                "Type below. Add a time and you'll be reminded: \u{201c}call Sam at 5pm\u{201d}",
                FormatId::CalculatorKey,
                list.x + tasks::PAD,
                list.y + 44.0,
                list.width - 2.0 * tasks::PAD,
                20.0,
                color(MUTED, 0.75 * alpha),
            )?;
        }
        for (index, line) in card.lines.iter().enumerate().take(tasks::LINES) {
            let row = at(TasksCard::line_rect(index));
            let row_hover = matches!(hover, Some(TaskItem::Check(id) | TaskItem::Remove(id)) if id == line.id);
            if row_hover {
                self.fill_round_rect(row.x, row.y + 1.0, row.width, row.height - 2.0, 12.0, color(WHITE, 0.06 * alpha))?;
            }
            // The tick circle.
            let (cx, cy) = (row.x + 18.0, row.y + row.height * 0.5);
            self.stroke_circle(cx, cy, 8.0, 1.5, self.ink(if line.overdue { OVERDUE } else { MUTED }, alpha))?;
            if hover == Some(TaskItem::Check(line.id)) {
                self.fill_circle(cx, cy, 8.0, color(FOCUS_COLOR, 0.25 * alpha))?;
                self.draw_tick(cx, cy, color(TEXT, 0.8 * alpha))?;
            }
            // The task, and when its reminder is.
            let when_width = if line.when.is_some() { 120.0 } else { 0.0 };
            let remove = if row_hover { 30.0 } else { 0.0 };
            let text_x = row.x + 36.0;
            let text_width = row.right() - text_x - when_width - remove - 8.0;
            self.draw_text(&line.text, FormatId::TaskText, text_x, row.y, text_width, row.height, color(TEXT, alpha))?;
            if let Some(when) = &line.when {
                let tint = if line.overdue { OVERDUE } else { MUTED };
                self.draw_text(when, FormatId::TaskWhen, row.right() - when_width - remove - 8.0, row.y, when_width, row.height, color(tint, alpha))?;
            }
            if row_hover {
                let button = at(TasksCard::remove_rect(index));
                let lit = hover == Some(TaskItem::Remove(line.id));
                if lit {
                    self.fill_round_rect(button.x, button.y, button.width, button.height, 9.0, color(OVERDUE, 0.22 * alpha))?;
                }
                self.draw_cross(button.x + button.width * 0.5, button.y + button.height * 0.5, 4.0, self.ink(if lit { TEXT } else { MUTED }, alpha))?;
            }
        }
        if card.open > tasks::LINES {
            let row = at(TasksCard::line_rect(tasks::LINES - 1));
            self.draw_text(
                &format!("+{} more in Settings", card.open - tasks::LINES),
                FormatId::TaskWhen,
                row.x,
                row.bottom() - 2.0,
                row.width - 8.0,
                14.0,
                color(MUTED, 0.7 * alpha),
            )?;
        }

        // Where a task is typed.
        let input = at(TasksCard::input_rect());
        let lit = card.typing || hover == Some(TaskItem::Input);
        self.fill_round_rect(input.x, input.y, input.width, input.height, 14.0, color(WHITE, if lit { 0.11 } else { 0.07 } * alpha))?;
        if card.typing {
            self.stroke_round_rect(input, 14.0, color(FOCUS_COLOR, 0.55 * alpha))?;
        } else {
            self.glass_edge(input, 14.0, alpha)?;
        }
        let text_x = input.x + 16.0;
        let text_width = input.width - 32.0 - 24.0;
        if card.draft.is_empty() {
            let hint = if card.typing { "Type a task, then Enter" } else { "Add a task\u{2026}" };
            self.draw_text(hint, FormatId::TaskText, text_x, input.y, text_width, input.height, color(MUTED, 0.85 * alpha))?;
        } else {
            self.draw_text(&card.draft, FormatId::TaskText, text_x, input.y, text_width, input.height, color(TEXT, alpha))?;
        }
        if card.typing {
            let caret = text_x + self.text_width(&card.draft, FormatId::TaskText, text_width, input.height)?.min(text_width) + 1.0;
            self.fill_round_rect(caret, input.y + 11.0, 1.5, input.height - 22.0, 0.75, color(FOCUS_COLOR, alpha))?;
            if !card.draft.is_empty() {
                // Enter adds it.
                self.draw_text("\u{21b5}", FormatId::TaskWhen, input.right() - 34.0, input.y, 20.0, input.height, color(MUTED, alpha))?;
            }
        } else {
            self.draw_plus(input.right() - 22.0, input.y + input.height * 0.5, 5.0, self.ink(MUTED, alpha))?;
        }
        Ok(())
    }

    /// Start, or the time left with a bar filling as it passes.
    unsafe fn draw_focus_button(&mut self, rect: Rect, card: &TasksCard, lit: bool, alpha: f32) -> RenderResult<()> {
        let tint = phase_color(card.focus);
        self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, 15.0, color(tint, if lit { 0.3 } else { 0.18 } * alpha))?;
        let label = match card.focus {
            None => format!("Focus {} min", card.focus_minutes),
            Some(focus) => {
                // The time passed fills the button from the left, inside its
                // rounded shape.
                let fill = rect.width * focus.progress.clamp(0.0, 1.0);
                self.dc.PushAxisAlignedClip(
                    &windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: rect.x,
                        top: rect.y,
                        right: rect.x + fill,
                        bottom: rect.bottom(),
                    },
                    windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                );
                self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, 15.0, color(tint, 0.28 * alpha))?;
                self.dc.PopAxisAlignedClip();
                let verb = if lit { "Stop" } else if focus.phase == FocusPhase::Break { "Break" } else { "Focus" };
                format!("{verb}  {}", tasks::countdown(focus.left))
            }
        };
        self.draw_text(&label, FormatId::Button, rect.x, rect.y, rect.width, rect.height, color(TEXT, alpha))?;
        Ok(())
    }

    /// The small pill: a ring of the time passing and the time left.
    unsafe fn draw_tasks_compact(&mut self, width: f32, height: f32, card: &TasksCard, alpha: f32) -> RenderResult<()> {
        let Some(focus) = card.focus else {
            let label = match card.open {
                0 => "All done".to_owned(),
                1 => "1 task".to_owned(),
                open => format!("{open} tasks"),
            };
            return self.draw_text(&label, FormatId::Tucked, 0.0, 0.0, width, height, color(TEXT, alpha)).map(|_| ());
        };
        let tint = phase_color(Some(focus));
        let radius = (height * 0.5 - 8.0).max(4.0);
        let (cx, cy) = (height * 0.5 + 4.0, height * 0.5);
        self.stroke_circle(cx, cy, radius, 2.5, color(WHITE, 0.16 * alpha))?;
        self.progress_arc(cx, cy, radius, focus.progress, 2.5, color(tint, alpha))?;
        let text_x = cx + radius + 9.0;
        self.draw_text(&tasks::countdown(focus.left), FormatId::Tucked, text_x, 0.0, width - text_x - 8.0, height, color(TEXT, alpha))?;
        Ok(())
    }

    pub(crate) unsafe fn text_width(&mut self, text: &str, format: FormatId, width: f32, height: f32) -> RenderResult<f32> {
        if text.is_empty() {
            return Ok(0.0);
        }
        let format_ref = self.formats.get(format);
        let layout = self.text_cache.layout(&self.dwrite, format_ref, format, text, width, height)?;
        let mut metrics = Default::default();
        layout.GetMetrics(&mut metrics)?;
        Ok(metrics.widthIncludingTrailingWhitespace)
    }

    pub(crate) unsafe fn stroke_circle(&mut self, x: f32, y: f32, radius: f32, width: f32, tint: D2D1_COLOR_F) -> RenderResult<()> {
        if tint.a <= 0.001 {
            return Ok(());
        }
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: Vector2 { X: x, Y: y },
            radiusX: radius,
            radiusY: radius,
        };
        self.set_color(tint);
        self.dc.DrawEllipse(&ellipse, &self.brush, width, None::<&ID2D1StrokeStyle>);
        Ok(())
    }

    unsafe fn stroke_round_rect(&mut self, rect: Rect, radius: f32, tint: D2D1_COLOR_F) -> RenderResult<()> {
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                left: rect.x + 0.5,
                top: rect.y + 0.5,
                right: rect.right() - 0.5,
                bottom: rect.bottom() - 0.5,
            },
            radiusX: radius,
            radiusY: radius,
        };
        self.set_color(tint);
        self.dc.DrawRoundedRectangle(&rounded, &self.brush, 1.0, None::<&ID2D1StrokeStyle>);
        Ok(())
    }

    /// An arc from the top, clockwise, `fraction` of the way round.
    pub(crate) unsafe fn progress_arc(&mut self, x: f32, y: f32, radius: f32, fraction: f32, width: f32, tint: D2D1_COLOR_F) -> RenderResult<()> {
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction <= 0.001 {
            return Ok(());
        }
        if fraction >= 0.999 {
            return self.stroke_circle(x, y, radius, width, tint);
        }
        let angle = fraction * std::f32::consts::TAU;
        let end = Vector2 { X: x + radius * angle.sin(), Y: y - radius * angle.cos() };
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(Vector2 { X: x, Y: y - radius }, D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: end,
            size: windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_F { width: radius, height: radius },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        let stroke = self.round_stroke()?;
        self.set_color(tint);
        self.dc.DrawGeometry(&geometry, &self.brush, width, &stroke);
        Ok(())
    }

    unsafe fn draw_tick(&mut self, x: f32, y: f32, tint: D2D1_COLOR_F) -> RenderResult<()> {
        let stroke = self.round_stroke()?;
        self.set_color(tint);
        let a = Vector2 { X: x - 3.5, Y: y + 0.2 };
        let b = Vector2 { X: x - 1.0, Y: y + 2.8 };
        let c = Vector2 { X: x + 3.8, Y: y - 2.6 };
        self.dc.DrawLine(a, b, &self.brush, 1.6, &stroke);
        self.dc.DrawLine(b, c, &self.brush, 1.6, &stroke);
        Ok(())
    }

    unsafe fn draw_cross(&mut self, x: f32, y: f32, half: f32, tint: D2D1_COLOR_F) -> RenderResult<()> {
        let stroke = self.round_stroke()?;
        self.set_color(tint);
        self.dc.DrawLine(Vector2 { X: x - half, Y: y - half }, Vector2 { X: x + half, Y: y + half }, &self.brush, 1.5, &stroke);
        self.dc.DrawLine(Vector2 { X: x + half, Y: y - half }, Vector2 { X: x - half, Y: y + half }, &self.brush, 1.5, &stroke);
        Ok(())
    }

    unsafe fn draw_plus(&mut self, x: f32, y: f32, half: f32, tint: D2D1_COLOR_F) -> RenderResult<()> {
        let stroke = self.round_stroke()?;
        self.set_color(tint);
        self.dc.DrawLine(Vector2 { X: x - half, Y: y }, Vector2 { X: x + half, Y: y }, &self.brush, 1.5, &stroke);
        self.dc.DrawLine(Vector2 { X: x, Y: y - half }, Vector2 { X: x, Y: y + half }, &self.brush, 1.5, &stroke);
        Ok(())
    }
}
