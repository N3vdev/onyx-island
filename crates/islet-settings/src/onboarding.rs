//! The welcome tour, shown once on first run (and from About any time):
//! what Onyx is, what the island does with your hands, downloading the
//! speech and cleanup models, a box to try dictation in, and connecting
//! Gmail for sign-in codes.
//!
//! It runs in the settings process (`islet-settings --onboarding`) in its own
//! window. The island refuses dictation into settings windows, except this
//! one, by its title ([`ONBOARDING_TITLE`]).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText,
    Sense, Stroke, StrokeKind, Vec2,
};
use islet_asr::{ModelSpec, ModelStatus, AI_CLEANUP};
use islet_core::config::ExpandTrigger;
use islet_core::handoff::ONBOARDING_TITLE;
use islet_core::hotkey::{modifier_bit, Combo};
use islet_core::Config;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use crate::dictation_page::DictationUi;
use crate::practice::Practice;
use crate::showcase::{self, Gesture, LAVENDER};
use crate::ui::{bold_font, toggle, ACCENT, BG, CARD, CARD_LINE, DANGER, MUTED, TEXT};

const OK: Color32 = Color32::from_rgb(64, 210, 122);
/// The content column's widest.
const COLUMN: f32 = 880.0;

pub fn run(config_file: PathBuf, icon: Arc<egui::IconData>) -> eframe::Result {
    // One tour at a time: a second launch brings the first forward.
    // SAFETY: plain named-mutex creation; the handle lives until exit.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\Islet.Onboarding")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let title: Vec<u16> = ONBOARDING_TITLE.encode_utf16().chain(Some(0)).collect();
        // SAFETY: window lookup and activation by title.
        unsafe {
            if let Ok(hwnd) = FindWindowW(None, PCWSTR(title.as_ptr())) {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            if let Ok(handle) = mutex {
                let _ = CloseHandle(handle);
            }
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(ONBOARDING_TITLE)
            .with_inner_size([1000.0, 780.0])
            .with_min_inner_size([900.0, 720.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        ONBOARDING_TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(Tour::new(cc, config_file)))),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Welcome,
    Island,
    /// Practising the swipes on a pretend island. Not skippable.
    Gestures,
    Models,
    Try,
    /// Connecting Gmail for sign-in codes. Optional.
    Gmail,
    Done,
}

const STEPS: [Step; 7] = [
    Step::Welcome,
    Step::Island,
    Step::Gestures,
    Step::Models,
    Step::Try,
    Step::Gmail,
    Step::Done,
];

impl Step {
    fn index(self) -> usize {
        STEPS.iter().position(|step| *step == self).unwrap_or(0)
    }

    fn next(self) -> Option<Self> {
        STEPS.get(self.index() + 1).copied()
    }

    fn previous(self) -> Option<Self> {
        self.index().checked_sub(1).map(|index| STEPS[index])
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Primary,
    Secondary,
    Quiet,
}

struct Tour {
    config_file: PathBuf,
    /// The config as last read or written; edits go through [`Tour::change`].
    config: Config,
    step: Step,
    step_changed: Instant,
    /// Which way the last step change went, for the slide.
    forward: bool,
    animations: bool,
    models: DictationUi,
    /// Recent (time, bytes) per download, for its speed.
    samples: HashMap<&'static str, VecDeque<(Instant, u64)>>,
    sample_text: String,
    focus_box: bool,
    island_running: bool,
    island_checked: Option<Instant>,
    status: Option<String>,
    practice: Practice,
    /// When Connect was last clicked: the island takes a moment to say it is
    /// signing in, and a second click meanwhile would start another.
    gmail_asked: Option<Instant>,
}

impl Tour {
    fn new(cc: &eframe::CreationContext<'_>, config_file: PathBuf) -> Self {
        crate::ui::install_theme(&cc.egui_ctx);
        crate::ui::install_scrolling(&cc.egui_ctx);
        crate::ui::install_titlebar_theme(ONBOARDING_TITLE);
        let config = Config::load(&config_file).unwrap_or_default();
        // `--tour-step models` opens on a step (for development).
        let first = match std::env::args()
            .skip_while(|arg| arg != "--tour-step")
            .nth(1)
            .as_deref()
        {
            Some("island") => Step::Island,
            Some("gestures") => Step::Gestures,
            Some("models") => Step::Models,
            Some("try") => Step::Try,
            Some("gmail") => Step::Gmail,
            Some("done") => Step::Done,
            _ => Step::Welcome,
        };
        let mut tour = Self {
            config_file,
            config,
            step: first,
            step_changed: Instant::now(),
            forward: true,
            animations: crate::preview::animations_enabled(),
            models: DictationUi::new(),
            samples: HashMap::new(),
            sample_text: String::new(),
            focus_box: first == Step::Try,
            island_running: true,
            island_checked: None,
            status: None,
            practice: Practice::default(),
            gmail_asked: None,
        };
        // The tour counts as done once the gestures have been practised
        // (see `update`); until then the island opens it at every start. It
        // stays a click away in Settings, under About.
        // `--tour-download` starts both downloads (for development).
        if std::env::args().any(|arg| arg == "--tour-download") {
            let speech = tour.speech_model().id;
            tour.start(speech);
            tour.start(AI_CLEANUP.id);
        }
        tour
    }

    /// Edits the config on disk: re-read first, so changes the island made
    /// meanwhile (it moves itself between edges) are kept.
    fn change(&mut self, edit: impl FnOnce(&mut Config)) {
        let mut config = Config::load(&self.config_file).unwrap_or_else(|_| self.config.clone());
        edit(&mut config);
        config.normalize();
        if let Some(parent) = self.config_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match config.save_atomic(&self.config_file) {
            Ok(()) => self.status = None,
            Err(error) => self.status = Some(format!("Could not save settings: {error}")),
        }
        self.config = config;
    }

    fn go(&mut self, step: Step) {
        if step == self.step {
            return;
        }
        self.forward = step.index() > self.step.index();
        self.step = step;
        self.step_changed = Instant::now();
        if step == Step::Try {
            self.focus_box = true;
            self.island_checked = None;
            if let Ok(config) = Config::load(&self.config_file) {
                self.config = config;
            }
        }
    }

    fn speech_model(&self) -> &'static ModelSpec {
        islet_asr::find_or_default(&self.config.dictation.model)
    }

    fn status_of(&self, id: &str) -> ModelStatus {
        let manager = self.models.manager();
        if id == AI_CLEANUP.id {
            manager.addon_status(&AI_CLEANUP)
        } else {
            manager.status(self.speech_model())
        }
    }

    fn start(&mut self, id: &'static str) {
        if self.models.download_progress(id).is_some()
            || self.status_of(id) == ModelStatus::Installed
        {
            return;
        }
        self.samples.remove(id);
        if id == AI_CLEANUP.id {
            self.models.start_addon_download(&AI_CLEANUP);
        } else {
            self.models.start_download(self.speech_model());
        }
    }

    /// Download speed in bytes a second, over the last few seconds.
    fn rate(&mut self, id: &'static str, done: u64) -> Option<f64> {
        let now = Instant::now();
        let samples = self.samples.entry(id).or_default();
        if samples.back().is_none_or(|(_, last)| *last != done) {
            samples.push_back((now, done));
        }
        while samples.len() > 2
            && samples
                .front()
                .is_some_and(|(at, _)| now - *at > Duration::from_secs(4))
        {
            samples.pop_front();
        }
        let (first_at, first) = *samples.front()?;
        let seconds = (now - first_at).as_secs_f64();
        (seconds > 0.8 && done > first).then(|| (done - first) as f64 / seconds)
    }

    fn check_island(&mut self) {
        if self
            .island_checked
            .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.island_checked = Some(Instant::now());
        // SAFETY: a window lookup by class name.
        self.island_running = unsafe { FindWindowW(w!("IsletIslandWindow"), None) }.is_ok();
    }
}

impl eframe::App for Tour {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.models.poll_jobs();
        if self.practice.complete() && !self.config.general.onboarded {
            self.change(|config| config.general.onboarded = true);
        }
        let mut next_step = None;
        let mut close = false;

        egui::TopBottomPanel::top("tour-top")
            .exact_height(64.0)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(28, 0)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| top_bar(self, ui));

        egui::TopBottomPanel::bottom("tour-footer")
            .exact_height(80.0)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(28, 0)),
            )
            .show_separator_line(false)
            .show(ctx, |ui| {
                let (step, finish) = footer(self, ui);
                next_step = step;
                close = finish;
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(28, 8)),
            )
            .show(ctx, |ui| {
                let area = ui.available_rect_before_wrap();
                let width = area.width().min(COLUMN);
                let progress = if self.animations {
                    (self.step_changed.elapsed().as_secs_f32() / 0.32).min(1.0)
                } else {
                    1.0
                };
                let eased = 1.0 - (1.0 - progress).powi(3);
                let slide = (1.0 - eased) * 28.0 * if self.forward { 1.0 } else { -1.0 };
                let column = Rect::from_min_size(
                    pos2(area.center().x - width / 2.0 + slide, area.top()),
                    vec2(width, area.height()),
                );
                if progress < 1.0 {
                    ctx.request_repaint();
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                    ui.set_opacity(eased);
                    egui::ScrollArea::vertical()
                        .id_salt(("tour", self.step.index()))
                        .auto_shrink([false, false])
                        .show(ui, |ui| match self.step {
                            Step::Welcome => welcome(self, ui),
                            Step::Island => island_page(self, ui),
                            Step::Gestures => gestures_page(self, ui),
                            Step::Models => models_page(self, ui),
                            Step::Try => {
                                if let Some(step) = try_page(self, ui) {
                                    next_step = Some(step);
                                }
                            }
                            Step::Gmail => gmail_page(self, ui),
                            Step::Done => done_page(self, ui),
                        });
                });
            });

        if let Some(step) = next_step {
            self.go(step);
        }
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

