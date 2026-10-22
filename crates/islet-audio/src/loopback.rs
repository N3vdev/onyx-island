//! WASAPI loopback on the default render device, reduced to five bar levels.
//!
//! The thread wakes on a high-resolution waitable timer at 30 Hz, drains
//! whatever the device produced, and writes [`BarLevels`]. The loop does not
//! allocate, lock, or log. It only runs while the island asks for it (media
//! playing and visible), so its cost is zero otherwise.

#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use islet_core::media::{BarLevels, Spectrum, SPECTRUM_SIZE};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
};

const TICK_MS: i32 = 33;
const BUFFER_HNS: i64 = 1_000_000; // 100 ms
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);

/// A running loopback visualizer. Dropping it stops and joins the thread.
pub struct Loopback {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Loopback {
    /// Starts capturing. `now_ms` must be the clock the renderer reads
    /// [`BarLevels`] with.
    pub fn start(levels: Arc<BarLevels>, now_ms: fn() -> u64) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("islet-loopback".to_owned())
            .spawn(move || run(&levels, now_ms, &thread_stop))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy)]
enum SampleFormat {
    F32,
    I16,
}

struct Timer(HANDLE);

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateWaitableTimerExW and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn run(levels: &BarLevels, now_ms: fn() -> u64, stop: &AtomicBool) {
    // SAFETY: balanced with CoUninitialize at the end of this thread.
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        tracing::warn!("loopback thread could not initialise COM");
        return;
    }
    // SAFETY: a fresh unnamed timer with default security.
    let timer = match unsafe {
        CreateWaitableTimerExW(
            None,
            PCWSTR::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS.0,
        )
    } {
        Ok(handle) => Timer(handle),
        Err(error) => {
            tracing::warn!(%error, "loopback timer unavailable");
            // SAFETY: matches CoInitializeEx above.
            unsafe { CoUninitialize() };
            return;
        }
    };
    let due = -i64::from(TICK_MS) * 10_000;
    // SAFETY: periodic relative timer on our own handle; no APC routine.
    if let Err(error) = unsafe { SetWaitableTimer(timer.0, &due, TICK_MS, None, None, false) } {
        tracing::warn!(%error, "loopback timer could not be armed");
    }

    let mut ring = [0.0f32; SPECTRUM_SIZE];
    let mut ordered = [0.0f32; SPECTRUM_SIZE];
    let mut write = 0usize;
    while !stop.load(Ordering::Acquire) {
        // SAFETY: COM is initialised on this thread; all objects stay local.
        match unsafe {
            capture_session(
                levels,
                now_ms,
                stop,
                &timer,
                &mut ring,
                &mut ordered,
                &mut write,
            )
        } {
            Ok(()) => break,
            Err(error) => {
                tracing::debug!(%error, "loopback capture interrupted; reopening");
                levels.store([0.0; 5], now_ms());
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    drop(timer);
    // SAFETY: matches CoInitializeEx above.
    unsafe { CoUninitialize() };
}

/// Runs until `stop` is set (Ok) or the device fails (Err, caller reopens).
#[allow(clippy::too_many_arguments)]
unsafe fn capture_session(
    levels: &BarLevels,
    now_ms: fn() -> u64,
    stop: &AtomicBool,
    timer: &Timer,
    ring: &mut [f32; SPECTRUM_SIZE],
    ordered: &mut [f32; SPECTRUM_SIZE],
    write: &mut usize,
) -> windows::core::Result<()> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    let format_ptr = client.GetMixFormat()?;
    let format = MixFormat::read(format_ptr);
    let init = client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
        BUFFER_HNS,
        0,
        format_ptr,
        None,
    );
    CoTaskMemFree(Some(format_ptr.cast()));
    init?;
    let Some(sample_format) = format.sample_format else {
        tracing::warn!(
            tag = format.tag,
            bits = format.bits,
            "unsupported loopback mix format; using procedural bars"
        );
        // Not a device failure: wait for stop without retry churn.
        while !stop.load(Ordering::Acquire) {
            WaitForSingleObject(timer.0, 200);
        }
        return Ok(());
    };
    let capture: IAudioCaptureClient = client.GetService()?;
    let mut spectrum = Spectrum::new(format.sample_rate);
    client.Start()?;

    let channels = usize::from(format.channels.max(1));
    let result = (|| {
        while !stop.load(Ordering::Acquire) {
            if WaitForSingleObject(timer.0, 200) != WAIT_OBJECT_0 {
                continue;
            }
            let mut received = false;
            loop {
                let packet = capture.GetNextPacketSize()?;
                if packet == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                for frame in 0..frames as usize {
                    let sample = if silent || data.is_null() {
                        0.0
                    } else {
                        mono_sample(data, frame, channels, sample_format)
                    };
                    ring[*write] = sample;
                    *write = (*write + 1) % SPECTRUM_SIZE;
                }
                capture.ReleaseBuffer(frames)?;
                received = true;
            }
            if !received {
                // Nothing rendered this tick: let the bars fall.
                ring.fill(0.0);
            }
            let (tail, head) = ring.split_at(*write);
            ordered[..head.len()].copy_from_slice(head);
            ordered[head.len()..].copy_from_slice(tail);
            levels.store(spectrum.process(ordered), now_ms());
        }
        Ok(())
    })();
    let _ = client.Stop();
    result
}

