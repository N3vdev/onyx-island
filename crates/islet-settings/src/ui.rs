//! Layout and look of the settings window.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout,
    Margin, RichText, Stroke, TextStyle,
};
use islet_core::config::{
    ExpandTrigger, IdleGlance, IdleStyle, IslandLook, MonitorChoice, ScreenPosition,
};

use crate::{Page, SettingsApp};

pub(crate) const BG: Color32 = Color32::from_rgb(16, 17, 19);
const SIDEBAR: Color32 = BG;
pub(crate) const CARD: Color32 = Color32::from_rgb(23, 24, 27);
pub(crate) const CARD_LINE: Color32 = Color32::from_rgb(47, 49, 53);
pub(crate) const TEXT: Color32 = Color32::from_rgb(236, 237, 242);
pub(crate) const MUTED: Color32 = Color32::from_rgb(166, 170, 178);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(164, 204, 180);
pub(crate) const DANGER: Color32 = Color32::from_rgb(255, 110, 110);

/// Mouse-wheel scrolling at the speed of other Windows apps. egui moves 40
/// points per wheel notch; Windows apps move the user's "lines to scroll at a
/// time" (3 by default), about 100 points a notch.
pub fn install_scrolling(ctx: &egui::Context) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    /// Points per line of text, as Windows apps scroll it.
    const LINE: f32 = 33.0;
    let mut lines: u32 = 3;
    // SAFETY: SPI_GETWHEELSCROLLLINES writes one u32 to the pointer.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some((&raw mut lines).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // 0 turns wheel scrolling off and u32::MAX means "a page at a time";
    // neither maps to a line count, so keep the Windows default there.
    if read.is_err() || lines == 0 || lines > 20 {
        lines = 3;
    }
    ctx.options_mut(|options| options.input_options.line_scroll_speed = lines as f32 * LINE);
}

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
        (TextStyle::Heading, bold_font(25.0)),
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
    visuals.selection.bg_fill = Color32::from_rgb(48, 51, 55);
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
pub fn install_titlebar_theme(title: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    // The native window exists before eframe invokes the app creation callback.
    unsafe {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let Ok(hwnd) = FindWindowW(None, PCWSTR(wide.as_ptr())) else {
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

/// A static grid like the user's canvas reference; no idle animation.
pub(crate) fn dot_grid(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter().with_clip_rect(rect);
    let spacing = 20.0;
    for y in 0..=(rect.height() / spacing) as usize {
        for x in 0..=(rect.width() / spacing) as usize {
            painter.circle_filled(
                rect.min + egui::vec2(x as f32 * spacing + 10.0, y as f32 * spacing + 10.0),
                0.65,
                Color32::from_rgb(38, 40, 43),
            );
        }
    }
}

pub fn draw(app: &mut SettingsApp, ctx: &egui::Context) {
    if app.customization.is_some() {
        crate::customize::show(app, ctx);
        return;
    }
    let previous_page = app.page;
    egui::SidePanel::left("nav")
        .exact_width(150.0)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(SIDEBAR)
                .inner_margin(Margin::symmetric(12, 18)),
        )
        .show(ctx, |ui| sidebar(app, ui));
    if previous_page != app.page {
        app.page_changed = std::time::Instant::now();
    }
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(BG)
                .inner_margin(Margin::symmetric(32, 20)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Settings").font(bold_font(17.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(if app.status.is_some() {
                            "Could not save changes"
                        } else {
                            "All changes saved"
                        })
                        .size(11.0)
                        .color(if app.status.is_some() {
                            DANGER
                        } else {
                            MUTED
                        }),
                    );
                });
            });
            // Reserve only the resting pill's space. The native transparent
            // window extends over the page so expansion never reflows controls.
            ui.add_space(12.0);
            let (dock, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 64.0),
                egui::Sense::hover(),
            );
            let slot = egui::Rect::from_min_size(
                egui::pos2(dock.center().x - dock.width().min(720.0) / 2.0, dock.top()),
                egui::vec2(dock.width().min(720.0), 280.0),
            );
            app.preview.update(ctx, slot, None);
            ui.add_space(20.0);
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt(("settings-page", app.page as usize))
                .auto_shrink([false, false]);
            if let Some(offset) = app.initial_scroll.take() {
                scroll = scroll.vertical_scroll_offset(offset);
            }
            scroll.show(ui, |ui| {
                let progress = if app.animations {
                    (app.page_changed.elapsed().as_secs_f32() / 0.16).min(1.0)
                } else {
                    1.0
                };
                ui.set_opacity(0.75 + 0.25 * (1.0 - (1.0 - progress).powi(3)));
                if progress < 1.0 {
                    ctx.request_repaint();
                }
                match app.page {
                    Page::General => general(app, ui),
                    Page::NowPlaying => now_playing(app, ui),
                    Page::Dictation => crate::dictation_page::show(app, ui),
                    Page::Widgets => widgets(app, ui),
                    Page::Tasks => crate::tasks_page::show(app, ui),
                    Page::NightLight => crate::night_light_page::show(app, ui),
                    Page::Lens => crate::lens_page::show(app, ui),
                    Page::About => about(app, ui),
                }
                if let Some(status) = &app.status {
                    ui.label(RichText::new(status).color(DANGER));
                }
            });
        });
}