// ---------------------------------------------------------------------------
// Frame: the step dots at the top, the buttons at the bottom.

fn top_bar(tour: &Tour, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    if tour.step != Step::Welcome {
        showcase::mark(painter, pos2(rect.left() + 22.0, rect.center().y), 40.0);
        painter.text(
            pos2(rect.left() + 52.0, rect.center().y),
            Align2::LEFT_CENTER,
            "Onyx",
            bold_font(17.0),
            TEXT,
        );
    }
    // One dash per step; the current one is long.
    let widths: Vec<f32> = STEPS
        .iter()
        .map(|step| if *step == tour.step { 30.0 } else { 8.0 })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0 * (STEPS.len() - 1) as f32;
    let mut x = rect.center().x - total / 2.0;
    for (step, width) in STEPS.iter().zip(widths) {
        let fill = if *step == tour.step {
            TEXT
        } else if step.index() < tour.step.index() {
            Color32::from_gray(120)
        } else {
            CARD_LINE
        };
        painter.rect_filled(
            Rect::from_min_size(pos2(x, rect.center().y - 3.0), vec2(width, 6.0)),
            CornerRadius::same(3),
            fill,
        );
        x += width + 6.0;
    }
}

/// Returns the step to go to, and whether to close the window.
fn footer(tour: &mut Tour, ui: &mut egui::Ui) -> (Option<Step>, bool) {
    let mut step = None;
    let mut close = false;
    // The gesture practice cannot be skipped: until it is done, skipping
    // goes to it, and it holds Continue back.
    let practised = tour.practice.complete();
    let held_back = tour.step == Step::Gestures && !practised;
    ui.horizontal_centered(|ui| {
        if !practised {
            if tour.step.index() < Step::Gestures.index()
                && button(ui, "Skip to gestures", Kind::Quiet, true)
                    .on_hover_text("Try the island's gestures, then you can skip the rest")
                    .clicked()
            {
                step = Some(Step::Gestures);
            }
        } else if tour.step != Step::Done
            && button(ui, "Skip tour", Kind::Quiet, true)
                .on_hover_text("You can open it again from Settings, under About")
                .clicked()
        {
            close = true;
        }
        if let Some(status) = &tour.status {
            ui.label(RichText::new(status).small().color(DANGER));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = match tour.step {
                Step::Welcome => "Get started",
                Step::Done => "Finish",
                _ => "Continue",
            };
            let next = button(ui, label, Kind::Primary, !held_back);
            let next = if held_back { next.on_hover_text("Try all four gestures to continue") } else { next };
            if next.clicked() {
                match tour.step.next() {
                    Some(next) => step = Some(next),
                    None => close = true,
                }
            }
            if let Some(previous) = tour.step.previous() {
                if button(ui, "Back", Kind::Secondary, true).clicked() {
                    step = Some(previous);
                }
            }
        });
    });
    (step, close)
}