/// Averages one interleaved frame down to mono.
unsafe fn mono_sample(data: *const u8, frame: usize, channels: usize, format: SampleFormat) -> f32 {
    let mut sum = 0.0f32;
    match format {
        SampleFormat::F32 => {
            let base = data.cast::<f32>().add(frame * channels);
            for channel in 0..channels {
                sum += base.add(channel).read_unaligned();
            }
        }
        SampleFormat::I16 => {
            let base = data.cast::<i16>().add(frame * channels);
            for channel in 0..channels {
                sum += f32::from(base.add(channel).read_unaligned()) / 32_768.0;
            }
        }
    }
    sum / channels as f32
}

struct MixFormat {
    tag: u16,
    bits: u16,
    channels: u16,
    sample_rate: u32,
    sample_format: Option<SampleFormat>,
}

impl MixFormat {
    unsafe fn read(format: *const WAVEFORMATEX) -> Self {
        let base = format.read_unaligned();
        let tag = base.wFormatTag;
        let subformat = (tag == WAVE_FORMAT_EXTENSIBLE && base.cbSize >= 22).then(|| {
            format
                .cast::<WAVEFORMATEXTENSIBLE>()
                .read_unaligned()
                .SubFormat
        });
        let is_float = tag == WAVE_FORMAT_IEEE_FLOAT || subformat == Some(SUBTYPE_IEEE_FLOAT);
        let is_pcm = tag == WAVE_FORMAT_PCM || subformat == Some(SUBTYPE_PCM);
        let sample_format = match (is_float, is_pcm, base.wBitsPerSample) {
            (true, _, 32) => Some(SampleFormat::F32),
            (_, true, 16) => Some(SampleFormat::I16),
            _ => None,
        };
        Self {
            tag,
            bits: base.wBitsPerSample,
            channels: base.nChannels,
            sample_rate: base.nSamplesPerSec,
            sample_format,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_mix_averages_channels() {
        let stereo: [f32; 4] = [0.5, -0.5, 1.0, 0.0];
        // SAFETY: indexes stay inside the 2-frame stereo buffer.
        unsafe {
            let data = stereo.as_ptr().cast::<u8>();
            assert_eq!(mono_sample(data, 0, 2, SampleFormat::F32), 0.0);
            assert_eq!(mono_sample(data, 1, 2, SampleFormat::F32), 0.5);
        }
        let pcm: [i16; 2] = [16_384, 16_384];
        // SAFETY: one stereo frame.
        unsafe {
            let value = mono_sample(pcm.as_ptr().cast(), 0, 2, SampleFormat::I16);
            assert!((value - 0.5).abs() < 1e-4);
        }
    }
}
