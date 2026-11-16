//! The island's lyrics view, opened from the music card's lyrics button.
//!
//! Lyrics come as LRC text: lines stamped with the time they are sung
//! (`[01:02.50]Somewhere only we know`). The view follows the song: the
//! line being sung sits a little above the middle, bright, and the list
//! glides up as the next one starts. Where it is drawn comes from the
//! playback position alone, so the renderer can animate it with no timers.

use std::sync::Arc;

use crate::geometry::Rect;
use crate::media::{ArtImage, Timeline};

/// One sung line and when it starts, in seconds into the track.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricLine {
    pub time: f64,
    /// Empty for an instrumental break (drawn as a note).
    pub text: String,
}

/// What the view has to show.
#[derive(Clone, Debug, PartialEq)]
pub enum LyricsBody {
    /// Being looked up.
    Loading,
    /// Lines with times: the view follows the song.
    Synced(Arc<Vec<LyricLine>>),
    /// Words only, no times: scrolled by hand.
    Plain(Arc<Vec<String>>),
    /// The song has no words.
    Instrumental,
    /// Nobody has lyrics for this song.
    NotFound,
    /// The lookup failed (offline, or the lyrics service is down).
    Unavailable,
}

/// The lyrics view as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricsCard {
    pub title: String,
    pub artist: String,
    pub art: Option<ArtImage>,
    pub accent: Option<[u8; 3]>,
    pub playing: bool,
    pub timeline: Option<Timeline>,
    pub body: LyricsBody,
    /// Pixels scrolled by hand from where following the song puts the
    /// lines (synced), or from the top (plain). `None`: following.
    pub scroll: Option<f32>,
    /// Spotify search is available: the header shows its button.
    pub search: bool,
    /// Which skip buttons work (the others are dimmed and do nothing).
    pub skips: crate::media::Skips,
}

/// Something on the lyrics view under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LyricsItem {
    Close,
    /// Spotify search.
    Search,
    /// A synced line: clicking it plays from there.
    Line(usize),
    Previous,
    PlayPause,
    Next,
    /// The progress bar.
    Bar,
}

/// Narrower than the music card: lyric lines are short.
pub const WIDTH: f32 = 420.0;
/// Three lines (the one before, the one being sung, the one after)
/// between a header and a footer that mirror each other.
pub const HEIGHT: f32 = 190.0;
pub const CORNER: f32 = crate::island::MEDIA.corner;

/// Where things are on the card, from its top-left, in logical pixels. The
/// header (art, song, buttons) and the footer (times, progress) sit the
/// same distance in from the top and bottom edges and from the sides, so
/// the spacing follows the card's round corners at both ends.
pub mod layout {
    use super::{HEIGHT, WIDTH};
    use crate::geometry::Rect;

    /// In from the sides, and from the top and bottom, to the header and
    /// footer rows.
    pub const INSET: f32 = 18.0;
    pub const ROW_HEIGHT: f32 = 34.0;
    /// The middle of the header row, and of the footer row.
    pub const HEADER_Y: f32 = 14.0 + ROW_HEIGHT * 0.5;
    pub const FOOTER_Y: f32 = HEIGHT - HEADER_Y;

