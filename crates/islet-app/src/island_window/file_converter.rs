use super::*;
use islet_core::converter::{ConversionOption, ConversionStage, ConverterAction, ConverterCard};
use std::path::{Path, PathBuf};

/// The argument that makes Explorer open `path`'s folder with it selected.
/// Explorer needs the quotes around the path alone (`/select,"C:\a b\x.pdf"`);
/// a fully quoted `"/select,C:\a b\x.pdf"`, which `Command::arg` produces for
/// any path with a space, makes it open Documents instead. Windows paths can't
/// contain `"`, so this quoting is always well formed.
fn explorer_select_arg(path: &Path) -> String {
    format!("/select,\"{}\"", path.display())
}

/// Opens the folder holding `path` in Explorer, with the file selected.
fn reveal_in_explorer(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer.exe")
        .raw_arg(explorer_select_arg(path))
        .spawn();
}

impl IslandHost {
    pub(super) fn converter_visible(&self) -> bool {
        self.state.expanded
            && matches!(
                self.state.primary_card(),
                Some(islet_core::ContentCard::Converter(_))
            )
    }

    fn converter_begin(&mut self, files: Vec<PathBuf>) {
        if self
            .state
            .converter
            .as_ref()
            .is_some_and(|c| c.stage == ConversionStage::Running)
        {
            return;
        }
        self.converter_generation = self.converter_generation.wrapping_add(1);
        self.converter_job = None;
        self.converter_paths = files;
        self.converter_outputs.clear();
        let title = if self.converter_paths.len() == 1 {
            self.converter_paths[0]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            format!("{} files", self.converter_paths.len())
        };
        let extension = self
            .converter_paths
            .first()
            .and_then(|p| p.extension())
            .map(|ext| ext.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let subtitle = if extension.is_empty() {
            "Choose a conversion format".into()
        } else {
            format!("{extension} · Choose a conversion format")
        };
        self.dispatch(IslandEvent::SetConverter(Some(ConverterCard::new(
            title, subtitle,
        ))));
        self.converter_start(None);
    }

    fn converter_start(&mut self, operation: Option<&str>) {
        self.converter_generation = self.converter_generation.wrapping_add(1);
        self.converter_job = None;
        match crate::converter::start(
            self.hwnd,
            self.converter_tx.clone(),
            self.converter_generation,
            &self.converter_paths,
            operation,
        ) {
            Ok(job) => self.converter_job = Some(job),
            Err(_) => {
                if let Some(mut card) = self.state.converter.clone() {
                    card.stage = ConversionStage::Error;
                    card.message =
                        "File conversion is unavailable. Reinstall Onyx and try again.".into();
                    self.dispatch(IslandEvent::SetConverter(Some(card)));
                }
            }
        }
    }

    pub(super) fn on_file_drop(&mut self) {
        let events: Vec<_> = self
            .file_drop
            .as_ref()
            .map(|target| target.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                crate::file_drop::Event::Enter(files) => self.converter_begin(files),
                crate::file_drop::Event::Over(mut point) => {
                    let _ = unsafe { ScreenToClient(self.hwnd, &mut point) };
                    self.converter_hover(point);
                }
                crate::file_drop::Event::Leave => {
                    if self.state.converter.as_ref().is_some_and(|c| c.dragging) {
                        self.converter_dismiss();
                    }
                }
                crate::file_drop::Event::Drop(files, mut point) => {
                    if self
                        .state
                        .converter
                        .as_ref()
                        .is_some_and(|c| c.stage == ConversionStage::Running)
                    {
                        continue;
                    }
                    if files != self.converter_paths {
                        self.converter_begin(files);
                    }
                    let _ = unsafe { ScreenToClient(self.hwnd, &mut point) };
                    let action = self.converter_action_at(point);
                    if let Some(mut card) = self.state.converter.clone() {
                        card.dragging = false;
                        self.dispatch(IslandEvent::SetConverter(Some(card)));
                    }
                    if matches!(action, Some(ConverterAction::Format(_))) {
                        self.converter_action(action.unwrap());
                    }
                }
            }
        }
    }