fn cr(radius: f32) -> CornerRadius {
    CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

/// A rounded button drawn to match the tour.
fn button(ui: &mut egui::Ui, label: &str, kind: Kind, enabled: bool) -> egui::Response {
    let font = bold_font(14.0);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), TEXT)
        .size()
        .x;
    let height = if kind == Kind::Quiet { 34.0 } else { 40.0 };
    let pad = if kind == Kind::Quiet { 14.0 } else { 22.0 };
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(text_width + pad * 2.0, height), sense);
    if ui.is_rect_visible(rect) {
        let hover = ui
            .ctx()
            .animate_bool_responsive(response.id, enabled && response.hovered());
        let (fill, text) = match kind {
            Kind::Primary => (mix(TEXT, Color32::WHITE, hover), BG),
            Kind::Secondary => (
                mix(
                    Color32::from_rgb(36, 37, 42),
                    Color32::from_rgb(50, 51, 58),
                    hover,
                ),
                TEXT,
            ),
            Kind::Quiet => (
                Color32::from_white_alpha((12.0 * hover) as u8),
                mix(MUTED, TEXT, hover),
            ),
        };
        let alpha = if enabled { 1.0 } else { 0.4 };
        let pressed = if response.is_pointer_button_down_on() {
            0.97
        } else {
            1.0
        };
        let shape = Rect::from_center_size(rect.center(), rect.size() * pressed);
        ui.painter()
            .rect_filled(shape, cr(height / 2.0), fill.gamma_multiply(alpha));
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            font,
            text.gamma_multiply(alpha),
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                cr(height / 2.0 + 2.0),
                Stroke::new(1.5, TEXT),
                StrokeKind::Outside,
            );
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).font(bold_font(30.0)).color(TEXT));
    ui.add_space(2.0);
    ui.label(RichText::new(subtitle).size(15.0).color(MUTED));
    ui.add_space(18.0);
}

fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(cr(16.0))
        .inner_margin(Margin::symmetric(20, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}

// ---------------------------------------------------------------------------
// Pages.

fn welcome(tour: &mut Tour, ui: &mut egui::Ui) {
    let t = if tour.animations {
        tour.step_changed.elapsed().as_secs_f32()
    } else {
        10.0
    };
    let time = ui.input(|input| input.time) as f32;
    let area = ui.available_rect_before_wrap();
    let logo = Rect::from_center_size(pos2(area.center().x, area.top() + 150.0), vec2(264.0, 96.0));
    showcase::logo(ui.painter(), logo, t - 0.2, time);
    ui.ctx().request_repaint();
    ui.add_space(270.0);

    let reveal = |from: f32| -> f32 { (((t - from) / 0.5).clamp(0.0, 1.0)).powf(0.7) };
    ui.vertical_centered(|ui| {
        ui.scope(|ui| {
            ui.set_opacity(reveal(1.2));
            ui.label(
                RichText::new("Welcome to Onyx")
                    .font(bold_font(38.0))
                    .color(TEXT),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "A Dynamic Island for Windows. Your music, voice typing and quick tools\n\
                     live in one small pill at the edge of your screen.",
                )
                .size(16.0)
                .color(MUTED),
            );
        });
    });
    ui.add_space(40.0);
    ui.scope(|ui| {
        ui.set_opacity(reveal(1.6));
        let features = [
            (
                "Music at a glance",
                "Album art, controls and gestures for whatever is playing, in any app.",
            ),
            (
                "Type with your voice",
                "Hold a key and speak. Clean text appears wherever you type, fully offline.",
            ),
            (
                "Tools in reach",
                "Convert files, do quick sums and keep an eye on your PC.",
            ),
        ];
        ui.columns(3, |columns| {
            for (column, (title, text)) in columns.iter_mut().zip(features) {
                card(column, |ui| {
                    ui.label(RichText::new(title).font(bold_font(15.0)).color(TEXT));
                    ui.add_space(2.0);
                    ui.label(RichText::new(text).size(13.0).color(MUTED));
                });
            }
        });
        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("The tour takes about two minutes.")
                    .small()
                    .color(MUTED),
            );
        });
    });
}