    pub const ART: Rect = Rect::new(INSET, 14.0, ROW_HEIGHT, ROW_HEIGHT);
    pub const ART_RADIUS: f32 = 9.0;
    pub const TEXT_X: f32 = INSET + ROW_HEIGHT + 10.0;
    pub const BUTTON_RADIUS: f32 = 13.0;
    /// Between the header's buttons, centre to centre.
    pub const BUTTON_STEP: f32 = 31.0;
    pub const CLOSE: (f32, f32) = (WIDTH - INSET - BUTTON_RADIUS - 2.0, HEADER_Y);
    pub const NEXT: (f32, f32) = (CLOSE.0 - BUTTON_STEP - 4.0, HEADER_Y);
    pub const PLAY: (f32, f32) = (NEXT.0 - BUTTON_STEP, HEADER_Y);
    pub const PREVIOUS: (f32, f32) = (PLAY.0 - BUTTON_STEP, HEADER_Y);
    /// Spotify search, apart from the playback buttons.
    pub const SEARCH: (f32, f32) = (PREVIOUS.0 - BUTTON_STEP - 4.0, HEADER_Y);
    pub const TITLE: Rect = Rect::new(TEXT_X, HEADER_Y - 17.0, SEARCH.0 - BUTTON_RADIUS - 8.0 - TEXT_X, 18.0);
    pub const ARTIST: Rect = Rect::new(TEXT_X, HEADER_Y + 1.0, TITLE.width, 15.0);
    /// Where the lines scroll: room for three.
    pub const LINES: Rect = Rect::new(INSET, 54.0, WIDTH - 2.0 * INSET - 14.0, 84.0);
    /// The line being sung sits in the middle.
    pub const ANCHOR: f32 = 0.5;
    /// The footer: the time played, the bar, the time left. The times line
    /// up under the art and the buttons.
    pub const ELAPSED: Rect = Rect::new(INSET, FOOTER_Y - 7.0, 36.0, 14.0);
    pub const REMAINING: Rect = Rect::new(WIDTH - INSET - 36.0, FOOTER_Y - 7.0, 36.0, 14.0);
    pub const BAR: Rect = Rect::new(TEXT_X, FOOTER_Y - 1.5, WIDTH - 2.0 * TEXT_X, 3.0);
    /// Slack above and below the thin bar for clicking.
    pub const BAR_SLOP: f32 = 8.0;
}

/// The view's text size, and how much of the lines' width a row may take
/// (a little is kept back: the estimate below is only an estimate).
const TEXT_SIZE: f32 = 15.0;
const ROW_WIDTH: f32 = layout::LINES.width * 0.95;
/// Height of one row of a line, and the gap below each line.
pub const ROW: f32 = 20.0;
pub const LINE_GAP: f32 = 8.0;
/// How long the list takes to glide to the next line.
const GLIDE: f64 = 0.45;
/// Lines light up a moment early: the sound reaches the ear a little after
/// the position Windows reports, and reading takes a moment too.
const LEAD: f64 = 0.15;

/// How many rows `text` takes (one, two or three), from the width of its
/// letters in the view's bold type.
#[must_use]
pub fn rows(text: &str) -> u8 {
    let width: f32 = text.chars().map(letter_width).sum::<f32>() * TEXT_SIZE;
    match width {
        w if w <= ROW_WIDTH => 1,
        // Words wrap whole, so a second row holds a little less.
        w if w <= ROW_WIDTH * 1.85 => 2,
        _ => 3,
    }
}

/// A letter's width in ems, near enough (Inter Bold).
fn letter_width(c: char) -> f32 {
    match c {
        ' ' => 0.26,
        'i' | 'j' | 'l' | '\'' | '\u{2019}' | '|' | '.' | ',' | ':' | ';' | '!' | 'I' => 0.29,
        'f' | 't' | 'r' | '(' | ')' | '"' | '-' => 0.4,
        'm' | 'w' => 0.86,
        'M' | 'W' => 0.92,
        'A'..='Z' => 0.7,
        '0'..='9' => 0.62,
        c if c.is_ascii() => 0.58,
        // Wide scripts (Chinese, Japanese, Korean) take a full em.
        c if ('\u{2E80}'..='\u{D7FF}').contains(&c) || ('\u{F900}'..='\u{FFEF}').contains(&c) => 1.0,
        _ => 0.6,
    }
}

/// A line's height on the card, gap included.
#[must_use]
pub fn line_height(text: &str) -> f32 {
    f32::from(rows(text)) * ROW + LINE_GAP
}

