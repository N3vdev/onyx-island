//! The island's platform-free state machine, size table, and content model.
//!
//! The UI thread owns an [`IslandState`], feeds it events, and sends the
//! resulting [`LayoutTarget`] and [`ContentSnapshot`] to the render thread.
//! Timers are described as [`Effect`]s rather than run here, which keeps the
//! reducer deterministic and easy to test.

use crate::config::IdleStyle;
use crate::geometry::Rect;
use crate::media::{ArtImage, Timeline};

pub const HOVER_EXPAND_MS: u32 = 250;
pub const HOVER_COLLAPSE_MS: u32 = 400;
pub const DONE_VISIBLE_MS: u32 = 4_000;
/// Default delay before paused media closes (`media.pause_hide_seconds`).
pub const MEDIA_PAUSED_MS: u32 = 5 * 1_000;
pub const SPLIT_GAP_DP: f32 = 6.0;
/// Gap between the island and the screen edge (work area), in DIPs.
pub const EDGE_GAP_DP: f32 = 8.0;

/// Sizes from PLAN.md §4.4, in logical pixels (DIPs).
pub const COMPACT: PillSize = PillSize::new(220.0, 36.0, 18.0);
pub const SPLIT_BUBBLE: PillSize = PillSize::new(36.0, 36.0, 18.0);
pub const NOTIFICATION: PillSize = PillSize::new(380.0, 76.0, 30.0);
pub const MEDIA: PillSize = PillSize::new(400.0, 168.0, 38.0);
pub const DICTATION: PillSize = PillSize::new(260.0, 44.0, 22.0);
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
    pub main: PillSize,
    pub bubble: Option<PillSize>,
    pub gap: f32,
    /// Distance from the screen edge. Springs to 0 when tucked.
    pub inset: f32,
    pub visible: bool,
}

impl LayoutTarget {
    pub const HIDDEN: Self = Self {
        main: HIDDEN,
        bubble: None,
        gap: 0.0,
        inset: EDGE_GAP_DP,
        visible: false,
    };