    pub(super) fn on_converter_update(&mut self) {
        let updates: Vec<_> = self.converter_rx.try_iter().collect();
        for update in updates {
            if update.generation != self.converter_generation {
                continue;
            }
            let Some(mut card) = self.state.converter.clone() else {
                continue;
            };
            let value = update.value;
            match value["type"].as_str() {
                Some("catalog") => {
                    if let Some(pixels) = value["preview"].as_array().filter(|pixels| pixels.len() == 256 * 256 * 4) {
                        let pixels: Vec<u8> = pixels.iter().filter_map(|v| v.as_u64().and_then(|v| u8::try_from(v).ok())).collect();
                        if pixels.len() == 256 * 256 * 4 {
                            card.preview = Some(islet_core::media::ArtImage { id: self.converter_generation.wrapping_add(0x8000000000000000), width:256, height:256, pixels:pixels.into() });
                        }
                    }

                    card.options = value["operations"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|op| {
                            Some(ConversionOption {
                                id: op["id"].as_str()?.into(),
                                label: op["label"].as_str()?.into(),
                            })
                        })
                        .take(8)
                        .collect();
                    card.stage = if card.options.is_empty() {
                        ConversionStage::Error
                    } else {
                        ConversionStage::Ready
                    };
                    card.message = if card.options.is_empty() {
                        "No compatible conversion formats for these files on this PC.".into()
                    } else {
                        String::new()
                    };
                    self.converter_job = None;
                }
                Some("progress") => {
                    card.progress = (value["fraction"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0)
                        * 100.0)
                        .round() as u8
                }
                Some("outcome") => {
                    if let Some(path) = value["output"].as_str() {
                        self.converter_outputs.push(PathBuf::from(path));
                    }
                    if let Some(message) = value["message"].as_str() {
                        card.message = message.into();
                    }
                }
                Some("complete") => {
                    tracing::info!(has_title=!card.title.is_empty(),has_subtitle=!card.subtitle.is_empty(),"File conversion finished");
                    let succeeded = value["succeeded"].as_u64().unwrap_or(0);
                    let failed = value["failed"].as_u64().unwrap_or(0);
                    let canceled = value["canceled"].as_bool().unwrap_or(false);
                    card.fully_succeeded = succeeded > 0 && failed == 0 && !canceled;
                    card.stage = if succeeded > 0 {
                        ConversionStage::Complete
                    } else {
                        ConversionStage::Error
                    };
                    card.progress = 100;
                    card.message = if canceled {
                        format!("Canceled · {succeeded} completed · Original files unchanged")
                    } else if failed > 0 {
                        if card.message.is_empty() {
                            format!("{succeeded} converted · {failed} failed")
                        } else {
                            card.message
                        }
                    } else {
                        format!(
                            "{succeeded} {} saved beside the original",
                            if succeeded == 1 { "result" } else { "results" }
                        )
                    };
                    self.converter_job = None;
                }
                Some("error") => {
                    card.stage = ConversionStage::Error;
                    card.message = value["message"]
                        .as_str()
                        .unwrap_or("The conversion failed. Try another format.")
                        .into();
                    self.converter_job = None;
                }
                _ => continue,
            }
            self.dispatch(IslandEvent::SetConverter(Some(card)));
        }
        if let Some(drop) = &self.file_drop {
            drop.set_enabled(
                read_config(&self.shared).widgets.file_converter
                    && !self
                        .state
                        .converter
                        .as_ref()
                        .is_some_and(|c| c.stage == ConversionStage::Running),
            );
        }
        self.update_converter_hover();
    }

    fn converter_action_at(&self, point: POINT) -> Option<ConverterAction> {
        if !self.converter_visible() {
            return None;
        }
        let (x, y) = self.widget_point(point)?;
        self.state.converter.as_ref()?.action_at(x, y)
    }

    fn converter_hover(&mut self, point: POINT) {
        let hover = self.converter_action_at(point);
        if let Some(mut card) = self.state.converter.clone() {
            if card.hover != hover {
                card.hover = hover;
                self.dispatch(IslandEvent::SetConverter(Some(card)));
            }
        }
    }

    pub(super) fn update_converter_hover(&mut self) {
        if !self.converter_visible() {
            return;
        }
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_ok() {
            let _ = unsafe { ScreenToClient(self.hwnd, &mut point) };
            self.converter_hover(point);
        }
    }