/// Parses LRC text. Lines without a time are dropped; a line stamped
/// twice (a chorus) appears at each time; the result is in time order.
#[must_use]
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some(end) = tag.find(']') else { break };
            if let Some(time) = parse_time(&tag[..end]) {
                times.push(time);
            }
            rest = &tag[end + 1..];
        }
        let words = clean_words(rest);
        for time in times {
            lines.push(LyricLine { time, text: words.clone() });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    // A break at the very start is just the intro: nothing to show.
    while lines.first().is_some_and(|line| line.text.is_empty()) {
        lines.remove(0);
    }
    lines
}

/// `mm:ss.xx` (or `mm:ss`) in seconds; `None` for other tags (`ar:`, `ti:`).
fn parse_time(tag: &str) -> Option<f64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().replace(',', ".").parse().ok()?;
    (seconds >= 0.0 && seconds < 60.0).then(|| f64::from(minutes) * 60.0 + seconds)
}

/// Word-timing tags (`<00:12.34>`) dropped, spaces tidied.
fn clean_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        match rest[start..].find('>') {
            Some(end) if parse_time(&rest[start + 1..start + end]).is_some() => rest = &rest[start + end + 1..],
            _ => {
                out.push('<');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Plain lyrics as lines, blank lines between verses kept as one.
#[must_use]
pub fn plain_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(line.to_owned());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// The line being sung at `position`, if one has started.
#[must_use]
pub fn current_line(lines: &[LyricLine], position: f64) -> Option<usize> {
    let position = position + LEAD;
    lines.partition_point(|line| line.time <= position).checked_sub(1)
}

/// What the lines' area shows this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The height down the list (all lines, top to bottom) at the top of the
    /// lines' area.
    pub top: f32,
    /// The line being sung.
    pub current: Option<usize>,
    /// How far the current line has lit up, 0..=1 (it glides in with the
    /// list).
    pub glow: f32,
}

/// Where each line starts down the list, and the list's height.
#[must_use]
pub fn tops<'a>(texts: impl Iterator<Item = &'a str>) -> (Vec<f32>, f32) {
    let mut tops = Vec::new();
    let mut y = 0.0;
    for text in texts {
        tops.push(y);
        y += line_height(text);
    }
    (tops, y)
}

impl LyricsCard {
    /// Position in the song now, if it has a timeline.
    #[must_use]
    pub fn position(&self, now: f64) -> Option<f64> {
        self.timeline.map(|timeline| timeline.position_at(now, self.playing))
    }

    /// The lines' starts down the list, and the list's height.
    #[must_use]
    pub fn tops(&self) -> (Vec<f32>, f32) {
        match &self.body {
            LyricsBody::Synced(lines) => tops(lines.iter().map(|line| line.text.as_str())),
            LyricsBody::Plain(lines) => tops(lines.iter().map(String::as_str)),
            _ => (Vec::new(), 0.0),
        }
    }

    /// Where the list is this frame.
    #[must_use]
    pub fn view(&self, now: f64) -> View {
        let (tops, total) = self.tops();
        let area = layout::LINES.height;
        match &self.body {
            LyricsBody::Synced(lines) if !lines.is_empty() => {
                let center = |index: usize| tops[index] + (line_height(&lines[index].text) - LINE_GAP) * 0.5;
                let anchor = area * layout::ANCHOR;
                let position = self.position(now).unwrap_or(0.0);
                let current = current_line(lines, position);
                let (follow, glow) = match current {
                    None => (center(0) - anchor, 0.0),
                    Some(index) => {
                        let since = position + LEAD - lines[index].time;
                        let t = ease((since / GLIDE).clamp(0.0, 1.0)) as f32;
                        let from = if index == 0 { center(0) } else { center(index - 1) };
                        (from + (center(index) - from) * t - anchor, t)
                    }
                };
                let top = match self.scroll {
                    Some(scroll) => (follow + scroll).clamp(center(0) - anchor, center(lines.len() - 1) - anchor),
                    None => follow,
                };
                View { top, current, glow }
            }
            LyricsBody::Plain(_) => {
                let most = (total - area + LINE_GAP).max(0.0);
                View { top: self.scroll.unwrap_or(0.0).clamp(0.0, most), current: None, glow: 0.0 }
            }
            _ => View { top: 0.0, current: None, glow: 0.0 },
        }
    }