fn island_page(tour: &mut Tour, ui: &mut egui::Ui) {
    heading(
        ui,
        "Meet your island",
        "It lives at the edge of your screen. Here's what your hands can do; try each one on the real island as you go.",
    );
    let click = tour.config.general.expand_on == ExpandTrigger::Click;
    let gestures = [
        Gesture::Open { click },
        Gesture::SwipeSongs,
        Gesture::PushPlay,
        Gesture::PullMove,
        Gesture::ScrollWidgets,
        Gesture::DropConvert,
    ];
    let gap = 14.0;
    let width = ui.available_width();
    let tile_w = ((width - gap * 2.0) / 3.0).floor();
    let scene_h = 132.0;
    let time = ui.input(|input| input.time) as f32;
    for row in gestures.chunks(3) {
        let row_top = ui.cursor().top();
        let mut row_h: f32 = 0.0;
        for (i, gesture) in row.iter().enumerate() {
            let left = ui.max_rect().left() + i as f32 * (tile_w + gap);
            let painter = ui.painter();
            let title = painter.layout(
                gesture.title().to_owned(),
                bold_font(15.0),
                TEXT,
                tile_w - 28.0,
            );
            let text = painter.layout(
                gesture.description().to_owned(),
                FontId::proportional(13.0),
                MUTED,
                tile_w - 28.0,
            );
            let tile_h = 12.0 + scene_h + 14.0 + title.size().y + 4.0 + text.size().y + 16.0;
            row_h = row_h.max(tile_h);
            let tile = Rect::from_min_size(pos2(left, row_top), vec2(tile_w, tile_h));
            painter.rect_filled(tile, cr(16.0), CARD);
            let scene =
                Rect::from_min_size(tile.min + vec2(12.0, 12.0), vec2(tile_w - 24.0, scene_h));
            showcase::scene(painter, scene, *gesture, time, tour.animations);
            let title_pos = pos2(tile.left() + 14.0, scene.bottom() + 14.0);
            let text_pos = title_pos + vec2(0.0, title.size().y + 4.0);
            painter.galley(title_pos, title, TEXT);
            painter.galley(text_pos, text, MUTED);
        }
        ui.allocate_space(vec2(width, row_h));
        ui.add_space(gap);
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Music gestures work while a song is playing. Turn on the calculator and system stats in Settings, under Widgets.",
        )
        .small()
        .color(MUTED),
    );
    if tour.animations {
        ui.ctx().request_repaint();
    }
}

fn gestures_page(tour: &mut Tour, ui: &mut egui::Ui) {
    heading(
        ui,
        "Try the gestures",
        "Drag the island with your mouse: left, right, down and up. Each does here what it does on your screen.",
    );
    tour.practice.show(ui, tour.animations);
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes + 500_000) / 1_000_000)
}

fn eta(rate: f64, left: u64) -> String {
    let seconds = (left as f64 / rate.max(1.0)).ceil() as u64;
    let left = if seconds < 60 {
        "under a minute left".to_owned()
    } else {
        let minutes = seconds.div_ceil(60);
        format!("about {minutes} min left")
    };
    format!("{:.1} MB/s \u{b7} {left}", rate / 1_000_000.0)
}

/// A thin progress bar.
fn bar(ui: &mut egui::Ui, width: f32, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, cr(3.0), CARD_LINE);
    let filled = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), 6.0));
    ui.painter().rect_filled(filled, cr(3.0), LAVENDER);
}

/// One model's row: what it is, then its download state on the right.
fn model_row(
    tour: &mut Tour,
    ui: &mut egui::Ui,
    id: &'static str,
    title: &str,
    meta: &str,
    text: &str,
    speech: bool,
) {
    let status = tour.status_of(id);
    let progress = tour.models.download_progress(id);
    let message = tour.models.message(id);
    let mut start = false;
    let mut cancel = false;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let (icon, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            model_icon(ui, icon, speech);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 290.0).max(200.0));
                ui.label(RichText::new(title).font(bold_font(15.0)).color(TEXT));
                ui.label(RichText::new(meta).small().color(MUTED));
                ui.add_space(2.0);
                ui.label(RichText::new(text).size(13.0).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.set_width(270.0);
                match (progress, status) {
                    (Some((done, total)), _) => {
                        if button(ui, "Cancel", Kind::Quiet, true).clicked() {
                            cancel = true;
                        }
                        ui.vertical(|ui| {
                            ui.set_width(190.0);
                            ui.add_space(4.0);
                            let rate = tour.rate(id, done);
                            ui.label(
                                RichText::new(format!(
                                    "{} of {}",
                                    megabytes(done),
                                    megabytes(total)
                                ))
                                .small()
                                .color(TEXT),
                            );
                            bar(ui, 190.0, done as f32 / total.max(1) as f32);
                            ui.label(
                                RichText::new(match rate {
                                    Some(rate) => eta(rate, total.saturating_sub(done)),
                                    None => "Starting\u{2026}".to_owned(),
                                })
                                .small()
                                .color(MUTED),
                            );
                        });
                    }
                    (None, ModelStatus::Installed) => {
                        ui.label(RichText::new("Ready").font(bold_font(14.0)).color(OK));
                        check(ui, OK);
                    }
                    (None, ModelStatus::Partial { bytes }) => {
                        if button(ui, "Resume", Kind::Secondary, true).clicked() {
                            start = true;
                        }
                        ui.label(
                            RichText::new(format!("{} so far", megabytes(bytes)))
                                .small()
                                .color(MUTED),
                        );
                    }
                    (None, ModelStatus::Missing) => {
                        if button(ui, "Download", Kind::Secondary, true).clicked() {
                            start = true;
                        }
                    }
                }
            });
        });
        if let Some((text, good)) = message.filter(|(_, good)| !good) {
            ui.add_space(4.0);
            ui.label(
                RichText::new(text)
                    .small()
                    .color(if good { OK } else { DANGER }),
            );
        }
    });
    if start {
        tour.start(id);
    }
    if cancel {
        tour.models.cancel_download(id);
    }
}

