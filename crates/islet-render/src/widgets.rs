//! Native calculator drawing; static until input changes its snapshot.
use super::*;
use islet_core::config::IdleGlance;
use islet_core::island::GlanceCard;
use islet_core::widgets::{self, Calculator, CalculatorKey, SystemStats};

const STATS_TEXT: [u8; 3] = [240, 241, 244];
const STATS_MUTED: [u8; 3] = [150, 155, 166];
const STATS_TRACK: [u8; 3] = [255, 255, 255];

/// Load colour: calm blue, then amber, then hot orange-red near the top.
fn load_color(load: f32) -> [u8; 3] {
    if load >= widgets::CPU_HOT {
        [255, 118, 58]
    } else if load >= 0.7 {
        [255, 190, 70]
    } else {
        [110, 190, 255]
    }
}

impl Gfx {
    pub(super) unsafe fn draw_calculator(
        &mut self,
        _width: f32,
        height: f32,
        calc: &Calculator,
        alpha: f32,
    ) -> RenderResult<()> {
        if height < 60.0 {
            return Ok(());
        }
        self.draw_text(
            "Calculator",
            FormatId::MediaTitle,
            24.0,
            17.0,
            130.0,
            20.0,
            color([240, 241, 244], alpha),
        )?;
        self.draw_text(
            calc.expression(),
            FormatId::Sub,
            24.0,
            56.0,
            200.0,
            20.0,
            color([166, 170, 178], alpha),
        )?;
        if calc.is_error() {
            self.draw_text(
                calc.display(),
                FormatId::Sub,
                24.0,
                83.0,
                200.0,
                36.0,
                color([255, 150, 150], alpha),
            )?;
        } else {
            // Follow the active input's tail in a bounded, cached layout.
            // Clipping replaces end trimming, which would hide new digits.
            let long_entry = !calc.is_result() && calc.display().len() > 13;
            if long_entry {
                self.draw_text(
                    "…",
                    FormatId::Sub,
                    24.0,
                    83.0,
                    12.0,
                    36.0,
                    color([166, 170, 178], alpha),
                )?;
            }
            self.dc.PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: if long_entry { 36.0 } else { 24.0 },
                    top: 83.0,
                    right: 224.0,
                    bottom: 119.0,
                },
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
            let drawn = self.draw_text(
                calc.display(),
                if calc.is_result() && calc.display().len() > 12 {
                    FormatId::CalculatorSmallResult
                } else {
                    FormatId::CalculatorResult
                },
                224.0 - 4096.0,
                83.0,
                4096.0,
                36.0,
                color([245, 246, 248], alpha),
            );
            self.dc.PopAxisAlignedClip();
            drawn?;
        }
        if !self.widget_navigation {
            self.draw_text(
                "Ready when you are",
                FormatId::CompactSub,
                24.0,
                137.0,
                200.0,
                16.0,
                color([166, 170, 178], alpha),
            )?;
        }
        for (index, key) in widgets::KEYS.iter().copied().enumerate() {
            let r = widgets::key_rect(index);
            let hover = self.widget_hover == Some(widgets::WidgetHover::Key(key));
            let operator = matches!(
                key,
                CalculatorKey::Add
                    | CalculatorKey::Subtract
                    | CalculatorKey::Multiply
                    | CalculatorKey::Divide
            );
            let equals = key == CalculatorKey::Equals;
            self.fill_round_rect(
                r.x,
                r.y,
                r.width,
                r.height,
                8.0,
                color(
                    if equals || operator {
                        [164, 204, 180]
                    } else {
                        [255, 255, 255]
                    },
                    alpha
                        * if equals {
                            if hover {
                                0.8
                            } else {
                                1.0
                            }
                        } else if hover {
                            0.2
                        } else if operator {
                            0.13
                        } else {
                            0.085
                        },
                ),
            )?;
            self.draw_text(
                key.label(),
                FormatId::CalculatorKey,
                r.x,
                r.y,
                r.width,
                r.height,
                color(
                    if equals {
                        [16, 20, 18]
                    } else {
                        [236, 237, 242]
                    },
                    alpha,
                ),
            )?;
        }
        Ok(())
    }

    pub(super) unsafe fn draw_widget_navigation(
        &mut self,
        width: f32,
        height: f32,
        alpha: f32,
    ) -> RenderResult<()> {
        let pages = self.widget_pages.max(2);
        let x = width - 8.0;
        let top = height * 0.5 - f32::from(pages - 1) * 6.0;
        for page in 0..pages {
            self.fill_circle(
                x,
                top + f32::from(page) * 12.0,
                2.0,
                color(
                    [236, 237, 242],
                    alpha * if page == self.widget_page { 0.9 } else { 0.25 },
                ),
            )?;
        }
        Ok(())
    }

    /// The always-on glance: the time, or CPU and memory. Opened, the clock
    /// adds the date and the stats become two tiles.
    pub(super) unsafe fn draw_glance(
        &mut self,
        width: f32,
        height: f32,
        glance: &GlanceCard,
        alpha: f32,
    ) -> RenderResult<()> {
        let expanded = height > 60.0;
        match (glance.kind, expanded) {
            (IdleGlance::Stats, false) => {
                let half = width * 0.5;
                for (index, (label, value)) in
                    [("CPU", glance.cpu), ("RAM", glance.memory)].into_iter().enumerate()
                {
                    let x = 18.0 + index as f32 * (half - 8.0);
                    self.draw_text(
                        label,
                        FormatId::CompactSub,
                        x,
                        10.5,
                        30.0,
                        13.0,
                        color(STATS_MUTED, alpha),
                    )?;
                    self.draw_text(
                        &widgets::format_percent(value),
                        FormatId::CompactTitle,
                        x + 26.0,
                        8.0,
                        44.0,
                        16.0,
                        color(load_color(value), alpha),
                    )?;
                }
            }
            (IdleGlance::Stats, true) => {
                self.draw_stat_tile(
                    28.0,
                    12.0,
                    "CPU",
                    &widgets::format_percent(glance.cpu),
                    Some(glance.cpu),
                    alpha,
                )?;
                self.draw_stat_tile(
                    160.0,
                    12.0,
                    "Memory",
                    &widgets::format_percent(glance.memory),
                    Some(glance.memory),
                    alpha,
                )?;
            }
            (_, false) => {
                self.draw_text(
                    &glance.time,
                    FormatId::Bubble,
                    0.0,
                    0.0,
                    width,
                    height,
                    color(STATS_TEXT, alpha),
                )?;
            }
            (_, true) => {
                self.draw_text(
                    &glance.time,
                    FormatId::StatsValue,
                    28.0,
                    6.0,
                    width - 56.0,
                    40.0,
                    color(STATS_TEXT, alpha),
                )?;
                self.draw_text(
                    &glance.long_date,
                    FormatId::Sub,
                    28.0,
                    46.0,
                    width - 56.0,
                    18.0,
                    color(STATS_MUTED, alpha),
                )?;
            }
        }
        Ok(())
    }

    /// The system stats card: CPU large with its recent history on the
    /// left, then GPU, memory, network, and temperature tiles.
    pub(super) unsafe fn draw_stats(
        &mut self,
        _width: f32,
        height: f32,
        stats: &SystemStats,
        alpha: f32,
    ) -> RenderResult<()> {
        if height < 60.0 {
            return Ok(());
        }
        let left = 28.0;
        self.draw_text(
            "CPU",
            FormatId::CompactSub,
            left,
            20.0,
            80.0,
            14.0,
            color(STATS_MUTED, alpha),
        )?;
        self.draw_text(
            &widgets::format_percent(stats.cpu),
            FormatId::StatsValue,
            left,
            34.0,
            150.0,
            40.0,
            color(load_color(stats.cpu), alpha),
        )?;
        self.draw_sparkline(left, 84.0, 160.0, 46.0, &stats.cpu_history, alpha)?;

        let tile = |column: f32, row: f32| (212.0 + column * 132.0, 18.0 + row * 60.0);
        let gpu = stats.gpu.map_or_else(|| "\u{2014}".to_owned(), widgets::format_percent);
        let (x, y) = tile(0.0, 0.0);
        self.draw_stat_tile(x, y, "GPU", &gpu, stats.gpu, alpha)?;
        let (x, y) = tile(1.0, 0.0);
        self.draw_stat_tile(
            x,
            y,
            "Memory",
            &widgets::format_memory(stats.ram_used, stats.ram_total),
            Some(stats.ram_fraction()),
            alpha,
        )?;
        let (x, y) = tile(0.0, 1.0);
        let network = format!(
            "\u{2193} {}",
            widgets::format_rate(stats.down_bps)
        );
        self.draw_stat_tile(x, y, "Network", &network, None, alpha)?;
        self.draw_text(
            &format!("\u{2191} {}", widgets::format_rate(stats.up_bps)),
            FormatId::CompactSub,
            x + 12.0,
            y + 37.0,
            100.0,
            14.0,
            color(STATS_MUTED, alpha),
        )?;
        let (x, y) = tile(1.0, 1.0);
        let temperature = stats
            .temp_c
            .map_or_else(|| "\u{2014}".to_owned(), |c| format!("{c:.0}\u{b0}C"));
        // Shown as a bar from 30 to 100 °C.
        let heat = stats.temp_c.map(|c| ((c - 30.0) / 70.0).clamp(0.0, 1.0));
        self.draw_stat_tile(x, y, "Temperature", &temperature, heat, alpha)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_stat_tile(
        &mut self,
        x: f32,
        y: f32,
        label: &str,
        value: &str,
        fill: Option<f32>,
        alpha: f32,
    ) -> RenderResult<()> {
        let (width, height) = (122.0, 52.0);
        self.fill_round_rect(x, y, width, height, 12.0, color(STATS_TRACK, 0.06 * alpha))?;
        self.draw_text(
            label,
            FormatId::CompactSub,
            x + 12.0,
            y + 8.0,
            width - 24.0,
            14.0,
            color(STATS_MUTED, alpha),
        )?;
        self.draw_text(
            value,
            FormatId::MediaTitle,
            x + 12.0,
            y + 21.0,
            width - 20.0,
            18.0,
            color(STATS_TEXT, alpha),
        )?;
        if let Some(fill) = fill {
            let track = width - 24.0;
            self.fill_round_rect(x + 12.0, y + 42.0, track, 3.0, 1.5, color(STATS_TRACK, 0.1 * alpha))?;
            self.fill_round_rect(
                x + 12.0,
                y + 42.0,
                track * fill.clamp(0.0, 1.0),
                3.0,
                1.5,
                color(load_color(fill), alpha),
            )?;
        }
        Ok(())
    }

    /// Recent CPU load as a filled area, newest on the right.
    unsafe fn draw_sparkline(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        history: &[f32],
        alpha: f32,
    ) -> RenderResult<()> {
        self.fill_round_rect(x, y + height - 1.0, width, 1.0, 0.5, color(STATS_TRACK, 0.1 * alpha))?;
        if history.len() < 2 {
            return Ok(());
        }
        let step = width / (widgets::STATS_HISTORY - 1) as f32;
        let start = x + width - step * (history.len() - 1) as f32;
        let point = |index: usize, load: f32| Vector2 {
            X: start + step * index as f32,
            Y: y + height - 1.0 - (height - 2.0) * load.clamp(0.0, 1.0),
        };
        let mut area: Vec<Vector2> = history
            .iter()
            .enumerate()
            .map(|(index, &load)| point(index, load))
            .collect();
        let latest = history.last().copied().unwrap_or(0.0);
        let tint = load_color(latest);
        area.push(Vector2 {
            X: x + width,
            Y: y + height,
        });
        area.push(Vector2 {
            X: start,
            Y: y + height,
        });
        self.fill_polygon(&area, color(tint, 0.18 * alpha))?;
        self.set_color(color(tint, 0.9 * alpha));
        for pair in history.windows(2).enumerate() {
            let (index, loads) = pair;
            let (a, b) = (point(index, loads[0]), point(index + 1, loads[1]));
            self.dc.DrawLine(a, b, &self.brush, 1.5, None::<&ID2D1StrokeStyle>);
        }
        Ok(())
    }
}