    /// The scroll that keeps the list where it is now, moved by `by` pixels
    /// (positive: further down the song), within the list.
    #[must_use]
    pub fn scrolled(&self, by: f32, now: f64) -> Option<f32> {
        let (tops, total) = self.tops();
        match &self.body {
            LyricsBody::Synced(lines) if !lines.is_empty() => {
                let follow = Self { scroll: None, ..self.clone() }.view(now).top;
                let anchor = layout::LINES.height * layout::ANCHOR;
                let last = tops[lines.len() - 1] + (line_height(&lines[lines.len() - 1].text) - LINE_GAP) * 0.5;
                let top = (self.view(now).top + by).clamp(-anchor, last - anchor);
                Some(top - follow)
            }
            LyricsBody::Plain(_) => {
                let most = (total - layout::LINES.height + LINE_GAP).max(0.0);
                Some((self.view(now).top + by).clamp(0.0, most))
            }
            _ => None,
        }
    }

    /// Line `index` on the card this frame.
    #[must_use]
    pub fn line_rect(&self, index: usize, view: View) -> Option<Rect> {
        let text = match &self.body {
            LyricsBody::Synced(lines) => lines.get(index).map(|line| line.text.as_str()),
            LyricsBody::Plain(lines) => lines.get(index).map(String::as_str),
            _ => None,
        }?;
        let (tops, _) = self.tops();
        let y = layout::LINES.y + tops[index] - view.top;
        Some(Rect::new(layout::LINES.x, y, layout::LINES.width, line_height(text) - LINE_GAP))
    }

    /// What is under (`x`, `y`) on the card at `now`.
    #[must_use]
    pub fn item_at(&self, x: f32, y: f32, now: f64) -> Option<LyricsItem> {
        let near = |(cx, cy): (f32, f32)| (x - cx).powi(2) + (y - cy).powi(2) <= (layout::BUTTON_RADIUS + 3.0).powi(2);
        if near(layout::CLOSE) {
            return Some(LyricsItem::Close);
        }
        if self.search && near(layout::SEARCH) {
            return Some(LyricsItem::Search);
        }
        if self.timeline.is_some() {
            if near(layout::PLAY) {
                return Some(LyricsItem::PlayPause);
            }
            if near(layout::PREVIOUS) {
                return self.skips.previous.then_some(LyricsItem::Previous);
            }
            if near(layout::NEXT) {
                return self.skips.next.then_some(LyricsItem::Next);
            }
            if self.bar_fraction(x, y, false).is_some() {
                return Some(LyricsItem::Bar);
            }
        }
        if let LyricsBody::Synced(lines) = &self.body {
            if !layout::LINES.contains(x, y) {
                return None;
            }
            let view = self.view(now);
            return (0..lines.len()).find_map(|index| {
                let rect = self.line_rect(index, view)?;
                (y >= rect.y - LINE_GAP * 0.5 && y < rect.bottom() + LINE_GAP * 0.5 && !lines[index].text.is_empty())
                    .then_some(LyricsItem::Line(index))
            });
        }
        None
    }

    /// Where along the progress bar (`x`, `y`) is, 0..=1. While `dragging`,
    /// any height counts and the ends clamp.
    #[must_use]
    pub fn bar_fraction(&self, x: f32, y: f32, dragging: bool) -> Option<f32> {
        let bar = layout::BAR;
        let on = dragging
            || (x >= bar.x - 4.0 && x <= bar.right() + 4.0 && (y - (bar.y + bar.height * 0.5)).abs() <= layout::BAR_SLOP);
        (on && self.timeline.is_some_and(|timeline| timeline.duration > 0.0))
            .then(|| ((x - bar.x) / bar.width).clamp(0.0, 1.0))
    }

    /// When line `index` starts, for a click on it.
    #[must_use]
    pub fn line_time(&self, index: usize) -> Option<f64> {
        match &self.body {
            LyricsBody::Synced(lines) => lines.get(index).map(|line| line.time),
            _ => None,
        }
    }
}

