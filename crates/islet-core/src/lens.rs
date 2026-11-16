//! The island as Onyx Lens's toolbar. While the lens overlay is open, the
//! island moves to the top of the screen and opens into the lens's tools;
//! clicking one there picks it in the lens.
//!
//! The lens reports which tools it shows and which are on as two bitmasks
//! (bit `i` is `LensTool::ALL[i]`); the island lays them out here. Pure
//! geometry, so the window and the renderer agree on where each button is.

use crate::island::PillSize;
use crate::Rect;

/// A lens tool, in the order the lens numbers them (its toolbar's order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LensTool {
    Lasso,
    Rectangle,
    FullScreen,
    Eyedropper,
    Measure,
    Snap,
    Brush,
    Erase,
}

impl LensTool {
    pub const ALL: [Self; 8] = [
        Self::Lasso,
        Self::Rectangle,
        Self::FullScreen,
        Self::Eyedropper,
        Self::Measure,
        Self::Snap,
        Self::Brush,
        Self::Erase,
    ];

    /// The bit and index the lens uses for this tool.
    #[must_use]
    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|tool| *tool == self).unwrap_or(0) as u8
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Lasso => "Lasso",
            Self::Rectangle => "Rectangle",
            Self::FullScreen => "Full screen",
            Self::Eyedropper => "Eyedropper",
            Self::Measure => "Measure",
            Self::Snap => "Snap",
            Self::Brush => "Brush",
            Self::Erase => "Erase",
        }
    }

    /// The toolbar's groups: drawing, the designer tools, then the brush.
    fn group(self) -> u8 {
        match self {
            Self::Lasso | Self::Rectangle | Self::FullScreen => 0,
            Self::Eyedropper | Self::Measure | Self::Snap => 1,
            Self::Brush | Self::Erase => 2,
        }
    }
}

/// The index the island sends for its close button.
pub const CLOSE_INDEX: u8 = 8;

/// Something on the lens pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensItem {
    /// The lens icon and name, shown while the lens has no tools out
    /// (reading text, or showing a result). Not a button.
    Title,
    Tool(LensTool),
    /// Closes the lens, like Esc.
    Close,
}

/// Where an item sits on the pill, in the pill's own logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensSlot {
    pub item: LensItem,
    pub rect: Rect,
    /// On (the chosen drawing tool, or Snap while it is on): shown with its
    /// name beside the icon.
    pub pressed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LensCard {
    /// Bit `i` set: `LensTool::ALL[i]` is on the lens's toolbar now.
    pub visible: u8,
    /// Bit `i` set: that tool is on.
    pub pressed: u8,
}

/// Padding inside the pill around the rows of buttons.
const PAD: f32 = 5.0;
const BUTTON: f32 = 34.0;
const GAP: f32 = 2.0;
const ROW_GAP: f32 = 4.0;
/// Room for a divider between two groups.
const DIVIDER: f32 = 13.0;
/// The widest a row may be, inside the 720 dp island window.
const MAX_ROW: f32 = 680.0;
/// A labelled button: padding, the 16 dp icon and a gap, then the name.
const LABEL_CHROME: f32 = 10.0 + 16.0 + 6.0 + 12.0;

impl LensTool {
    /// The name's width at the island's 12 px button text, with a little
    /// to spare (Segoe UI Variable; layout here cannot measure text).
    fn label_width(self) -> f32 {
        match self {
            Self::Lasso => 38.0,
            Self::Rectangle => 64.0,
            Self::FullScreen => 72.0,
            Self::Eyedropper => 74.0,
            Self::Measure => 58.0,
            Self::Snap => 32.0,
            Self::Brush => 40.0,
            Self::Erase => 38.0,
        }
    }
}

impl LensItem {
    fn width(self) -> f32 {
        match self {
            Self::Title => LABEL_CHROME + 58.0,
            Self::Tool(tool) => LABEL_CHROME + tool.label_width(),
            Self::Close => BUTTON,
        }
    }
}

fn group_width(group: &[LensItem]) -> f32 {
    group.iter().map(|item| item.width()).sum::<f32>() + GAP * group.len().saturating_sub(1) as f32
}

fn row_width(row: &[Vec<LensItem>]) -> f32 {
    row.iter().map(|group| group_width(group)).sum::<f32>() + DIVIDER * row.len().saturating_sub(1) as f32
}

