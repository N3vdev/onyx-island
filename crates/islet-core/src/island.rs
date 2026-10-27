//! The island's platform-free state machine, size table, and content model.
//!
//! The UI thread owns an [`IslandState`], feeds it events, and sends the
//! resulting [`LayoutTarget`] and [`ContentSnapshot`] to the render thread.
//! Timers are described as [`Effect`]s rather than run here, which keeps the
//! reducer deterministic and easy to test.

use crate::config::{ExpandTrigger, IdleGlance, IdleStyle};
use crate::geometry::Rect;
use crate::media::{ArtImage, Timeline};

pub const HOVER_EXPAND_MS: u32 = 250;
pub const HOVER_COLLAPSE_MS: u32 = 400;
pub const DONE_VISIBLE_MS: u32 = 4_000;
/// Plain information ("Stopped") is brief; it is not news worth four seconds.
pub const INFO_VISIBLE_MS: u32 = 2_500;

/// How long a transient notice stays on the island.
#[must_use]
pub const fn visible_ms(level: AlertLevel) -> u32 {
    match level {
        AlertLevel::Info => INFO_VISIBLE_MS,
        _ => DONE_VISIBLE_MS,
    }
}
/// Default delay before paused media closes (`media.pause_hide_seconds`).
pub const MEDIA_PAUSED_MS: u32 = 5 * 1_000;
/// Keep the track title and artist visible this long before shrinking to art and bars.
pub const MEDIA_CONDENSE_MS: u32 = 5_000;
pub const SPLIT_GAP_DP: f32 = 6.0;
/// Gap between the island and the screen edge (work area), in DIPs.
pub const EDGE_GAP_DP: f32 = 8.0;

/// Sizes from PLAN.md §4.4, in logical pixels (DIPs).
pub const COMPACT: PillSize = PillSize::new(220.0, 36.0, 18.0);
pub const SPLIT_BUBBLE: PillSize = PillSize::new(36.0, 36.0, 18.0);
/// The second player's bubble beside the expanded player: a little larger, so
/// it holds its own next to the big card and is easy to click.
pub const SWITCH_BUBBLE: PillSize = PillSize::new(44.0, 44.0, 22.0);
pub const NOTIFICATION: PillSize = PillSize::new(380.0, 76.0, 30.0);
pub const MEDIA: PillSize = PillSize::new(496.0, 120.0, 38.0);
pub const CALCULATOR: PillSize = PillSize::new(
    crate::widgets::CALCULATOR_WIDTH,
    crate::widgets::CALCULATOR_HEIGHT,
    38.0,
);
/// The always-on glance pills: the time, or CPU and memory.
pub const GLANCE_CLOCK: PillSize = PillSize::new(108.0, 32.0, 16.0);
pub const GLANCE_STATS: PillSize = PillSize::new(164.0, 32.0, 16.0);
pub const STATS: PillSize = PillSize::new(
    crate::widgets::STATS_WIDTH,
    crate::widgets::STATS_HEIGHT,
    38.0,
);
/// Small and sleek: just six bars (see the renderer), no text or timer.
pub const DICTATION: PillSize = PillSize::new(92.0, 32.0, 16.0);
/// Bar colour of a music pill whose art gave no colour.
pub const DEFAULT_MEDIA_ACCENT: [u8; 3] = [92, 132, 255];
/// Dictation bar colour when no music has played yet.
pub const DICTATION_FALLBACK_ACCENT: [u8; 3] = [255, 255, 255];
/// The shrunken music pill: the dictation pill's height, a little wider so the
/// art and the bars sit apart with clear room between them.
pub const MEDIA_CONDENSED: PillSize = PillSize::new(108.0, 32.0, 16.0);
pub const SLIVER: PillSize = PillSize::new(120.0, 6.0, 3.0);
pub const HIDDEN: PillSize = PillSize::new(0.0, 0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillSize {
    pub width: f32,
    pub height: f32,
    pub corner: f32,
}

impl PillSize {
    #[must_use]
    pub fn scaled(self, scale: f32) -> Self {
        Self::new(self.width * scale, self.height * scale, self.corner * scale)
    }
    #[must_use]
    pub const fn new(width: f32, height: f32, corner: f32) -> Self {
        Self {
            width,
            height,
            corner,
        }
    }
}

/// The geometry the render thread should spring toward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutTarget {
    /// Uniform scale for the expanded card's contents and associated bubble.
    pub content_scale: f32,
    pub main: PillSize,
    pub bubble: Option<PillSize>,
    pub gap: f32,
    /// Distance from the screen edge. Springs to 0 when tucked.
    pub inset: f32,
    pub visible: bool,
}

impl LayoutTarget {
    pub const HIDDEN: Self = Self {
        content_scale: 1.0,
        main: HIDDEN,
        bubble: None,
        gap: 0.0,
        inset: EDGE_GAP_DP,
        visible: false,
    };

    #[must_use]
    pub const fn single(size: PillSize) -> Self {
        Self {
            content_scale: 1.0,
            main: size,
            bubble: None,
            gap: 0.0,
            inset: EDGE_GAP_DP,
            visible: true,
        }
    }

    #[must_use]
    pub const fn split(main: PillSize, bubble: PillSize) -> Self {
        Self {
            content_scale: 1.0,
            main,
            bubble: Some(bubble),
            gap: SPLIT_GAP_DP,
            inset: EDGE_GAP_DP,
            visible: true,
        }
    }

    #[must_use]
    pub fn idle(style: IdleStyle) -> Self {
        match style {
            IdleStyle::Hidden => Self::HIDDEN,
            IdleStyle::Sliver => Self::single(SLIVER),
        }
    }

    #[must_use]
    pub fn is_split(self) -> bool {
        self.bubble.is_some()
    }

    /// Scale an expanded layout proportionally; compact layouts stay unchanged.
    pub fn customized(
        mut self,
        expanded_scale: f32,
        margin_percent: f32,
        screen_height: f32,
    ) -> Self {
        if self.main.height > 60.0 {
            let scale = expanded_scale.clamp(0.8, 1.25);
            self.main = self.main.scaled(scale);
            self.bubble = self.bubble.map(|size| size.scaled(scale));
            self.gap *= scale;
            self.content_scale = scale;
        }
        self.inset += screen_height.max(0.0) * margin_percent.clamp(0.0, 5.0) / 100.0;
        self
    }
}

/// Pill rectangles placed inside the island window, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillFrame {
    pub main: Rect,
    pub main_corner: f32,
    pub bubble: Option<Rect>,
    pub bubble_corner: f32,
}

impl PillFrame {
    /// The smallest rectangle containing every visible pill.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        let main = (!self.main.is_empty()).then_some(self.main);
        match (main, self.bubble) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Places the main pill and optional bubble in a `window_width` x
/// `window_height` window. The group is centred horizontally and sits
/// `inset` away from the edge nearest the screen edge; the bubble sits to
/// the right, centred vertically on the main pill. Sizes below half a pixel
/// are treated as gone.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn arrange(
    main: PillSize,
    bubble: Option<PillSize>,
    gap: f32,
    inset: f32,
    window_width: f32,
    window_height: f32,
    anchor_top: bool,
) -> PillFrame {
    arrange_at(
        main,
        bubble,
        gap,
        inset,
        window_width,
        window_height,
        if anchor_top { 0.0 } else { 1.0 },
    )
}

