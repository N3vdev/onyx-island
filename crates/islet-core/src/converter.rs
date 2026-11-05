//! Expanded-only, event-driven file converter presentation and hit geometry.
use crate::Rect;

pub const WIDTH: f32 = 544.0;
pub const HEIGHT: f32 = 184.0;
pub const CLOSE: Rect = Rect::new(500.0, 16.0, 28.0, 28.0);
pub const FOOTER: Rect = Rect::new(196.0, 146.0, 324.0, 28.0);
pub const PREVIEW: Rect = Rect::new(4.0, 4.0, 176.0, 176.0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversionOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionStage {
    Loading,
    Ready,
    Running,
    Complete,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConverterCard {
    pub preview: Option<crate::media::ArtImage>,
    pub title: String,
    pub subtitle: String,
    pub options: Vec<ConversionOption>,
    pub stage: ConversionStage,
    pub progress: u8,
    pub fully_succeeded: bool,
    pub message: String,
    pub dragging: bool,
    pub hover: Option<ConverterAction>,
    pub keyboard_focus: Option<ConverterAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConverterAction {
    Format(usize),
    Close,
    Footer,
}

impl ConverterCard {
    pub fn new(title: String, subtitle: String) -> Self {
        Self {
            preview: None,
            title,
            subtitle,
            options: Vec::new(),
            stage: ConversionStage::Loading,
            progress: 0,
            fully_succeeded: false,
            message: "Finding available formats…".into(),
            dragging: true,
            hover: None,
            keyboard_focus: None,
        }
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<ConverterAction> {
        if CLOSE.contains(x, y) {
            return Some(ConverterAction::Close);
        }
        if self.stage == ConversionStage::Ready {
            return (0..self.options.len())
                .find(|&i| option_rect(i, self.options.len()).contains(x, y))
                .map(ConverterAction::Format);
        }
        if matches!(
            self.stage,
            ConversionStage::Running | ConversionStage::Complete | ConversionStage::Error
        ) && FOOTER.contains(x, y)
        {
            return Some(ConverterAction::Footer);
        }
        None
    }
}

pub fn option_rect(index: usize, count: usize) -> Rect {
    let cols = count.clamp(1, 4);
    let width = (324.0 - (cols - 1) as f32 * 8.0) / cols as f32;
    Rect::new(
        196.0 + (index % cols) as f32 * (width + 8.0),
        70.0 + (index / cols) as f32 * 44.0,
        width,
        36.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_format_is_inside_the_scaled_card_and_has_a_distinct_hit_target() {
        for count in 1..=8 {
            let mut card = ConverterCard::new("Sample.pdf".into(), "PDF document".into());
            card.stage = ConversionStage::Ready;
            card.options = (0..count)
                .map(|i| ConversionOption {
                    id: i.to_string(),
                    label: i.to_string(),
                })
                .collect();
            for i in 0..count {
                let r = option_rect(i, count);
                assert!(
                    r.x >= 24.0
                        && r.y >= 52.0
                        && r.x + r.width <= WIDTH - 24.0
                        && r.y + r.height <= HEIGHT - 8.0
                );
                assert_eq!(
                    card.action_at(r.x + r.width / 2.0, r.y + 16.0),
                    Some(ConverterAction::Format(i))
                );
            }
        }
    }
    #[test]
    fn running_cards_cannot_start_another_conversion() {
        let mut card = ConverterCard::new("Sample.pdf".into(), "PDF document".into());
        card.stage = ConversionStage::Running;
        assert_eq!(card.action_at(60.0, 88.0), None);
        assert_eq!(card.action_at(CLOSE.x + 10.0, CLOSE.y + 10.0), Some(ConverterAction::Close));
        assert_eq!(card.action_at(FOOTER.x + 10.0, FOOTER.y + 10.0), Some(ConverterAction::Footer));
    }
}