fn check(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    let c = rect.center();
    ui.painter()
        .circle_filled(c, 10.0, color.gamma_multiply(0.18));
    ui.painter().add(egui::Shape::line(
        vec![
            c + vec2(-4.5, 0.3),
            c + vec2(-1.3, 3.5),
            c + vec2(4.6, -3.2),
        ],
        Stroke::new(2.0, color),
    ));
}

fn model_icon(ui: &egui::Ui, rect: Rect, speech: bool) {
    let painter = ui.painter();
    painter.rect_filled(rect, cr(12.0), Color32::from_rgb(34, 35, 41));
    let c = rect.center();
    let stroke = Stroke::new(1.8, LAVENDER);
    if speech {
        // Sound waves.
        for (dx, h) in [
            (-9.0, 5.0),
            (-4.5, 11.0),
            (0.0, 16.0),
            (4.5, 9.0),
            (9.0, 4.0),
        ] {
            painter.line_segment([c + vec2(dx, -h / 2.0), c + vec2(dx, h / 2.0)], stroke);
        }
    } else {
        // A sparkle: four-pointed star.
        let (long, short) = (10.0, 3.2);
        let points: Vec<_> = (0..8)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
                let r = if i % 2 == 0 { long } else { short };
                c + vec2(angle.cos(), angle.sin()) * r
            })
            .collect();
        painter.add(egui::Shape::convex_polygon(points, LAVENDER, Stroke::NONE));
        painter.circle_filled(c + vec2(9.0, -9.0), 2.0, LAVENDER);
    }
}

fn models_page(tour: &mut Tour, ui: &mut egui::Ui) {
    heading(
        ui,
        "Set up voice typing",
        "Onyx turns speech into text on this PC. Download two models once; after that it works offline and nothing you say leaves your computer.",
    );
    let speech = tour.speech_model();
    let speech_size = speech.total_size() + islet_asr::registry::SILERO_VAD.size;
    let cleanup_size = tour.models.manager().addon_download_size(&AI_CLEANUP);
    let ids = [speech.id, AI_CLEANUP.id];

    model_row(
        tour,
        ui,
        speech.id,
        "Speech recognition",
        &format!(
            "{} by {} \u{b7} {} \u{b7} {}",
            speech.name,
            speech.maker,
            speech.languages,
            megabytes(speech_size)
        ),
        "Hears what you say and writes it down. Needed for dictation and the voice assistant.",
        true,
    );
    ui.add_space(12.0);
    model_row(
        tour,
        ui,
        AI_CLEANUP.id,
        "AI cleanup",
        &format!("Onyx Cleanup, based on Gemma 3 270M \u{b7} Optional \u{b7} {}", megabytes(cleanup_size)),
        "Tidies what you said: drops the ums and false starts, applies your corrections and formats lists.",
        false,
    );
    ui.add_space(18.0);

    let running: Vec<(u64, u64)> = ids
        .iter()
        .filter_map(|id| tour.models.download_progress(id))
        .collect();
    let missing: Vec<&'static str> = ids
        .iter()
        .copied()
        .filter(|id| {
            tour.status_of(id) != ModelStatus::Installed
                && tour.models.download_progress(id).is_none()
        })
        .collect();
    ui.horizontal(|ui| {
        if !running.is_empty() {
            let done: u64 = running.iter().map(|(done, _)| done).sum();
            let total: u64 = running.iter().map(|(_, total)| total).sum();
            ui.label(
                RichText::new(format!(
                    "Downloading {}% \u{b7} you can keep going meanwhile.",
                    done * 100 / total.max(1)
                ))
                .color(MUTED),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        } else if missing.is_empty() {
            check(ui, OK);
            ui.label(
                RichText::new("Voice typing is ready.")
                    .font(bold_font(15.0))
                    .color(TEXT),
            );
        }
        if missing.len() == 2 {
            let size = megabytes(speech_size + cleanup_size);
            if button(ui, &format!("Download both ({size})"), Kind::Primary, true).clicked() {
                for id in missing {
                    tour.start(id);
                }
            }
        }
    });
    ui.add_space(26.0);
    ui.label(
        RichText::new(
            "Parakeet by NVIDIA, licensed CC-BY-4.0. The cleanup model is a fine-tune of Gemma 3; Gemma is \
             provided under and subject to the Gemma Terms of Use found at ai.google.dev/gemma/terms.",
        )
        .small()
        .color(MUTED),
    );
}

/// Whether the keys of `combo` are held right now.
fn held(combo: &Combo) -> bool {
    let down = |vk: u16| -> bool {
        // SAFETY: reads global key state.
        unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
    };
    let mut modifiers = 0u8;
    for vk in [0xA0u16, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C] {
        if down(vk) {
            modifiers |= modifier_bit(vk).unwrap_or(0);
        }
    }
    combo.modifiers.matches(modifiers) && combo.key.is_none_or(down)
}

/// The hotkey as keyboard keys, pressed in while `pressed`.
fn keycaps(ui: &mut egui::Ui, hotkey: &str, pressed: bool) {
    let parts: Vec<&str> = hotkey.split('+').map(str::trim).collect();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            ui.label(RichText::new("+").size(16.0).color(MUTED));
        }
        let font = bold_font(15.0);
        let width = ui
            .painter()
            .layout_no_wrap((*part).to_owned(), font.clone(), TEXT)
            .size()
            .x
            + 30.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width.max(48.0), 46.0), Sense::hover());
        let press = ui
            .ctx()
            .animate_bool_with_time(ui.id().with(("key", i)), pressed, 0.08);
        let base = Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(rect.width(), 40.0));
        ui.painter()
            .rect_filled(base, cr(10.0), Color32::from_rgb(14, 14, 17));
        let top = base.translate(vec2(0.0, -4.0 + 3.0 * press));
        ui.painter().rect_filled(
            top,
            cr(10.0),
            mix(
                Color32::from_rgb(40, 41, 47),
                Color32::from_rgb(56, 58, 86),
                press,
            ),
        );
        ui.painter().rect_stroke(
            top,
            cr(10.0),
            Stroke::new(1.0, mix(CARD_LINE, LAVENDER, press)),
            StrokeKind::Inside,
        );
        ui.painter()
            .text(top.center(), Align2::CENTER_CENTER, *part, font, TEXT);
    }
}