/// Like [`arrange`], but with a continuous edge anchor: `0` is the top edge
/// and `1` is the bottom edge. The renderer animates this so the island can
/// glide between edges instead of teleporting.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn arrange_at(
    main: PillSize,
    bubble: Option<PillSize>,
    gap: f32,
    inset: f32,
    window_width: f32,
    window_height: f32,
    anchor: f32,
) -> PillFrame {
    let main_w = main.width.max(0.0);
    let main_h = main.height.max(0.0);
    let bubble = bubble.filter(|bubble| bubble.width > 0.5);
    let (bubble_w, bubble_h, gap) = bubble.map_or((0.0, 0.0, 0.0), |bubble| {
        (bubble.width, bubble.height.max(0.0), gap.max(0.0))
    });
    let group_w = main_w
        + if bubble.is_some() {
            gap + bubble_w
        } else {
            0.0
        };
    let main_x = (window_width - group_w) * 0.5;
    let inset = inset.max(0.0);
    let top_y = inset;
    let bottom_y = window_height - main_h - inset;
    let anchor = anchor.clamp(0.0, 1.0);
    let main_y = top_y + (bottom_y - top_y) * anchor;
    PillFrame {
        main: Rect::new(main_x, main_y, main_w, main_h),
        main_corner: main.corner.max(0.0),
        bubble: bubble.map(|_| {
            Rect::new(
                main_x + main_w + gap,
                main_y + (main_h - bubble_h) * 0.5,
                bubble_w,
                bubble_h,
            )
        }),
        bubble_corner: bubble.map_or(0.0, |bubble| bubble.corner.max(0.0)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertLevel {
    Info,
    Success,
    NeedsInput,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Running,
    NeedsInput,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationPhase {
    /// The speech model is still loading ("warming up").
    Loading,
    Recording,
    Transcribing,
}

/// Bars in the live waveform; the newest is last.
pub const DICTATION_BARS: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub struct MediaCard {
    pub title: String,
    pub artist: String,
    /// Short app name, e.g. "Spotify".
    pub source: String,
    pub playing: bool,
    pub timeline: Option<Timeline>,
    pub art: Option<ArtImage>,
    /// Dominant album-art colour, when tinting is enabled and the art has one.
    pub accent: Option<[u8; 3]>,
    /// Scrubber position while the user drags it, 0..=1.
    pub scrub: Option<f32>,
    /// System output volume, 0..=1, when it can be read.
    pub volume: Option<f32>,
}

impl MediaCard {
    /// Same track, so an update should not run a content transition.
    #[must_use]
    pub fn same_track(&self, other: &Self) -> bool {
        self.title == other.title && self.artist == other.artist && self.source == other.source
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotificationCard {
    pub source: String,
    pub title: String,
    pub body: String,
    pub level: AlertLevel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCard {
    pub project: String,
    pub activity: String,
    pub state: AgentState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DictationCard {
    pub elapsed_ms: u32,
    /// Current microphone level, 0..=1.
    pub level: f32,
    pub phase: DictationPhase,
    /// Recent levels (0..=255) for the waveform, oldest first.
    pub bars: [u8; DICTATION_BARS],
    /// Locked on with the hands-free hotkey; shown as a ring around the dot.
    pub hands_free: bool,
}

impl Default for DictationCard {
    fn default() -> Self {
        Self {
            elapsed_ms: 0,
            level: 0.0,
            phase: DictationPhase::Recording,
            bars: [0; DICTATION_BARS],
            hands_free: false,
        }
    }
}

/// What the always-on island shows when nothing else is going on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlanceCard {
    /// `Clock` or `Stats` (resume-or-clock shows the clock here).
    pub kind: IdleGlance,
    /// "9:41 PM", in the user's own time format.
    pub time: String,
    /// "Tuesday, 27 October 2026".
    pub long_date: String,
    /// 0..1.
    pub cpu: f32,
    pub memory: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentCard {
    Glance(GlanceCard),
    Calculator(crate::widgets::Calculator),
    Stats(crate::widgets::SystemStats),
    Media(MediaCard),
    Notification(NotificationCard),
    Agent(AgentCard),
    Dictation(DictationCard),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Glance,
    Calculator,
    Stats,
    Media,
    Notification,
    Agent,
    Dictation,
}

impl ContentCard {
    #[must_use]
    pub fn kind(&self) -> CardKind {
        match self {
            Self::Glance(_) => CardKind::Glance,
            Self::Calculator(_) => CardKind::Calculator,
            Self::Stats(_) => CardKind::Stats,
            Self::Media(_) => CardKind::Media,
            Self::Notification(_) => CardKind::Notification,
            Self::Agent(_) => CardKind::Agent,
            Self::Dictation(_) => CardKind::Dictation,
        }
    }

    /// Whether `next` is this same item updating in place (new timeline,
    /// level, or play state), which should not run a content morph.
    #[must_use]
    pub fn updates_in_place(&self, next: &Self) -> bool {
        match (self, next) {
            (Self::Calculator(_), Self::Calculator(_)) => true,
            (Self::Glance(a), Self::Glance(b)) => a.kind == b.kind,
            (Self::Stats(_), Self::Stats(_)) => true,
            (Self::Media(a), Self::Media(b)) => a.same_track(b) && a.art == b.art,
            (Self::Dictation(a), Self::Dictation(b)) => a.phase == b.phase,
            (a, b) => a == b,
        }
    }

    #[must_use]
    pub fn animates(&self) -> bool {
        match self {
            Self::Media(media) => media.playing,
            Self::Dictation(_) => true,
            Self::Notification(_)
            | Self::Agent(_)
            | Self::Calculator(_)
            | Self::Stats(_)
            | Self::Glance(_) => false,
        }
    }
}

/// Stable content sent from the UI thread to the render thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentSnapshot {
    pub widget_navigation: bool,
    /// Pages the wheel steps through, and the one shown (for the dots).
    pub widget_pages: u8,
    pub widget_page: u8,
    /// The CPU has been pegged for a few seconds: the pill edge glows.
    pub cpu_hot: bool,
    pub primary: Option<ContentCard>,
    pub secondary: Option<ContentCard>,
    pub accent: [u8; 3],
}

impl ContentSnapshot {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primary.is_none() && self.secondary.is_none()
    }

    /// Whether `next` only updates the same items in the same slots.
    #[must_use]
    pub fn updates_in_place(&self, next: &Self) -> bool {
        let same = |a: &Option<ContentCard>, b: &Option<ContentCard>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.updates_in_place(b),
            _ => false,
        };
        same(&self.primary, &next.primary) && same(&self.secondary, &next.secondary)
    }

    #[must_use]
    pub fn animates(&self) -> bool {
        self.primary.as_ref().is_some_and(ContentCard::animates)
            || self.secondary.as_ref().is_some_and(ContentCard::animates)
    }
}

/// A page the opened island can show instead of the music player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    /// The running agent, while there is one.
    Agent,
    Calculator,
    Stats,
}

/// The opened island's player when nothing has ever played.
fn idle_player() -> MediaCard {
    MediaCard {
        title: "Nothing playing".to_owned(),
        artist: "Play music in any app to see it here".to_owned(),
        source: String::new(),
        playing: false,
        timeline: None,
        art: None,
        accent: None,
        scrub: None,
        volume: None,
    }
}

#[derive(Clone, Debug)]
pub struct IslandState {
    pub calculator: Option<crate::widgets::Calculator>,
    /// Live system readings while the stats widget is enabled.
    pub stats: Option<crate::widgets::SystemStats>,
    /// The page the user scrolled to; `None` is the music player, which
    /// the opened island always starts on. Kept across collapse.
    pub widget: Option<WidgetKind>,
    pub cpu_hot: bool,
    /// The last song played (paused), kept across restarts. Opening the
    /// island with no player open shows it first; the pill never does.
    pub remembered: Option<MediaCard>,
    /// The island never hides: this is shown when nothing else is.
    pub glance: Option<IdleGlance>,
    /// The time and readings the glance shows (`kind` is set when drawn).
    pub glance_info: GlanceCard,
    pub dictation: Option<DictationCard>,
    pub needs_input: Option<NotificationCard>,
    pub done: Option<NotificationCard>,
    pub running: Option<AgentCard>,
    /// The player shown in the main pill.
    pub media: Option<MediaCard>,
    /// A second player, playing at the same time. It waits in the round
    /// bubble beside the pill until it is clicked.
    pub other_media: Option<MediaCard>,
    /// The paused track was closed after the pause delay; ignore paused
    /// updates until something plays again.
    pub media_dismissed: bool,
    /// The main music pill has played long enough to show only art and bars.
    pub media_condensed: bool,
    /// The bar colour of the most recent music pill. The dictation pill
    /// borrows it so the island keeps one colour while you talk.
    pub last_music_accent: Option<[u8; 3]>,
    /// How long paused media stays before closing.
    pub media_paused_ms: u32,
    pub idle_style: IdleStyle,
    /// Hovering opens the island, or only clicking does.
    pub expand_on: ExpandTrigger,
    /// An app window is behind the island: dock at the bottom edge.
    pub tucked: bool,
    pub expanded: bool,
    pub pointer_inside: bool,
    /// The pointer rests on the second player's bubble. It is a button, so
    /// hovering it never expands the island.
    pub pointer_on_switcher: bool,
    pub done_deadline: Option<f64>,
}

impl Default for IslandState {
    fn default() -> Self {
        Self {
            calculator: None,
            stats: None,
            widget: None,
            cpu_hot: false,
            remembered: None,
            glance: None,
            glance_info: GlanceCard::default(),
            dictation: None,
            needs_input: None,
            done: None,
            running: None,
            media: None,
            other_media: None,
            media_dismissed: false,
            media_condensed: false,
            last_music_accent: None,
            media_paused_ms: MEDIA_PAUSED_MS,
            idle_style: IdleStyle::Hidden,
            expand_on: ExpandTrigger::Hover,
            tucked: false,
            expanded: false,
            pointer_inside: false,
            pointer_on_switcher: false,
            done_deadline: None,
        }
    }
}

impl IslandState {
    /// Urgent notices and dictation keep priority over optional widgets.
    pub fn widget_navigation_available(&self) -> bool {
        self.expanded
            && self.dictation.is_none()
            && self.needs_input.is_none()
            && self.done.is_none()
            && self.pages().len() >= 2
    }

    fn widget_enabled(&self, kind: WidgetKind) -> bool {
        match kind {
            WidgetKind::Agent => self.running.is_some(),
            WidgetKind::Calculator => self.calculator.is_some(),
            WidgetKind::Stats => self.stats.is_some(),
        }
    }

    /// What the wheel steps through: the music player, always first, then
    /// the running agent and each enabled widget.
    fn pages(&self) -> Vec<Option<WidgetKind>> {
        let mut pages = Vec::with_capacity(4);
        pages.push(None);
        for kind in [WidgetKind::Agent, WidgetKind::Calculator, WidgetKind::Stats] {
            if self.widget_enabled(kind) {
                pages.push(Some(kind));
            }
        }
        pages
    }

    /// The page on screen when opened: the chosen one, else the player.
    fn current_page(&self) -> Option<WidgetKind> {
        self.widget.filter(|&kind| self.widget_enabled(kind))
    }

    /// The opened island's player: the live one, else the last song
    /// played, else an empty player.
    fn player_card(&self) -> MediaCard {
        self.media
            .clone()
            .or_else(|| self.other_media.clone())
            .or_else(|| self.remembered.clone())
            .unwrap_or_else(idle_player)
    }

    fn widget_card(&self, kind: WidgetKind) -> Option<ContentCard> {
        match kind {
            WidgetKind::Agent => self.running.clone().map(ContentCard::Agent),
            WidgetKind::Calculator => self.calculator.clone().map(ContentCard::Calculator),
            WidgetKind::Stats => self.stats.clone().map(ContentCard::Stats),
        }
    }

    /// The stats page is on screen, so readings should be taken.
    #[must_use]
    pub fn stats_visible(&self) -> bool {
        self.primary_kind() == Some(CardKind::Stats)
    }

    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.dictation.is_some()
            || self.needs_input.is_some()
            || self.done.is_some()
            || self.running.is_some()
            || self.media.is_some()
            || self.other_media.is_some()
            || self.calculator.is_some()
            || self.stats.is_some()
            || self.glance.is_some()
    }

    fn glance_card(&self) -> Option<ContentCard> {
        Some(ContentCard::Glance(GlanceCard {
            kind: self.glance?,
            ..self.glance_info.clone()
        }))
    }

    /// Resting the pointer here should open the island: hover mode, and not
    /// on the second player's bubble (a button).
    fn hover_expands(&self) -> bool {
        self.expand_on == ExpandTrigger::Hover && !self.pointer_on_switcher
    }

    #[must_use]
    pub fn persistent_count(&self) -> usize {
        usize::from(self.needs_input.is_some())
            + usize::from(self.done.is_some())
            + usize::from(self.running.is_some())
            + usize::from(self.media.is_some())
            + usize::from(self.other_media.is_some())
    }

    #[must_use]
    pub fn primary_card(&self) -> Option<ContentCard> {
        if let Some(dictation) = &self.dictation {
            return Some(ContentCard::Dictation(dictation.clone()));
        }
        if let Some(notification) = &self.needs_input {
            return Some(ContentCard::Notification(notification.clone()));
        }
        if let Some(notification) = &self.done {
            return Some(ContentCard::Notification(notification.clone()));
        }
        if self.expanded {
            // Opened: the music player, unless another page was chosen.
            return match self.current_page() {
                Some(kind) => self.widget_card(kind),
                None => Some(ContentCard::Media(self.player_card())),
            };
        }
        if let Some(agent) = &self.running {
            return Some(ContentCard::Agent(agent.clone()));
        }
        self.media
            .clone()
            .or_else(|| self.other_media.clone())
            .map(ContentCard::Media)
            .or_else(|| self.glance_card())
    }

    fn primary_kind(&self) -> Option<CardKind> {
        self.primary_card().map(|card| card.kind())
    }

    fn slot_card(&self, slot: Slot) -> Option<ContentCard> {
        match slot {
            Slot::Media => self.media.clone().map(ContentCard::Media),
            Slot::OtherMedia => self.other_media.clone().map(ContentCard::Media),
            Slot::NeedsInput => self.needs_input.clone().map(ContentCard::Notification),
            Slot::Running => self.running.clone().map(ContentCard::Agent),
            Slot::Done => self.done.clone().map(ContentCard::Notification),
        }
    }

    /// Picks the main and split-bubble cards. Media is preferred as the main
    /// pill so a running agent can live in the small satellite bubble; a
    /// second player only gets the bubble when nothing more urgent wants it.
    fn split_cards(&self) -> (Option<ContentCard>, Option<ContentCard>) {
        let main = [Slot::Media, Slot::NeedsInput, Slot::Running, Slot::Done]
            .into_iter()
            .find(|&slot| self.slot_card(slot).is_some());
        let bubble = [
            Slot::NeedsInput,
            Slot::Running,
            Slot::Done,
            Slot::OtherMedia,
            Slot::Media,
        ]
        .into_iter()
        .filter(|&slot| Some(slot) != main)
        .find_map(|slot| self.slot_card(slot));
        (main.and_then(|slot| self.slot_card(slot)), bubble)
    }

    /// The expanded player keeps the other player's bubble beside it, so
    /// switching between them is one click.
    fn expanded_switcher(&self) -> bool {
        self.other_media.is_some() && self.primary_kind() == Some(CardKind::Media)
    }

    /// Whether the main pill is the music player (and so can shrink).
    fn media_is_main(&self) -> bool {
        if self.dictation.is_some() || self.media.is_none() {
            return false;
        }
        if self.splits() {
            // Media is always the main pill of a split.
            true
        } else {
            self.primary_kind() == Some(CardKind::Media)
        }
    }

    /// Size of the resting pill: the music pill shrinks to art and bars once it
    /// has been playing a while; everything else keeps the compact size.
    #[must_use]
    pub fn resting_size(&self) -> PillSize {
        if self.media_condensed && self.media_is_main() {
            return MEDIA_CONDENSED;
        }
        match self.primary_card() {
            Some(ContentCard::Glance(glance)) if glance.kind == IdleGlance::Stats => GLANCE_STATS,
            Some(ContentCard::Glance(_)) => GLANCE_CLOCK,
            _ => COMPACT,
        }
    }

    /// Two activities share the island as a main pill plus a small bubble.
    /// A transient notice never goes in the bubble (a lone icon cannot say
    /// "Stopped" or "Claude finished"), so it takes over the pill for its few
    /// seconds instead.
    fn splits(&self) -> bool {
        self.persistent_count() >= 2 && self.done.is_none()
    }

    #[must_use]
    pub fn layout(&self) -> LayoutTarget {
        if self.dictation.is_some() {
            return LayoutTarget::single(DICTATION);
        }

        // Out of the way of the window behind; hovering expands as usual.
        // The pill keeps its normal shape and just docks at the other edge.
        if self.tucked && !self.expanded {
            if self.splits() {
                return LayoutTarget::split(self.resting_size(), SPLIT_BUBBLE);
            }
            if self.primary_kind().is_some() {
                return LayoutTarget::single(self.resting_size());
            }
            return LayoutTarget::idle(self.idle_style);
        }

        if self.expanded {
            return match self.primary_kind() {
                Some(CardKind::Calculator) => LayoutTarget::single(CALCULATOR),
                Some(CardKind::Stats) => LayoutTarget::single(STATS),
                Some(CardKind::Media) if self.expanded_switcher() => {
                    LayoutTarget::split(MEDIA, SWITCH_BUBBLE)
                }
                Some(CardKind::Media) => LayoutTarget::single(MEDIA),
                Some(_) => LayoutTarget::single(NOTIFICATION),
                None => LayoutTarget::idle(self.idle_style),
            };
        }

        if self.splits() {
            return LayoutTarget::split(self.resting_size(), SPLIT_BUBBLE);
        }

        if self.primary_kind().is_some() {
            return LayoutTarget::single(self.resting_size());
        }

        LayoutTarget::idle(self.idle_style)
    }

    #[must_use]
    pub fn content(&self) -> ContentSnapshot {
        let layout = self.layout();
        if !layout.visible {
            return ContentSnapshot::default();
        }

        let (primary, secondary) = if self.dictation.is_some()
            || (self.expanded
                && matches!(
                    self.primary_kind(),
                    Some(CardKind::Calculator | CardKind::Stats)
                ))
        {
            (self.primary_card(), None)
        } else if self.expanded && layout.is_split() {
            (self.primary_card(), self.slot_card(Slot::OtherMedia))
        } else if layout.is_split() {
            self.split_cards()
        } else {
            (self.primary_card(), None)
        };

        let accent = match &primary {
            Some(ContentCard::Dictation(_)) => {
                self.last_music_accent.unwrap_or(DICTATION_FALLBACK_ACCENT)
            }
            Some(card) => accent_for_card(card),
            None => [120, 128, 150],
        };
        ContentSnapshot {
            widget_navigation: self.widget_navigation_available(),
            widget_pages: self.pages().len() as u8,
            widget_page: {
                let current = self.current_page();
                self.pages()
                    .iter()
                    .position(|page| *page == current)
                    .unwrap_or(0) as u8
            },
            cpu_hot: self.cpu_hot,
            primary,
            secondary,
            accent,
        }
    }
}

/// Where a card comes from, so two cards of one kind (two players) can still
/// be told apart when choosing the main pill and the bubble.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Media,
    OtherMedia,
    NeedsInput,
    Running,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerKind {
    HoverExpand,
    HoverCollapse,
    DoneExpiry,
    MediaPaused,
    MediaCondense,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    ArmTimer { kind: TimerKind, after_ms: u32 },
    CancelTimer(TimerKind),
    LayoutChanged,
    ContentChanged,
}

impl Effect {
    #[must_use]
    pub fn changed() -> Vec<Self> {
        vec![Self::ContentChanged, Self::LayoutChanged]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum IslandEvent {
    SetCalculatorEnabled(bool),
    SetStatsEnabled(bool),
    /// A new reading (ignored unless the stats widget is enabled).
    SetStats(crate::widgets::SystemStats),
    SetCpuHot(bool),
    /// Always-on mode and what it shows, or `None` to hide when idle.
    SetGlance(Option<IdleGlance>),
    /// The last song played, for the opened island when no player is open.
    SetRemembered(Option<MediaCard>),
    SetGlanceTime {
        time: String,
        long_date: String,
    },
    SetGlanceStats {
        cpu: f32,
        memory: f32,
    },
    CycleWidget,
    CalculatorKey(crate::widgets::CalculatorKey),
    HoverEnter,
    HoverLeave,
    /// Whether the pointer is over the second player's bubble.
    HoverSwitcher(bool),
    HoverExpandElapsed,
    HoverCollapseElapsed,
    MediaPausedElapsed,
    MediaCondenseElapsed,
    SetMediaPauseDelay(u32),
    Click,
    Tick {
        now: f64,
    },
    SetExpanded(bool),
    SetIdleStyle(IdleStyle),
    SetExpandTrigger(ExpandTrigger),
    SetTucked(bool),
    SetDictation(Option<DictationCard>),
    SetMedia(Option<MediaCard>),
    /// The second player, or `None` when only one is active.
    SetOtherMedia(Option<MediaCard>),
    SetNeedsInput(Option<NotificationCard>),
    SetDone(Option<NotificationCard>),
    SetRunning(Option<AgentCard>),
    ClearAll,
}

/// Pure reducer. `now` is monotonic seconds.
pub fn reduce(state: &mut IslandState, event: IslandEvent, now: f64) -> Vec<Effect> {
    match event {
        IslandEvent::SetCalculatorEnabled(enabled) => {
            if enabled == state.calculator.is_some() {
                return Vec::new();
            }
            state.calculator = enabled.then(crate::widgets::Calculator::default);
            if state.widget == Some(WidgetKind::Calculator) {
                state.widget = None;
            }
            Effect::changed()
        }
        IslandEvent::SetStatsEnabled(enabled) => {
            if enabled == state.stats.is_some() {
                return Vec::new();
            }
            state.stats = enabled.then(crate::widgets::SystemStats::default);
            if !enabled {
                state.cpu_hot = false;
                if state.widget == Some(WidgetKind::Stats) {
                    state.widget = None;
                }
            }
            Effect::changed()
        }
        IslandEvent::SetStats(stats) => {
            let Some(current) = &mut state.stats else {
                return Vec::new();
            };
            if *current == stats {
                return Vec::new();
            }
            *current = stats;
            if state.stats_visible() {
                vec![Effect::ContentChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetCpuHot(hot) => {
            let hot = hot && state.stats.is_some();
            if hot == state.cpu_hot {
                return Vec::new();
            }
            state.cpu_hot = hot;
            vec![Effect::ContentChanged]
        }
        IslandEvent::SetGlance(glance) => {
            if state.glance == glance {
                return Vec::new();
            }
            state.glance = glance;
            Effect::changed()
        }
        IslandEvent::SetRemembered(card) => {
            if state.remembered == card {
                return Vec::new();
            }
            state.remembered = card;
            // Only on screen while opened with no player of its own.
            if state.expanded && state.media.is_none() && state.other_media.is_none() {
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetGlanceTime { time, long_date } => {
            if state.glance_info.time == time && state.glance_info.long_date == long_date {
                return Vec::new();
            }
            state.glance_info.time = time;
            state.glance_info.long_date = long_date;
            glance_repaint(state)
        }
        IslandEvent::SetGlanceStats { cpu, memory } => {
            // Whole percent steps: smaller changes are not visible.
            let same = |a: f32, b: f32| (a * 100.0).round() == (b * 100.0).round();
            if same(state.glance_info.cpu, cpu) && same(state.glance_info.memory, memory) {
                return Vec::new();
            }
            state.glance_info.cpu = cpu;
            state.glance_info.memory = memory;
            glance_repaint(state)
        }
        IslandEvent::CycleWidget => {
            if state.widget_navigation_available() {
                let pages = state.pages();
                let current = state.current_page();
                let index = pages.iter().position(|page| *page == current).unwrap_or(0);
                state.widget = pages[(index + 1) % pages.len()];
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::CalculatorKey(key) => {
            if state.expanded && state.primary_kind() == Some(CardKind::Calculator) {
                if let Some(calculator) = &mut state.calculator {
                    let before = calculator.clone();
                    calculator.press(key);
                    if *calculator != before {
                        return vec![Effect::ContentChanged];
                    }
                }
            }
            Vec::new()
        }
        IslandEvent::HoverEnter => {
            if state.pointer_inside {
                return Vec::new();
            }
            state.pointer_inside = true;
            let mut effects = vec![Effect::CancelTimer(TimerKind::HoverCollapse)];
            if state.hover_expands() && !state.expanded && state.has_activity() {
                effects.push(Effect::ArmTimer {
                    kind: TimerKind::HoverExpand,
                    after_ms: HOVER_EXPAND_MS,
                });
            }
            effects
        }
        IslandEvent::HoverLeave => {
            if !state.pointer_inside {
                return Vec::new();
            }
            state.pointer_inside = false;
            let mut effects = vec![Effect::CancelTimer(TimerKind::HoverExpand)];
            if state.expanded {
                effects.push(Effect::ArmTimer {
                    kind: TimerKind::HoverCollapse,
                    after_ms: HOVER_COLLAPSE_MS,
                });
            }
            effects
        }
        IslandEvent::HoverSwitcher(on) => {
            if state.pointer_on_switcher == on {
                return Vec::new();
            }
            state.pointer_on_switcher = on;
            if on {
                vec![Effect::CancelTimer(TimerKind::HoverExpand)]
            } else if state.hover_expands()
                && state.pointer_inside
                && !state.expanded
                && state.has_activity()
            {
                // Moved from the bubble onto the pill: expand as usual.
                vec![Effect::ArmTimer {
                    kind: TimerKind::HoverExpand,
                    after_ms: HOVER_EXPAND_MS,
                }]
            } else {
                Vec::new()
            }
        }
        IslandEvent::HoverExpandElapsed => {
            if state.pointer_inside
                && state.hover_expands()
                && !state.expanded
                && state.has_activity()
            {
                state.expanded = true;
                vec![Effect::LayoutChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::HoverCollapseElapsed => {
            if !state.pointer_inside && state.expanded {
                state.expanded = false;
                vec![Effect::LayoutChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::Click => {
            if state.has_activity() {
                state.expanded = !state.expanded;
                let mut effects = vec![Effect::LayoutChanged];
                if state.expanded {
                    effects.push(Effect::CancelTimer(TimerKind::HoverExpand));
                    effects.push(Effect::CancelTimer(TimerKind::HoverCollapse));
                } else {
                    effects.push(Effect::CancelTimer(TimerKind::HoverCollapse));
                }
                effects
            } else {
                Vec::new()
            }
        }
        IslandEvent::Tick { now } => {
            if state.done_deadline.is_some_and(|deadline| now >= deadline) {
                state.done = None;
                state.done_deadline = None;
                let mut effects = Effect::changed();
                effects.push(Effect::CancelTimer(TimerKind::DoneExpiry));
                effects
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetExpanded(expanded) => {
            if state.expanded != expanded {
                state.expanded = expanded;
                vec![Effect::LayoutChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetTucked(tucked) => {
            if state.tucked != tucked {
                state.tucked = tucked;
                vec![Effect::LayoutChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetExpandTrigger(trigger) => {
            state.expand_on = trigger;
            Vec::new()
        }
        IslandEvent::SetIdleStyle(style) => {
            if state.idle_style != style {
                state.idle_style = style;
                vec![Effect::LayoutChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetDictation(card) => {
            if state.dictation != card {
                state.dictation = card;
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetMedia(card) => {
            let was_paused = state.media.as_ref().is_some_and(|media| !media.playing);
            let card = match card {
                Some(card) if card.playing => {
                    state.media_dismissed = false;
                    Some(card)
                }
                // Paused media only stays if it was already on the island
                // (it was playing); a stale paused session never pops up.
                Some(_) if state.media_dismissed || state.media.is_none() => None,
                Some(card) => Some(card),
                None => {
                    state.media_dismissed = false;
                    None
                }
            };
            if let Some(media) = &card {
                state.last_music_accent = Some(media.accent.unwrap_or(DEFAULT_MEDIA_ACCENT));
            }
            let mut effects = Vec::new();
            // A track that keeps playing keeps its timer; a new track, or one
            // that just started, begins the wait before the pill shrinks.
            let continues = card.as_ref().is_some_and(|new| {
                new.playing
                    && state
                        .media
                        .as_ref()
                        .is_some_and(|old| old.playing && old.same_track(new))
            });
            match &card {
                Some(media) if media.playing => {
                    if !continues {
                        state.media_condensed = false;
                        effects.push(Effect::ArmTimer {
                            kind: TimerKind::MediaCondense,
                            after_ms: MEDIA_CONDENSE_MS,
                        });
                    }
                }
                _ => {
                    state.media_condensed = false;
                    effects.push(Effect::CancelTimer(TimerKind::MediaCondense));
                }
            }
            match &card {
                Some(media) if !media.playing && !was_paused => {
                    effects.push(Effect::ArmTimer {
                        kind: TimerKind::MediaPaused,
                        after_ms: state.media_paused_ms,
                    });
                }
                Some(media) if !media.playing => {}
                _ => effects.push(Effect::CancelTimer(TimerKind::MediaPaused)),
            }
            if state.media != card {
                state.media = card;
                effects.extend(Effect::changed());
            }
            effects
        }
        IslandEvent::MediaCondenseElapsed => {
            let playing = state.media.as_ref().is_some_and(|media| media.playing);
            if playing && !state.media_condensed {
                state.media_condensed = true;
                vec![Effect::LayoutChanged]
            } else {
                Vec::new()
            }
        }
        IslandEvent::MediaPausedElapsed => {
            let paused = state.media.as_ref().is_some_and(|media| !media.playing);
            // With a second player the media service decides when a paused
            // one leaves (the other then takes the pill); check back later
            // in case it ends up alone.
            if paused && (state.pointer_inside || state.other_media.is_some()) {
                // The user is looking at it (maybe about to press play).
                return vec![Effect::ArmTimer {
                    kind: TimerKind::MediaPaused,
                    after_ms: state.media_paused_ms,
                }];
            }
            if paused {
                state.media = None;
                state.media_dismissed = true;
                if !state.has_activity() {
                    state.expanded = false;
                }
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetOtherMedia(card) => {
            if state.other_media != card {
                state.other_media = card;
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetMediaPauseDelay(ms) => {
            state.media_paused_ms = ms.max(1);
            Vec::new()
        }
        IslandEvent::SetNeedsInput(card) => {
            if state.needs_input != card {
                state.needs_input = card;
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::SetDone(card) => {
            if state.done == card {
                return Vec::new();
            }
            state.done = card;
            if state.done.is_some() {
                let visible = state
                    .done
                    .as_ref()
                    .map_or(DONE_VISIBLE_MS, |card| visible_ms(card.level));
                state.done_deadline = Some(now + f64::from(visible) / 1000.0);
                let mut effects = Effect::changed();
                effects.push(Effect::ArmTimer {
                    kind: TimerKind::DoneExpiry,
                    after_ms: visible,
                });
                effects
            } else {
                state.done_deadline = None;
                let mut effects = Effect::changed();
                effects.push(Effect::CancelTimer(TimerKind::DoneExpiry));
                effects
            }
        }
        IslandEvent::SetRunning(card) => {
            if state.running != card {
                state.running = card;
                Effect::changed()
            } else {
                Vec::new()
            }
        }
        IslandEvent::ClearAll => {
            let had_activity = state.has_activity();
            state.dictation = None;
            state.needs_input = None;
            state.done = None;
            state.running = None;
            state.media = None;
            state.other_media = None;
            state.done_deadline = None;
            state.expanded = false;
            if had_activity {
                let mut effects = Effect::changed();
                effects.push(Effect::CancelTimer(TimerKind::DoneExpiry));
                effects
            } else {
                Vec::new()
            }
        }
    }
}

/// A glance reading changed: repaint only if the glance is on screen.
fn glance_repaint(state: &IslandState) -> Vec<Effect> {
    if state.primary_kind() == Some(CardKind::Glance) {
        vec![Effect::ContentChanged]
    } else {
        Vec::new()
    }
}

fn accent_for_card(card: &ContentCard) -> [u8; 3] {
    match card {
        ContentCard::Glance(_) => [214, 218, 228],
        ContentCard::Calculator(_) => [164, 204, 180],
        ContentCard::Stats(_) => [110, 190, 255],
        ContentCard::Media(media) => media.accent.unwrap_or(DEFAULT_MEDIA_ACCENT),
        ContentCard::Notification(notification) => match notification.level {
            AlertLevel::Info => [110, 150, 255],
            AlertLevel::Success => [64, 210, 122],
            AlertLevel::NeedsInput => [255, 159, 46],
            AlertLevel::Error => [255, 92, 92],
        },
        ContentCard::Agent(agent) => match agent.state {
            AgentState::Running => [120, 160, 255],
            AgentState::NeedsInput => [255, 159, 46],
            AgentState::Done => [64, 210, 122],
        },
        // Chosen in `IslandState::content` from the last music colour.
        ContentCard::Dictation(_) => DICTATION_FALLBACK_ACCENT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_work_without_media_and_keep_urgent_activity_in_front() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetCalculatorEnabled(true), 0.0);
        assert!(state.has_activity());
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
        assert!(state.content().primary.is_none());
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.1);
        // Opened, it always starts on the music player (empty here).
        assert_eq!(state.layout().main, MEDIA);
        assert!(state.widget_navigation_available());
        reduce(&mut state, IslandEvent::CycleWidget, 0.15);
        assert_eq!(state.layout().main, CALCULATOR);
        assert!(!state.content().animates());
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.2);
        assert!(state.widget_navigation_available());
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        reduce(
            &mut state,
            IslandEvent::CalculatorKey(crate::widgets::CalculatorKey::Digit(7)),
            0.4,
        );
        assert_eq!(state.calculator.as_ref().unwrap().display(), "7");
        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(DictationCard::default())),
            0.5,
        );
        assert_eq!(state.primary_kind(), Some(CardKind::Dictation));
        assert!(!state.widget_navigation_available());
        reduce(&mut state, IslandEvent::SetDictation(None), 0.6);
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        reduce(&mut state, IslandEvent::SetCalculatorEnabled(false), 0.7);
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
        assert!(!state.content().widget_navigation);
        reduce(&mut state, IslandEvent::SetMedia(None), 0.8);
        // Still open: the empty player. Closed: hidden again.
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));
        reduce(&mut state, IslandEvent::SetExpanded(false), 0.9);
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
    }

    #[test]
    fn running_agent_round_trips_through_calculator_navigation() {
        let mut state = IslandState {
            calculator: Some(crate::widgets::Calculator::default()),
            running: Some(agent()),
            expanded: true,
            ..Default::default()
        };
        assert!(state.widget_navigation_available());
        // Music first, then the agent, then the widgets, and round again.
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
        reduce(&mut state, IslandEvent::CycleWidget, 0.0);
        assert_eq!(state.primary_kind(), Some(CardKind::Agent));
        reduce(&mut state, IslandEvent::CycleWidget, 0.5);
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        reduce(&mut state, IslandEvent::CycleWidget, 1.0);
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
        // Collapsed, the agent keeps the pill as before.
        reduce(&mut state, IslandEvent::SetExpanded(false), 1.5);
        assert_eq!(state.primary_kind(), Some(CardKind::Agent));
    }

    #[test]
    fn enabled_widgets_preserve_idle_appearance_until_expanded() {
        for idle_style in [IdleStyle::Hidden, IdleStyle::Sliver] {
            let mut state = IslandState {
                idle_style,
                calculator: Some(crate::widgets::Calculator::default()),
                widget: Some(WidgetKind::Calculator),
                ..Default::default()
            };
            assert_eq!(state.layout(), LayoutTarget::idle(idle_style));
            assert!(state.content().primary.is_none());
            assert!(!state.content().widget_navigation);
            reduce(&mut state, IslandEvent::HoverEnter, 0.0);
            reduce(&mut state, IslandEvent::HoverExpandElapsed, 0.3);
            assert_eq!(state.layout(), LayoutTarget::single(CALCULATOR));
            reduce(&mut state, IslandEvent::HoverLeave, 0.4);
            reduce(&mut state, IslandEvent::HoverCollapseElapsed, 0.9);
            assert_eq!(state.layout(), LayoutTarget::idle(idle_style));
            assert!(state.content().primary.is_none());
        }
    }

    #[test]
    fn collapsing_a_widget_restores_music_or_agent_content() {
        let mut state = IslandState {
            calculator: Some(crate::widgets::Calculator::default()),
            widget: Some(WidgetKind::Calculator),
            media: Some(media()),
            media_condensed: true,
            expanded: true,
            ..Default::default()
        };
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        reduce(&mut state, IslandEvent::SetExpanded(false), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA_CONDENSED));
        assert!(matches!(
            state.content().primary,
            Some(ContentCard::Media(_))
        ));
        reduce(&mut state, IslandEvent::SetMedia(None), 0.1);
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.2);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
        assert!(matches!(
            state.content().primary,
            Some(ContentCard::Agent(_))
        ));
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.3);
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
    }

    #[test]
    fn customization_preserves_compact_size_and_expanded_proportions() {
        let compact = LayoutTarget::single(MEDIA_CONDENSED).customized(1.25, 5.0, 1000.0);
        assert_eq!(compact.main, MEDIA_CONDENSED);
        assert_eq!(compact.content_scale, 1.0);
        assert_eq!(compact.inset, EDGE_GAP_DP + 50.0);
        for scale in [0.8, 1.0, 1.25] {
            let expanded = LayoutTarget::split(MEDIA, SWITCH_BUBBLE).customized(scale, 2.0, 1000.0);
            // Within float rounding of the original size.
            assert!((expanded.main.width / scale - MEDIA.width).abs() < 1e-3);
            assert!((expanded.main.height / scale - MEDIA.height).abs() < 1e-3);
            assert!((expanded.bubble.unwrap().width / scale - SWITCH_BUBBLE.width).abs() < 1e-3);
            assert_eq!(expanded.content_scale, scale);
            // Inverting the draw scale keeps a transport control's hit point fixed.
            let original = (120.0_f32, 60.0_f32);
            assert!(((original.0 * scale) / expanded.content_scale - original.0).abs() < 0.001);
        }
    }

    fn media() -> MediaCard {
        MediaCard {
            title: "Midnight City".to_owned(),
            artist: "M83".to_owned(),
            source: "Spotify".to_owned(),
            playing: true,
            timeline: None,
            art: None,
            accent: None,
            scrub: None,
            volume: None,
        }
    }

    fn paused() -> MediaCard {
        MediaCard {
            playing: false,
            ..media()
        }
    }

    #[test]
    fn paused_media_closes_after_the_delay_until_it_plays_again() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(paused())), 1.0);
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::MediaPaused,
            after_ms: MEDIA_PAUSED_MS,
        }));
        // Still shown while paused.
        assert!(state.media.as_ref().is_some_and(|media| !media.playing));

        // A further paused update (e.g. a timeline change) does not re-arm.
        let mut later = paused();
        later.timeline = Some(Timeline {
            position: 3.0,
            duration: 100.0,
            sampled_at: 2.0,
            rate: 1.0,
        });
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(later)), 2.0);
        assert!(!effects
            .iter()
            .any(|effect| matches!(effect, Effect::ArmTimer { .. })));

        reduce(&mut state, IslandEvent::MediaPausedElapsed, 6.0);
        assert!(state.media.is_none());
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);

        // Still paused: stays hidden.
        reduce(&mut state, IslandEvent::SetMedia(Some(paused())), 7.0);
        assert!(state.media.is_none());

        // Playing again brings it back.
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(media())), 8.0);
        assert!(effects.contains(&Effect::CancelTimer(TimerKind::MediaPaused)));
        assert!(state.media.is_some());
    }

    #[test]
    fn stale_paused_media_never_pops_up() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(paused())), 0.0);
        assert!(state.media.is_none());
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
    }

    #[test]
    fn hovering_keeps_paused_media_open_and_the_delay_is_configurable() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMediaPauseDelay(12_000), 0.0);
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(paused())), 1.0);
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::MediaPaused,
            after_ms: 12_000,
        }));
        reduce(&mut state, IslandEvent::HoverEnter, 2.0);
        let effects = reduce(&mut state, IslandEvent::MediaPausedElapsed, 13.0);
        assert!(state.media.is_some());
        assert_eq!(
            effects,
            vec![Effect::ArmTimer {
                kind: TimerKind::MediaPaused,
                after_ms: 12_000,
            }]
        );
    }

    #[test]
    fn timeline_and_play_state_update_in_place_but_tracks_do_not() {
        let before = ContentSnapshot {
            primary: Some(ContentCard::Media(media())),
            ..ContentSnapshot::default()
        };
        let mut moved = media();
        moved.playing = false;
        moved.scrub = Some(0.5);
        let after = ContentSnapshot {
            primary: Some(ContentCard::Media(moved)),
            ..ContentSnapshot::default()
        };
        assert!(before.updates_in_place(&after));

        let mut other = media();
        other.title = "Wait".to_owned();
        let changed = ContentSnapshot {
            primary: Some(ContentCard::Media(other)),
            ..ContentSnapshot::default()
        };
        assert!(!before.updates_in_place(&changed));
        assert!(!before.updates_in_place(&ContentSnapshot::default()));
    }

    #[test]
    fn media_accent_follows_album_art() {
        let mut state = IslandState::default();
        let mut card = media();
        card.accent = Some([200, 40, 60]);
        reduce(&mut state, IslandEvent::SetMedia(Some(card)), 0.0);
        assert_eq!(state.content().accent, [200, 40, 60]);
    }

    fn agent() -> AgentCard {
        AgentCard {
            project: "islet".to_owned(),
            activity: "Running cargo test".to_owned(),
            state: AgentState::Running,
        }
    }

    fn notification(level: AlertLevel) -> NotificationCard {
        NotificationCard {
            source: "Claude Code".to_owned(),
            title: "Claude finished".to_owned(),
            body: "Refactored auth.rs".to_owned(),
            level,
        }
    }

    #[test]
    fn priorities_prefer_dictation_then_needs_input_then_media() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
        assert_eq!(state.primary_kind(), Some(CardKind::Media));

        reduce(
            &mut state,
            IslandEvent::SetNeedsInput(Some(notification(AlertLevel::NeedsInput))),
            0.0,
        );
        assert_eq!(state.primary_kind(), Some(CardKind::Notification));

        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(DictationCard {
                elapsed_ms: 2_000,
                level: 0.4,
                phase: DictationPhase::Recording,
                ..DictationCard::default()
            })),
            0.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(DICTATION));
        assert_eq!(state.primary_kind(), Some(CardKind::Dictation));
    }

    #[test]
    fn two_persistent_activities_use_split_layout_with_media_as_main() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.0);

        assert_eq!(state.layout(), LayoutTarget::split(COMPACT, SPLIT_BUBBLE));
        let content = state.content();
        assert!(matches!(content.primary, Some(ContentCard::Media(_))));
        assert!(matches!(content.secondary, Some(ContentCard::Agent(_))));
    }

    fn browser() -> MediaCard {
        MediaCard {
            title: "Lo-fi beats".to_owned(),
            artist: "YouTube".to_owned(),
            source: "Chrome".to_owned(),
            ..media()
        }
    }

    fn media_in(card: &Option<ContentCard>) -> Option<&str> {
        match card {
            Some(ContentCard::Media(media)) => Some(media.source.as_str()),
            _ => None,
        }
    }

    #[test]
    fn a_second_player_waits_in_the_bubble_and_stays_beside_the_expanded_player() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetOtherMedia(Some(browser())), 0.0);

        assert_eq!(state.layout(), LayoutTarget::split(COMPACT, SPLIT_BUBBLE));
        let content = state.content();
        assert_eq!(media_in(&content.primary), Some("Spotify"));
        assert_eq!(media_in(&content.secondary), Some("Chrome"));

        reduce(&mut state, IslandEvent::SetExpanded(true), 0.0);
        assert_eq!(state.layout(), LayoutTarget::split(MEDIA, SWITCH_BUBBLE));
        let content = state.content();
        assert_eq!(media_in(&content.primary), Some("Spotify"));
        assert_eq!(media_in(&content.secondary), Some("Chrome"));

        reduce(&mut state, IslandEvent::SetOtherMedia(None), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));
    }

    fn arms_condense(effects: &[Effect]) -> bool {
        effects.contains(&Effect::ArmTimer {
            kind: TimerKind::MediaCondense,
            after_ms: MEDIA_CONDENSE_MS,
        })
    }

    #[test]
    fn music_shrinks_after_five_seconds() {
        const {
            assert!(MEDIA_CONDENSED.height == DICTATION.height);
            assert!(MEDIA_CONDENSED.width < COMPACT.width);
        };
        let mut state = IslandState::default();
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        assert!(arms_condense(&effects));
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::MediaCondense,
            after_ms: 5_000,
        }));
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));

        // Position and volume updates of the same track keep the same timer.
        let mut later = media();
        later.volume = Some(0.5);
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(later)), 1.0);
        assert!(!arms_condense(&effects));

        reduce(&mut state, IslandEvent::MediaCondenseElapsed, 5.0);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA_CONDENSED));
        // The card itself is unchanged: only its size, so no content morph.
        assert!(matches!(
            state.content().primary,
            Some(ContentCard::Media(_))
        ));

        // Opening the island still shows the full player.
        reduce(&mut state, IslandEvent::SetExpanded(true), 6.0);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));
        reduce(&mut state, IslandEvent::SetExpanded(false), 7.0);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA_CONDENSED));
    }

    #[test]
    fn pausing_or_a_new_track_brings_the_full_pill_back() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::MediaCondenseElapsed, 5.0);
        assert_eq!(state.resting_size(), MEDIA_CONDENSED);

        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(paused())), 6.0);
        assert!(effects.contains(&Effect::CancelTimer(TimerKind::MediaCondense)));
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
        // A late timer for a paused track does nothing.
        reduce(&mut state, IslandEvent::MediaCondenseElapsed, 8.0);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));

        // Playing again waits another five seconds.
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(media())), 9.0);
        assert!(arms_condense(&effects));
        reduce(&mut state, IslandEvent::MediaCondenseElapsed, 14.0);
        assert_eq!(state.resting_size(), MEDIA_CONDENSED);

        // A different track shows its title again first.
        let effects = reduce(&mut state, IslandEvent::SetMedia(Some(browser())), 15.0);
        assert!(arms_condense(&effects));
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
    }

    #[test]
    fn only_the_music_pill_shrinks() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.0);
        reduce(&mut state, IslandEvent::MediaCondenseElapsed, 3.0);
        // Music stays the main pill of a split, shrunk, with its bubble.
        assert_eq!(
            state.layout(),
            LayoutTarget::split(MEDIA_CONDENSED, SPLIT_BUBBLE)
        );

        // A notification takes the pill: full size for its message.
        reduce(
            &mut state,
            IslandEvent::SetDone(Some(notification(AlertLevel::Success))),
            4.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));

        // Dictation keeps its own size, and music returns shrunk afterwards.
        reduce(&mut state, IslandEvent::SetDone(None), 5.0);
        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(DictationCard::default())),
            5.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(DICTATION));
        reduce(&mut state, IslandEvent::SetDictation(None), 6.0);
        assert_eq!(
            state.layout(),
            LayoutTarget::split(MEDIA_CONDENSED, SPLIT_BUBBLE)
        );

        // Nothing playing: back to defaults.
        reduce(&mut state, IslandEvent::SetRunning(None), 7.0);
        reduce(&mut state, IslandEvent::SetMedia(None), 7.0);
        assert!(!state.media_condensed);
    }

    #[test]
    fn click_mode_opens_only_on_click_and_still_closes_on_leave() {
        let mut state = IslandState::default();
        reduce(
            &mut state,
            IslandEvent::SetExpandTrigger(ExpandTrigger::Click),
            0.0,
        );
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);

        let effects = reduce(&mut state, IslandEvent::HoverEnter, 0.0);
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::ArmTimer {
                kind: TimerKind::HoverExpand,
                ..
            }
        )));
        reduce(&mut state, IslandEvent::HoverExpandElapsed, 0.3);
        assert!(!state.expanded);

        reduce(&mut state, IslandEvent::Click, 0.5);
        assert!(state.expanded);
        let effects = reduce(&mut state, IslandEvent::HoverLeave, 1.0);
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::HoverCollapse,
            after_ms: HOVER_COLLAPSE_MS,
        }));
    }

    #[test]
    fn hovering_the_second_player_bubble_does_not_expand() {
        let arm = Effect::ArmTimer {
            kind: TimerKind::HoverExpand,
            after_ms: HOVER_EXPAND_MS,
        };
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetOtherMedia(Some(browser())), 0.0);

        reduce(&mut state, IslandEvent::HoverSwitcher(true), 0.0);
        let effects = reduce(&mut state, IslandEvent::HoverEnter, 0.0);
        assert!(!effects.contains(&arm));
        reduce(&mut state, IslandEvent::HoverExpandElapsed, 0.3);
        assert!(!state.expanded);

        // Sliding over onto the pill expands as usual.
        let effects = reduce(&mut state, IslandEvent::HoverSwitcher(false), 0.4);
        assert!(effects.contains(&arm));
        reduce(&mut state, IslandEvent::HoverExpandElapsed, 0.7);
        assert!(state.expanded);
    }

    #[test]
    fn alerts_take_the_bubble_before_a_second_player() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetOtherMedia(Some(browser())), 0.0);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.0);
        let content = state.content();
        assert_eq!(media_in(&content.primary), Some("Spotify"));
        assert!(matches!(content.secondary, Some(ContentCard::Agent(_))));
    }

    #[test]
    fn paused_main_player_is_not_dismissed_while_another_plays() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetOtherMedia(Some(browser())), 0.0);
        reduce(&mut state, IslandEvent::SetMedia(Some(paused())), 1.0);
        let effects = reduce(&mut state, IslandEvent::MediaPausedElapsed, 6.0);
        assert!(state.media.is_some());
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::MediaPaused,
            after_ms: MEDIA_PAUSED_MS,
        }));

        // Alone again: the usual delay closes it.
        reduce(&mut state, IslandEvent::SetOtherMedia(None), 7.0);
        reduce(&mut state, IslandEvent::MediaPausedElapsed, 12.0);
        assert!(state.media.is_none());
    }

    #[test]
    fn hover_expands_after_250ms_and_collapses_after_400ms() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);

        let effects = reduce(&mut state, IslandEvent::HoverEnter, 1.0);
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::HoverExpand,
            after_ms: HOVER_EXPAND_MS,
        }));
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));

        // The timer fires after the pointer remained inside.
        let effects = reduce(&mut state, IslandEvent::HoverExpandElapsed, 1.25);
        assert_eq!(effects, vec![Effect::LayoutChanged]);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));

        let effects = reduce(&mut state, IslandEvent::HoverLeave, 2.0);
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::HoverCollapse,
            after_ms: HOVER_COLLAPSE_MS,
        }));
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));

        // The collapse timer fires while the pointer is still outside.
        let effects = reduce(&mut state, IslandEvent::HoverCollapseElapsed, 2.4);
        assert_eq!(effects, vec![Effect::LayoutChanged]);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
    }

    #[test]
    fn notification_expands_to_its_own_size() {
        let mut state = IslandState::default();
        reduce(
            &mut state,
            IslandEvent::SetNeedsInput(Some(notification(AlertLevel::NeedsInput))),
            0.0,
        );
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(NOTIFICATION));
    }

    #[test]
    fn done_notifications_expire_after_four_seconds() {
        let mut state = IslandState::default();
        let effects = reduce(
            &mut state,
            IslandEvent::SetDone(Some(notification(AlertLevel::Success))),
            10.0,
        );
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::DoneExpiry,
            after_ms: DONE_VISIBLE_MS,
        }));
        assert!(state.done.is_some());

        assert!(reduce(&mut state, IslandEvent::Tick { now: 13.9 }, 13.9).is_empty());
        assert!(state.done.is_some());

        let effects = reduce(&mut state, IslandEvent::Tick { now: 14.0 }, 14.0);
        assert!(effects.contains(&Effect::LayoutChanged));
        assert!(state.done.is_none());
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
    }

    #[test]
    fn idle_style_controls_hidden_and_sliver_states() {
        let mut state = IslandState::default();
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);

        reduce(
            &mut state,
            IslandEvent::SetIdleStyle(IdleStyle::Sliver),
            0.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(SLIVER));

        reduce(
            &mut state,
            IslandEvent::SetIdleStyle(IdleStyle::Hidden),
            0.0,
        );
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
    }

    #[test]
    fn click_toggles_expansion_only_when_activity_exists() {
        let mut state = IslandState::default();
        assert!(reduce(&mut state, IslandEvent::Click, 0.0).is_empty());

        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        assert!(!state.expanded);
        reduce(&mut state, IslandEvent::Click, 0.0);
        assert!(state.expanded);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));
        reduce(&mut state, IslandEvent::Click, 0.0);
        assert!(!state.expanded);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
    }

    #[test]
    fn arrange_centres_and_anchors_the_main_pill() {
        let bottom = arrange(COMPACT, None, 0.0, 0.0, 440.0, 200.0, false);
        assert_eq!(bottom.main, Rect::new(110.0, 164.0, 220.0, 36.0));
        assert_eq!(bottom.bubble, None);
        assert_eq!(bottom.bounds(), Some(bottom.main));

        let top = arrange(COMPACT, None, 0.0, 0.0, 440.0, 200.0, true);
        assert_eq!(top.main.y, 0.0);
    }

    #[test]
    fn arrange_places_the_bubble_right_of_the_main_pill() {
        let frame = arrange(
            COMPACT,
            Some(SPLIT_BUBBLE),
            SPLIT_GAP_DP,
            0.0,
            440.0,
            200.0,
            false,
        );
        // Group width 220 + 6 + 36 = 262, centred in 440.
        assert_eq!(frame.main.x, 89.0);
        let bubble = frame.bubble.expect("bubble");
        assert_eq!(bubble, Rect::new(315.0, 164.0, 36.0, 36.0));
        assert_eq!(frame.bounds(), Some(Rect::new(89.0, 164.0, 262.0, 36.0)));
    }

    #[test]
    fn arrange_applies_the_edge_inset() {
        let bottom = arrange(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 200.0, false);
        assert_eq!(bottom.main.bottom(), 200.0 - EDGE_GAP_DP);
        let top = arrange(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 200.0, true);
        assert_eq!(top.main.y, EDGE_GAP_DP);
    }

    #[test]
    fn arrange_at_interpolates_between_edges() {
        // A full-height island window (for the dock animation) places the
        // pill at the top, bottom, or anywhere in between.
        let top = arrange_at(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 1040.0, 0.0);
        let bottom = arrange_at(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 1040.0, 1.0);
        let middle = arrange_at(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 1040.0, 0.5);
        assert_eq!(top.main.y, EDGE_GAP_DP);
        assert_eq!(bottom.main.y, 1040.0 - COMPACT.height - EDGE_GAP_DP);
        assert_eq!(
            middle.main.y,
            (top.main.y + bottom.main.y) * 0.5,
            "the pill should travel linearly between the two edges"
        );
        // Out-of-range anchors are clamped, never off screen.
        assert_eq!(
            arrange_at(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 1040.0, -3.0),
            top
        );
        assert_eq!(
            arrange_at(COMPACT, None, 0.0, EDGE_GAP_DP, 440.0, 1040.0, 3.0),
            bottom
        );
    }

    #[test]
    fn tucked_island_keeps_its_shape_and_docks_until_hovered() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));

        let effects = reduce(&mut state, IslandEvent::SetTucked(true), 0.0);
        assert_eq!(effects, vec![Effect::LayoutChanged]);
        // The pill itself glides to the bottom; it does not shrink into a
        // different shape.
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
        assert!(matches!(
            state.content().primary,
            Some(ContentCard::Media(_))
        ));

        // Hover expands to the full media view as usual.
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));

        reduce(&mut state, IslandEvent::SetExpanded(false), 0.0);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.0);
        assert_eq!(state.layout(), LayoutTarget::split(COMPACT, SPLIT_BUBBLE));

        reduce(&mut state, IslandEvent::SetTucked(false), 0.0);
        assert_eq!(state.layout(), LayoutTarget::split(COMPACT, SPLIT_BUBBLE));
    }

    #[test]
    fn dictation_is_never_tucked() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetTucked(true), 0.0);
        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(DictationCard {
                elapsed_ms: 0,
                level: 0.5,
                phase: DictationPhase::Recording,
                ..DictationCard::default()
            })),
            0.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(DICTATION));
    }

    #[test]
    fn arrange_of_hidden_has_no_bounds() {
        let frame = arrange(HIDDEN, None, 0.0, 0.0, 440.0, 200.0, false);
        assert_eq!(frame.bounds(), None);
        // A collapsing sub-pixel bubble is treated as gone.
        let frame = arrange(
            COMPACT,
            Some(PillSize::new(0.3, 0.3, 0.1)),
            6.0,
            0.0,
            440.0,
            200.0,
            false,
        );
        assert_eq!(frame.bubble, None);
    }

    #[test]
    fn a_transient_notice_takes_over_instead_of_hiding_in_the_bubble() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.0);
        assert_eq!(state.layout(), LayoutTarget::split(COMPACT, SPLIT_BUBBLE));

        // A "Stopped"-style notice arrives: one readable pill, no bubble.
        reduce(
            &mut state,
            IslandEvent::SetDone(Some(notification(AlertLevel::Info))),
            1.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
        let content = state.content();
        assert!(matches!(
            content.primary,
            Some(ContentCard::Notification(_))
        ));
        assert!(content.secondary.is_none());

        // When it expires the music and the agent share the island again.
        reduce(&mut state, IslandEvent::Tick { now: 6.0 }, 6.0);
        assert_eq!(state.layout(), LayoutTarget::split(COMPACT, SPLIT_BUBBLE));
    }

    #[test]
    fn info_notices_are_brief_and_others_stay_four_seconds() {
        let mut state = IslandState::default();
        let effects = reduce(
            &mut state,
            IslandEvent::SetDone(Some(notification(AlertLevel::Info))),
            10.0,
        );
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::DoneExpiry,
            after_ms: INFO_VISIBLE_MS,
        }));
        assert!(reduce(&mut state, IslandEvent::Tick { now: 12.4 }, 12.4).is_empty());
        assert!(!reduce(&mut state, IslandEvent::Tick { now: 12.5 }, 12.5).is_empty());
        assert!(state.done.is_none());

        let effects = reduce(
            &mut state,
            IslandEvent::SetDone(Some(notification(AlertLevel::Error))),
            20.0,
        );
        assert!(effects.contains(&Effect::ArmTimer {
            kind: TimerKind::DoneExpiry,
            after_ms: DONE_VISIBLE_MS,
        }));
    }

    #[test]
    fn dictation_bars_borrow_the_last_music_colour_or_are_white() {
        let mut state = IslandState::default();
        let dictation = || {
            IslandEvent::SetDictation(Some(DictationCard {
                phase: DictationPhase::Recording,
                ..DictationCard::default()
            }))
        };
        // No music has ever played: white.
        reduce(&mut state, dictation(), 0.0);
        assert_eq!(state.content().accent, DICTATION_FALLBACK_ACCENT);
        reduce(&mut state, IslandEvent::SetDictation(None), 0.0);

        // Music whose art gave a colour: dictation uses that colour.
        let mut card = media();
        card.accent = Some([200, 40, 60]);
        reduce(&mut state, IslandEvent::SetMedia(Some(card)), 0.0);
        reduce(&mut state, dictation(), 0.0);
        assert_eq!(state.content().accent, [200, 40, 60]);

        // The colour is remembered after the music is gone.
        reduce(&mut state, IslandEvent::SetMedia(None), 0.0);
        assert!(state.media.is_none());
        assert_eq!(state.content().accent, [200, 40, 60]);
        reduce(&mut state, IslandEvent::SetDictation(None), 0.0);

        // Music without a colour used the default blue bars, so dictation does too.
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, dictation(), 0.0);
        assert_eq!(state.content().accent, DEFAULT_MEDIA_ACCENT);
    }

    #[test]
    fn dictation_pill_is_smaller_than_the_music_pill_and_the_music_returns() {
        const { assert!(DICTATION.width < COMPACT.width && DICTATION.height <= COMPACT.height) };
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));
        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(DictationCard::default())),
            0.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(DICTATION));
        reduce(&mut state, IslandEvent::SetDictation(None), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(COMPACT));

        // With no music the island hides again afterwards.
        let mut quiet = IslandState::default();
        reduce(
            &mut quiet,
            IslandEvent::SetDictation(Some(DictationCard::default())),
            0.0,
        );
        reduce(&mut quiet, IslandEvent::SetDictation(None), 0.0);
        assert_eq!(quiet.layout(), LayoutTarget::HIDDEN);
    }

    #[test]
    fn clearing_activities_returns_to_idle() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        reduce(&mut state, IslandEvent::SetRunning(Some(agent())), 0.0);
        assert_eq!(state.persistent_count(), 2);

        reduce(&mut state, IslandEvent::ClearAll, 0.0);
        assert!(!state.has_activity());
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
        assert!(state.content().is_empty());
    }

    #[test]
    fn content_animation_flag_follows_media_and_dictation() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 0.0);
        assert!(state.content().animates());

        state.media = Some(MediaCard {
            playing: false,
            ..media()
        });
        assert!(!state.content().animates());

        // Warming up, recording and transcribing all animate.
        for (phase, animates) in [
            (DictationPhase::Loading, true),
            (DictationPhase::Recording, true),
            (DictationPhase::Transcribing, true),
        ] {
            reduce(
                &mut state,
                IslandEvent::SetDictation(Some(DictationCard {
                    phase,
                    ..DictationCard::default()
                })),
                0.0,
            );
            assert_eq!(state.content().animates(), animates, "{phase:?}");
        }
    }

    #[test]
    fn dictation_waveform_updates_do_not_run_a_content_morph() {
        let mut state = IslandState::default();
        let mut card = DictationCard::default();
        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(card.clone())),
            0.0,
        );
        let before = state.content();
        card.level = 0.8;
        card.elapsed_ms = 1_500;
        card.bars[DICTATION_BARS - 1] = 200;
        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(card.clone())),
            0.0,
        );
        assert!(before.updates_in_place(&state.content()));
        // A phase change is a real transition.
        card.phase = DictationPhase::Transcribing;
        reduce(&mut state, IslandEvent::SetDictation(Some(card)), 0.0);
        assert!(!before.updates_in_place(&state.content()));
    }

    #[test]
    fn the_wheel_steps_through_activity_and_every_widget() {
        let mut state = IslandState {
            calculator: Some(crate::widgets::Calculator::default()),
            stats: Some(crate::widgets::SystemStats::default()),
            media: Some(media()),
            expanded: true,
            ..Default::default()
        };
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
        assert_eq!(state.content().widget_pages, 3);
        reduce(&mut state, IslandEvent::CycleWidget, 0.0);
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        reduce(&mut state, IslandEvent::CycleWidget, 0.1);
        assert_eq!(state.primary_kind(), Some(CardKind::Stats));
        assert_eq!(state.layout(), LayoutTarget::single(STATS));
        assert_eq!(state.content().widget_page, 2);
        assert!(state.stats_visible());
        reduce(&mut state, IslandEvent::CycleWidget, 0.2);
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
    }

    #[test]
    fn stats_alone_open_on_expand_and_readings_only_repaint_when_shown() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetStatsEnabled(true), 0.0);
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
        let reading = crate::widgets::SystemStats {
            cpu: 0.5,
            ..Default::default()
        };
        assert!(reduce(&mut state, IslandEvent::SetStats(reading.clone()), 0.1).is_empty());
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.2);
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
        reduce(&mut state, IslandEvent::CycleWidget, 0.25);
        assert_eq!(state.primary_kind(), Some(CardKind::Stats));
        let busier = crate::widgets::SystemStats {
            cpu: 0.9,
            ..reading
        };
        assert_eq!(
            reduce(&mut state, IslandEvent::SetStats(busier), 0.3),
            vec![Effect::ContentChanged]
        );
        reduce(&mut state, IslandEvent::SetCpuHot(true), 0.4);
        assert!(state.content().cpu_hot);
        reduce(&mut state, IslandEvent::SetStatsEnabled(false), 0.5);
        assert!(!state.cpu_hot);
        assert!(state.stats.is_none());
    }

    #[test]
    fn always_on_shows_the_clock_when_music_stops() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetGlance(Some(IdleGlance::Clock)), 0.0);
        reduce(
            &mut state,
            IslandEvent::SetGlanceTime {
                time: "9:41".to_owned(),
                long_date: "Tuesday".to_owned(),
            },
            0.0,
        );
        assert_eq!(state.layout(), LayoutTarget::single(GLANCE_CLOCK));
        assert!(matches!(
            state.content().primary,
            Some(ContentCard::Glance(GlanceCard { ref time, kind: IdleGlance::Clock, .. })) if time == "9:41"
        ));

        reduce(&mut state, IslandEvent::SetMedia(Some(media())), 1.0);
        let mut paused = media();
        paused.playing = false;
        reduce(&mut state, IslandEvent::SetMedia(Some(paused.clone())), 2.0);
        // Paused: the song stays for the pause delay, then the time.
        assert!(matches!(state.content().primary, Some(ContentCard::Media(_))));
        reduce(&mut state, IslandEvent::MediaPausedElapsed, 7.0);
        assert!(state.media.is_none());
        assert_eq!(state.layout(), LayoutTarget::single(GLANCE_CLOCK));
        // A repeated pause report does not bring it back.
        reduce(&mut state, IslandEvent::SetMedia(Some(paused)), 8.0);
        assert!(state.media.is_none());
    }

    #[test]
    fn the_stats_glance_repaints_only_on_visible_changes() {
        let mut state = IslandState::default();
        reduce(&mut state, IslandEvent::SetGlance(Some(IdleGlance::Stats)), 0.0);
        assert_eq!(state.layout(), LayoutTarget::single(GLANCE_STATS));
        let changed = reduce(
            &mut state,
            IslandEvent::SetGlanceStats {
                cpu: 0.231,
                memory: 0.6,
            },
            0.1,
        );
        assert_eq!(changed, vec![Effect::ContentChanged]);
        let same = reduce(
            &mut state,
            IslandEvent::SetGlanceStats {
                cpu: 0.2312,
                memory: 0.6001,
            },
            0.2,
        );
        assert!(same.is_empty());
        reduce(&mut state, IslandEvent::SetGlance(None), 0.3);
        assert_eq!(state.layout(), LayoutTarget::HIDDEN);
    }

    #[test]
    fn the_opened_island_starts_on_the_last_music_while_the_pill_keeps_the_time() {
        let mut state = IslandState {
            calculator: Some(crate::widgets::Calculator::default()),
            glance: Some(IdleGlance::Clock),
            ..Default::default()
        };
        let mut last = media();
        last.playing = false;
        reduce(&mut state, IslandEvent::SetRemembered(Some(last)), 0.0);
        // Collapsed: the time, not the old song.
        assert_eq!(state.layout(), LayoutTarget::single(GLANCE_CLOCK));
        // Opened: the last music first, then the widgets.
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.1);
        assert_eq!(state.primary_kind(), Some(CardKind::Media));
        assert_eq!(state.layout(), LayoutTarget::single(MEDIA));
        assert!(state.widget_navigation_available());
        reduce(&mut state, IslandEvent::CycleWidget, 0.2);
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        // The choice holds across closing and reopening.
        reduce(&mut state, IslandEvent::SetExpanded(false), 0.3);
        assert_eq!(state.layout(), LayoutTarget::single(GLANCE_CLOCK));
        reduce(&mut state, IslandEvent::SetExpanded(true), 0.4);
        assert_eq!(state.primary_kind(), Some(CardKind::Calculator));
        // A stale paused session still does not pop up on the pill.
        let mut stale = media();
        stale.playing = false;
        reduce(&mut state, IslandEvent::SetExpanded(false), 0.5);
        reduce(&mut state, IslandEvent::SetMedia(Some(stale)), 0.6);
        assert!(state.media.is_none());
    }
}
