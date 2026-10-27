//! Readings for the system stats widget.
//!
//! Only cheap, documented sources with no admin rights: `GetSystemTimes` for
//! CPU load, `GlobalMemoryStatusEx` for memory, `GetIfTable2` byte counters
//! for network speed, and PDH counters (the ones Task Manager reads) for GPU
//! load and ACPI thermal zones. The island asks for a full reading only while
//! the widget is on screen; otherwise just the CPU, for the hot-CPU glow.

use std::collections::VecDeque;
use std::time::Instant;

use islet_core::widgets::{SystemStats, CPU_HOT, STATS_HISTORY};
use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

/// Readings above `CPU_HOT` in a row before the glow lights.
const HOT_AFTER: u8 = 3;
/// The glow goes out once load falls below this.
const COOL_BELOW: f32 = 0.8;

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// GPU and temperature counters; absent counters stay `None`.
struct Pdh {
    query: PDH_HQUERY,
    gpu: Option<PDH_HCOUNTER>,
    thermal: Option<PDH_HCOUNTER>,
}

impl Pdh {
    fn open() -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: plain PDH calls with valid out-pointers; the query is
        // closed in `Drop`.
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let add = |path: windows::core::PCWSTR| {
                let mut counter = PDH_HCOUNTER::default();
                (PdhAddEnglishCounterW(query, path, 0, &mut counter) == 0).then_some(counter)
            };
            let gpu = add(windows::core::w!(
                "\\GPU Engine(*engtype_3D)\\Utilization Percentage"
            ));
            let thermal = add(windows::core::w!(
                "\\Thermal Zone Information(*)\\Temperature"
            ));
            // Rate counters need a first sample to compare against.
            let _ = PdhCollectQueryData(query);
            Some(Self {
                query,
                gpu,
                thermal,
            })
        }
    }

    /// Every instance's value of `counter`.
    fn values(counter: PDH_HCOUNTER) -> Vec<f64> {
        let mut bytes = 0u32;
        let mut count = 0u32;
        // SAFETY: the first call sizes the buffer, the second fills it; the
        // items point into that buffer, which outlives the reads.
        unsafe {
            let status =
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None);
            if status != PDH_MORE_DATA || bytes == 0 {
                return Vec::new();
            }
            let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let mut buffer = vec![0u8; bytes as usize + item];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                Some(items),
            ) != 0
            {
                return Vec::new();
            }
            (0..count as usize)
                .map(|index| &*items.add(index))
                .filter(|item| item.FmtValue.CStatus == 0)
                .map(|item| item.FmtValue.Anonymous.doubleValue)
                .collect()
        }
    }

    /// (GPU load 0..1, hottest zone in °C).
    fn read(&self) -> (Option<f32>, Option<f32>) {
        // SAFETY: the query is open for the life of `self`.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return (None, None);
        }
        let gpu = self.gpu.map(|counter| {
            // Summed per engine across processes, as Task Manager does.
            (Self::values(counter).iter().sum::<f64>() / 100.0).clamp(0.0, 1.0) as f32
        });
        let temperature = self.thermal.and_then(|counter| {
            Self::values(counter)
                .into_iter()
                .filter(|kelvin| *kelvin > 200.0 && *kelvin < 400.0)
                .map(|kelvin| (kelvin - 273.15) as f32)
                .reduce(f32::max)
        });
        (gpu, temperature)
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        // SAFETY: closes the query opened in `open`.
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