fn try_page(tour: &mut Tour, ui: &mut egui::Ui) -> Option<Step> {
    let mut go = None;
    let speech = tour.speech_model();
    let installed = tour.models.manager().status(speech) == ModelStatus::Installed;
    let hotkey = tour.config.dictation.push_to_talk.trim().to_owned();
    heading(
        ui,
        "Try it",
        if hotkey.is_empty() {
            "Click in the box, start dictation with your hotkey and say something."
        } else {
            "Click in the box, hold the key below, say something, then let go."
        },
    );
    if !installed {
        card(ui, |ui| {
            ui.label(
                RichText::new("The speech model isn't ready yet")
                    .font(bold_font(15.0))
                    .color(TEXT),
            );
            ui.add_space(4.0);
            match tour.models.download_progress(speech.id) {
                Some((done, total)) => {
                    ui.label(
                        RichText::new(format!(
                            "Downloading, {}% done. You can try dictation as soon as it finishes.",
                            done * 100 / total.max(1)
                        ))
                        .color(MUTED),
                    );
                    ui.add_space(6.0);
                    bar(
                        ui,
                        ui.available_width().min(420.0),
                        done as f32 / total.max(1) as f32,
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                }
                None => {
                    ui.label(RichText::new("Download it first, then come back here.").color(MUTED));
                    ui.add_space(8.0);
                    if button(ui, "Go to downloads", Kind::Secondary, true).clicked() {
                        go = Some(Step::Models);
                    }
                }
            }
        });
        return go;
    }

    tour.check_island();
    if !tour.island_running {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Onyx isn't running, so dictation can't start.").color(TEXT),
                );
                if button(ui, "Start Onyx", Kind::Secondary, true).clicked() {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new(exe.with_file_name("islet.exe")).spawn();
                    }
                    tour.island_checked = None;
                }
            });
        });
        ui.add_space(12.0);
    }
    if !tour.config.dictation.enabled {
        let mut turn_on = false;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Dictation is turned off.").color(TEXT));
                turn_on = button(ui, "Turn it on", Kind::Secondary, true).clicked();
            });
        });
        if turn_on {
            tour.change(|config| config.dictation.enabled = true);
        }
        ui.add_space(12.0);
    }

    // The microphone, beside the key, in case the default one isn't yours.
    let devices: Vec<(String, String)> = tour
        .models
        .microphones()
        .iter()
        .map(|device| (device.id.clone(), device.name.clone()))
        .collect();
    let mut microphone = tour.config.dictation.microphone.clone();
    let selected = if microphone.is_empty() {
        "System default".to_owned()
    } else {
        devices
            .iter()
            .find(|(id, _)| *id == microphone)
            .map_or_else(|| "Unavailable device".to_owned(), |(_, name)| name.clone())
    };
    let combo = Combo::parse(&hotkey).ok();
    let holding = combo.as_ref().is_some_and(held);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            egui::ComboBox::from_id_salt("tour-microphone")
                .selected_text(selected)
                .width(240.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut microphone, String::new(), "System default");
                    for (id, name) in &devices {
                        ui.selectable_value(&mut microphone, id.clone(), name);
                    }
                });
            ui.label(RichText::new("Microphone").color(MUTED));
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if hotkey.is_empty() {
                    ui.label(
                        RichText::new(
                            "No dictation hotkey is set. Choose one in Settings, under Dictation.",
                        )
                        .color(DANGER),
                    );
                } else {
                    keycaps(ui, &hotkey, holding);
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(if holding {
                            "Listening\u{2026} let go when you're done."
                        } else {
                            "Hold to talk"
                        })
                        .size(15.0)
                        .color(if holding { LAVENDER } else { MUTED }),
                    );
                }
            });
        });
    });
    if microphone != tour.config.dictation.microphone {
        tour.change(|config| config.dictation.microphone = microphone);
    }
    ui.add_space(14.0);

    let edit = ui.add(
        egui::TextEdit::multiline(&mut tour.sample_text)
            .id_salt("try-box")
            .font(FontId::proportional(17.0))
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(16, 14))
            .hint_text("Your words appear here."),
    );
    if tour.focus_box {
        edit.request_focus();
        tour.focus_box = false;
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("The first dictation loads the model, which takes a few seconds.")
                .small()
                .color(MUTED),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !tour.sample_text.is_empty() && button(ui, "Clear", Kind::Quiet, true).clicked() {
                tour.sample_text.clear();
                tour.focus_box = true;
            }
        });
    });
    ui.add_space(14.0);

    ui.columns(2, |columns| {
        card(&mut columns[0], |ui| {
            ui.label(RichText::new("Things to try").font(bold_font(15.0)).color(TEXT));
            ui.add_space(4.0);
            for (say, why) in [
                ("\u{201c}Um, so I think we should, uh, meet on Friday.\u{201d}", "The ums disappear."),
                ("\u{201c}Let's meet at four, no sorry, at five.\u{201d}", "Your correction wins."),
                ("\u{201c}I need three things: milk, eggs and bread.\u{201d}", "Lists become bullets."),
            ] {
                ui.label(RichText::new(say).size(13.5).color(TEXT));
                ui.label(RichText::new(why).small().color(MUTED));
            }
        });
        let assistant = tour.config.dictation.assistant_enabled
            && !tour.config.dictation.assistant_hotkey.trim().is_empty();
        let assistant_key = tour.config.dictation.assistant_hotkey.trim().to_owned();
        card(&mut columns[1], |ui| {
            ui.label(RichText::new("The voice assistant").font(bold_font(15.0)).color(TEXT));
            ui.add_space(4.0);
            if assistant {
                ui.label(
                    RichText::new(format!(
                        "Hold {assistant_key} and say what to do. The bars turn lavender while it listens."
                    ))
                    .size(13.5)
                    .color(MUTED),
                );
                ui.add_space(4.0);
                for say in [
                    "\u{201c}Open Calculator\u{201d}",
                    "\u{201c}Next song\u{201d}",
                    "\u{201c}Search for flights to Dubai\u{201d}",
                ] {
                    ui.label(RichText::new(say).size(13.5).color(TEXT));
                }
            } else {
                ui.label(
                    RichText::new("It's turned off. Turn it on in Settings, under Dictation.")
                        .size(13.5)
                        .color(MUTED),
                );
            }
        });
    });

    // Follow the key while this page is up.
    ui.ctx()
        .request_repaint_after(Duration::from_millis(if holding { 16 } else { 60 }));
    go
}