pub(crate) fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Nunito Bold".into()))
}

fn sidebar(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
        ui.painter().rect_stroke(
            egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 10.0)),
            CornerRadius::same(5),
            Stroke::new(2.0, TEXT),
            egui::StrokeKind::Middle,
        );
        ui.label(RichText::new("Onyx").font(bold_font(19.0)));
    });
    ui.add_space(38.0);
    let pages = [
        (Page::General, "General"),
        (Page::NowPlaying, "Now Playing"),
        (Page::Dictation, "Dictation"),
        (Page::Widgets, "Widgets"),
        (Page::Tasks, "To-do"),
        (Page::NightLight, "Night light"),
        (Page::Lens, "Lens"),
    ];
    for (page, label) in pages {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
        if response.clicked() {
            app.page = page;
        }
        nav_item(ui, app.page == page, page, label, rect, response);
    }
    ui.add_space((ui.available_height() - 102.0).max(12.0));
    divider(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
    if response.clicked() {
        app.page = Page::About;
    }
    nav_item(
        ui,
        app.page == Page::About,
        Page::About,
        "About Onyx",
        rect,
        response,
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
            .size(11.0)
            .color(MUTED),
    );
}

fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    page: Page,
    label: &str,
    rect: egui::Rect,
    response: egui::Response,
) {
    let hover = ui
        .ctx()
        .animate_bool_responsive(response.id.with("hover"), response.hovered());
    let active = ui
        .ctx()
        .animate_bool_responsive(response.id.with("selected"), selected);
    let fill = Color32::from_white_alpha((active * 23.0 + (1.0 - active) * hover * 10.0) as u8);
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    if selected {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, CARD_LINE),
            egui::StrokeKind::Inside,
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(6),
            Stroke::new(1.0, TEXT),
            egui::StrokeKind::Inside,
        );
    }
    let center = egui::pos2(rect.left() + 17.0, rect.center().y);
    let color = if selected || response.hovered() {
        TEXT
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
        Page::Widgets => {
            for dy in [-4.0, 4.0] {
                for dx in [-4.0, 4.0] {
                    ui.painter().rect_stroke(
                        egui::Rect::from_center_size(
                            center + egui::vec2(dx, dy),
                            egui::vec2(5.0, 5.0),
                        ),
                        CornerRadius::same(1),
                        stroke,
                        egui::StrokeKind::Middle,
                    );
                }
            }
        }
        Page::NightLight => {
            // A crescent moon: a circle with a bite taken out of it.
            let points: Vec<egui::Pos2> = (0..=24)
                .map(|step| {
                    let a = -2.3 + 4.6 * step as f32 / 24.0;
                    center + egui::vec2(-7.0 * a.cos() + 1.0, 7.0 * a.sin())
                })
                .chain((0..=24).map(|step| {
                    let a = 2.0 - 4.0 * step as f32 / 24.0;
                    center + egui::vec2(-5.0 * a.cos() + 3.4, 5.6 * a.sin())
                }))
                .collect();
            ui.painter().add(egui::Shape::closed_line(points, stroke));
        }
        Page::Tasks => {
            // A ticked box.
            ui.painter().rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(13.0, 13.0)),
                CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Middle,
            );
            ui.painter().line_segment(
                [center + egui::vec2(-3.5, 0.2), center + egui::vec2(-1.0, 2.8)],
                stroke,
            );
            ui.painter().line_segment(
                [center + egui::vec2(-1.0, 2.8), center + egui::vec2(3.8, -2.6)],
                stroke,
            );
        }
        Page::Lens => {
            // A magnifying glass: a lens and its handle.
            let lens = center + egui::vec2(-1.5, -1.5);
            ui.painter().circle_stroke(lens, 5.0, stroke);
            ui.painter().line_segment(
                [lens + egui::vec2(3.6, 3.6), center + egui::vec2(6.5, 6.5)],
                Stroke::new(1.8, color),
            );
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
        egui::pos2(rect.left() + 35.0, rect.center().y),
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
    ui.label(RichText::new(title).font(bold_font(14.0)).color(TEXT));
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(14))
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

