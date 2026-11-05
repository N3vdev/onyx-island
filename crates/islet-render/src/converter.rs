//! File conversion shares the island's typography, shell and finite morph.
use super::*;
use islet_core::converter::{self, ConversionStage, ConverterAction, ConverterCard};

impl Gfx {
    pub(super) unsafe fn draw_converter(
        &mut self,
        _width: f32,
        height: f32,
        card: &ConverterCard,
        alpha: f32,
    ) -> RenderResult<()> {
        if height < 60.0 {
            return Ok(());
        }
        let preview = converter::PREVIEW;
        if let Some(art) = &card.preview {
            self.draw_art(preview, islet_core::media::expanded::ART_RADIUS, Some(art), [80, 85, 94], alpha)?;
        } else {
            self.fill_round_rect(preview.x, preview.y, preview.width, preview.height, islet_core::media::expanded::ART_RADIUS, color([255,255,255], 0.085 * alpha))?;
            self.fill_round_rect(72.0, 54.0, 40.0, 48.0, 8.0, color([210,215,224], 0.17 * alpha))?;
            self.set_color(color([210,215,224], alpha));
            for y in [67.0, 77.0, 87.0] {
                self.dc.DrawLine(Vector2 { X: 81.0, Y: y }, Vector2 { X: 103.0, Y: y }, &self.brush, 2.0, None::<&ID2D1StrokeStyle>);
            }
            let extension = card.title.rsplit_once('.').map(|(_, ext)| ext).filter(|ext| ext.len() <= 7).unwrap_or("FILE").to_uppercase();
            self.draw_text(&extension, FormatId::CalculatorKey, 44.0, 106.0, 96.0, 24.0, color([190,194,202], alpha))?;
        }
        self.draw_text(
            &card.title,
            FormatId::MediaTitle,
            196.0,
            20.0,
            296.0,
            19.0,
            color([242, 243, 246], alpha),
        )?;
        self.draw_text(
            match card.stage {
                ConversionStage::Ready | ConversionStage::Loading => &card.subtitle,
                ConversionStage::Running => "Processing on this device",
                ConversionStage::Complete if card.fully_succeeded => "Conversion complete",
                ConversionStage::Complete => "Conversion stopped with saved results",
                ConversionStage::Error => "Conversion needs attention",
            },
            FormatId::Sub,
            196.0,
            41.0,
            296.0,
            16.0,
            color([166, 170, 178], alpha),
        )?;
        let close = converter::CLOSE;
        if card.hover == Some(ConverterAction::Close)
            || card.keyboard_focus == Some(ConverterAction::Close)
        {
            self.fill_round_rect(
                close.x,
                close.y,
                close.width,
                close.height,
                8.0,
                color([255, 255, 255], 0.12 * alpha),
            )?;
        }
        self.set_color(color([190, 194, 202], alpha));
        self.dc.DrawLine(
            Vector2 { X: 510.0, Y: 26.0 },
            Vector2 { X: 518.0, Y: 34.0 },
            &self.brush,
            1.5,
            None::<&ID2D1StrokeStyle>,
        );
        self.dc.DrawLine(
            Vector2 { X: 518.0, Y: 26.0 },
            Vector2 { X: 510.0, Y: 34.0 },
            &self.brush,
            1.5,
            None::<&ID2D1StrokeStyle>,
        );
        if card.stage == ConversionStage::Ready {
            for (index, option) in card.options.iter().enumerate() {
                let r = converter::option_rect(index, card.options.len());
                let hover = card.hover == Some(ConverterAction::Format(index))
                    || card.keyboard_focus == Some(ConverterAction::Format(index));
                self.fill_round_rect(
                    r.x,
                    r.y,
                    r.width,
                    r.height,
                    10.0,
                    color(
                        [255, 255, 255],
                        if hover { 0.20 * alpha } else { 0.11 * alpha },
                    ),
                )?;
                self.draw_text(
                    &option.label,
                    FormatId::CalculatorKey,
                    r.x,
                    r.y,
                    r.width,
                    r.height,
                    color([240, 241, 244], alpha),
                )?;
            }
        } else {
            let heading = match card.stage {
                ConversionStage::Loading => "Finding available formats",
                ConversionStage::Running => "Converting locally",
                ConversionStage::Complete if card.fully_succeeded => "Ready to use",
                ConversionStage::Complete => "Some results are ready",
                ConversionStage::Error => "Couldn’t convert these files",
                ConversionStage::Ready => "",
            };
            self.draw_text(
                heading,
                FormatId::MediaTitle,
                196.0,
                78.0,
                324.0,
                26.0,
                color([240, 241, 244], alpha),
            )?;
            self.draw_text(
                &card.message,
                FormatId::Sub,
                196.0,
                108.0,
                324.0,
                24.0,
                color([166, 170, 178], alpha),
            )?;
            if card.stage == ConversionStage::Running {
                self.fill_round_rect(
                    196.0,
                    140.0,
                    324.0,
                    2.0,
                    1.0,
                    color([255, 255, 255], 0.10 * alpha),
                )?;
                self.fill_round_rect(
                    196.0,
                    140.0,
                    324.0 * card.progress as f32 / 100.0,
                    2.0,
                    1.0,
                    color([164, 204, 180], alpha),
                )?;
            }
        }
        let footer = match card.stage {
            ConversionStage::Ready if card.dragging => "Drop on a format to convert",
            ConversionStage::Ready => "Will save beside originals · Originals stay unchanged",
            ConversionStage::Running => "Cancel conversion",
            ConversionStage::Complete => "Show in folder",
            ConversionStage::Error if card.options.is_empty() => "Choose other files",
            ConversionStage::Error => "Choose another format",
            ConversionStage::Loading => "Original files stay unchanged",
        };
        let interactive = matches!(
            card.stage,
            ConversionStage::Running | ConversionStage::Complete | ConversionStage::Error
        );
        let focused = card.hover == Some(ConverterAction::Footer)
            || card.keyboard_focus == Some(ConverterAction::Footer);
        if interactive {
            let primary = card.stage == ConversionStage::Complete;
            self.fill_round_rect(196.0, 146.0, 324.0, 28.0, 10.0,
                color(if primary { [164, 204, 180] } else { [255, 255, 255] },
                    alpha * if primary { if focused { 0.27 } else { 0.18 } }
                    else if focused { 0.16 } else { 0.08 }))?;
            self.draw_text(footer, FormatId::CalculatorKey, 196.0, 146.0, 324.0, 28.0,
                color(if primary { [193, 225, 205] } else { [220, 224, 230] }, alpha))?;
        }
        Ok(())
    }
}
