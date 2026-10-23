//! Layout and look of the settings window.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout,
    Margin, RichText, Stroke, TextStyle,
};
use islet_core::config::{IdleStyle, MonitorChoice, ScreenPosition};

use crate::{Page, SettingsApp};

const BG: Color32 = Color32::from_rgb(12, 9, 15);
const SIDEBAR: Color32 = Color32::from_rgb(3, 3, 4);
const CARD: Color32 = Color32::from_rgb(18, 15, 21);
const CARD_LINE: Color32 = Color32::from_rgb(46, 39, 51);
const TEXT: Color32 = Color32::from_rgb(236, 237, 242);
const MUTED: Color32 = Color32::from_rgb(170, 160, 178);
const ACCENT: Color32 = Color32::from_rgb(184, 119, 226);
const DANGER: Color32 = Color32::from_rgb(255, 110, 110);

pub fn install_theme(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    // Bundled under the SIL Open Font License; consistent on every PC.
    fonts.font_data.insert(
        "nunito".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Nunito-Regular.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "nunito".to_owned());
    fonts.font_data.insert(
        "nunito-bold".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Nunito-Bold.ttf"
        ))),
    );
    fonts.families.insert(
        FontFamily::Name("Nunito Bold".into()),
        vec!["nunito-bold".to_owned()],
    );
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Heading, bold_font(28.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 32.0;
    style.animation_time = if crate::preview::animations_enabled() {
        0.16
    } else {
        0.0
    };

    let visuals = &mut style.visuals;
    *visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = Color32::from_rgb(14, 14, 17);
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = Color32::from_rgb(67, 39, 83);
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

/// Keep Windows' native caption controls while matching the settings frame.
/// Older Windows versions retain their system caption if an attribute is unsupported.
pub fn install_titlebar_theme() {
    use windows::core::w;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    // The native window exists before eframe invokes the app creation callback.
    unsafe {
        let Ok(hwnd) = FindWindowW(None, w!("Islet Settings")) else {
            return;
        };
        let colorref = |color: Color32| {
            u32::from(color.r()) | (u32::from(color.g()) << 8) | (u32::from(color.b()) << 16)
        };
        for (attribute, value) in [
            (DWMWA_USE_IMMERSIVE_DARK_MODE, 1_u32),
            (DWMWA_CAPTION_COLOR, colorref(SIDEBAR)),
            (DWMWA_BORDER_COLOR, colorref(SIDEBAR)),
            (DWMWA_TEXT_COLOR, colorref(TEXT)),
        ] {
            let _ = DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&value as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

/// Static vertex colors: no blur pass, textures, or idle animation.
fn atmosphere(ui: &egui::Ui, rect: egui::Rect) {
    let mut mesh = egui::Mesh::default();
    const N: u32 = 64;
    for y in 0..=N {
        for x in 0..=N {
            let u = x as f32 / N as f32;
            let v = y as f32 / N as f32;
            let glow = (-((u - 0.78).powi(2) * 3.0 + v.powi(2) * 10.0)).exp();
            let color = Color32::from_rgb(
                (12.0 + 47.0 * glow) as u8,
                (9.0 + 21.0 * glow) as u8,
                (15.0 + 53.0 * glow) as u8,
            );
            let py = v * rect.height();
            let edge = py.min(rect.height() - py);
            let inset = if edge < 22.0 {
                22.0 - (22.0_f32.powi(2) - (22.0 - edge).powi(2)).sqrt()
            } else {
                0.0
            };
            mesh.colored_vertex(
                egui::pos2(
                    rect.left() + inset + u * (rect.width() - 2.0 * inset),
                    rect.top() + py,
                ),
                color,
            );
        }
    }
    for y in 0..N {
        for x in 0..N {
            let i = y * (N + 1) + x;
            mesh.add_triangle(i, i + 1, i + N + 1);
            mesh.add_triangle(i + 1, i + N + 2, i + N + 1);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
}

pub fn draw(app: &mut SettingsApp, ctx: &egui::Context) {
    let previous_page = app.page;
    egui::SidePanel::left("nav")
        .exact_width(224.0)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(SIDEBAR)
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| sidebar(app, ui));
    if previous_page != app.page {
        app.page_changed = std::time::Instant::now();
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin {
            left: 0,
            right: 8,
            top: 8,
            bottom: 8,
        }))
        .show(ctx, |ui| {
            let panel = ui.max_rect();
            ui.painter().rect_filled(panel, CornerRadius::same(22), BG);
            // Keep the mesh inside the curved panel, with a narrow dark rim.
            atmosphere(ui, panel);
            egui::Frame::new()
                .inner_margin(Margin::symmetric(30, 20))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        egui::Frame::new()
                            .fill(Color32::BLACK)
                            .stroke(Stroke::new(1.0, CARD_LINE))
                            .corner_radius(CornerRadius::same(18))
                            .inner_margin(Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.label(RichText::new("Personalize your island").size(11.0));
                            });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(if app.status.is_some() {
                                    "Check changes below"
                                } else {
                                    "Changes saved automatically"
                                })
                                .size(11.0)
                                .color(MUTED),
                            );
                        });
                    });
                    ui.add_space(10.0);
                    let (stage, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 148.0),
                        egui::Sense::hover(),
                    );
                    let slot = egui::Rect::from_min_size(
                        egui::pos2(stage.center().x - 244.0, stage.top()),
                        egui::vec2(488.0, 130.0),
                    );
                    app.preview.update(ctx, slot);
                    ui.painter().text(
                        egui::pos2(stage.center().x, stage.bottom() - 5.0),
                        egui::Align2::CENTER_CENTER,
                        if app.preview.connected() {
                            "Live preview  /  Hover to expand"
                        } else {
                            "Connecting to your island..."
                        },
                        FontId::proportional(11.0),
                        MUTED,
                    );
                    ui.add_space(18.0);
                    let mut scroll_area = egui::ScrollArea::vertical();
                    if let Some(offset) = app.initial_scroll.take() {
                        scroll_area = scroll_area.vertical_scroll_offset(offset);
                    }
                    scroll_area
                        .id_salt(match app.page {
                            Page::General => "general",
                            Page::NowPlaying => "media",
                            Page::Dictation => "dictation",
                            Page::About => "about",
                        })
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let progress = if app.animations {
                                (app.page_changed.elapsed().as_secs_f32() / 0.2).min(1.0)
                            } else {
                                1.0
                            };
                            ui.set_opacity(0.5 + 0.5 * (1.0 - (1.0 - progress).powi(3)));
                            if progress < 1.0 {
                                ctx.request_repaint();
                            }
                            match app.page {
                                Page::General => general(app, ui),
                                Page::NowPlaying => now_playing(app, ui),
                                Page::Dictation => crate::dictation_page::show(app, ui),
                                Page::About => about(app, ui),
                            }
                            if let Some(status) = &app.status {
                                ui.add_space(8.0);
                                ui.label(RichText::new(status).color(DANGER));
                            }
                        });
                });
        });
}

fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Nunito Bold".into()))
}

fn sidebar(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (logo, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
        ui.painter().rect(
            logo,
            CornerRadius::same(13),
            Color32::from_rgb(26, 20, 32),
            Stroke::new(1.0, CARD_LINE),
            egui::StrokeKind::Inside,
        );
        let pill = egui::Rect::from_center_size(logo.center(), egui::vec2(23.0, 12.0));
        ui.painter().rect_filled(pill, CornerRadius::same(6), TEXT);
        ui.painter()
            .circle_filled(logo.center() + egui::vec2(6.0, 0.0), 2.5, ACCENT);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("Islet").font(bold_font(22.0)));
            ui.label(RichText::new("Personal settings").size(11.0).color(MUTED));
        });
    });
    ui.add_space(32.0);
    let (nav, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 256.0),
        egui::Sense::hover(),
    );
    let pages = [
        (Page::General, "General", "Position & behavior"),
        (Page::NowPlaying, "Now Playing", "Playback & music sites"),
        (Page::Dictation, "Dictation", "Voice typing & models"),
        (Page::About, "About Islet", "App & privacy"),
    ];
    // Resolve input first, then move one shared selection surface between rows.
    let items: Vec<_> = pages
        .iter()
        .enumerate()
        .map(|(i, (page, label, subtitle))| {
            let rect = egui::Rect::from_min_size(
                nav.min + egui::vec2(0.0, i as f32 * 66.0),
                egui::vec2(nav.width(), 58.0),
            );
            let response = ui
                .interact(
                    rect,
                    ui.make_persistent_id(("settings-nav", i)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                app.page = *page;
            }
            (rect, response, *page, *label, *subtitle)
        })
        .collect();
    let index = pages
        .iter()
        .position(|(page, _, _)| *page == app.page)
        .unwrap_or(0) as f32;
    let slide = ui.ctx().animate_value_with_time(
        ui.make_persistent_id("nav-selection"),
        index,
        if app.animations { 0.22 } else { 0.0 },
    );
    let selection = egui::Rect::from_min_size(
        nav.min + egui::vec2(0.0, slide * 66.0),
        egui::vec2(nav.width(), 58.0),
    );
    ui.painter().rect(
        selection,
        CornerRadius::same(15),
        Color32::from_rgb(33, 24, 41),
        Stroke::new(1.0, Color32::from_rgb(57, 40, 69)),
        egui::StrokeKind::Inside,
    );
    for (rect, response, page, label, subtitle) in items {
        nav_item(ui, app.page == page, page, label, subtitle, rect, response);
    }
    ui.add_space(20.0);
    // A quiet footer replaces the promotional card; it reports the real preview connection.
    ui.add_space((ui.available_height() - 79.0).max(12.0));
    divider(ui);
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(
            dot.center(),
            3.0,
            if app.preview.connected() {
                ACCENT
            } else {
                MUTED
            },
        );
        ui.label(
            RichText::new(if app.preview.connected() {
                "Island connected"
            } else {
                "Connecting island"
            })
            .size(11.0)
            .color(MUTED),
        );
    });
    ui.add_space(2.0);
    ui.label(
        RichText::new(format!(
            "Islet for Windows  /  {}",
            env!("CARGO_PKG_VERSION")
        ))
        .size(10.0)
        .color(MUTED),
    );
}

fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    page: Page,
    label: &str,
    subtitle: &str,
    rect: egui::Rect,
    response: egui::Response,
) {
    let hover = ui
        .ctx()
        .animate_bool_responsive(response.id.with("hover"), response.hovered());
    let active = ui
        .ctx()
        .animate_bool_responsive(response.id.with("selected"), selected);
    if !selected {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(15),
            Color32::from_white_alpha((hover * 12.0) as u8),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(2.0),
            CornerRadius::same(13),
            Stroke::new(1.0, TEXT),
            egui::StrokeKind::Inside,
        );
    }
    let center = egui::pos2(rect.left() + 23.0 + hover, rect.center().y);
    let icon_rect = egui::Rect::from_center_size(center, egui::vec2(30.0, 30.0));
    ui.painter().rect_filled(
        icon_rect,
        CornerRadius::same(10),
        Color32::from_rgba_unmultiplied(154, 91, 193, (active * 35.0) as u8),
    );
    let color = if selected {
        Color32::from_rgb(219, 177, 247)
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.4, color);
    match page {
        Page::General => {
            for (dy, dx) in [(-5.0, -2.0), (0.0, 3.0), (5.0, -1.0)] {
                ui.painter().line_segment(
                    [center + egui::vec2(-7.0, dy), center + egui::vec2(7.0, dy)],
                    stroke,
                );
                ui.painter()
                    .circle_filled(center + egui::vec2(dx, dy), 2.2, color);
            }
        }
        Page::NowPlaying => {
            for (dx, h) in [(-6.0, 3.0), (-2.0, 7.0), (2.0, 5.0), (6.0, 2.0)] {
                ui.painter().line_segment(
                    [center + egui::vec2(dx, -h), center + egui::vec2(dx, h)],
                    stroke,
                );
            }
        }
        Page::Dictation => {
            // A microphone: capsule, cradle, and stand.
            let capsule =
                egui::Rect::from_center_size(center + egui::vec2(0.0, -2.5), egui::vec2(6.0, 10.0));
            ui.painter().rect_stroke(
                capsule,
                CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Middle,
            );
            for (from, to) in [
                ((-5.5, -1.0), (-5.5, 1.5)),
                ((-5.5, 1.5), (-3.0, 4.0)),
                ((-3.0, 4.0), (3.0, 4.0)),
                ((3.0, 4.0), (5.5, 1.5)),
                ((5.5, 1.5), (5.5, -1.0)),
                ((0.0, 4.0), (0.0, 7.0)),
            ] {
                ui.painter().line_segment(
                    [
                        center + egui::vec2(from.0, from.1),
                        center + egui::vec2(to.0, to.1),
                    ],
                    stroke,
                );
            }
        }
        Page::About => {
            ui.painter().circle_stroke(center, 7.0, stroke);
            ui.painter()
                .circle_filled(center + egui::vec2(0.0, -3.0), 0.9, color);
            ui.painter().line_segment(
                [center + egui::vec2(0.0, 0.0), center + egui::vec2(0.0, 4.0)],
                stroke,
            );
        }
    }
    ui.painter().text(
        egui::pos2(rect.left() + 48.0, rect.center().y - 9.0),
        egui::Align2::LEFT_CENTER,
        label,
        if selected {
            bold_font(13.0)
        } else {
            FontId::proportional(13.0)
        },
        if selected || response.hovered() {
            TEXT
        } else {
            MUTED
        },
    );
    ui.painter().text(
        egui::pos2(rect.left() + 48.0, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        subtitle,
        FontId::proportional(10.0),
        MUTED,
    );
    let arrow = egui::pos2(rect.right() - 13.0, rect.center().y);
    let stroke = Stroke::new(1.3, ACCENT.gamma_multiply(active));
    ui.painter().line_segment(
        [arrow + egui::vec2(-2.0, -3.0), arrow + egui::vec2(1.0, 0.0)],
        stroke,
    );
    ui.painter().line_segment(
        [arrow + egui::vec2(1.0, 0.0), arrow + egui::vec2(-2.0, 3.0)],
        stroke,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
}

pub(crate) fn header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).heading());
    ui.label(RichText::new(subtitle).color(MUTED));
    ui.add_space(18.0);
}