/// Windows' "Transparency effects" switch (Settings > Personalization >
/// Colors). Acrylic, the island's glass included, is solid while it is off.
fn windows_transparency_on() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into a local of that size.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            windows::core::w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    read.is_err() || value != 0
}

fn open_color_settings() {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // SAFETY: opens a fixed settings URI.
    unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            windows::core::w!("ms-settings:colors"),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

fn general(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "General", "Where your island lives and how it behaves.");
    let mut customize = false;
    let general = &mut app.config.general;
    card(ui, "Placement", |ui| {
        row(
            ui,
            "Position",
            "Dynamic sits at the top and moves to the bottom when an app is behind it. Top and Bottom stay put.",
            |ui| {
                segmented(
                    ui,
                    &mut general.position,
                    &[
                        (ScreenPosition::Dynamic, "Dynamic"),
                        (ScreenPosition::Top, "Top"),
                        (ScreenPosition::Bottom, "Bottom"),
                    ],
                );
            },
        );
        divider(ui);
        row(
            ui,
            "Look",
            match general.look {
                IslandLook::Black => "Solid black, like a phone's island.",
                IslandLook::Glass => "Frosted glass: what's behind shows through, blurred, under a dark tint.",
            },
            |ui| {
                segmented(
                    ui,
                    &mut general.look,
                    &[(IslandLook::Black, "Black"), (IslandLook::Glass, "Glass")],
                );
            },
        );
        if general.look == IslandLook::Glass {
            divider(ui);
            row(
                ui,
                "Glass tint",
                "How dark the glass is. Darker keeps text easy to read over bright pages.",
                |ui| {
                    let mut percent = (general.glass_tint * 100.0).round();
                    ui.add(egui::Slider::new(&mut percent, 0.0..=90.0).suffix("%").integer());
                    general.glass_tint = (percent / 100.0).clamp(0.0, 0.9);
                },
            );
        }
        if general.look == IslandLook::Glass && !windows_transparency_on() {
            divider(ui);
            row(
                ui,
                "Transparency effects are off",
                "Without them Windows gives no blur, so the glass is clear. Turn them on in Windows' color settings.",
                |ui| {
                    if ui.button("Open color settings").clicked() {
                        open_color_settings();
                    }
                },
            );
        }
        divider(ui);
        row(
            ui,
            "Customize sizing",
            "Adjust edge spacing and the size of the expanded view.",
            |ui| {
                customize = ui.button("Customize...").clicked();
            },
        );
        if general.position.docks_over_apps() {
            divider(ui);
            row(
                ui,
                "Stay over empty space",
                "Keep the island at the top when the app behind it has nothing there.",
                |ui| {
                    toggle(
                        ui,
                        &mut general.stay_over_empty_space,
                        "Stay over empty space",
                    );
                },
            );
        }
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
            "Open the island",
            "Show the full view when you hover over it, or only when you click.",
            |ui| {
                segmented(
                    ui,
                    &mut general.expand_on,
                    &[
                        (ExpandTrigger::Hover, "Hover"),
                        (ExpandTrigger::Click, "Click"),
                    ],
                );
            },
        );
        divider(ui);
        row(
            ui,
            "Always show the island",
            "Keep it on screen with something useful when nothing is playing.",
            |ui| {
                toggle(ui, &mut general.always_on, "Always show the island");
            },
        );
        divider(ui);
        if general.always_on {
            row(
                ui,
                "When nothing is playing",
                match general.glance {
                    IdleGlance::Clock => {
                        "The time. Hover the island for your music player."
                    }
                    IdleGlance::Stats => "CPU and memory use, updated every two seconds.",
                    IdleGlance::Eyes => "Two eyes that blink and look where you pull the island.",
                },
                |ui| {
                    segmented(
                        ui,
                        &mut general.glance,
                        &[
                            (IdleGlance::Clock, "Clock"),
                            (IdleGlance::Stats, "Stats"),
                            (IdleGlance::Eyes, "Eyes"),
                        ],
                    );
                },
            );
        } else {
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
        }
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
            "Start with Windows",
            "Launch Onyx when you sign in.",
            |ui| {
                toggle(ui, &mut general.start_with_windows, "Start with Windows");
            },
        );
    });
    if customize {
        app.open_customization(ui.ctx());
    }
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
        divider(ui);
        row(
            ui,
            "Earbuds and headphones",
            "Show when Bluetooth earbuds connect or run low, with their battery.",
            |ui| {
                toggle(ui, &mut media.bluetooth, "Earbuds and headphones");
            },
        );
        divider(ui);
        let status = spotify_status();
        let detail = match status {
            SpotifyStatus::Premium => "Connected. Search and play songs from the music card.",
            SpotifyStatus::Free => "Connected, but searching and playing from Onyx needs Premium.",
            SpotifyStatus::Disconnected => "Search and play songs from the island. Needs Spotify Premium.",
        };
        row(ui, "Spotify", detail, |ui| {
            ui.horizontal(|ui| {
                if media.spotify {
                    let connected = status != SpotifyStatus::Disconnected;
                    if ui.button(if connected { "Disconnect" } else { "Connect" }).clicked() {
                        tell_island_spotify(!connected);
                    }
                }
                toggle(ui, &mut media.spotify, "Spotify");
            });
        });
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

