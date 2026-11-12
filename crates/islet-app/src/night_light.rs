//! The night light: warms every screen from the evening until morning.
//!
//! It sets each display's gamma ramp, a colour table the graphics card
//! already applies to every frame, so warming the screen costs nothing to
//! draw. One small thread does the work and sleeps in between: it wakes when
//! the night begins or ends, every quarter second for the minute a fade
//! takes, and once a minute while warm (Windows resets the ramp after sleep
//! or a display change, so it is put back).

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, RecvTimeoutError, Sender};
use islet_core::config::NightLightConfig;
use islet_core::night_light::{blend, is_night, kelvin, minutes_until, parse_time, ramp, white_point};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::ColorSystem::SetDeviceGammaRamp;

/// A scheduled start or end fades over a minute, too slowly to notice.
const SCHEDULED_FADE: Duration = Duration::from_secs(60);
/// Turning it on or changing the warmth in Settings shows at once.
const QUICK_FADE: Duration = Duration::from_millis(1_500);
const FADE_STEP: Duration = Duration::from_millis(250);
/// While warm, the ramp is put back this often.
const REAPPLY: Duration = Duration::from_secs(60);
/// Asleep before the night, it still checks the clock this often (the PC
/// may have slept, or the clock changed).
const MAX_SLEEP: Duration = Duration::from_secs(5 * 60);

enum Command {
    Configure(NightLightConfig),
    Shutdown,
}

