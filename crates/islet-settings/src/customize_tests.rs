//! Exercise the actual editor widgets without native windows or user preferences.
use super::*;
use crate::{dictation_page, preview, Page};

struct Editor {
    ctx: egui::Context,
    app: SettingsApp,
    time: f64,
}

impl Editor {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let ctx = egui::Context::default();
        ui::install_theme(&ctx);
        let mut config = islet_core::Config::default();
        config.general.position = islet_core::config::ScreenPosition::Top;
        config.general.top_margin_percent = 2.0;
        config.general.bottom_margin_percent = 4.0;
        config.general.top_expanded_scale = 1.1;
        config.general.bottom_expanded_scale = 0.9;
        let mut editor = Self {
            ctx,
            app: SettingsApp {
                customization: None,
                preview: preview::PreviewLink::disconnected_for_test(),
                page_changed: std::time::Instant::now(),
                animations: false,
                config_file: std::env::temp_dir().join(format!(
                    "islet-sizing-interaction-{}-{}.toml",
                    std::process::id(),
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                )),
                saved: config.clone(),
                config,
                page: Page::General,
                new_site: String::new(),
                status: None,
                dictation_ui: dictation_page::DictationUi::new(),
                initial_scroll: None,
            },
            time: 0.0,
        };
        editor.app.open_customization(&editor.ctx);
        for _ in 0..3 {
            editor.frame(Vec::new());
        }
        editor
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.time += 1.0 / 60.0;
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 1080.0),
                )),
                time: Some(self.time),
                events,
                focused: true,
                ..Default::default()
            },
            |ctx| ui::draw(&mut self.app, ctx),
        )
    }

    fn press(&mut self, key: egui::Key) -> egui::FullOutput {
        let result = self.frame(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        self.frame(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        result
    }

    fn focus_widget(&mut self, matches: impl Fn(&egui::WidgetInfo) -> bool) {
        let mut visited = Vec::new();
        for _ in 0..30 {
            let output = self.press(egui::Key::Tab);
            for event in output.platform_output.events {
                if let egui::output::OutputEvent::FocusGained(info) = event {
                    if matches(&info) {
                        return;
                    }
                    visited.push(info.label);
                }
            }
        }
        panic!("Could not focus editor widget; visited {visited:?}");
    }

    fn activate(&mut self, label: &str) {
        self.focus_widget(|info| info.label.as_deref() == Some(label));
        self.press(egui::Key::Enter);
    }
}

#[test]
fn widgets_toggle_persists_through_settings() {
    let mut editor = Editor::new();
    editor.press(egui::Key::Escape);
    editor.app.page = Page::Widgets;
    for _ in 0..3 {
        editor.frame(Vec::new());
    }
    assert!(!editor.app.config.widgets.calculator);
    editor.activate("Calculator");
    assert!(editor.app.config.widgets.calculator);
    editor.app.save_if_changed();
    assert!(
        islet_core::Config::load(&editor.app.config_file)
            .unwrap()
            .widgets
            .calculator
    );
    editor.activate("Calculator");
    assert!(!editor.app.config.widgets.calculator);
    let _ = std::fs::remove_file(&editor.app.config_file);
}

#[test]
fn editor_apply_and_cancel_keep_drafts_and_edges_separate() {
    let mut editor = Editor::new();
    let original = editor.app.config.clone();
    editor.activate("Reset edge");
    assert_eq!(
        editor
            .app
            .customization
            .as_ref()
            .unwrap()
            .draft
            .top_margin_percent,
        0.0
    );
    assert_eq!(editor.app.config, original, "editing must remain a draft");
    editor.activate("Cancel");
    assert!(editor.app.customization.is_none());
    assert_eq!(editor.app.config, original);

    editor.app.open_customization(&editor.ctx);
    editor.frame(Vec::new());
    editor.activate("Reset edge");
    editor.activate("Apply changes");
    assert!(editor.app.customization.is_none());
    let mut expected = original.clone();
    expected.general.top_margin_percent = 0.0;
    expected.general.top_expanded_scale = 1.0;
    assert_eq!(editor.app.config, expected, "only the edited edge changes");
    editor.app.save_if_changed();
    assert_eq!(
        islet_core::Config::load(&editor.app.config_file).unwrap(),
        expected
    );

    editor.app.open_customization(&editor.ctx);
    editor.frame(Vec::new());
    editor.activate("Bottom edge");
    editor.activate("Reset edge");
    editor.press(egui::Key::Escape);
    assert!(editor.app.customization.is_none());
    assert_eq!(
        editor.app.config, expected,
        "Escape must discard bottom draft"
    );

    editor.app.open_customization(&editor.ctx);
    editor.frame(Vec::new());
    editor.focus_widget(|info| {
        info.typ == egui::WidgetType::Slider && info.label.as_deref() == Some("of screen height")
    });
    for _ in 0..120 {
        editor.press(egui::Key::ArrowRight);
    }
    editor.focus_widget(|info| info.typ == egui::WidgetType::Slider && info.label.is_none());
    for _ in 0..60 {
        editor.press(egui::Key::ArrowRight);
    }
    let draft = &editor.app.customization.as_ref().unwrap().draft;
    assert_eq!(draft.top_margin_percent, 5.0);
    assert_eq!(draft.top_expanded_scale, 1.25);
    assert_eq!(
        editor.app.config, expected,
        "slider adjustments remain a draft"
    );
    editor.activate("Apply changes");
    expected.general.top_margin_percent = 5.0;
    expected.general.top_expanded_scale = 1.25;
    assert_eq!(editor.app.config, expected);
    editor.app.save_if_changed();
    assert_eq!(
        islet_core::Config::load(&editor.app.config_file).unwrap(),
        expected
    );
    let _ = std::fs::remove_file(&editor.app.config_file);
}
