//! An invisible geometry anchor for the application's existing native island.
//! The renderer stays in its own process; settings never duplicates its GPU,
//! audio, or media workers. Messages are sent only when geometry changes.

use eframe::egui;
use islet_core::handoff::{PREVIEW_ATTACH, PREVIEW_DETACH};
use std::time::{Duration, Instant};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, FindWindowW, GetPropW, IsWindow, PostMessageW, SetPropW,
    SetWindowPos, SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SWP_NOACTIVATE, SWP_NOZORDER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WS_CHILD,
};

pub fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    enabled.as_bool()
}

pub struct PreviewLink {
    anchor: Option<HWND>,
    island: Option<HWND>,
    geometry: Option<[i32; 4]>,
    started: Instant,
    launched: bool,
    closing: Option<Instant>,
    mode: Option<[usize; 3]>,
    last_attach: Option<Instant>,
}

impl Default for PreviewLink {
    fn default() -> Self {
        Self {
            anchor: None,
            island: None,
            geometry: None,
            started: Instant::now(),
            launched: false,
            closing: None,
            mode: None,
            last_attach: None,
        }
    }
}

impl PreviewLink {
    #[cfg(test)]
    pub(crate) fn disconnected_for_test() -> Self {
        Self {
            closing: Some(Instant::now()),
            ..Self::default()
        }
    }

    pub fn connected(&self) -> bool {
        self.anchor.is_some_and(|anchor| unsafe {
            GetPropW(anchor, w!("IsletPreviewAttached")).0 as usize == 2
        })
    }

    pub fn update(
        &mut self,
        ctx: &egui::Context,
        rect: egui::Rect,
        customization: Option<(bool, f32, f32)>,
    ) {
        if self.closing.is_some() {
            return;
        }
        if self.anchor.is_none() {
            let Ok(parent) = (unsafe { FindWindowW(None, w!("Onyx Settings")) }) else {
                if self.started.elapsed() < Duration::from_secs(3) {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
                return;
            };
            self.anchor = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("IsletPreviewAnchor"),
                    WS_CHILD,
                    0,
                    0,
                    1,
                    1,
                    Some(parent),
                    None,
                    None,
                    None,
                )
            }
            .ok();
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        let mode = customization.map_or([0, 1, 1000], |(top, margin, scale)| {
            [
                if top { 1 } else { 2 },
                (margin.clamp(0.0, 5.0) * 100.0).round() as usize + 1,
                (scale.clamp(0.8, 1.25) * 1000.0).round() as usize,
            ]
        });
        let mode_changed = self.mode != Some(mode);
        if mode_changed {
            for (key, value) in [
                (w!("IsletCustomize"), mode[0]),
                (w!("IsletMargin"), mode[1]),
                (w!("IsletScale"), mode[2]),
            ] {
                unsafe {
                    let _ = SetPropW(anchor, key, Some(HANDLE(value as *mut _)));
                }
            }
            self.mode = Some(mode);
        }
        let scale = ctx.pixels_per_point();
        let geometry = [rect.left(), rect.top(), rect.width(), rect.height()]
            .map(|v| (v * scale).round() as i32);
        let changed = self.geometry != Some(geometry) || mode_changed;
        if changed {
            unsafe {
                let _ = SetWindowPos(
                    anchor,
                    None,
                    geometry[0],
                    geometry[1],
                    geometry[2],
                    geometry[3],
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            self.geometry = Some(geometry);
        }
        if self
            .island
            .is_some_and(|hwnd| !unsafe { IsWindow(Some(hwnd)) }.as_bool())
        {
            self.island = None;
        }
        if self.island.is_none() {
            self.island = unsafe { FindWindowW(w!("IsletIslandWindow"), None) }.ok();
            if self.island.is_none() && !self.launched {
                self.launched = true;
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe.with_file_name("islet.exe")).spawn();
                }
            }
            if let Some(island) = self.island {
                unsafe {
                    let _ = PostMessageW(
                        Some(island),
                        PREVIEW_ATTACH,
                        WPARAM(anchor.0 as usize),
                        LPARAM(0),
                    );
                }
                self.last_attach = Some(Instant::now());
            }
        } else if changed
            || self.last_attach.is_none_or(|last| {
                last.elapsed()
                    >= if self.connected() {
                        Duration::from_secs(2)
                    } else {
                        Duration::from_millis(500)
                    }
            })
        {
            unsafe {
                let _ = PostMessageW(
                    self.island,
                    PREVIEW_ATTACH,
                    WPARAM(anchor.0 as usize),
                    LPARAM(0),
                );
            }
            self.last_attach = Some(Instant::now());
        }
        // Slow health checks recover from a restarted island; attachment retries
        // never expire while a slow GPU startup or another handoff is pending.
        ctx.request_repaint_after(if self.connected() {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(250)
        });
    }

    pub fn close_requested(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) && self.closing.is_none() {
            self.closing = Some(Instant::now());
            self.detach();
        }
        if let Some(started) = self.closing {
            let attached = self.anchor.is_some_and(|hwnd| {
                !unsafe { GetPropW(hwnd, w!("IsletPreviewAttached")) }.is_invalid()
            });
            if attached && started.elapsed() < Duration::from_millis(1200) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.request_repaint_after(Duration::from_millis(30));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn detach(&self) {
        if let (Some(island), Some(anchor)) = (self.island, self.anchor) {
            unsafe {
                let _ = PostMessageW(
                    Some(island),
                    PREVIEW_DETACH,
                    WPARAM(anchor.0 as usize),
                    LPARAM(0),
                );
            }
        }
    }
}

impl Drop for PreviewLink {
    fn drop(&mut self) {
        self.detach();
        if let Some(anchor) = self.anchor {
            unsafe {
                let _ = DestroyWindow(anchor);
            }
        }
    }
}
