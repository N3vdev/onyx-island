//! Microphone capture as 16 kHz mono `f32`, the format speech models want.
//!
//! WASAPI is asked for 16 kHz mono 16-bit PCM directly with
//! `AUTOCONVERTPCM`, so Windows' own sample-rate converter does the work and
//! no resampler crate is needed. The device is opened *warm* (initialised
//! but stopped): pressing the hotkey only calls `Start()`, and Windows shows
//! its "microphone in use" indicator only while a dictation is recording.
//!
//! Audio arrives on a channel as `Vec<f32>` chunks (~10 ms each). The capture
//! thread sleeps on events when idle and never logs per chunk.

#![allow(unsafe_op_in_unsafe_fn)]

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use windows::core::PCWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE};

pub const SAMPLE_RATE: u32 = 16_000;
/// WASAPI buffer length (100 ns units): 200 ms leaves room for scheduling
/// hiccups without dropping audio.
const BUFFER_HNS: i64 = 2_000_000;
const WAVE_FORMAT_PCM: u16 = 1;
/// How long `open` waits for the device to initialise.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Chunks buffered between the capture thread and the consumer.
const CHANNEL_CHUNKS: usize = 2048;

#[derive(Debug)]
pub enum MicError {
    /// No capture device, or the requested one is unplugged.
    NoDevice,
    /// Windows' microphone privacy setting blocks desktop apps.
    AccessDenied,
    Other(String),
}

impl fmt::Display for MicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "no microphone found"),
            Self::AccessDenied => write!(
                f,
                "microphone access is blocked (Settings > Privacy > Microphone)"
            ),
            Self::Other(message) => write!(f, "microphone error: {message}"),
        }
    }
}

impl std::error::Error for MicError {}

impl From<windows::core::Error> for MicError {
    fn from(error: windows::core::Error) -> Self {
        // E_ACCESSDENIED, and the "no devices" / "device invalidated" codes.
        match error.code().0 as u32 {
            0x8007_0005 => Self::AccessDenied,
            0x8889_0004 | 0x8889_0008 | 0x8007_0490 => Self::NoDevice,
            _ => Self::Other(error.message()),
        }
    }
}

/// An input device for the settings picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    /// Stable endpoint id to store in the config.
    pub id: String,
    pub name: String,
}

struct Event(HANDLE);

// SAFETY: a Win32 event handle may be signalled and waited on from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: an unnamed auto-reset event.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map(Self)
    }

    fn set(&self) {
        // SAFETY: the handle is valid until `Drop`.
        unsafe {
            let _ = SetEvent(self.0);
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct Shared {
    capturing: AtomicBool,
    stop: AtomicBool,
    wake: Event,
}

/// A warm microphone. Dropping it releases the device.
pub struct Microphone {
    shared: Arc<Shared>,
    audio: Receiver<Vec<f32>>,
    thread: Option<JoinHandle<()>>,
}

impl Microphone {
    /// Opens `device_id` (or the default input when `None` or empty) and
    /// blocks until it is initialised. Nothing is recorded until
    /// [`Microphone::start`].
    pub fn open(device_id: Option<&str>) -> Result<Self, MicError> {
        let shared = Arc::new(Shared {
            capturing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            wake: Event::new()?,
        });
        let (audio_tx, audio) = bounded(CHANNEL_CHUNKS);
        let (ready_tx, ready_rx) = bounded::<Result<(), MicError>>(1);

        let thread_shared = Arc::clone(&shared);
        let device_id = device_id.filter(|id| !id.is_empty()).map(str::to_owned);
        let thread = std::thread::Builder::new()
            .name("islet-mic".to_owned())
            .spawn(move || run(&thread_shared, device_id.as_deref(), &audio_tx, &ready_tx))
            .map_err(|error| MicError::Other(error.to_string()))?;

        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                shared,
                audio,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                shared.stop.store(true, Ordering::Release);
                shared.wake.set();
                Err(MicError::Other("the microphone did not respond".to_owned()))
            }
        }
    }

    /// 16 kHz mono chunks while capturing.
    #[must_use]
    pub fn audio(&self) -> &Receiver<Vec<f32>> {
        &self.audio
    }

    /// Begins delivering audio. Anything left over from an earlier recording
    /// is discarded first.
    pub fn start(&self) {
        while self.audio.try_recv().is_ok() {}
        self.shared.capturing.store(true, Ordering::Release);
        self.shared.wake.set();
    }

    /// Stops delivering audio (the device stays open and warm).
    pub fn stop(&self) {
        self.shared.capturing.store(false, Ordering::Release);
        self.shared.wake.set();
    }

    /// Whether the capture thread is still running (it ends if the device
    /// disappears).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.set();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Lists active input devices.
pub fn list_input_devices() -> Result<Vec<InputDevice>, MicError> {
    // SAFETY: COM is initialised for the duration of the call and every
    // interface is released before CoUninitialize.
    unsafe {
        let initialised = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = list_devices_inner();
        if initialised {
            CoUninitialize();
        }
        result
    }
}

unsafe fn list_devices_inner() -> Result<Vec<InputDevice>, MicError> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
    let count = collection.GetCount()?;
    let mut devices = Vec::with_capacity(count as usize);
    for index in 0..count {
        let device = collection.Item(index)?;
        let id = device.GetId()?;
        let id_text = id.to_string().unwrap_or_default();
        CoTaskMemFree(Some(id.0.cast()));
        let name = friendly_name(&device).unwrap_or_else(|| id_text.clone());
        devices.push(InputDevice { id: id_text, name });
    }
    Ok(devices)
}