pub struct Sampler {
    cpu_previous: Option<(u64, u64)>,
    network_previous: Option<(u64, u64, Instant)>,
    pdh: Option<Option<Pdh>>,
    history: VecDeque<f32>,
    hot_streak: u8,
    hot: bool,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            cpu_previous: None,
            network_previous: None,
            pdh: None,
            history: VecDeque::with_capacity(STATS_HISTORY),
            hot_streak: 0,
            hot: false,
        }
    }

    /// CPU load since the last call, 0..1 (`None` on the first call).
    pub fn cpu(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = Default::default();
        // SAFETY: three valid out-pointers.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        // Kernel time includes idle time.
        let (idle, total) = (ticks(idle), ticks(kernel) + ticks(user));
        let load = self.cpu_previous.and_then(|(last_idle, last_total)| {
            let spent = total.saturating_sub(last_total);
            (spent > 0).then(|| {
                let resting = idle.saturating_sub(last_idle).min(spent);
                (spent - resting) as f32 / spent as f32
            })
        });
        self.cpu_previous = Some((idle, total));
        if let Some(load) = load {
            if self.history.len() == STATS_HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(load);
            self.update_hot(load);
        }
        load
    }

    /// Whether the CPU has been pegged long enough to glow.
    pub fn hot(&self) -> bool {
        self.hot
    }

    fn update_hot(&mut self, load: f32) {
        if load >= CPU_HOT {
            self.hot_streak = self.hot_streak.saturating_add(1);
            if self.hot_streak >= HOT_AFTER {
                self.hot = true;
            }
        } else {
            self.hot_streak = 0;
            if load < COOL_BELOW {
                self.hot = false;
            }
        }
    }

    /// CPU and memory use, 0..1, for the always-on glance.
    pub fn light(&mut self) -> (f32, f32) {
        let cpu = self
            .cpu()
            .or_else(|| self.history.back().copied())
            .unwrap_or(0.0);
        let (used, total) = memory();
        let memory = if total == 0 {
            0.0
        } else {
            (used as f64 / total as f64) as f32
        };
        (cpu, memory)
    }

    /// Everything the widget shows.
    pub fn full(&mut self) -> SystemStats {
        let cpu = self
            .cpu()
            .or_else(|| self.history.back().copied())
            .unwrap_or(0.0);
        let (ram_used, ram_total) = memory();
        let (down_bps, up_bps) = self.network();
        let pdh = self.pdh.get_or_insert_with(Pdh::open);
        let (gpu, temp_c) = pdh.as_ref().map_or((None, None), Pdh::read);
        SystemStats {
            cpu,
            gpu,
            ram_used,
            ram_total,
            down_bps,
            up_bps,
            temp_c,
            cpu_history: self.history.iter().copied().collect(),
        }
    }

    /// Bytes per second down and up since the last call.
    fn network(&mut self) -> (f64, f64) {
        let Some((received, sent)) = interface_bytes() else {
            return (0.0, 0.0);
        };
        let now = Instant::now();
        let rates = self
            .network_previous
            .map_or((0.0, 0.0), |(last_in, last_out, at)| {
                let seconds = now.duration_since(at).as_secs_f64().max(0.001);
                (
                    received.saturating_sub(last_in) as f64 / seconds,
                    sent.saturating_sub(last_out) as f64 / seconds,
                )
            });
        self.network_previous = Some((received, sent, now));
        rates
    }
}

/// (used, total) physical memory in bytes.
fn memory() -> (u64, u64) {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `dwLength` is set as the API requires.
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return (0, 0);
    }
    (
        status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        status.ullTotalPhys,
    )
}

/// Total bytes received and sent by physical adapters that are up. Virtual
/// switches and VPN tunnels carry the same traffic again, so they are skipped.
fn interface_bytes() -> Option<(u64, u64)> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: the table is allocated by `GetIfTable2` and freed below; rows
    // are read only while it is alive.
    unsafe {
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts(
            (*table).Table.as_ptr(),
            (*table).NumEntries as usize,
        );
        let mut totals = (0u64, 0u64);
        for row in rows {
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.OperStatus == IfOperStatusUp {
                totals.0 += row.InOctets;
                totals.1 += row.OutOctets;
            }
        }
        FreeMibTable(table.cast());
        Some(totals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_are_sane_on_this_machine() {
        let mut sampler = Sampler::new();
        assert_eq!(sampler.cpu(), None);
        std::thread::sleep(std::time::Duration::from_millis(120));
        let stats = sampler.full();
        assert!((0.0..=1.0).contains(&stats.cpu));
        assert!(stats.ram_total > 0 && stats.ram_used <= stats.ram_total);
        assert!(stats.down_bps >= 0.0 && stats.up_bps >= 0.0);
        if let Some(gpu) = stats.gpu {
            assert!((0.0..=1.0).contains(&gpu));
        }
        assert_eq!(stats.cpu_history.len(), 1);
    }

    #[test]
    fn the_glow_needs_a_sustained_peak_and_a_real_cool_down() {
        let mut sampler = Sampler::new();
        for load in [0.95, 0.97] {
            sampler.update_hot(load);
        }
        assert!(!sampler.hot());
        sampler.update_hot(0.99);
        assert!(sampler.hot());
        sampler.update_hot(0.85);
        assert!(sampler.hot(), "a small dip keeps it lit");
        sampler.update_hot(0.5);
        assert!(!sampler.hot());
    }
}