    #[must_use]
    pub const fn single(size: PillSize) -> Self {
        Self {
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
    Recording,
    Transcribing,
}

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
    pub level: f32,
    pub phase: DictationPhase,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentCard {
    Media(MediaCard),
    Notification(NotificationCard),
    Agent(AgentCard),
    Dictation(DictationCard),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Media,
    Notification,
    Agent,
    Dictation,
}

impl ContentCard {
    #[must_use]
    pub fn kind(&self) -> CardKind {
        match self {
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
            (Self::Media(a), Self::Media(b)) => a.same_track(b) && a.art == b.art,
            (Self::Dictation(a), Self::Dictation(b)) => a.phase == b.phase,
            (a, b) => a == b,
        }
    }

    #[must_use]
    pub fn animates(&self) -> bool {
        match self {
            Self::Media(media) => media.playing,
            Self::Dictation(dictation) => dictation.phase == DictationPhase::Recording,
            Self::Notification(_) | Self::Agent(_) => false,
        }
    }
}

/// Stable content sent from the UI thread to the render thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentSnapshot {
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

#[derive(Clone, Debug)]
pub struct IslandState {
    pub dictation: Option<DictationCard>,
    pub needs_input: Option<NotificationCard>,
    pub done: Option<NotificationCard>,
    pub running: Option<AgentCard>,
    pub media: Option<MediaCard>,
    /// The paused track was closed after the pause delay; ignore paused
    /// updates until something plays again.
    pub media_dismissed: bool,
    /// How long paused media stays before closing.
    pub media_paused_ms: u32,
    pub idle_style: IdleStyle,
    /// An app window is behind the island: dock at the bottom edge.
    pub tucked: bool,
    pub expanded: bool,
    pub pointer_inside: bool,
    pub done_deadline: Option<f64>,
}

impl Default for IslandState {
    fn default() -> Self {
        Self {
            dictation: None,
            needs_input: None,
            done: None,
            running: None,
            media: None,
            media_dismissed: false,
            media_paused_ms: MEDIA_PAUSED_MS,
            idle_style: IdleStyle::Hidden,
            tucked: false,
            expanded: false,
            pointer_inside: false,
            done_deadline: None,
        }
    }
}

impl IslandState {
    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.dictation.is_some()
            || self.needs_input.is_some()
            || self.done.is_some()
            || self.running.is_some()
            || self.media.is_some()
    }

    #[must_use]
    pub fn persistent_count(&self) -> usize {
        usize::from(self.needs_input.is_some())
            + usize::from(self.done.is_some())
            + usize::from(self.running.is_some())
            + usize::from(self.media.is_some())
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
        if let Some(agent) = &self.running {
            return Some(ContentCard::Agent(agent.clone()));
        }
        self.media.clone().map(ContentCard::Media)
    }

    fn primary_kind(&self) -> Option<CardKind> {
        self.primary_card().map(|card| card.kind())
    }

    /// Picks the main and split-bubble cards. Media is preferred as the main
    /// pill so a running agent can live in the small satellite bubble.
    fn split_cards(&self) -> (Option<ContentCard>, Option<ContentCard>) {
        let main_candidates = [
            self.media.clone().map(ContentCard::Media),
            self.needs_input.clone().map(ContentCard::Notification),
            self.running.clone().map(ContentCard::Agent),
            self.done.clone().map(ContentCard::Notification),
        ];
        let bubble_candidates = [
            self.needs_input.clone().map(ContentCard::Notification),
            self.running.clone().map(ContentCard::Agent),
            self.done.clone().map(ContentCard::Notification),
            self.media.clone().map(ContentCard::Media),
        ];

        let main = main_candidates.into_iter().flatten().next();
        let main_kind = main.as_ref().map(ContentCard::kind);
        let bubble = bubble_candidates
            .into_iter()
            .flatten()
            .find(|card| Some(card.kind()) != main_kind);
        (main, bubble)
    }

    #[must_use]
    pub fn layout(&self) -> LayoutTarget {
        if self.dictation.is_some() {
            return LayoutTarget::single(DICTATION);
        }

        // Out of the way of the window behind; hovering expands as usual.
        // The pill keeps its normal shape and just docks at the other edge.
        if self.tucked && !self.expanded {
            if self.persistent_count() >= 2 {
                return LayoutTarget::split(COMPACT, SPLIT_BUBBLE);
            }
            if self.primary_kind().is_some() {
                return LayoutTarget::single(COMPACT);
            }
            return LayoutTarget::idle(self.idle_style);
        }

        if self.expanded {
            return match self.primary_kind() {
                Some(CardKind::Media) => LayoutTarget::single(MEDIA),
                Some(_) => LayoutTarget::single(NOTIFICATION),
                None => LayoutTarget::idle(self.idle_style),
            };
        }

        if self.persistent_count() >= 2 {
            return LayoutTarget::split(COMPACT, SPLIT_BUBBLE);
        }

        if self.primary_kind().is_some() {
            return LayoutTarget::single(COMPACT);
        }

        LayoutTarget::idle(self.idle_style)
    }

    #[must_use]
    pub fn content(&self) -> ContentSnapshot {
        let layout = self.layout();
        if !layout.visible {
            return ContentSnapshot::default();
        }

        let (primary, secondary) = if self.dictation.is_some() {
            (self.primary_card(), None)
        } else if layout.is_split() {
            self.split_cards()
        } else {
            (self.primary_card(), None)
        };

        let accent = primary.as_ref().map_or([120, 128, 150], accent_for_card);
        ContentSnapshot {
            primary,
            secondary,
            accent,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerKind {
    HoverExpand,
    HoverCollapse,
    DoneExpiry,
    MediaPaused,
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
    HoverEnter,
    HoverLeave,
    HoverExpandElapsed,
    HoverCollapseElapsed,
    MediaPausedElapsed,
    SetMediaPauseDelay(u32),
    Click,
    Tick { now: f64 },
    SetExpanded(bool),
    SetIdleStyle(IdleStyle),
    SetTucked(bool),
    SetDictation(Option<DictationCard>),
    SetMedia(Option<MediaCard>),
    SetNeedsInput(Option<NotificationCard>),
    SetDone(Option<NotificationCard>),
    SetRunning(Option<AgentCard>),
    ClearAll,
}

/// Pure reducer. `now` is monotonic seconds.
pub fn reduce(state: &mut IslandState, event: IslandEvent, now: f64) -> Vec<Effect> {
    match event {
        IslandEvent::HoverEnter => {
            if state.pointer_inside {
                return Vec::new();
            }
            state.pointer_inside = true;
            let mut effects = vec![Effect::CancelTimer(TimerKind::HoverCollapse)];
            if !state.expanded && state.has_activity() {
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
        IslandEvent::HoverExpandElapsed => {
            if state.pointer_inside && !state.expanded && state.has_activity() {
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
            let mut effects = Vec::new();
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
        IslandEvent::MediaPausedElapsed => {
            let paused = state.media.as_ref().is_some_and(|media| !media.playing);
            if paused && state.pointer_inside {
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
                state.done_deadline = Some(now + f64::from(DONE_VISIBLE_MS) / 1000.0);
                let mut effects = Effect::changed();
                effects.push(Effect::ArmTimer {
                    kind: TimerKind::DoneExpiry,
                    after_ms: DONE_VISIBLE_MS,
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

fn accent_for_card(card: &ContentCard) -> [u8; 3] {
    match card {
        ContentCard::Media(media) => media.accent.unwrap_or([92, 132, 255]),
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
        ContentCard::Dictation(_) => [255, 78, 92],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        reduce(
            &mut state,
            IslandEvent::SetDictation(Some(DictationCard {
                elapsed_ms: 0,
                level: 0.1,
                phase: DictationPhase::Transcribing,
            })),
            0.0,
        );
        assert!(!state.content().animates());
    }
}