fn widgets(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(ui, "Widgets", "Small tools, right inside your island.");
    card(ui, "Available widgets", |ui| {
        row(ui,"File converter","Drag files onto the island, then drop on a format or choose one. Results are saved beside the originals.",|ui| {
            toggle(ui,&mut app.config.widgets.file_converter,"File converter");
        });
        divider(ui);
        row(
            ui,
            "Calculator",
            "Quick arithmetic with percentages, sign changes, and normal operator precedence.",
            |ui| {
                toggle(ui, &mut app.config.widgets.calculator, "Calculator");
            },
        );
        divider(ui);
        row(
            ui,
            "System stats",
            "Live CPU, GPU, memory, network, and temperature. The island's edge glows orange \
             while the CPU is maxed out.",
            |ui| {
                toggle(ui, &mut app.config.widgets.system_stats, "System stats");
            },
        );
    });
    crate::mail_page::show(app, ui);
    ui.add_space(4.0);
    ui.label(RichText::new("Using widgets").font(bold_font(14.0)));
    ui.label(RichText::new("Expand the island, then scroll to switch between your current activity and each widget. The to-do list has its own page in the sidebar.").color(MUTED));
    ui.add_space(4.0);
    ui.label(RichText::new("Widgets appear only in expanded mode. The collapsed island keeps your current activity or configured idle appearance.").small().color(MUTED));
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
            "Welcome tour",
            "Set up dictation again, try it, and see what the island can do.",
            |ui| {
                if ui.button("Open tour").clicked() {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe).arg("--onboarding").spawn();
                    }
                }
            },
        );
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
                "Everything runs on this PC. To tell music from video, Onyx reads the titles of \
                 open browser tabs in memory when a track changes; titles are never stored or sent \
                 anywhere.",
            )
            .color(MUTED),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Dictation listens for your hotkey system-wide, but only to start and stop \
                 recording: other keystrokes are never stored, logged, or sent anywhere. The \
                 microphone is on only while you hold the hotkey (or in hands-free mode); audio \
                 is turned into text by a model on this PC and discarded. What you say is not \
                 saved or logged.",
            )
            .color(MUTED),
        );
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpotifyStatus {
    Premium,
    Free,
    Disconnected,
}

/// What the island last found of the Spotify account (Settings never sees
/// the sign-in itself).
fn spotify_status() -> SpotifyStatus {
    let path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join("Islet")
        .join("spotify-status.txt");
    match std::fs::read_to_string(path).as_deref().map(str::trim) {
        Ok("premium") => SpotifyStatus::Premium,
        Ok("free") => SpotifyStatus::Free,
        _ => SpotifyStatus::Disconnected,
    }
}

/// Asks the island to connect Spotify (it opens the sign-in page) or to
/// forget the account.
fn tell_island_spotify(connect: bool) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};
    // SAFETY: registers a message name and posts it to the island window.
    unsafe {
        let message = RegisterWindowMessageW(windows::core::w!("Onyx.Spotify"));
        if let Ok(island) = FindWindowW(windows::core::w!("IsletIslandWindow"), None) {
            let _ = PostMessageW(Some(island), message, WPARAM(usize::from(connect)), LPARAM(0));
        }
    }
}