    pub(super) fn handle_converter_click(&mut self, lparam: LPARAM) -> bool {
        if !self.converter_visible() {
            return false;
        }
        let point = point_from_lparam(lparam);
        if let Some(action) = self.converter_action_at(point) {
            self.converter_action(action);
        }
        self.widget_point(point).is_some()
    }

    pub(super) fn handle_converter_key(&mut self, key: u16) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB};
        if key == VK_ESCAPE.0 {
            self.converter_action(ConverterAction::Close);
            return true;
        }
        if key == VK_TAB.0 {
            let Some(mut card) = self.state.converter.clone() else {
                return false;
            };
            let mut actions = Vec::new();
            if card.stage == ConversionStage::Ready {
                actions.extend((0..card.options.len()).map(ConverterAction::Format));
            }
            if matches!(
                card.stage,
                ConversionStage::Running | ConversionStage::Complete | ConversionStage::Error
            ) {
                actions.push(ConverterAction::Footer);
            }
            actions.push(ConverterAction::Close);
            let backwards = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            let current = card
                .keyboard_focus
                .and_then(|focus| actions.iter().position(|action| *action == focus));
            let next = match current {
                None => {
                    if backwards {
                        actions.len() - 1
                    } else {
                        0
                    }
                }
                Some(i) => {
                    if backwards {
                        (i + actions.len() - 1) % actions.len()
                    } else {
                        (i + 1) % actions.len()
                    }
                }
            };
            card.keyboard_focus = Some(actions[next]);
            self.dispatch(IslandEvent::SetConverter(Some(card)));
            return true;
        }
        if key == VK_RETURN.0 || key == VK_SPACE.0 {
            if let Some(action) = self
                .state
                .converter
                .as_ref()
                .and_then(|card| card.keyboard_focus.or(card.hover))
            {
                self.converter_action(action);
            }
            return true;
        }
        false
    }

    fn converter_action(&mut self, action: ConverterAction) {
        let Some(mut card) = self.state.converter.clone() else {
            return;
        };
        match action {
            ConverterAction::Format(index) if card.stage == ConversionStage::Ready => {
                let Some(option) = card.options.get(index).cloned() else {
                    return;
                };
                card.stage = ConversionStage::Running;
                card.dragging = false;
                card.progress = 0;
                card.message = format!(
                    "Converting to {} · Original files stay unchanged",
                    option.label
                );
                card.hover = None;
                self.converter_outputs.clear();
                self.dispatch(IslandEvent::SetConverter(Some(card)));
                if let Some(drop) = &self.file_drop {
                    drop.set_enabled(false);
                }
                self.converter_start(Some(&option.id));
            }
            ConverterAction::Close | ConverterAction::Footer
                if card.stage == ConversionStage::Running =>
            {
                if let Some(job) = &self.converter_job {
                    job.cancel();
                }
                card.message = "Canceling · Keeping completed results and original files".into();
                self.dispatch(IslandEvent::SetConverter(Some(card)));
            }
            ConverterAction::Footer if card.stage == ConversionStage::Complete => {
                if let Some(path) = self.converter_outputs.first() {
                    reveal_in_explorer(path);
                }
            }
            ConverterAction::Footer
                if card.stage == ConversionStage::Error && !card.options.is_empty() =>
            {
                card.stage = ConversionStage::Ready;
                card.message.clear();
                card.hover = None;
                self.dispatch(IslandEvent::SetConverter(Some(card)));
            }
            ConverterAction::Close | ConverterAction::Footer => self.converter_dismiss(),
            _ => {}
        }
    }

    fn converter_dismiss(&mut self) {
        self.converter_generation = self.converter_generation.wrapping_add(1);
        self.converter_job = None;
        self.converter_paths.clear();
        self.converter_outputs.clear();
        self.dispatch(IslandEvent::SetConverter(None));
        self.release_calculator_focus();
    }
}

#[cfg(test)]
mod reveal_tests {
    use super::*;

    #[test]
    fn explorer_gets_the_path_quoted_on_its_own() {
        let path = Path::new(r"C:\Users\me\My Files\report (watermarked).pdf");
        assert_eq!(
            explorer_select_arg(path),
            r#"/select,"C:\Users\me\My Files\report (watermarked).pdf""#
        );
    }
}