/// The island's code card, as it looks when a code arrives.
fn code_preview(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0), Sense::hover());
    let painter = ui.painter();
    let pill = Rect::from_center_size(rect.center(), vec2(300.0, 58.0));
    painter.rect_filled(pill, cr(29.0), Color32::BLACK);
    // Envelope.
    let mail = Rect::from_center_size(pos2(pill.left() + 29.0, pill.center().y), vec2(20.0, 14.0));
    let stroke = Stroke::new(1.6, LAVENDER);
    painter.rect_stroke(mail, cr(2.5), stroke, StrokeKind::Middle);
    painter.add(egui::Shape::line(
        vec![mail.left_top() + vec2(1.5, 1.5), pos2(mail.center().x, mail.center().y + 1.0), mail.right_top() + vec2(-1.5, 1.5)],
        stroke,
    ));
    let text_x = pill.left() + 52.0;
    painter.text(pos2(text_x, pill.top() + 15.0), Align2::LEFT_CENTER, "Code from github.com", FontId::proportional(11.0), MUTED);
    painter.text(pos2(text_x, pill.top() + 36.0), Align2::LEFT_CENTER, "482 913", bold_font(20.0), TEXT);
    // Copy mark: two overlapping sheets.
    let copy = pos2(pill.right() - 34.0, pill.center().y);
    let sheet = |offset: Vec2| Rect::from_center_size(copy + offset, vec2(10.0, 12.0));
    let mark = Stroke::new(1.4, MUTED);
    painter.rect_stroke(sheet(vec2(-2.0, 2.0)), cr(2.0), mark, StrokeKind::Middle);
    painter.rect_filled(sheet(vec2(2.0, -2.0)), cr(2.0), Color32::BLACK);
    painter.rect_stroke(sheet(vec2(2.0, -2.0)), cr(2.0), mark, StrokeKind::Middle);
}

fn gmail_page(tour: &mut Tour, ui: &mut egui::Ui) {
    heading(
        ui,
        "Sign-in codes from Gmail",
        "Connect Gmail, and the codes sites send you show on the island the moment they arrive. Click one to copy it. This step is optional.",
    );
    card(ui, code_preview);
    ui.add_space(12.0);

    let status = crate::mail_page::status();
    tour.check_island();
    let asked = tour.gmail_asked.is_some_and(|at| at.elapsed() < Duration::from_secs(4));
    let connecting = status.connecting || asked;
    let mut connect = false;
    let mut start_island = false;
    let mut on = tour.config.mail.gmail_codes;
    card(ui, |ui| {
        if !status.available {
            ui.label(RichText::new("Gmail sign-in isn't set up in this build of Onyx.").color(MUTED));
            return;
        }
        for email in &status.accounts {
            ui.horizontal(|ui| {
                check(ui, OK);
                ui.add_space(4.0);
                let shown = if email.is_empty() { "Gmail account" } else { email.as_str() };
                ui.label(RichText::new(shown).size(15.0).color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("Connected").small().color(OK));
                });
            });
            ui.add_space(8.0);
        }
        ui.horizontal(|ui| {
            if connecting {
                button(ui, "Waiting for your browser\u{2026}", Kind::Secondary, false);
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Finish signing in in the browser window that opened.")
                        .size(13.0)
                        .color(MUTED),
                );
            } else if status.accounts.len() >= crate::mail_page::MOST_ACCOUNTS {
                ui.label(RichText::new("Onyx checks up to 5 accounts.").size(13.0).color(MUTED));
            } else {
                let first = status.accounts.is_empty();
                let (label, kind, about) = if first {
                    ("Connect Google account", Kind::Primary, "Opens Google in your browser, where you allow read-only access.")
                } else {
                    ("Connect another account", Kind::Secondary, "Codes from every connected account show on the island.")
                };
                connect = button(ui, label, kind, tour.island_running).clicked();
                ui.add_space(6.0);
                ui.label(RichText::new(about).size(13.0).color(MUTED));
            }
        });
        if !tour.island_running {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Onyx isn't running, so it can't sign in.").color(TEXT));
                start_island = button(ui, "Start Onyx", Kind::Secondary, true).clicked();
            });
        }
        // Connected before, then switched off in Settings.
        if !status.accounts.is_empty() && !on {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Gmail on the island is turned off.").color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    toggle(ui, &mut on, "Gmail on the island");
                });
            });
        }
    });
    if start_island {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe.with_file_name("islet.exe")).spawn();
        }
        tour.island_checked = None;
    }
    if connect {
        // On in the settings first, so the island keeps checking after it
        // next reads them.
        on = true;
        tour.gmail_asked = Some(Instant::now());
    }
    if on != tour.config.mail.gmail_codes {
        tour.change(|config| config.mail.gmail_codes = on);
    }
    if connect {
        crate::mail_page::tell_island(crate::mail_page::ADD);
    }

    ui.add_space(16.0);
    ui.label(
        RichText::new(
            "Read-only: Onyx never sends, changes or deletes mail, and your mail goes only between this PC and Google.",
        )
        .small()
        .color(MUTED),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "You can add or remove accounts later, and choose whether the island shows only codes, all new mail or your own mix, in Settings, under Widgets.",
        )
        .small()
        .color(MUTED),
    );
    // The island writes how sign-in is going.
    ui.ctx().request_repaint_after(Duration::from_millis(500));
}