fn ease(t: f64) -> f64 {
    // Ease out: quick to start moving, gentle to settle.
    1.0 - (1.0 - t).powi(3)
}

/// The song as asked of a lyrics service: the title without "(Official
/// Video)", "[Lyrics]", "- Remastered 2011" and the like, and the artist
/// without "- Topic" or "VEVO". A video titled "Artist - Song" is split.
#[must_use]
pub fn query(title: &str, artist: &str) -> (String, String) {
    let mut title = strip_extras(title);
    let mut artist = artist.trim().to_owned();
    for suffix in [" - Topic", "VEVO", " Official"] {
        if let Some(stripped) = artist.strip_suffix(suffix) {
            artist = stripped.trim().to_owned();
        }
    }
    // "Artist - Song", from the artist's own channel or from a channel
    // that is not a person's name. A player that names the artist (a song
    // titled "Love Story - Live") keeps the whole title.
    if let Some((left, right)) = title.split_once(" - ") {
        let (left, right) = (left.trim().to_owned(), right.trim().to_owned());
        let by_artist = !artist.is_empty() && left.to_lowercase().contains(&artist.to_lowercase());
        if artist.is_empty() || by_artist || !artist_looks_named(&artist) {
            artist = left;
            title = right;
        }
    }
    (title.trim().to_owned(), artist)
}

/// A channel name rather than a person's or a band's ("lofi girl", "cactus nev" stays, "Music" goes).
fn artist_looks_named(artist: &str) -> bool {
    let lower = artist.to_lowercase();
    !["music", "records", "official", "channel", "lyrics"].iter().any(|word| lower.contains(word))
}