unsafe fn friendly_name(device: &IMMDevice) -> Option<String> {
    let store = device.OpenPropertyStore(STGM_READ).ok()?;
    let value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    let text = value.to_string();
    (!text.is_empty()).then_some(text)
}

fn run(
    shared: &Shared,
    device_id: Option<&str>,
    audio: &Sender<Vec<f32>>,
    ready: &Sender<Result<(), MicError>>,
) {
    // SAFETY: balanced with CoUninitialize at the end of this function.
    if let Err(error) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        let _ = ready.send(Err(error.into()));
        return;
    }
    // SAFETY: all COM objects live inside `capture` and are dropped before
    // CoUninitialize below.
    unsafe {
        match open_device(device_id) {
            Ok(opened) => {
                let _ = ready.send(Ok(()));
                capture_loop(shared, &opened, audio);
            }
            Err(error) => {
                let _ = ready.send(Err(error));
            }
        }
        CoUninitialize();
    }
}

struct Opened {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    data_ready: Event,
}

unsafe fn open_device(device_id: Option<&str>) -> Result<Opened, MicError> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = match device_id {
        Some(id) => {
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            enumerator
                .GetDevice(PCWSTR(wide.as_ptr()))
                .map_err(|_| MicError::NoDevice)?
        }
        None => enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .map_err(|_| MicError::NoDevice)?,
    };
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        BUFFER_HNS,
        0,
        &format,
        None,
    )?;
    let data_ready = Event::new()?;
    client.SetEventHandle(data_ready.0)?;
    let capture: IAudioCaptureClient = client.GetService()?;
    Ok(Opened {
        client,
        capture,
        data_ready,
    })
}

unsafe fn capture_loop(shared: &Shared, opened: &Opened, audio: &Sender<Vec<f32>>) {
    let handles = [shared.wake.0, opened.data_ready.0];
    let mut running = false;
    while !shared.stop.load(Ordering::Acquire) {
        let wanted = shared.capturing.load(Ordering::Acquire);
        if wanted && !running {
            if opened.client.Reset().is_err() || opened.client.Start().is_err() {
                return;
            }
            running = true;
        } else if !wanted && running {
            let _ = opened.client.Stop();
            let _ = opened.client.Reset();
            running = false;
        }

        // Idle: sleep until told to start. Recording: wake for each packet
        // (and at least every 200 ms to notice a vanished device).
        let timeout = if running { 200 } else { INFINITE };
        let waited = WaitForMultipleObjects(&handles, false, timeout);
        let packet_ready = waited.0 == WAIT_OBJECT_0.0 + 1 || waited == WAIT_TIMEOUT;
        if packet_ready && running && drain(&opened.capture, audio).is_err() {
            return; // The device went away; `is_alive` turns false.
        }
    }
    if running {
        let _ = opened.client.Stop();
    }
}

