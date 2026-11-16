//! The lyrics view, and the music card's lyrics button.

use islet_core::geometry::Rect;
use islet_core::lyrics::{layout, LyricsBody, LyricsCard, LyricsItem, View, LINE_GAP};
use islet_core::widgets::WidgetHover;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT};
use windows_numerics::{Matrix3x2, Vector2};

use crate::{color, FormatId, Gfx, RenderResult, TransportIcon};

const WHITE: [u8; 3] = [255, 255, 255];
const INK: [u8; 3] = [244, 245, 250];
const MUTED: [u8; 3] = [173, 180, 199];
/// Lines not being sung.
const RESTING: f32 = 0.36;
/// The line being sung grows this much.
const GROW: f32 = 0.035;
/// Lines fade out over this far from the top and bottom of their area.
const FADE: f32 = 12.0;

impl Gfx {
    pub(crate) unsafe fn draw_lyrics(
        &mut self,
        now: f64,
        card: &LyricsCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        let hover = match self.widget_hover {
            Some(WidgetHover::Lyrics(item)) => Some(item),
            _ => None,
        };
        // A faint wash of the album's colour.
        self.fill_round_rect(
            0.0,
            0.0,
            islet_core::lyrics::WIDTH,
            islet_core::lyrics::HEIGHT,
            islet_core::lyrics::CORNER,
            color(accent, 0.05 * alpha),
        )?;

        // The song, and its two buttons.
        self.draw_art(layout::ART, layout::ART_RADIUS, card.art.as_ref(), accent, alpha)?;
        let title = layout::TITLE;
        self.draw_text(&card.title, FormatId::CompactTitle, title.x, title.y, title.width, title.height, color(INK, 0.96 * alpha))?;
        let artist = layout::ARTIST;
        self.draw_text(&card.artist, FormatId::CompactSub, artist.x, artist.y, artist.width, artist.height, color(MUTED, 0.88 * alpha))?;
        if card.timeline.is_some() {
            for (item, (x, y), icon, enabled) in [
                (LyricsItem::Previous, layout::PREVIOUS, TransportIcon::Previous, card.skips.previous),
                (LyricsItem::PlayPause, layout::PLAY, TransportIcon::PlayPause(card.playing), true),
                (LyricsItem::Next, layout::NEXT, TransportIcon::Next, card.skips.next),
            ] {
                if hover == Some(item) && enabled {
                    self.fill_circle(x, y, layout::BUTTON_RADIUS, color(WHITE, 0.14 * alpha))?;
                }
                // A skip the player does not offer is dimmed.
                self.draw_transport(x, y, icon, if enabled { 0.92 } else { crate::DISABLED } * alpha)?;
            }
        }
        if card.search {
            let (sx, sy) = layout::SEARCH;
            if hover == Some(LyricsItem::Search) {
                self.fill_circle(sx, sy, layout::BUTTON_RADIUS, color(WHITE, 0.14 * alpha))?;
            }
            self.draw_magnifier(sx, sy, 0.95, color([240, 240, 242], 0.92 * alpha))?;
        }
        let (cx, cy) = layout::CLOSE;
        self.fill_circle(cx, cy, layout::BUTTON_RADIUS, color(WHITE, if hover == Some(LyricsItem::Close) { 0.16 } else { 0.08 } * alpha))?;
        self.draw_close_mark(cx, cy, 4.0, color(INK, 0.9 * alpha))?;

        // The words.
        match &card.body {
            LyricsBody::Synced(_) | LyricsBody::Plain(_) => self.draw_lyric_lines(now, card, hover, alpha)?,
            body => {
                let message = match body {
                    LyricsBody::Loading => "Finding the lyrics\u{2026}",
                    LyricsBody::Instrumental => "\u{266a}  Instrumental",
                    LyricsBody::NotFound => "No lyrics for this song",
                    _ => "Lyrics aren't available right now",
                };
                let area = layout::LINES;
                self.draw_text(message, FormatId::Lyric, area.x, area.y + (area.height - islet_core::lyrics::ROW) * 0.5, area.width, islet_core::lyrics::ROW + 4.0, color(INK, 0.55 * alpha))?;
            }
        }

        // The footer: time played, progress, time left.
        if let Some(timeline) = card.timeline.filter(|timeline| timeline.duration > 0.0) {
            let position = timeline.position_at(now, card.playing);
            let progress = (position / timeline.duration).clamp(0.0, 1.0) as f32;
            let lit = hover == Some(LyricsItem::Bar);
            let bar = layout::BAR;
            let bar = if lit { Rect::new(bar.x, bar.y - 0.5, bar.width, bar.height + 1.0) } else { bar };
            self.fill_round_rect(bar.x, bar.y, bar.width, bar.height, bar.height * 0.5, color(WHITE, 0.14 * alpha))?;
            self.fill_round_rect(bar.x, bar.y, (bar.width * progress).max(bar.height), bar.height, bar.height * 0.5, color(INK, 0.8 * alpha))?;
            if lit {
                self.fill_circle(bar.x + bar.width * progress, bar.y + bar.height * 0.5, 4.5, color(INK, alpha))?;
            }
            let (elapsed, remaining) = (layout::ELAPSED, layout::REMAINING);
            let left = format!("-{}", islet_core::media::format_clock((timeline.duration - position).max(0.0)));
            let label = color(MUTED, 0.85 * alpha);
            self.draw_text(&islet_core::media::format_clock(position), FormatId::Tiny, elapsed.x, elapsed.y, elapsed.width, elapsed.height, label)?;
            self.draw_text(&left, FormatId::TinyRight, remaining.x, remaining.y, remaining.width, remaining.height, label)?;
        }
        Ok(())
    }

