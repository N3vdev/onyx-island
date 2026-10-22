//! Layout and look of the settings window.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout,
    Margin, RichText, Stroke, StrokeKind, TextStyle,
};
use islet_core::config::{IdleStyle, MonitorChoice, ScreenPosition};

use crate::{Page, SettingsApp};

const BG: Color32 = Color32::from_rgb(10, 10, 12);
const SIDEBAR: Color32 = Color32::from_rgb(16, 16, 19);
const CARD: Color32 = Color32::from_rgb(22, 22, 26);
const CARD_LINE: Color32 = Color32::from_rgb(36, 36, 42);
const TEXT: Color32 = Color32::from_rgb(236, 237, 242);
const MUTED: Color32 = Color32::from_rgb(140, 145, 160);
const ACCENT: Color32 = Color32::from_rgb(92, 132, 255);
const DANGER: Color32 = Color32::from_rgb(255, 110, 110);

pub fn install_theme(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // Match Windows 11 (and the island) when the font is installed.
    for (name, path) in [
        ("segoe-variable", "C:\\Windows\\Fonts\\SegUIVar.ttf"),
        ("segoe", "C:\\Windows\\Fonts\\segoeui.ttf"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
                family.insert(0, name.to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(24.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 28.0;

    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = Color32::from_rgb(14, 14, 17);
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(8);
    }
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(32, 32, 38);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(32, 32, 38);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(44, 44, 52);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(44, 44, 52);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(54, 54, 64);
    ctx.set_style(style);
}

pub fn draw(app: &mut SettingsApp, ctx: &egui::Context) {
    egui::SidePanel::left("nav")
        .exact_width(196.0)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(SIDEBAR)
                .inner_margin(Margin::same(16)),
        )
        .show(ctx, |ui| sidebar(app, ui));

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(BG)
                .inner_margin(Margin::symmetric(28, 24)),
        )
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    match app.page {
                        Page::General => general(app, ui),
                        Page::NowPlaying => now_playing(app, ui),
                        Page::About => about(app, ui),
                    }
                    if let Some(status) = &app.status {
                        ui.add_space(8.0);
                        ui.label(RichText::new(status).color(DANGER));
                    }
                });
        });
}

fn sidebar(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        // A tiny island as the logo.
        let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 14.0), egui::Sense::hover());
        ui.painter().rect(
            rect,
            CornerRadius::same(7),
            Color32::BLACK,
            Stroke::new(1.0, Color32::from_white_alpha(28)),
            StrokeKind::Inside,
        );
        ui.painter()
            .circle_filled(egui::pos2(rect.right() - 7.0, rect.center().y), 2.5, ACCENT);
        ui.label(RichText::new("Islet").size(18.0).strong());
    });
    ui.add_space(18.0);
    for (page, label) in [
        (Page::General, "General"),
        (Page::NowPlaying, "Now Playing"),
        (Page::About, "About"),
    ] {
        nav_item(ui, &mut app.page, page, label);
    }
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        ui.label(
            RichText::new("Changes apply instantly.")
                .small()
                .color(MUTED),
        );
    });
}

fn nav_item(ui: &mut egui::Ui, current: &mut Page, page: Page, label: &str) {
    let selected = *current == page;
    let desired = egui::vec2(ui.available_width(), 34.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    if response.clicked() {
        *current = page;
    }
    let fill = if selected {
        Color32::from_rgb(34, 36, 46)
    } else if response.hovered() {
        Color32::from_rgb(26, 26, 31)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    if selected {
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 4.0, rect.top() + 9.0),
            egui::vec2(3.0, 16.0),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), ACCENT);
    }
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        if selected { TEXT } else { MUTED },
    );
}

fn header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).heading().strong());
    ui.label(RichText::new(subtitle).color(MUTED));
    ui.add_space(12.0);
}

fn card(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).small().strong().color(MUTED));
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
    ui.add_space(14.0);
}

/// A setting: title and description on the left, the control on the right.
fn row(ui: &mut egui::Ui, title: &str, description: &str, control: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let text_width = (ui.available_width() - 250.0).max(180.0);
        ui.vertical(|ui| {
            ui.set_width(text_width);
            ui.label(RichText::new(title).color(TEXT));
            if !description.is_empty() {
                ui.label(RichText::new(description).small().color(MUTED));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, CARD_LINE);
}

/// An iOS-style switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(44.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let off = Color32::from_rgb(52, 52, 60);
        let fill = Color32::from_rgb(
            egui::lerp(f32::from(off.r())..=f32::from(ACCENT.r()), t) as u8,
            egui::lerp(f32::from(off.g())..=f32::from(ACCENT.g()), t) as u8,
            egui::lerp(f32::from(off.b())..=f32::from(ACCENT.b()), t) as u8,
        );
        let radius = rect.height() * 0.5;
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius as u8), fill);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, Color32::WHITE);
    }
    response
}

/// Two or more options as a segmented control. Drawn right to left (it
/// sits in a right-aligned row), so options are passed in reverse.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) {
    ui.spacing_mut().item_spacing.x = 4.0;
    for (option, label) in options.iter().rev() {
        ui.selectable_value(value, *option, *label);
    }
}