/// The title without bracketed extras and "- Remastered" tails.
fn strip_extras(title: &str) -> String {
    const EXTRAS: [&str; 16] = [
        "official", "video", "audio", "lyric", "visualizer", "visualiser", "remaster", "live", "hd", "4k", "mv", "m/v",
        "explicit", "clean", "color coded", "prod",
    ];
    let mut out = String::new();
    let mut rest = title;
    while let Some(start) = rest.find(['(', '[']) {
        let close = if rest[start..].starts_with('(') { ')' } else { ']' };
        let Some(end) = rest[start..].find(close) else { break };
        let inside = rest[start + 1..start + end].to_lowercase();
        out.push_str(&rest[..start]);
        if !EXTRAS.iter().any(|extra| inside.contains(extra)) && !inside.starts_with("feat") && !inside.starts_with("ft") {
            out.push_str(&rest[start..=start + end]);
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    // "Song - Remastered 2011", "Song - Live at Wembley".
    if let Some((song, tail)) = out.rsplit_once(" - ") {
        let tail = tail.to_lowercase();
        if ["remaster", "live", "mono", "stereo", "radio edit", "version", "mix"].iter().any(|word| tail.contains(word)) {
            out = song.to_owned();
        }
    }
    // "Song | Movie Name" and "Song ft. Someone".
    if let Some((song, _)) = out.split_once(" | ") {
        out = song.to_owned();
    }
    for marker in [" ft. ", " feat. ", " ft ", " feat "] {
        if let Some(index) = out.to_lowercase().find(marker) {
            out.truncate(index);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONG: &str = "[ar:Keane]\n[00:12.50]Oh simple thing, where have you gone?\n[00:16.00]I'm getting old\n[00:20.25][01:20.25]And I need something to rely on\n[00:25.00]\n[00:30.00]So tell me <00:30.40>when <00:30.80>you're gonna let me in\n";

    fn card(body: LyricsBody, position: f64) -> LyricsCard {
        LyricsCard {
            title: "Somewhere Only We Know".into(),
            artist: "Keane".into(),
            art: None,
            accent: None,
            playing: true,
            timeline: Some(Timeline { position, duration: 237.0, sampled_at: 0.0, rate: 1.0 }),
            body,
            scroll: None,
            search: false,
            skips: Default::default(),
        }
    }

    #[test]
    fn lrc_lines_come_out_timed_in_order_with_repeats_and_breaks() {
        let lines = parse_lrc(SONG);
        let times: Vec<f64> = lines.iter().map(|line| line.time).collect();
        assert_eq!(times, vec![12.5, 16.0, 20.25, 25.0, 30.0, 80.25]);
        assert_eq!(lines[2].text, "And I need something to rely on");
        assert_eq!(lines[5].text, lines[2].text);
        assert!(lines[3].text.is_empty(), "a break");
        assert_eq!(lines[4].text, "So tell me when you're gonna let me in");
        // A leading break is dropped; a non-time tag is not a line.
        assert_eq!(parse_lrc("[00:01.00]\n[00:05.00]Hi").len(), 1);
        assert!(parse_lrc("[ti:Title]\n[length: 3:20]").is_empty());
    }

    #[test]
    fn the_line_being_sung_follows_the_position() {
        let lines = parse_lrc(SONG);
        assert_eq!(current_line(&lines, 5.0), None);
        assert_eq!(current_line(&lines, 12.5), Some(0));
        assert_eq!(current_line(&lines, 15.9), Some(1), "a moment early");
        assert_eq!(current_line(&lines, 21.0), Some(2));
        assert_eq!(current_line(&lines, 500.0), Some(5));
    }

    #[test]
    fn the_list_glides_to_each_line_and_settles_on_it() {
        let lines = Arc::new(parse_lrc(SONG));
        let view = |position: f64| card(LyricsBody::Synced(lines.clone()), position).view(0.0);
        let start = view(16.0 - LEAD);
        let mid = view(16.0 - LEAD + GLIDE * 0.5);
        let settled = view(16.0 - LEAD + GLIDE + 0.1);
        assert_eq!(start.current, Some(1));
        assert!(start.top < mid.top && mid.top < settled.top, "{start:?} {mid:?} {settled:?}");
        assert!(settled.glow > 0.999 && start.glow < 0.01);
        // Settled, the line sits at the anchor.
        let c = card(LyricsBody::Synced(lines.clone()), 18.0);
        let rect = c.line_rect(1, c.view(0.0)).unwrap();
        let anchor = layout::LINES.y + layout::LINES.height * layout::ANCHOR;
        assert!((rect.center().1 - anchor).abs() < 0.5, "{rect:?}");
    }

    #[test]
    fn a_click_on_a_line_finds_it_and_the_buttons_win_over_lines() {
        let c = card(LyricsBody::Synced(Arc::new(parse_lrc(SONG))), 18.0);
        let view = c.view(0.0);
        let rect = c.line_rect(2, view).unwrap();
        let (x, y) = rect.center();
        assert_eq!(c.item_at(x, y, 0.0), Some(LyricsItem::Line(2)));
        assert_eq!(c.line_time(2), Some(20.25));
        // The break is not a line to click.
        let gap = c.line_rect(3, view).unwrap();
        assert_ne!(c.item_at(gap.center().0, gap.center().1, 0.0), Some(LyricsItem::Line(3)));
        assert_eq!(c.item_at(layout::CLOSE.0, layout::CLOSE.1, 0.0), Some(LyricsItem::Close));
        assert_eq!(c.item_at(layout::PLAY.0, layout::PLAY.1, 0.0), Some(LyricsItem::PlayPause));
        assert_eq!(c.item_at(layout::PREVIOUS.0, layout::PREVIOUS.1, 0.0), Some(LyricsItem::Previous));
        assert_eq!(c.item_at(layout::NEXT.0, layout::NEXT.1, 0.0), Some(LyricsItem::Next));
        // A player without next (a radio stream): the button does nothing.
        let radio = LyricsCard { skips: crate::media::Skips { previous: true, next: false }, ..c.clone() };
        assert_eq!(radio.item_at(layout::NEXT.0, layout::NEXT.1, 0.0), None);
        assert_eq!(radio.item_at(layout::PREVIOUS.0, layout::PREVIOUS.1, 0.0), Some(LyricsItem::Previous));
        // Header and footer mirror each other.
        assert!((layout::HEADER_Y - (HEIGHT - layout::FOOTER_Y)).abs() < 0.01);
        // The buttons do not overlap one another.
        assert!(layout::NEXT.0 - layout::PLAY.0 >= 2.0 * layout::BUTTON_RADIUS);
        assert!(layout::CLOSE.0 - layout::NEXT.0 >= 2.0 * layout::BUTTON_RADIUS);
        let bar = layout::BAR;
        assert_eq!(c.item_at(bar.x + bar.width * 0.5, bar.y, 0.0), Some(LyricsItem::Bar));
        assert!((c.bar_fraction(bar.x + bar.width * 0.25, bar.y, false).unwrap() - 0.25).abs() < 1e-4);
    }

    #[test]
    fn scrolling_by_hand_moves_within_the_song_and_plain_lyrics_scroll_from_the_top() {
        let mut c = card(LyricsBody::Synced(Arc::new(parse_lrc(SONG))), 18.0);
        let before = c.view(0.0).top;
        c.scroll = c.scrolled(40.0, 0.0);
        assert!((c.view(0.0).top - before - 40.0).abs() < 0.01);
        c.scroll = c.scrolled(-10_000.0, 0.0);
        let top = c.view(0.0).top;
        assert!(top < before && top >= -layout::LINES.height);

        let words: Vec<String> = (0..30).map(|i| format!("Line {i}")).collect();
        let mut plain = card(LyricsBody::Plain(Arc::new(words)), 0.0);
        assert_eq!(plain.view(0.0).top, 0.0);
        plain.scroll = plain.scrolled(-50.0, 0.0);
        assert_eq!(plain.view(0.0).top, 0.0, "not above the first line");
        plain.scroll = plain.scrolled(100.0, 0.0);
        assert_eq!(plain.view(0.0).top, 100.0);
    }

    #[test]
    fn long_lines_take_more_rows() {
        assert_eq!(rows("I'm getting old"), 1);
        assert_eq!(rows("I'm getting old and I need something"), 1);
        assert_eq!(rows("I'm getting tired and I need somewhere to begin"), 1);
        assert_eq!(rows("So tell me when you're gonna let me in, I'm getting tired and I need somewhere to begin"), 2);
        assert_eq!(rows(&"la ".repeat(60)), 3);
    }

    #[test]
    fn plain_lyrics_keep_one_blank_between_verses() {
        assert_eq!(plain_lines("a\nb\n\n\n\nc\n\n"), vec!["a", "b", "", "c"]);
    }

    #[test]
    fn video_titles_are_tidied_into_a_song_and_its_artist() {
        assert_eq!(query("Somewhere Only We Know", "Keane"), ("Somewhere Only We Know".into(), "Keane".into()));
        assert_eq!(
            query("Keane - Somewhere Only We Know (Official Music Video)", "KeaneVEVO"),
            ("Somewhere Only We Know".into(), "Keane".into())
        );
        assert_eq!(query("Blinding Lights [Lyrics]", "The Weeknd - Topic"), ("Blinding Lights".into(), "The Weeknd".into()));
        assert_eq!(query("Here Comes The Sun - Remastered 2009", "The Beatles"), ("Here Comes The Sun".into(), "The Beatles".into()));
        assert_eq!(query("Kesariya | Brahmastra", "Pritam"), ("Kesariya".into(), "Pritam".into()));
        assert_eq!(query("Peaches (feat. Daniel Caesar)", "Justin Bieber"), ("Peaches".into(), "Justin Bieber".into()));
        assert_eq!(query("Peaches ft. Daniel Caesar", "Justin Bieber"), ("Peaches".into(), "Justin Bieber".into()));
        assert_eq!(query("Ed Sheeran - Perfect", "Lyrics Channel"), ("Perfect".into(), "Ed Sheeran".into()));
    }
}