/// Moves every ready packet to the channel.
unsafe fn drain(
    capture: &IAudioCaptureClient,
    audio: &Sender<Vec<f32>>,
) -> windows::core::Result<()> {
    loop {
        if capture.GetNextPacketSize()? == 0 {
            return Ok(());
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
        let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
        let chunk = if silent || data.is_null() {
            vec![0.0f32; frames as usize]
        } else {
            std::slice::from_raw_parts(data.cast::<i16>(), frames as usize)
                .iter()
                .map(|&sample| f32::from(sample) / 32_768.0)
                .collect()
        };
        capture.ReleaseBuffer(frames)?;
        // A full channel means the consumer stalled; dropping audio beats
        // blocking the capture thread.
        let _ = audio.try_send(chunk);
    }
}

// ----- testing hook ---------------------------------------------------------

/// Reads a 16 kHz mono 16-bit PCM WAV (the format of the test fixtures).
fn read_wav(path: &std::path::Path) -> Result<Vec<f32>, MicError> {
    let bytes = std::fs::read(path).map_err(|error| MicError::Other(error.to_string()))?;
    let bad = || MicError::Other("the fake microphone needs a 16 kHz mono 16-bit WAV".to_owned());
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad());
    }
    // Walk the chunks to find `fmt ` and `data`.
    let (mut format_ok, mut data) = (false, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let body = at + 8..(at + 8 + len).min(bytes.len());
        if id == b"fmt " && body.len() >= 16 {
            let f = &bytes[body.clone()];
            let tag = u16::from_le_bytes([f[0], f[1]]);
            let channels = u16::from_le_bytes([f[2], f[3]]);
            let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            let bits = u16::from_le_bytes([f[14], f[15]]);
            format_ok = tag == 1 && channels == 1 && rate == SAMPLE_RATE && bits == 16;
        } else if id == b"data" {
            data = Some(body);
        }
        at += 8 + len + (len & 1);
    }
    let data = data.filter(|_| format_ok).ok_or_else(bad)?;
    Ok(bytes[data]
        .chunks_exact(2)
        .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
        .collect())
}

impl Microphone {
    /// A stand-in microphone that plays `path` in real time on each
    /// [`Microphone::start`], then silence. For automated testing only; the
    /// dictation controller uses it when `ISLET_FAKE_MIC` is set.
    pub fn from_wav(path: &std::path::Path) -> Result<Self, MicError> {
        let samples = read_wav(path)?;
        let shared = Arc::new(Shared {
            capturing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            wake: Event::new()?,
        });
        let (audio_tx, audio) = bounded(CHANNEL_CHUNKS);
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("islet-fake-mic".to_owned())
            .spawn(move || {
                const CHUNK: usize = 160; // 10 ms
                let mut position = 0usize;
                let mut was_capturing = false;
                while !thread_shared.stop.load(Ordering::Acquire) {
                    let capturing = thread_shared.capturing.load(Ordering::Acquire);
                    if capturing && !was_capturing {
                        position = 0;
                    }
                    was_capturing = capturing;
                    if capturing {
                        let end = (position + CHUNK).min(samples.len());
                        let mut chunk = samples[position.min(end)..end].to_vec();
                        chunk.resize(CHUNK, 0.0);
                        position = end;
                        let _ = audio_tx.try_send(chunk);
                        std::thread::sleep(Duration::from_millis(10));
                    } else {
                        // SAFETY: waits on our own event; woken by start/stop/drop.
                        unsafe {
                            WaitForMultipleObjects(&[thread_shared.wake.0], false, 100);
                        }
                    }
                }
            })
            .map_err(|error| MicError::Other(error.to_string()))?;
        Ok(Self {
            shared,
            audio,
            thread: Some(thread),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_helpful_messages() {
        let denied: MicError =
            windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_0005u32 as i32))
                .into();
        assert!(matches!(denied, MicError::AccessDenied));
        assert!(denied.to_string().contains("Privacy"));
        let none: MicError =
            windows::core::Error::from_hresult(windows::core::HRESULT(0x8889_0004u32 as i32))
                .into();
        assert!(matches!(none, MicError::NoDevice));
    }

    fn wav_bytes(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn fake_mic_reads_16k_mono_wavs_and_rejects_others() {
        let dir = std::env::temp_dir().join(format!("islet-mic-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.wav");
        std::fs::write(&good, wav_bytes(16_000, &[0, 16_384, -16_384])).unwrap();
        let samples = read_wav(&good).unwrap();
        assert_eq!(samples, [0.0, 0.5, -0.5]);

        let wrong_rate = dir.join("wrong.wav");
        std::fs::write(&wrong_rate, wav_bytes(44_100, &[0, 1])).unwrap();
        assert!(read_wav(&wrong_rate).is_err());
        assert!(read_wav(&dir.join("missing.wav")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
