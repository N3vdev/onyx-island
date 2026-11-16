//! Spotify search, and the Spotify pill on the music card.

use islet_core::spotify::{duration, layout, SearchCard, SearchItem, SearchStatus, SpotifyPill, ROWS};
use islet_core::widgets::WidgetHover;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1StrokeStyle, D2D1_ELLIPSE, D2D1_ROUNDED_RECT};
use windows_numerics::Vector2;

use crate::{color, FormatId, Gfx, RenderResult};

const WHITE: [u8; 3] = [255, 255, 255];
const INK: [u8; 3] = [244, 245, 250];
const MUTED: [u8; 3] = [173, 180, 199];
/// Spotify's green.
pub(crate) const GREEN: [u8; 3] = [30, 215, 96];

impl Gfx {
    pub(crate) unsafe fn draw_search(&mut self, card: &SearchCard, alpha: f32) -> RenderResult<()> {
        let hover = match self.widget_hover {
            Some(WidgetHover::Search(item)) => Some(item),
            _ => None,
        };
        // The field.
        let field = layout::FIELD;
        let lit = card.typing || hover == Some(SearchItem::Field);
        self.fill_round_rect(field.x, field.y, field.width, field.height, field.height * 0.5, color(WHITE, if lit { 0.12 } else { 0.08 } * alpha))?;
        if card.typing {
            self.set_color(color(GREEN, 0.55 * alpha));
            let rounded = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: field.x + 0.5, top: field.y + 0.5, right: field.right() - 0.5, bottom: field.bottom() - 0.5 },
                radiusX: field.height * 0.5 - 0.5,
                radiusY: field.height * 0.5 - 0.5,
            };
            self.dc.DrawRoundedRectangle(&rounded, &self.brush, 1.0, None::<&ID2D1StrokeStyle>);
        }
        let (gx, gy) = (field.x + 18.0, field.center().1);
        self.draw_magnifier(gx, gy, 1.0, color(MUTED, alpha))?;
        let text_x = field.x + 34.0;
        let text_w = field.right() - 14.0 - text_x;
        if card.query.is_empty() {
            self.draw_text("Search Spotify", FormatId::TaskText, text_x, field.y, text_w, field.height, color(MUTED, 0.85 * alpha))?;
        } else {
            self.draw_text(&card.query, FormatId::TaskText, text_x, field.y, text_w, field.height, color(INK, alpha))?;
        }
        if card.typing {
            let caret = text_x + self.text_width(&card.query, FormatId::TaskText, text_w, field.height)?.min(text_w) + 1.0;
            self.fill_round_rect(caret, field.y + 10.0, 1.5, field.height - 20.0, 0.75, color(GREEN, alpha))?;
        }
        let (cx, cy) = layout::CLOSE;
        self.fill_circle(cx, cy, layout::BUTTON_RADIUS, color(WHITE, if hover == Some(SearchItem::Close) { 0.16 } else { 0.08 } * alpha))?;
        self.draw_close_mark(cx, cy, 4.0, color(INK, 0.9 * alpha))?;

        // The songs, or what is happening.
        let message = match card.status {
            SearchStatus::Empty => Some("Find a song, an artist or an album"),
            SearchStatus::Searching if card.results.is_empty() => Some("Searching\u{2026}"),
            SearchStatus::NoResults => Some("Nothing found"),
            SearchStatus::Failed => Some("Spotify can't be reached right now"),
            _ => None,
        };
        if let Some(message) = message {
            let list = layout::row(0);
            let y = layout::LIST_Y + (layout::ROW_HEIGHT * ROWS as f32 - 20.0) * 0.45;
            return self
                .draw_text(message, FormatId::Button, list.x, y, list.width, 20.0, color(MUTED, 0.8 * alpha))
                .map(|_| ());
        }
        for (index, result) in card.results.iter().take(ROWS).enumerate() {
            let row = layout::row(index);
            let starting = card.starting == Some(index);
            if hover == Some(SearchItem::Result(index)) || starting {
                self.fill_round_rect(row.x, row.y + 2.0, row.width, row.height - 4.0, 12.0, color(WHITE, 0.07 * alpha))?;
            }
            let art = islet_core::geometry::Rect::new(
                layout::INSET,
                row.y + (row.height - layout::ART_SIZE) * 0.5,
                layout::ART_SIZE,
                layout::ART_SIZE,
            );
            self.draw_art(art, layout::ART_RADIUS, result.art.as_ref(), [70, 74, 84], alpha)?;
            let right = layout::INSET + 8.0 + 44.0;
            let text_w = row.right() - right - layout::TEXT_X + 8.0;
            self.draw_text(&result.title, FormatId::CompactTitle, layout::TEXT_X, row.y + 5.0, text_w, 17.0, color(if starting { GREEN } else { INK }, alpha))?;
            // "E" marks a song with explicit words, as Spotify does.
            let artist = if result.explicit { format!("E \u{b7} {}", result.artist) } else { result.artist.clone() };
            self.draw_text(&artist, FormatId::CompactSub, layout::TEXT_X, row.y + 23.0, text_w, 14.0, color(MUTED, 0.88 * alpha))?;
            let label = if starting { "Playing\u{2026}".to_owned() } else { duration(result.duration_ms) };
            self.draw_text(&label, FormatId::TaskWhen, row.right() - 8.0 - 70.0, row.y, 70.0, row.height, color(MUTED, 0.85 * alpha))?;
        }
        Ok(())
    }

    /// A magnifying glass, `scale` times its 14 px size, centred.
    pub(crate) unsafe fn draw_magnifier(&mut self, x: f32, y: f32, scale: f32, ink: D2D1_COLOR_F) -> RenderResult<()> {
        let r = 4.6 * scale;
        let (cx, cy) = (x - 1.2 * scale, y - 1.2 * scale);
        self.set_color(ink);
        let lens = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        self.dc.DrawEllipse(&lens, &self.brush, 1.6 * scale, None::<&ID2D1StrokeStyle>);
        let reach = r * std::f32::consts::FRAC_1_SQRT_2;
        self.dc.DrawLine(
            Vector2 { X: cx + reach + 0.4, Y: cy + reach + 0.4 },
            Vector2 { X: cx + reach + 4.0 * scale, Y: cy + reach + 4.0 * scale },
            &self.brush,
            1.9 * scale,
            None::<&ID2D1StrokeStyle>,
        );
        Ok(())
    }

    /// The music card's Spotify pill, round, at `rect`: green "connect"
    /// or a search glass.
    pub(crate) unsafe fn draw_spotify_pill(
        &mut self,
        rect: islet_core::geometry::Rect,
        pill: SpotifyPill,
        hovered: bool,
        alpha: f32,
    ) -> RenderResult<()> {
        let (cx, cy) = rect.center();
        let radius = rect.height * 0.5;
        match pill {
            SpotifyPill::Hidden => Ok(()),
            SpotifyPill::Search => {
                self.fill_circle(cx, cy, radius, color(WHITE, if hovered { 0.22 } else { 0.115 } * alpha))?;
                self.draw_magnifier(cx, cy, 1.15, color([240, 240, 242], alpha))
            }
            SpotifyPill::Connect => {
                // Spotify's green, with a plus: "add Spotify".
                self.fill_circle(cx, cy, radius, color(GREEN, if hovered { 0.34 } else { 0.22 } * alpha))?;
                let ink = color(GREEN, alpha);
                self.fill_round_rect(cx - 6.0, cy - 0.9, 12.0, 1.8, 0.9, ink)?;
                self.fill_round_rect(cx - 0.9, cy - 6.0, 1.8, 12.0, 0.9, ink)
            }
        }
    }
}