    /// The lines: the one being sung bright and a touch larger, the rest
    /// quiet, fading out toward the top and bottom of their area.
    unsafe fn draw_lyric_lines(&mut self, now: f64, card: &LyricsCard, hover: Option<LyricsItem>, alpha: f32) -> RenderResult<()> {
        let view: View = card.view(now);
        let area = layout::LINES;
        let texts: Vec<&str> = match &card.body {
            LyricsBody::Synced(lines) => lines.iter().map(|line| line.text.as_str()).collect(),
            LyricsBody::Plain(lines) => lines.iter().map(String::as_str).collect(),
            _ => return Ok(()),
        };
        let synced = matches!(card.body, LyricsBody::Synced(_));
        self.dc.PushAxisAlignedClip(
            &D2D_RECT_F { left: 0.0, top: area.y, right: islet_core::lyrics::WIDTH, bottom: area.bottom() },
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
        let base = self.dc_transform();
        let mut result = Ok(());
        for (index, text) in texts.iter().enumerate() {
            let Some(rect) = card.line_rect(index, view) else { continue };
            if rect.bottom() < area.y - 4.0 || rect.y > area.bottom() + 4.0 {
                continue;
            }
            // How lit: the current line rises as the list glides to it;
            // the one before it dims as it leaves.
            let lit = match (synced, view.current) {
                (false, _) => 0.82,
                (true, Some(current)) if index == current => RESTING + (1.0 - RESTING) * view.glow,
                (true, Some(current)) if index + 1 == current => 1.0 - (1.0 - RESTING) * view.glow,
                _ => RESTING,
            };
            let hovered = hover == Some(LyricsItem::Line(index));
            let lit = if hovered { lit.max(0.7) } else { lit };
            let center = rect.y + rect.height * 0.5;
            // Only where there is more beyond: the first lines of plain
            // lyrics, unscrolled, stay clear.
            let above = if view.top > 0.5 { center - area.y } else { f32::MAX };
            let edge = above.min(area.bottom() - center);
            let fade = (edge / FADE).clamp(0.0, 1.0);
            let fade = fade * fade * (3.0 - 2.0 * fade);
            let ink = color(INK, lit * fade * alpha);
            if ink.a <= 0.003 {
                continue;
            }
            let grow = if synced && view.current == Some(index) { 1.0 + GROW * view.glow } else { 1.0 };
            let word = if text.is_empty() { "\u{266a}" } else { text };
            if grow > 1.0 {
                // About the line's left middle, so it grows in place.
                let pivot = Vector2 { X: rect.x, Y: center };
                let scale = Matrix3x2 {
                    M11: grow,
                    M12: 0.0,
                    M21: 0.0,
                    M22: grow,
                    M31: pivot.X * (1.0 - grow),
                    M32: pivot.Y * (1.0 - grow),
                };
                self.dc.SetTransform(&(scale * base));
            }
            // The same width either way, so a line never rewraps as it
            // grows (it reaches a little into the margin).
            result = self.draw_text(word, FormatId::Lyric, rect.x, rect.y, rect.width, rect.height + LINE_GAP, ink);
            self.dc.SetTransform(&base);
            if result.is_err() {
                break;
            }
        }
        self.dc.PopAxisAlignedClip();
        result
    }

    unsafe fn dc_transform(&self) -> Matrix3x2 {
        let mut transform = Matrix3x2::default();
        self.dc.GetTransform(&mut transform);
        transform
    }

    /// An ×, `half` from its centre to each tip.
    pub(crate) unsafe fn draw_close_mark(&mut self, x: f32, y: f32, half: f32, ink: D2D1_COLOR_F) -> RenderResult<()> {
        self.set_color(ink);
        let stroke = None::<&ID2D1StrokeStyle>;
        self.dc.DrawLine(Vector2 { X: x - half, Y: y - half }, Vector2 { X: x + half, Y: y + half }, &self.brush, 1.6, stroke);
        self.dc.DrawLine(Vector2 { X: x + half, Y: y - half }, Vector2 { X: x - half, Y: y + half }, &self.brush, 1.6, stroke);
        Ok(())
    }

    /// The music card's lyrics button: a speech bubble with two lines of
    /// words in it.
    pub(crate) unsafe fn draw_lyrics_button(&mut self, x: f32, y: f32, hovered: bool, alpha: f32) -> RenderResult<()> {
        if hovered {
            self.fill_circle(x, y, islet_core::media::expanded::CONTROL_HIT_RADIUS, color(WHITE, 0.14 * alpha))?;
        }
        let ink = color([240, 240, 242], alpha);
        let (w, h) = (15.0, 11.5);
        let (left, top) = (x - w * 0.5, y - h * 0.5 - 1.0);
        let bubble = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left, top, right: left + w, bottom: top + h },
            radiusX: 3.5,
            radiusY: 3.5,
        };
        self.set_color(ink);
        self.dc.DrawRoundedRectangle(&bubble, &self.brush, 1.4, None::<&ID2D1StrokeStyle>);
        // The tail, under the bubble's left.
        self.fill_polygon(
            &[
                Vector2 { X: left + 3.0, Y: top + h - 0.6 },
                Vector2 { X: left + 7.5, Y: top + h - 0.6 },
                Vector2 { X: left + 2.6, Y: top + h + 3.8 },
            ],
            ink,
        )?;
        self.fill_round_rect(left + 3.5, top + 3.4, 8.0, 1.5, 0.75, ink)?;
        self.fill_round_rect(left + 3.5, top + 6.6, 5.5, 1.5, 0.75, ink)?;
        Ok(())
    }
}

/// Where the lines are this frame, for the frame cache: the list's height
/// (to half a pixel), which line is lit and how far.
#[must_use]
pub(crate) fn lyrics_key(card: &LyricsCard, now: f64) -> (i64, Option<usize>, i64, i64, bool) {
    let view = card.view(now);
    let progress = card
        .timeline
        .filter(|timeline| timeline.duration > 0.0)
        .map_or(0, |timeline| {
            let position = timeline.position_at(now, card.playing);
            // The bar moves a pixel at a time; the clock a second.
            ((position / timeline.duration) * f64::from(layout::BAR.width) * 2.0) as i64 * 1_000_000 + position as i64
        });
    ((view.top * 2.0).round() as i64, view.current, (view.glow * 64.0).round() as i64, progress, card.playing)
}
