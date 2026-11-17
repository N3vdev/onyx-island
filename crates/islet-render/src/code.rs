//! Mail on the island: a one-time code (who sent it, the code in large type,
//! and a copy button) or a new email (who sent it, its subject, and an open
//! button).

use islet_core::otp::{layout, MailBody, MailCard, MailItem, HEIGHT};
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
    pub(crate) unsafe fn draw_code(&mut self, card: &MailCard, accent: [u8; 3], alpha: f32) -> RenderResult<()> {
        let hover = match self.widget_hover {
            Some(WidgetHover::Mail(item)) => Some(item),
            _ => None,
        };
        let width = card.width();
        let copied = matches!(card.body, MailBody::Code { copied: true, .. });
        // The envelope, in a soft round badge.
        let mail = layout::MAIL;
        let (mx, my) = mail.center();
        self.fill_circle(mx, my, mail.width * 0.5, color(accent, 0.2 * alpha))?;
        self.draw_envelope(mx, my, color(accent, alpha))?;

        // Who it is from, over the code or the subject.
        let button = layout::button(width);
        let text_x = layout::TEXT_X;
        let text_w = button.x - 10.0 - text_x;
        let label_color = if copied { color(DONE, alpha) } else { color(MUTED, 0.9 * alpha) };
        self.draw_text(&card.label(), FormatId::Tiny, text_x, 10.0, text_w, 14.0, label_color)?;
        match card.body {
            MailBody::Code { .. } => {
                self.draw_text(&card.shown(), FormatId::Code, text_x, 22.0, text_w, HEIGHT - 28.0, color(INK, alpha))?;
            }
            MailBody::Message { .. } => {
                self.draw_text(&card.shown(), FormatId::Title, text_x, 25.0, text_w, 22.0, color(INK, alpha))?;
            }
        }

        // Copy (a tick once copied), or open.
        let (cx, cy) = button.center();
        let lit = matches!(hover, Some(MailItem::Copy | MailItem::Open)) || copied;
        let fill = if copied { color(DONE, 0.22 * alpha) } else { color(WHITE, if lit { 0.16 } else { 0.09 } * alpha) };
        self.fill_circle(cx, cy, button.width * 0.5, fill)?;
        match card.body {
            MailBody::Code { copied: true, .. } => self.draw_code_tick(cx, cy, color(DONE, alpha))?,
            MailBody::Code { .. } => self.draw_copy_mark(cx, cy, color(INK, 0.92 * alpha))?,
            MailBody::Message { .. } => self.draw_open_mark(cx, cy, color(INK, 0.92 * alpha))?,
        }

        let close = layout::close_mark(width);
        let (kx, ky) = close.center();
        if hover == Some(MailItem::Close) {
            self.fill_circle(kx, ky, close.width * 0.5, color(WHITE, 0.12 * alpha))?;
        }
        self.draw_close_mark(kx, ky, 3.5, color(MUTED, 0.9 * alpha))
    }

    /// An arrow out of the card, up and right: open.
    unsafe fn draw_open_mark(&mut self, x: f32, y: f32, ink: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) -> RenderResult<()> {
        let r = 4.2;
        self.stroke_polyline(&[Vector2 { X: x - r, Y: y + r }, Vector2 { X: x + r, Y: y - r }], 1.6, ink)?;
        self.stroke_polyline(
            &[Vector2 { X: x - 1.4, Y: y - r }, Vector2 { X: x + r, Y: y - r }, Vector2 { X: x + r, Y: y + 1.4 }],
            1.6,
            ink,
        )
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
