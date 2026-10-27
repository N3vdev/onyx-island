//! Draft-only, full-screen adjustment of the real island.
#[cfg(test)]
#[path = "customize_tests.rs"]
mod tests;

use crate::{ui, SettingsApp};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
use islet_core::config::GeneralConfig;

pub struct Customization {
    pub draft: GeneralConfig,
    pub top: bool,
    size: egui::Vec2,
    position: Option<egui::Pos2>,
    maximized: bool,
}

impl Customization {
    pub fn new(config: &GeneralConfig, ctx: &egui::Context) -> Self {
        let viewport = ctx.input(|input| input.viewport().clone());
        Self {
            draft: config.clone(),
            top: config.position.home_is_top(),
            size: viewport
                .inner_rect
                .map_or(egui::vec2(1040.0, 820.0), |rect| rect.size()),
            position: viewport.outer_rect.map(|rect| rect.min),
            maximized: viewport.maximized.unwrap_or(false),
        }
    }

    pub fn restore(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.size));
        if let Some(position) = self.position {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.maximized));
    }
}

pub fn show(app: &mut SettingsApp, ctx: &egui::Context) {
    let Some(custom) = &mut app.customization else {
        return;
    };
    let mut apply = false;
    let mut cancel = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let mut reset = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(Color32::from_rgb(15, 16, 18)))
        .show(ctx, |ui| {
            let canvas = ui.max_rect();
            ui::dot_grid(ui, canvas);
            let band = canvas.height() * 0.05;
            let edge_y = if custom.top {
                canvas.top() + band
            } else {
                canvas.bottom() - band
            };
            let guide_color = Color32::from_rgb(67, 77, 71);
            ui.painter().line_segment(
                [
                    egui::pos2(canvas.left(), edge_y),
                    egui::pos2(canvas.right(), edge_y),
                ],
                Stroke::new(1.0, guide_color),
            );
            ui.painter().text(
                egui::pos2(
                    canvas.left() + 24.0,
                    edge_y + if custom.top { 12.0 } else { -12.0 },
                ),
                egui::Align2::LEFT_CENTER,
                "5% spacing limit",
                egui::FontId::proportional(11.0),
                Color32::from_gray(155),
            );
            let (margin, scale) = if custom.top {
                (
                    custom.draft.top_margin_percent,
                    custom.draft.top_expanded_scale,
                )
            } else {
                (
                    custom.draft.bottom_margin_percent,
                    custom.draft.bottom_expanded_scale,
                )
            };
            // The anchor covers the monitor; the island uses exactly these dimensions
            // for both its visual preview and the configured percentage calculation.
            app.preview
                .update(ctx, ctx.screen_rect(), Some((custom.top, margin, scale)));
            egui::Area::new("customize-controls".into())
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(25, 26, 29))
                        .stroke(Stroke::new(1.0, Color32::from_gray(58)))
                        .corner_radius(CornerRadius::same(14))
                        .inner_margin(Margin::same(26))
                        .show(ui, |ui| {
                            ui.set_width(390.0);
                            ui.label(RichText::new("Customize your island").heading());
                            ui.label(
                                RichText::new(
                                    "Fine-tune each edge. Your compact island stays the same size.",
                                )
                                .small()
                                .color(Color32::from_gray(175)),
                            );
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut custom.top, true, "Top edge");
                                ui.selectable_value(&mut custom.top, false, "Bottom edge");
                            });
                            ui.add_space(16.0);
                            let (margin, scale) = if custom.top {
                                (
                                    &mut custom.draft.top_margin_percent,
                                    &mut custom.draft.top_expanded_scale,
                                )
                            } else {
                                (
                                    &mut custom.draft.bottom_margin_percent,
                                    &mut custom.draft.bottom_expanded_scale,
                                )
                            };
                            ui.label(
                                RichText::new(if custom.top {
                                    "Top spacing"
                                } else {
                                    "Bottom spacing"
                                })
                                .strong(),
                            );
                            ui.add(
                                egui::Slider::new(margin, 0.0..=5.0)
                                    .suffix("%")
                                    .fixed_decimals(1)
                                    .text("of screen height"),
                            );
                            ui.add_space(14.0);
                            ui.label(RichText::new("Expanded size").strong());
                            let mut percent = *scale * 100.0;
                            if ui
                                .add(
                                    egui::Slider::new(&mut percent, 80.0..=125.0)
                                        .suffix("%")
                                        .integer(),
                                )
                                .changed()
                            {
                                *scale = percent / 100.0;
                            }
                            ui.label(
                                RichText::new("Text, artwork, and controls scale together.")
                                    .small()
                                    .color(Color32::from_gray(175)),
                            );
                            ui.add_space(18.0);
                            ui.separator();
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                reset = ui.button("Reset edge").clicked();
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        apply = ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("Apply changes")
                                                        .color(Color32::from_gray(20)),
                                                )
                                                .fill(Color32::from_gray(235)),
                                            )
                                            .clicked();
                                        cancel |= ui.button("Cancel").clicked();
                                    },
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "Esc to cancel · Changes apply to this display position",
                                )
                                .small()
                                .color(Color32::from_gray(155)),
                            );
                        });
                });
        });
    if reset {
        if custom.top {
            custom.draft.top_margin_percent = 0.0;
            custom.draft.top_expanded_scale = 1.0;
        } else {
            custom.draft.bottom_margin_percent = 0.0;
            custom.draft.bottom_expanded_scale = 1.0;
        }
    }
    if apply || cancel {
        if let Some(custom) = app.customization.take() {
            if apply {
                app.config.general.top_margin_percent = custom.draft.top_margin_percent;
                app.config.general.bottom_margin_percent = custom.draft.bottom_margin_percent;
                app.config.general.top_expanded_scale = custom.draft.top_expanded_scale;
                app.config.general.bottom_expanded_scale = custom.draft.bottom_expanded_scale;
            }
            custom.restore(ctx);
            ctx.request_repaint();
        }
    }
}
