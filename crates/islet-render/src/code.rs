//! A one-time code from email on the island: who sent it, the code in
//! large type, and a copy button.

use islet_core::otp::{layout, CodeCard, CodeItem, HEIGHT};
use islet_core::widgets::WidgetHover;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle;
use windows_numerics::Vector2;

use crate::{color, FormatId, Gfx, RenderResult};

const WHITE: [u8; 3] = [255, 255, 255];
const INK: [u8; 3] = [244, 245, 250];
const MUTED: [u8; 3] = [173, 180, 199];
/// "Copied": the success green of Onyx's notices.
const DONE: [u8; 3] = [64, 210, 122];

impl Gfx {
    pub(crate) unsafe fn draw_code(&mut self, card: &CodeCard, accent: [u8; 3], alpha: f32) -> RenderResult<()> {
        let hover = match self.widget_hover {
            Some(WidgetHover::Code(item)) => Some(item),
            _ => None,
        };
        // The envelope, in a soft round badge.
        let mail = layout::MAIL;
        let (mx, my) = mail.center();
        self.fill_circle(mx, my, mail.width * 0.5, color(accent, 0.2 * alpha))?;
        self.draw_envelope(mx, my, color(accent, alpha))?;

        // Who it is from, over the code.
        let text_x = layout::TEXT_X;
        let text_w = layout::COPY.x - 10.0 - text_x;
        let label = if card.copied {
            "Copied".to_owned()
        } else if card.from.is_empty() {
            "Code from Gmail".to_owned()
        } else {
            format!("Code from {}", card.from)
        };
        let label_color = if card.copied { color(DONE, alpha) } else { color(MUTED, 0.9 * alpha) };
        self.draw_text(&label, FormatId::Tiny, text_x, 10.0, text_w, 14.0, label_color)?;
        self.draw_text(&card.shown(), FormatId::Code, text_x, 22.0, text_w, HEIGHT - 28.0, color(INK, alpha))?;

        // Copy, then a tick once copied.
        let copy = layout::COPY;
        let (cx, cy) = copy.center();
        let lit = hover == Some(CodeItem::Copy) || card.copied;
        let fill = if card.copied { color(DONE, 0.22 * alpha) } else { color(WHITE, if lit { 0.16 } else { 0.09 } * alpha) };
        self.fill_circle(cx, cy, copy.width * 0.5, fill)?;
        if card.copied {
            self.draw_code_tick(cx, cy, color(DONE, alpha))?;
        } else {
            self.draw_copy_mark(cx, cy, color(INK, 0.92 * alpha))?;
        }

        let close = layout::CLOSE_MARK;
        let (kx, ky) = close.center();
        if hover == Some(CodeItem::Close) {
            self.fill_circle(kx, ky, close.width * 0.5, color(WHITE, 0.12 * alpha))?;
        }
        self.draw_close_mark(kx, ky, 3.5, color(MUTED, 0.9 * alpha))
    }

    /// A small envelope centred on (`x`, `y`).
    unsafe fn draw_envelope(&mut self, x: f32, y: f32, ink: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) -> RenderResult<()> {
        let (w, h) = (12.0, 9.0);
        let (left, top) = (x - w * 0.5, y - h * 0.5);
        self.set_color(ink);
        let rect = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left, top, right: left + w, bottom: top + h },
            radiusX: 1.8,
            radiusY: 1.8,
        };
        self.dc.DrawRoundedRectangle(&rect, &self.brush, 1.4, None::<&ID2D1StrokeStyle>);
        self.stroke_polyline(
            &[
                Vector2 { X: left + 1.2, Y: top + 1.4 },
                Vector2 { X: x, Y: y + 0.8 },
                Vector2 { X: left + w - 1.2, Y: top + 1.4 },
            ],
            1.4,
            ink,
        )
    }

    /// Two overlapping rounded squares: copy.
    unsafe fn draw_copy_mark(&mut self, x: f32, y: f32, ink: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) -> RenderResult<()> {
        let side = 8.0;
        self.set_color(ink);
        for (dx, dy) in [(-1.6f32, -1.6f32), (1.6, 1.6)] {
            let (left, top) = (x + dx - side * 0.5, y + dy - side * 0.5);
            let rect = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left, top, right: left + side, bottom: top + side },
                radiusX: 1.8,
                radiusY: 1.8,
            };
            self.dc.DrawRoundedRectangle(&rect, &self.brush, 1.3, None::<&ID2D1StrokeStyle>);
        }
        Ok(())
    }

    unsafe fn draw_code_tick(&mut self, x: f32, y: f32, ink: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) -> RenderResult<()> {
        self.stroke_polyline(
            &[
                Vector2 { X: x - 4.5, Y: y + 0.2 },
                Vector2 { X: x - 1.4, Y: y + 3.2 },
                Vector2 { X: x + 4.6, Y: y - 3.4 },
            ],
            1.8,
            ink,
        )
    }
}