/// The running night light. Dropping it puts the screens back as they were.
pub struct NightLight {
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl NightLight {
    /// Starts the night light. `on_change(on, announce)` runs (on its
    /// thread) whenever it turns on or off: `announce` when that happened
    /// while Onyx was running (the eyes greet it), not when Onyx starts up
    /// at night.
    pub fn spawn(config: &NightLightConfig, on_change: Box<dyn Fn(bool, bool) + Send>) -> Self {
        let (tx, rx) = unbounded();
        let config = config.clone();
        let thread = std::thread::Builder::new()
            .name("night-light".to_owned())
            .spawn(move || {
                let mut light = Schedule::new(config);
                loop {
                    let wait = light.step(&on_change);
                    match rx.recv_timeout(wait) {
                        Ok(Command::Configure(config)) => light.configure(config),
                        Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
                if light.level > 0.0 {
                    let _ = apply([1.0; 3]);
                }
            })
            .ok();
        Self { tx, thread }
    }

    pub fn configure(&self, config: &NightLightConfig) {
        let _ = self.tx.send(Command::Configure(config.clone()));
    }
}

impl Drop for NightLight {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Schedule {
    config: NightLightConfig,
    /// How much of the night colour is on the screens, 0..=1.
    level: f32,
    fade: Duration,
    was_night: Option<bool>,
    last_step: Instant,
    last_applied: Option<(Instant, [f32; 3])>,
}

impl Schedule {
    fn new(config: NightLightConfig) -> Self {
        Self {
            config,
            level: 0.0,
            fade: QUICK_FADE,
            was_night: None,
            last_step: Instant::now(),
            last_applied: None,
        }
    }

    fn configure(&mut self, config: NightLightConfig) {
        if config != self.config {
            self.config = config;
            // A change made in Settings shows straight away.
            self.fade = QUICK_FADE;
        }
    }

    fn times(&self) -> (u32, u32) {
        (
            parse_time(&self.config.start).unwrap_or(20 * 60),
            parse_time(&self.config.end).unwrap_or(5 * 60),
        )
    }

    /// Moves toward what the clock says, applies it, and returns how long
    /// to sleep.
    fn step(&mut self, on_change: &dyn Fn(bool, bool)) -> Duration {
        let now = local_minutes();
        let (start, end) = self.times();
        let on = self.config.enabled && self.config.intensity > 0.0;
        let night = on && is_night(now, start, end);
        match self.was_night {
            // Onyx started: at night, warm up quickly and quietly.
            None => {
                self.fade = QUICK_FADE;
                if night {
                    on_change(true, false);
                }
            }
            Some(false) if night => {
                on_change(true, true);
                // The clock reached the start: fade slowly. Turned on in
                // Settings: `configure` already chose a quick fade.
                if self.fade != QUICK_FADE {
                    self.fade = SCHEDULED_FADE;
                }
            }
            Some(true) if !night => {
                on_change(false, true);
                if self.fade != QUICK_FADE {
                    self.fade = SCHEDULED_FADE;
                }
            }
            _ => {}
        }
        self.was_night = Some(night);

        let target = if night { 1.0 } else { 0.0 };
        let elapsed = self.last_step.elapsed().min(Duration::from_secs(1));
        self.last_step = Instant::now();
        let step = elapsed.as_secs_f32() / self.fade.as_secs_f32().max(0.001);
        self.level = if self.level < target {
            (self.level + step).min(target)
        } else {
            (self.level - step).max(target)
        };
        let colour = blend(white_point(kelvin(self.config.intensity)), self.level);
        let stale = self.last_applied.is_none_or(|(at, applied)| {
            applied != colour || (self.level > 0.0 && at.elapsed() >= REAPPLY)
        });
        // Daylight is only written once, when coming back from warm.
        let needed = self.level > 0.0 || self.last_applied.is_some_and(|(_, applied)| applied != [1.0; 3]);
        if stale && needed {
            let _ = apply(colour);
            self.last_applied = Some((Instant::now(), colour));
        }

        if (self.level - target).abs() > f32::EPSILON {
            return FADE_STEP;
        }
        // Settled: later changes are slow unless Settings asks otherwise.
        self.fade = SCHEDULED_FADE;
        if night {
            return REAPPLY;
        }
        let until = if on { Duration::from_secs(u64::from(minutes_until(now, start).max(1)) * 60) } else { MAX_SLEEP };
        until.min(MAX_SLEEP)
    }
}

fn local_minutes() -> u32 {
    // SAFETY: plain query of the local clock.
    let now = unsafe { GetLocalTime() };
    u32::from(now.wHour) * 60 + u32::from(now.wMinute)
}

/// Puts `multipliers` on every display. Windows refuses ramps it thinks are
/// too extreme on some systems; then the warmest it accepts is used.
/// Returns how much of the warmth the most limited display took (0..=1).
fn apply(multipliers: [f32; 3]) -> f32 {
    let mut least = 1.0f32;
    for device in display_devices() {
        let wide: Vec<u16> = device.iter().copied().chain(std::iter::once(0)).collect();
        // SAFETY: the DC is created, used and deleted here.
        unsafe {
            let dc = CreateDCW(windows::core::PCWSTR::null(), windows::core::PCWSTR(wide.as_ptr()), windows::core::PCWSTR::null(), None);
            if dc.is_invalid() {
                continue;
            }
            let mut amount = 1.0f32;
            loop {
                let table = ramp(blend(multipliers, amount));
                if SetDeviceGammaRamp(dc, table.as_ptr().cast()).as_bool() || amount <= 0.0 {
                    if amount < 1.0 {
                        tracing::info!(amount, "night light: Windows allowed only part of the warmth");
                    }
                    least = least.min(amount.max(0.0));
                    break;
                }
                amount -= 0.1;
            }
            let _ = DeleteDC(dc);
        }
    }
    least
}

/// The device names ("\\.\DISPLAY1") of the attached displays.
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

#[cfg(test)]
mod tests {
    use super::*;
    use islet_core::night_light::WARMEST_KELVIN;
    use windows::Win32::UI::ColorSystem::GetDeviceGammaRamp;

    /// Warms the real screens for a moment, reads the ramp back from the
    /// driver, and puts them back. Changes what is on screen, so it only
    /// runs on request.
    #[test]
    #[ignore = "briefly warms the real screens"]
    fn the_screens_take_the_warm_ramp_and_get_it_back_off() {
        let devices = display_devices();
        assert!(!devices.is_empty(), "no displays found");
        let read_blue_top = |device: &Vec<u16>| unsafe {
            let wide: Vec<u16> = device.iter().copied().chain(std::iter::once(0)).collect();
            let dc = CreateDCW(windows::core::PCWSTR::null(), windows::core::PCWSTR(wide.as_ptr()), windows::core::PCWSTR::null(), None);
            let mut table = [[0u16; 256]; 3];
            let ok = GetDeviceGammaRamp(dc, table.as_mut_ptr().cast()).as_bool();
            let _ = DeleteDC(dc);
            ok.then_some(table[2][255])
        };
        for (intensity, label) in [(0.75f32, "default 75%"), (1.0, "warmest")] {
            let took = apply(white_point(kelvin(intensity)));
            let blue: Vec<_> = devices.iter().map(read_blue_top).collect();
            println!("{label}: {:.0} K, Windows took {:.0}% of it, blue at full white now {blue:?} of 65535", kelvin(intensity), took * 100.0);
            std::thread::sleep(Duration::from_millis(700));
        }
        let _ = WARMEST_KELVIN;
        assert_eq!(apply([1.0; 3]), 1.0);
        let blue: Vec<_> = devices.iter().map(read_blue_top).collect();
        println!("restored: blue at full white {blue:?}");
        assert!(blue.iter().all(|b| b.is_none_or(|b| b >= 65_000)));
    }
}