pub(crate) fn card(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).font(bold_font(12.0)).color(MUTED));
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_LINE))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::symmetric(20, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
    ui.add_space(14.0);
}

/// A setting: title and description on the left, the control on the right.
pub(crate) fn row(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        let text_width = (ui.available_width() * 0.53).max(180.0);
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

pub(crate) fn divider(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, CARD_LINE);
}

/// An iOS-style switch.
pub(crate) fn toggle(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
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
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(3.0),
            CornerRadius::same(15),
            Stroke::new(1.5, TEXT),
            egui::StrokeKind::Outside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, label)
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Two or more options as a segmented control. Drawn right to left (it
/// sits in a right-aligned row), so options are passed in reverse.
pub(crate) fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str)],
) {
    ui.spacing_mut().item_spacing.x = 4.0;
    for (option, label) in options.iter().rev() {
        ui.selectable_value(value, *option, *label);
    }
}

fn general(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "General", "Make a little room for what matters.");
    let general = &mut app.config.general;
    card(ui, "Placement", |ui| {
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
    card(ui, "Behavior", |ui| {
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
                toggle(ui, &mut general.hide_in_fullscreen, "Hide in full screen");
            },
        );
        divider(ui);
        row(
            ui,
            "Dock over apps",
            "Move to the bottom when an app overlaps. Return when the space is clear.",
            |ui| {
                toggle(ui, &mut general.tuck_behind_windows, "Dock over apps");
            },
        );
        divider(ui);
        row(
            ui,
            "Start with Windows",
            "Launch Islet when you sign in.",
            |ui| {
                toggle(ui, &mut general.start_with_windows, "Start with Windows");
            },
        );
    });
}

fn now_playing(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "Now Playing", "Your soundtrack, a little closer.");
    let media = &mut app.config.media;
    card(ui, "Playback", |ui| {
        row(
            ui,
            "Show now playing",
            "Album art, controls, and bars.",
            |ui| {
                toggle(ui, &mut media.enabled, "Show now playing");
            },
        );
        divider(ui);
        row(
            ui,
            "Music only",
            "Filter out videos. Recognize browser music using the sites below.",
            |ui| {
                toggle(ui, &mut media.music_only, "Music only");
            },
        );
        divider(ui);
        row(
            ui,
            "Close after pausing",
            "Keep paused music visible for this long. Hover to keep it open.",
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
                toggle(ui, &mut media.real_bars, "Live audio bars");
            },
        );
        divider(ui);
        row(
            ui,
            "Tint from album art",
            "Colour the bars and progress.",
            |ui| {
                toggle(ui, &mut media.color_tint, "Tint from album art");
            },
        );
    });

    let mut remove = None;
    let mut add = false;
    let mut reset = false;
    card(ui, "Music sites", |ui| {
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
                                egui::Button::new(RichText::new("x").small().color(MUTED))
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
                    .hint_text("Add a music site or URL")
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
    card(ui, "Application", |ui| {
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
    card(ui, "Privacy", |ui| {
        ui.label(
            RichText::new(
                "Everything runs on this PC. To tell music from video, Islet reads the titles of \
                 open browser tabs in memory when a track changes; titles are never stored or sent \
                 anywhere.",
            )
            .color(MUTED),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Dictation listens for your hotkey system-wide, but only to start and stop                  recording: other keystrokes are never stored, logged, or sent anywhere. The                  microphone is on only while you hold the hotkey (or in hands-free mode); audio                  is turned into text by a model on this PC and discarded. What you say is not                  saved or logged.",
            )
            .color(MUTED),
        );
    });
}
