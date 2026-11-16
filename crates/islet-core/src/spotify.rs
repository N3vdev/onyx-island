//! Spotify inside the island: the music card's Spotify pill, and the search
//! view it opens (type, see songs, click one to play it).

use crate::media::ArtImage;

/// The music card's Spotify pill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpotifyPill {
    /// Nothing: not connected and not playing Spotify, or a free account.
    #[default]
    Hidden,
    /// Spotify plays but Onyx is not connected: the pill connects.
    Connect,
    /// Connected with Premium: the pill opens search.
    Search,
}

/// One song in the results.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    /// `spotify:track:…`, what playing it asks for.
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    pub explicit: bool,
    /// The cover, once fetched.
    pub art: Option<ArtImage>,
}

/// Where the search is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchStatus {
    /// Nothing typed yet.
    #[default]
    Empty,
    Searching,
    Results,
    NoResults,
    /// Spotify could not be reached.
    Failed,
}

/// The search view as drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchCard {
    pub query: String,
    /// The field has the keyboard (its caret shows).
    pub typing: bool,
    pub status: SearchStatus,
    pub results: Vec<SearchResult>,
    /// The result just clicked, while Spotify starts it.
    pub starting: Option<usize>,
}

/// Something on the search view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchItem {
    Close,
    Field,
    Result(usize),
}

pub const WIDTH: f32 = crate::lyrics::WIDTH;
pub const ROWS: usize = 5;
pub const HEIGHT: f32 = layout::LIST_Y + ROWS as f32 * layout::ROW_HEIGHT + layout::INSET - 4.0;
pub const CORNER: f32 = crate::lyrics::CORNER;

/// Where things are on the card, in logical pixels from its top-left. The
/// header mirrors the lyrics view's: the same insets from the edges.
pub mod layout {
    use super::WIDTH;
    use crate::geometry::Rect;

    pub const INSET: f32 = crate::lyrics::layout::INSET;
    pub const BUTTON_RADIUS: f32 = crate::lyrics::layout::BUTTON_RADIUS;
    pub const HEADER_Y: f32 = crate::lyrics::layout::HEADER_Y;
    pub const CLOSE: (f32, f32) = crate::lyrics::layout::CLOSE;
    /// The search field, the header's height, up to the close button.
    pub const FIELD: Rect = Rect::new(INSET, HEADER_Y - 17.0, CLOSE.0 - BUTTON_RADIUS - 10.0 - INSET, 34.0);
    pub const LIST_Y: f32 = 58.0;
    pub const ROW_HEIGHT: f32 = 44.0;
    pub const ART_SIZE: f32 = 32.0;
    pub const ART_RADIUS: f32 = 6.0;
    pub const TEXT_X: f32 = INSET + ART_SIZE + 12.0;
    /// A row, its hover filling it to the card's insets.
    #[must_use]
    pub fn row(index: usize) -> Rect {
        Rect::new(INSET - 8.0, LIST_Y + index as f32 * ROW_HEIGHT, WIDTH - 2.0 * (INSET - 8.0), ROW_HEIGHT)
    }
}

impl SearchCard {
    /// What is under (`x`, `y`) on the card.
    #[must_use]
    pub fn item_at(&self, x: f32, y: f32) -> Option<SearchItem> {
        let (cx, cy) = layout::CLOSE;
        if (x - cx).powi(2) + (y - cy).powi(2) <= (layout::BUTTON_RADIUS + 3.0).powi(2) {
            return Some(SearchItem::Close);
        }
        if layout::FIELD.contains(x, y) {
            return Some(SearchItem::Field);
        }
        (0..self.results.len().min(ROWS))
            .find(|&index| layout::row(index).contains(x, y))
            .map(SearchItem::Result)
    }

    /// A typed character; returns whether the query changed.
    pub fn type_char(&mut self, c: char) -> bool {
        match c {
            // Backspace.
            '\u{8}' => self.query.pop().is_some(),
            c if c.is_control() => false,
            c if self.query.chars().count() < 80 => {
                self.query.push(c);
                true
            }
            _ => false,
        }
    }
}

/// `3:07` from milliseconds.
#[must_use]
pub fn duration(ms: u32) -> String {
    crate::media::format_clock(f64::from(ms) / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(title: &str) -> SearchResult {
        SearchResult {
            uri: format!("spotify:track:{title}"),
            title: title.into(),
            artist: "Artist".into(),
            duration_ms: 187_000,
            explicit: false,
            art: None,
        }
    }

    #[test]
    fn clicks_find_the_close_button_the_field_and_each_result() {
        let card = SearchCard { results: vec![result("a"), result("b")], status: SearchStatus::Results, ..SearchCard::default() };
        assert_eq!(card.item_at(layout::CLOSE.0, layout::CLOSE.1), Some(SearchItem::Close));
        let field = layout::FIELD;
        assert_eq!(card.item_at(field.x + 20.0, field.center().1), Some(SearchItem::Field));
        let (x, y) = layout::row(1).center();
        assert_eq!(card.item_at(x, y), Some(SearchItem::Result(1)));
        let (x, y) = layout::row(3).center();
        assert_eq!(card.item_at(x, y), None, "no fourth result");
        // Everything fits on the card.
        assert!(layout::row(ROWS - 1).bottom() <= HEIGHT);
        assert!(field.right() < layout::CLOSE.0 - layout::BUTTON_RADIUS);
    }

    #[test]
    fn typing_edits_the_query() {
        let mut card = SearchCard::default();
        for c in "lofi".chars() {
            assert!(card.type_char(c));
        }
        assert!(card.type_char('\u{8}'));
        assert!(!card.type_char('\r'), "Enter is not text");
        assert_eq!(card.query, "lof");
        assert_eq!(duration(187_000), "3:07");
    }
}
