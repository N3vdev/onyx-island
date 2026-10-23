//! Master output volume, read and written on a small worker thread.
//!
//! The island's slider talks to this handle; COM, device enumeration and the
//! endpoint itself live on the worker so the UI thread never blocks on audio.
//! While the island is idle the worker sleeps on its channel.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

enum Message {
    Set(f32),
    Refresh,
    Stop,
}

struct Shared {
    /// `f32::to_bits`; only meaningful while `valid` is true.
    value: AtomicU32,
    valid: AtomicBool,
}

/// Handle to the system output volume.
pub struct VolumeHandle {
    shared: Arc<Shared>,
    tx: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl VolumeHandle {
    pub fn start() -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            value: AtomicU32::new(0.0f32.to_bits()),
            valid: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel();
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("islet-volume".to_owned())
            .spawn(move || run(worker, rx))?;
        Ok(Self {
            shared,
            tx,
            thread: Some(thread),
        })
    }

    /// Latest volume, 0..=1, or `None` while no output device is available.
    pub fn read(&self) -> Option<f32> {
        self.shared
            .valid
            .load(Ordering::Acquire)
            .then(|| f32::from_bits(self.shared.value.load(Ordering::Relaxed)).clamp(0.0, 1.0))
    }

    /// Writes the system volume. The next [`read`](Self::read) returns the
    /// value the device accepted.
    pub fn set(&self, value: f32) {
        let _ = self.tx.send(Message::Set(value.clamp(0.0, 1.0)));
    }

    /// Asks the worker to re-read the system volume. The fresh value appears
    /// on a later [`read`](Self::read); this never blocks the caller.
    pub fn refresh(&self) {
        let _ = self.tx.send(Message::Refresh);
    }
}

impl Drop for VolumeHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: Arc<Shared>, rx: Receiver<Message>) {
    // SAFETY: this thread owns its COM apartment for its whole life.
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        tracing::warn!("volume thread could not initialise COM");
        // Stay responsive to shutdown; there is simply nothing to publish.
        while let Ok(message) = rx.recv() {
            if matches!(message, Message::Stop) {
                break;
            }
        }
        return;
    }

    let mut endpoint = open_endpoint();
    publish(&shared, &endpoint);
    while let Ok(message) = rx.recv() {
        match message {
            Message::Stop => break,
            Message::Refresh => {
                if endpoint.is_none() {
                    endpoint = open_endpoint();
                }
                publish(&shared, &endpoint);
            }
            Message::Set(value) => {
                let wrote = endpoint
                    .as_ref()
                    .is_some_and(|device| unsafe { write(device, value) });
                if !wrote {
                    // The device may have been unplugged since the last read.
                    endpoint = open_endpoint();
                    if let Some(device) = &endpoint {
                        let _ = unsafe { write(device, value) };
                    }
                }
                publish(&shared, &endpoint);
            }
        }
    }
    // SAFETY: matches CoInitializeEx above.
    unsafe { CoUninitialize() };
}

/// Writes one scalar level. The null event context means "no caller origin".
///
/// # Safety
/// `device` must belong to a COM apartment on the calling thread.
unsafe fn write(device: &IAudioEndpointVolume, value: f32) -> bool {
    device
        .SetMasterVolumeLevelScalar(value.clamp(0.0, 1.0), std::ptr::null())
        .is_ok()
}

fn open_endpoint() -> Option<IAudioEndpointVolume> {
    // SAFETY: COM is initialised on this thread.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let interface: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        Some(interface)
    }
}

fn publish(shared: &Shared, endpoint: &Option<IAudioEndpointVolume>) {
    let Some(device) = endpoint else {
        shared.valid.store(false, Ordering::Release);
        return;
    };
    // SAFETY: the interface lives on this thread's apartment.
    match unsafe { device.GetMasterVolumeLevelScalar() } {
        Ok(value) => {
            shared
                .value
                .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            shared.valid.store(true, Ordering::Release);
        }
        Err(_) => shared.valid.store(false, Ordering::Release),
    }
}
