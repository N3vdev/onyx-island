//! Bluetooth earbuds, headphones and speakers: their icon, and a ring that
//! shows how charged they are.

use islet_core::island::{DeviceBadge, DeviceKind};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::{ID2D1StrokeStyle, D2D1_ELLIPSE};
use windows_numerics::Vector2;

use crate::{color, FormatId, Gfx, RenderResult};

const GOOD: [u8; 3] = [64, 210, 122];
const LOW: [u8; 3] = [255, 176, 64];
const EMPTY: [u8; 3] = [255, 92, 92];

/// The ring's colour for `battery` percent.
#[must_use]
pub(crate) fn battery_color(battery: u8) -> [u8; 3] {
    match battery {
        0..=10 => EMPTY,
        11..=20 => LOW,
        _ => GOOD,
    }
}

impl Gfx {
    /// The device's icon, `size` across, centred on (`x`, `y`).
    pub(crate) unsafe fn draw_device_icon(&mut self, x: f32, y: f32, size: f32, kind: DeviceKind, ink: D2D1_COLOR_F) -> RenderResult<()> {
        let u = size / 24.0;
        match kind {
            DeviceKind::Earbuds => {
                // Two buds, each a round head with a stem down from it.
                for side in [-1.0f32, 1.0] {
                    let head_x = x + side * 4.6 * u;
                    let head_y = y - 3.0 * u;
                    self.fill_circle(head_x, head_y, 3.6 * u, ink)?;
                    let stem_x = head_x + side * 1.4 * u - 1.3 * u;
                    self.fill_round_rect(stem_x, head_y, 2.6 * u, 9.5 * u, 1.3 * u, ink)?;
                }
            }
            DeviceKind::Headphones => {
                self.set_color(ink);
                self.stroke_arc(x, y + 1.0 * u, 7.5 * u, 180.0, 360.0, 1.8 * u, ink)?;
                for side in [-1.0f32, 1.0] {
                    let cup_x = x + side * 7.5 * u - 2.2 * u;
                    self.fill_round_rect(cup_x, y + 0.5 * u, 4.4 * u, 7.0 * u, 1.8 * u, ink)?;
                }
            }
            DeviceKind::Speaker => {
                let (w, h) = (11.0 * u, 16.0 * u);
                self.set_color(ink);
                let body = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                    rect: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F {
                        left: x - w * 0.5,
                        top: y - h * 0.5,
                        right: x + w * 0.5,
                        bottom: y + h * 0.5,
                    },
                    radiusX: 2.5 * u,
                    radiusY: 2.5 * u,
                };
                self.dc.DrawRoundedRectangle(&body, &self.brush, 1.6 * u, None::<&ID2D1StrokeStyle>);
                let cone = D2D1_ELLIPSE { point: Vector2 { X: x, Y: y + 2.2 * u }, radiusX: 2.8 * u, radiusY: 2.8 * u };
                self.dc.DrawEllipse(&cone, &self.brush, 1.6 * u, None::<&ID2D1StrokeStyle>);
                self.fill_circle(x, y - 4.2 * u, 1.1 * u, ink)?;
            }
        }
        Ok(())
    }

    /// A ring filled to the battery level, with the percent inside (or the
    /// device's icon while the level is unknown).
    pub(crate) unsafe fn draw_battery_ring(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        device: DeviceBadge,
        format: FormatId,
        alpha: f32,
    ) -> RenderResult<()> {
        let width = (radius * 0.16).clamp(1.8, 3.0);
        self.stroke_circle(x, y, radius, width, color([255, 255, 255], 0.16 * alpha))?;
        match device.battery {
            Some(battery) => {
                let fraction = f32::from(battery.min(100)) / 100.0;
                self.progress_arc(x, y, radius, fraction, width, color(battery_color(battery), alpha))?;
                let label = battery.min(100).to_string();
                self.draw_text(&label, format, x - radius, y - radius, radius * 2.0, radius * 2.0, color([244, 245, 250], alpha))
                    .map(|_| ())
            }
            None => self.draw_device_icon(x, y, radius * 1.3, device.kind, color([214, 218, 230], alpha)),
        }
    }
}