fn done_page(tour: &mut Tour, ui: &mut egui::Ui) {
    let t = if tour.animations {
        tour.step_changed.elapsed().as_secs_f32()
    } else {
        10.0
    };
    let time = ui.input(|input| input.time) as f32;
    ui.add_space(30.0);
    let area = ui.available_rect_before_wrap();
    let logo = Rect::from_center_size(pos2(area.center().x, area.top() + 50.0), vec2(160.0, 58.0));
    showcase::logo(ui.painter(), logo, t + 0.4, time);
    if t < 3.0 {
        ui.ctx().request_repaint();
    }
    ui.add_space(118.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("You're all set")
                .font(bold_font(34.0))
                .color(TEXT),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Onyx is at the edge of your screen. Right-click the island, or the Onyx icon\n\
                 in the corner of the taskbar, any time for Settings.",
            )
            .size(15.0)
            .color(MUTED),
        );
    });
    ui.add_space(26.0);

    let dictation = &tour.config.dictation;
    let mut tips: Vec<(String, &str)> = Vec::new();
    if !dictation.push_to_talk.trim().is_empty() {
        tips.push((
            format!("Hold {}", dictation.push_to_talk.trim()),
            "to dictate anywhere",
        ));
    }
    if dictation.assistant_enabled && !dictation.assistant_hotkey.trim().is_empty() {
        tips.push((
            format!("Hold {}", dictation.assistant_hotkey.trim()),
            "for the voice assistant",
        ));
    }
    tips.push((
        if tour.config.general.expand_on == ExpandTrigger::Click {
            "Click the island".to_owned()
        } else {
            "Hover the island".to_owned()
        },
        "for music and widgets",
    ));
    let count = tips.len();
    ui.columns(count, |columns| {
        for (column, (what, why)) in columns.iter_mut().zip(&tips) {
            card(column, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(what).font(bold_font(15.0)).color(TEXT));
                    ui.label(RichText::new(*why).size(13.0).color(MUTED));
                });
            });
        }
    });
    ui.add_space(14.0);

    let mut start_with_windows = tour.config.general.start_with_windows;
    let mut open_settings = false;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Start Onyx with Windows").color(TEXT));
                ui.label(
                    RichText::new("Have the island ready every time you sign in.")
                        .small()
                        .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                toggle(ui, &mut start_with_windows, "Start Onyx with Windows");
            });
        });
    });
    if start_with_windows != tour.config.general.start_with_windows {
        match crate::autostart::set(start_with_windows) {
            Ok(()) => tour.change(|config| config.general.start_with_windows = start_with_windows),
            Err(error) => {
                tour.status = Some(format!("Could not change the startup setting: {error}"))
            }
        }
    }
    ui.add_space(10.0);
    let downloading = [tour.speech_model().id, AI_CLEANUP.id]
        .iter()
        .any(|id| tour.models.download_progress(id).is_some());
    ui.horizontal(|ui| {
        if downloading {
            ui.label(
                RichText::new(
                    "Downloads are still running. Keep this window open until they finish, or resume them later in Settings.",
                )
                .small()
                .color(ACCENT),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            open_settings = button(ui, "Open Settings", Kind::Quiet, true).clicked();
        });
    });
    if open_settings {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_run_in_order_and_stop_at_the_ends() {
        assert_eq!(Step::Welcome.previous(), None);
        assert_eq!(Step::Welcome.next(), Some(Step::Island));
        assert_eq!(Step::Island.next(), Some(Step::Gestures));
        assert_eq!(Step::Try.next(), Some(Step::Gmail));
        assert_eq!(Step::Gmail.next(), Some(Step::Done));
        assert_eq!(Step::Done.next(), None);
        assert_eq!(Step::Models.previous(), Some(Step::Gestures));
    }

    #[test]
    fn download_estimates_read_naturally() {
        assert_eq!(megabytes(661_000_000), "661 MB");
        assert_eq!(
            eta(10_000_000.0, 50_000_000),
            "10.0 MB/s \u{b7} under a minute left"
        );
        assert_eq!(
            eta(5_000_000.0, 600_000_000),
            "5.0 MB/s \u{b7} about 2 min left"
        );
    }
}