fn general(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "General", "Where the island lives and how it behaves.");
    let general = &mut app.config.general;
    card(ui, "PLACEMENT", |ui| {
        row(ui, "Position", "Screen edge the island sits on.", |ui| {
            segmented(
                ui,
                &mut general.position,
                &[
                    (ScreenPosition::Bottom, "Bottom"),
                    (ScreenPosition::Top, "Top"),
                ],
            );
        });
        divider(ui);
        row(
            ui,
            "Monitor",
            "Stay on the primary display, or follow the mouse.",
            |ui| {
                segmented(
                    ui,
                    &mut general.monitor,
                    &[
                        (MonitorChoice::Primary, "Primary"),
                        (MonitorChoice::FollowCursor, "Follow cursor"),
                    ],
                );
            },
        );
    });
    card(ui, "BEHAVIOUR", |ui| {
        row(
            ui,
            "When idle",
            "Disappear completely, or leave a thin sliver.",
            |ui| {
                segmented(
                    ui,
                    &mut general.idle_style,
                    &[(IdleStyle::Hidden, "Hidden"), (IdleStyle::Sliver, "Sliver")],
                );
            },
        );
        divider(ui);
        row(
            ui,
            "Hide in full screen",
            "Get out of the way of games and full-screen video.",
            |ui| {
                toggle(ui, &mut general.hide_in_fullscreen);
            },
        );
        divider(ui);
        row(
            ui,
            "Dock over apps",
            "When a window is behind the island, glide it to the bottom edge, out of the way of tabs and title bars. It glides back when the spot is clear. Hover to expand.",
            |ui| {
                toggle(ui, &mut general.tuck_behind_windows);
            },
        );
        divider(ui);
        row(
            ui,
            "Start with Windows",
            "Launch Islet when you sign in.",
            |ui| {
                toggle(ui, &mut general.start_with_windows);
            },
        );
    });
}

fn now_playing(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(
        ui,
        "Now Playing",
        "Music from apps and music sites, never videos.",
    );
    let media = &mut app.config.media;
    card(ui, "MEDIA", |ui| {
        row(
            ui,
            "Show now playing",
            "Album art, controls, and bars.",
            |ui| {
                toggle(ui, &mut media.enabled);
            },
        );
        divider(ui);
        row(
            ui,
            "Music only",
            "Ignore videos. Browsers count only on the music sites below; youtube.com only for music videos.",
            |ui| {
                toggle(ui, &mut media.music_only);
            },
        );
        divider(ui);
        row(
            ui,
            "Close after pausing",
            "How long a paused song stays before the island closes. Hovering keeps it open.",
            |ui| {
                ui.add(
                    egui::Slider::new(&mut media.pause_hide_seconds, 1..=300)
                        .logarithmic(true)
                        .suffix(" s"),
                );
            },
        );
        divider(ui);
        row(
            ui,
            "Live audio bars",
            "Bars follow the sound. Off uses a gentle animation instead.",
            |ui| {
                toggle(ui, &mut media.real_bars);
            },
        );
        divider(ui);
        row(
            ui,
            "Tint from album art",
            "Colour the bars and progress.",
            |ui| {
                toggle(ui, &mut media.color_tint);
            },
        );
    });

    let mut remove = None;
    let mut add = false;
    let mut reset = false;
    card(ui, "MUSIC SITES", |ui| {
        ui.label(
            RichText::new(
                "Browser playback counts as music on these sites. Add a site by name or URL; \
                 it is matched against the site name in the tab title.",
            )
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (index, site) in app.config.media.music_sites.iter().enumerate() {
                egui::Frame::new()
                    .fill(Color32::from_rgb(32, 32, 38))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin {
                        left: 12,
                        right: 4,
                        top: 3,
                        bottom: 3,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(site);
                            let close = ui.add(
                                egui::Button::new(RichText::new("✕").small().color(MUTED))
                                    .frame(false),
                            );
                            if close.on_hover_text("Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                    });
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut app.new_site)
                    .hint_text("e.g. jiosaavn.com or https://music.example.com")
                    .desired_width((ui.available_width() - 190.0).max(160.0)),
            );
            let submitted =
                edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("Add site").clicked() || submitted {
                add = true;
            }
            if ui
                .add(egui::Button::new(RichText::new("Reset").color(MUTED)).frame(false))
                .on_hover_text("Restore the default list")
                .clicked()
            {
                reset = true;
            }
        });
    });
    if let Some(index) = remove {
        app.config.media.music_sites.remove(index);
    }
    if add {
        app.add_site();
    }
    if reset {
        app.reset_sites();
    }
}

fn about(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "About", "A Dynamic Island for Windows.");
    card(ui, "ISLET", |ui| {
        row(ui, "Version", "", |ui| {
            ui.label(RichText::new(env!("CARGO_PKG_VERSION")).color(MUTED));
        });
        divider(ui);
        row(
            ui,
            "Settings file",
            "Edited by this window; safe to edit by hand.",
            |ui| {
                if ui.button("Show file").clicked() {
                    let _ = std::process::Command::new("explorer")
                        .arg("/select,")
                        .arg(&app.config_file)
                        .spawn();
                }
            },
        );
    });
    card(ui, "PRIVACY", |ui| {
        ui.label(
            RichText::new(
                "Everything runs on this PC. To tell music from video, Islet reads the titles of \
                 open browser tabs in memory when a track changes; titles are never stored or sent \
                 anywhere.",
            )
            .color(MUTED),
        );
    });
}
