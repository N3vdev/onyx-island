//! The Night light page: on or off, how warm, and when.
//!
//! Dragging the warmth slider (or holding Preview) shows the colour on the
//! screen right away, even in the daytime; letting go puts back whatever the
//! schedule says now.

use eframe::egui::{self, RichText};
use islet_core::night_light::{blend, display_time, format_time, is_night, kelvin, parse_time, ramp, white_point};
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::ColorSystem::SetDeviceGammaRamp;

use crate::ui::{card, divider, header, row, toggle, MUTED};
use crate::SettingsApp;

/// Times offered for turning on and off: every quarter hour.
const STEP_MINUTES: u32 = 15;

/// Whether the screen is showing a preview, so letting go restores it.
#[derive(Default)]
pub struct NightLightUi {
    /// The warmth on screen while previewing.
    previewing: Option<f32>,
}

fn now_minutes() -> u32 {
    // SAFETY: plain query of the local clock.
    let now = unsafe { GetLocalTime() };
    u32::from(now.wHour) * 60 + u32::from(now.wMinute)
}

fn time_picker(ui: &mut egui::Ui, id: &str, value: &mut String) {
    let current = parse_time(value).unwrap_or(0);
    egui::ComboBox::from_id_salt(id)
        .selected_text(display_time(current))
        .width(130.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            for minutes in (0..24 * 60).step_by(STEP_MINUTES as usize) {
                if ui.selectable_label(minutes == current, display_time(minutes)).clicked() {
                    *value = format_time(minutes);
                }
            }
        });
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    header(
        ui,
        "Night light",
        "Warms your screen in the evening, with less blue light, so it is easier on your eyes and your sleep.",
    );
    let night = &mut app.config.night_light;
    let start = parse_time(&night.start).unwrap_or(20 * 60);
    let end = parse_time(&night.end).unwrap_or(5 * 60);
    let mut preview = false;
    card(ui, "Night light", |ui| {
        row(
            ui,
            "Night light",
            "Turns on and off by itself at the times below. When it starts, the island's eyes get sleepy.",
            |ui| {
                toggle(ui, &mut night.enabled, "Night light");
            },
        );
        divider(ui);
        row(
            ui,
            "Warmth",
            &format!("{:.0} K. Drag to see it on your screen now.", kelvin(night.intensity)),
            |ui| {
                let mut percent = (night.intensity * 100.0).round();
                let slider = ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%").integer());
                night.intensity = (percent / 100.0).clamp(0.0, 1.0);
                let held = ui.button("Preview").on_hover_text("Hold to see the night colour");
                preview = slider.dragged() || held.is_pointer_button_down_on();
            },
        );
        divider(ui);
        let status = if !night.enabled {
            "Off.".to_owned()
        } else if is_night(now_minutes(), start, end) {
            format!("On now, until {}.", display_time(end))
        } else {
            format!("Comes on at {}.", display_time(start))
        };
        ui.label(RichText::new(status).small().color(MUTED));
    });
    card(ui, "Island", |ui| {
        row(
            ui,
            "Eyes on the island",
            "The eyes are yellow while it is on. As it turns on they grow sleepy and yawn; as it turns off they flutter awake and smile.",
            |ui| {
                if ui.button("Turning off").clicked() {
                    play_greeting(false);
                }
                if ui.button("Turning on").clicked() {
                    play_greeting(true);
                }
            },
        );
    });
    card(ui, "Schedule", |ui| {
        row(ui, "Turns on", "The screen warms gradually over a minute.", |ui| {
            time_picker(ui, "night_start", &mut night.start);
        });
        divider(ui);
        row(ui, "Turns off", "Can be the next morning.", |ui| {
            time_picker(ui, "night_end", &mut night.end);
        });
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Night light changes the colour table your graphics card already applies, so it uses no processor or \
             graphics time. Screenshots and recordings are not tinted.",
        )
        .small()
        .color(MUTED),
    );

    let state = &mut app.night_light_ui;
    if preview {
        if state.previewing != Some(night.intensity) {
            apply(white_point(kelvin(night.intensity)));
            state.previewing = Some(night.intensity);
        }
    } else if state.previewing.take().is_some() {
        // Back to what the schedule says now; Onyx keeps it from there.
        let on = night.enabled && is_night(now_minutes(), start, end);
        apply(if on { white_point(kelvin(night.intensity)) } else { [1.0; 3] });
    }
}

/// Asks the island to play the night light's eyes now.
fn play_greeting(on: bool) {
    use windows::core::w;
    use windows::Win32::Foundation::WPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
    // SAFETY: finds Onyx's island window and posts it a message.
    unsafe {
        if let Ok(island) = FindWindowW(w!("IsletIslandWindow"), None) {
            let _ = PostMessageW(
                Some(island),
                islet_core::handoff::NIGHT_LIGHT_GREETING,
                WPARAM(usize::from(on)),
                LPARAM(0),
            );
        }
    }
}

/// Puts `multipliers` on every display (as Onyx does; see its night light).
fn apply(multipliers: [f32; 3]) {
    for device in display_devices() {
        let wide: Vec<u16> = device.iter().copied().chain(std::iter::once(0)).collect();
        // SAFETY: the DC is created, used and deleted here.
        unsafe {
            let dc = CreateDCW(PCWSTR::null(), PCWSTR(wide.as_ptr()), PCWSTR::null(), None);
            if dc.is_invalid() {
                continue;
            }
            let mut amount = 1.0f32;
            while amount > 0.0 && !SetDeviceGammaRamp(dc, ramp(blend(multipliers, amount)).as_ptr().cast()).as_bool() {
                amount -= 0.1;
            }
            let _ = DeleteDC(dc);
        }
    }
}

fn display_devices() -> Vec<Vec<u16>> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec passed below, alive for the call.
        let devices = unsafe { &mut *(data.0 as *mut Vec<Vec<u16>>) };
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO { cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32, ..Default::default() },
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
            let end = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
            devices.push(info.szDevice[..end].to_vec());
        }
        BOOL(1)
    }
    let mut devices: Vec<Vec<u16>> = Vec::new();
    // SAFETY: the callback only runs during this call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut devices as *mut _ as isize));
    }
    devices
}