impl LensCard {
    #[must_use]
    pub fn new(visible: u8, pressed: u8) -> Self {
        Self { visible, pressed: pressed & visible }
    }

    #[must_use]
    pub fn shows(self, tool: LensTool) -> bool {
        self.visible & (1 << tool.index()) != 0
    }

    #[must_use]
    pub fn is_pressed(self, tool: LensTool) -> bool {
        self.pressed & (1 << tool.index()) != 0
    }

    /// The buttons in their groups (drawing, designer, brush, then close).
    fn groups(self) -> Vec<Vec<LensItem>> {
        let mut groups: Vec<Vec<LensItem>> = Vec::with_capacity(4);
        if self.visible == 0 {
            groups.push(vec![LensItem::Title]);
        }
        let mut last_group = None;
        for tool in LensTool::ALL.into_iter().filter(|tool| self.shows(*tool)) {
            if last_group != Some(tool.group()) {
                groups.push(Vec::new());
                last_group = Some(tool.group());
            }
            if let Some(group) = groups.last_mut() {
                group.push(LensItem::Tool(tool));
            }
        }
        groups.push(vec![LensItem::Close]);
        groups
    }

    /// The groups packed into rows no wider than the window allows, each
    /// group kept whole, like the lens's own toolbar wrapping. Close stays
    /// with the last group, never alone on a row.
    fn rows(self) -> Vec<Vec<Vec<LensItem>>> {
        let groups = self.groups();
        let close = group_width(&[LensItem::Close]) + DIVIDER;
        let last = groups.len().saturating_sub(1);
        let mut rows: Vec<Vec<Vec<LensItem>>> = vec![Vec::new()];
        let mut width = 0.0;
        for (index, group) in groups.into_iter().enumerate() {
            let own = group_width(&group);
            let with_close = own + if index + 1 == last { close } else { 0.0 };
            if index != last && width > 0.0 && width + DIVIDER + with_close > MAX_ROW {
                rows.push(Vec::new());
                width = own;
            } else {
                width += own + if width > 0.0 { DIVIDER } else { 0.0 };
            }
            if let Some(row) = rows.last_mut() {
                row.push(group);
            }
        }
        rows
    }

    /// Every item with where it sits (rows centred in the pill), the
    /// dividers' centres, and the pill's size.
    fn arrange(self) -> (Vec<LensSlot>, Vec<(f32, f32)>, PillSize) {
        let rows = self.rows();
        let inner = rows.iter().map(|row| row_width(row)).fold(0.0, f32::max);
        let width = inner + PAD * 2.0;
        let height =
            PAD * 2.0 + BUTTON * rows.len() as f32 + ROW_GAP * rows.len().saturating_sub(1) as f32;
        let mut slots = Vec::with_capacity(10);
        let mut dividers = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let y = PAD + (BUTTON + ROW_GAP) * index as f32;
            let mut x = (width - row_width(row)) * 0.5;
            for (group_index, group) in row.iter().enumerate() {
                if group_index > 0 {
                    dividers.push((x + DIVIDER * 0.5, y + BUTTON * 0.5));
                    x += DIVIDER;
                }
                for (item_index, item) in group.iter().enumerate() {
                    if item_index > 0 {
                        x += GAP;
                    }
                    let pressed = matches!(item, LensItem::Tool(tool) if self.is_pressed(*tool));
                    slots.push(LensSlot { item: *item, rect: Rect::new(x, y, item.width(), BUTTON), pressed });
                    x += item.width();
                }
            }
        }
        // One row is a capsule; two keep that rounding at the corners.
        (slots, dividers, PillSize::new(width, height, PAD + BUTTON * 0.5))
    }

    #[must_use]
    pub fn slots(self) -> Vec<LensSlot> {
        self.arrange().0
    }

    /// The pill this card needs.
    #[must_use]
    pub fn size(self) -> PillSize {
        self.arrange().2
    }

    /// The button under a point on the pill, if any.
    #[must_use]
    pub fn item_at(self, x: f32, y: f32) -> Option<LensItem> {
        self.slots()
            .into_iter()
            .find(|slot| slot.item != LensItem::Title && slot.rect.inflate(GAP * 0.5).contains(x, y))
            .map(|slot| slot.item)
    }

    /// Divider centres, between groups on a row.
    #[must_use]
    pub fn dividers(self) -> Vec<(f32, f32)> {
        self.arrange().1
    }
}

impl LensItem {
    /// The index sent back to the lens for a click.
    #[must_use]
    pub fn index(self) -> Option<u8> {
        match self {
            Self::Title => None,
            Self::Tool(tool) => Some(tool.index()),
            Self::Close => Some(CLOSE_INDEX),
        }
    }
}

/// Packs the lens's report into one message parameter: the visible and
/// pressed masks and whether the lens is open.
#[must_use]
pub fn pack(open: bool, visible: u8, pressed: u8) -> usize {
    usize::from(visible) | usize::from(pressed) << 8 | usize::from(open) << 16
}

/// The lens's report, or `None` when it closed.
#[must_use]
pub fn unpack(value: usize) -> Option<LensCard> {
    (value & 1 << 16 != 0).then(|| LensCard::new(value as u8, (value >> 8) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: u8 = 0b0011_1111;

    #[test]
    fn report_round_trips_and_closing_clears_it() {
        let card = unpack(pack(true, ALL, 0b10_0001)).unwrap();
        assert_eq!(card, LensCard { visible: ALL, pressed: 0b10_0001 });
        assert_eq!(unpack(pack(false, ALL, 1)), None);
        // A tool cannot be on and hidden.
        assert_eq!(LensCard::new(0b1, 0b11).pressed, 0b1);
    }

    #[test]
    fn every_tool_is_labelled_and_groups_are_divided() {
        let card = LensCard::new(ALL, 1);
        let slots = card.slots();
        assert_eq!(slots.len(), 7);
        assert_eq!(slots[0].item, LensItem::Tool(LensTool::Lasso));
        assert!(slots[0].pressed && !slots[1].pressed);
        // A name on every tool: each is wider than the icon-only close.
        assert!(slots[..6].iter().all(|slot| slot.rect.width > BUTTON));
        assert_eq!(slots.last().unwrap().item, LensItem::Close);
        // Between drawing and designer tools, and before close.
        assert_eq!(card.dividers().len(), 2);
        let size = card.size();
        assert!(size.width <= MAX_ROW + PAD * 2.0, "fits the island window: {}", size.width);
        assert_eq!(size.height, BUTTON + PAD * 2.0);
    }

    #[test]
    fn brush_tools_wrap_to_a_second_row_when_the_first_is_full() {
        let card = LensCard::new(0xFF, 1 << LensTool::Brush.index());
        let slots = card.slots();
        let size = card.size();
        assert!(size.width <= MAX_ROW + PAD * 2.0, "{}", size.width);
        assert!(size.height > BUTTON * 2.0, "two rows: {}", size.height);
        let brush = slots.iter().find(|slot| slot.item == LensItem::Tool(LensTool::Brush)).unwrap();
        let lasso = slots.iter().find(|slot| slot.item == LensItem::Tool(LensTool::Lasso)).unwrap();
        assert!(brush.rect.y > lasso.rect.y);
        // Close sits with the brush tools, not alone.
        let close = slots.iter().find(|slot| slot.item == LensItem::Close).unwrap();
        assert_eq!(close.rect.y, brush.rect.y);
        // Nothing overlaps, and everything is inside the pill.
        for (index, a) in slots.iter().enumerate() {
            assert!(a.rect.x >= 0.0 && a.rect.right() <= size.width && a.rect.bottom() <= size.height);
            for b in &slots[index + 1..] {
                let apart = a.rect.right() <= b.rect.x
                    || b.rect.right() <= a.rect.x
                    || a.rect.bottom() <= b.rect.y
                    || b.rect.bottom() <= a.rect.y;
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn clicks_find_buttons_and_the_title_is_not_one() {
        let card = LensCard::new(ALL, 1);
        for slot in card.slots() {
            let (x, y) = slot.rect.center();
            assert_eq!(card.item_at(x, y), Some(slot.item));
        }
        assert_eq!(card.item_at(1.0, 1.0), None);

        let reading = LensCard::new(0, 0);
        let slots = reading.slots();
        assert_eq!(slots.iter().map(|slot| slot.item).collect::<Vec<_>>(), [LensItem::Title, LensItem::Close]);
        let (x, y) = slots[0].rect.center();
        assert_eq!(reading.item_at(x, y), None);
        assert_eq!(LensItem::Close.index(), Some(CLOSE_INDEX));
        assert_eq!(LensItem::Tool(LensTool::Erase).index(), Some(7));
    }
}
