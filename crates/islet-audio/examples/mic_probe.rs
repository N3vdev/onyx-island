//! Lists input devices, opens the default microphone, captures 2 seconds, and
//! prints timings and the signal level. Nothing is saved.

use std::time::{Duration, Instant};

use islet_audio::{list_input_devices, Microphone};

fn main() {
    // Optional first argument: part of a device name to use instead of the default.
    let wanted = std::env::args().nth(1).map(|text| text.to_lowercase());
    let mut chosen: Option<String> = None;
    match list_input_devices() {
        Ok(devices) => {
            chosen = wanted.as_ref().and_then(|text| {
                devices
                    .iter()
                    .find(|device| device.name.to_lowercase().contains(text))
                    .map(|device| device.id.clone())
            });
            for device in &devices {
                println!("device: {}", device.name);
            }
            if devices.is_empty() {
                println!("no input devices");
            }
        }
        Err(error) => println!("could not list devices: {error}"),
    }

    let opened = Instant::now();
    let mic = match Microphone::open(chosen.as_deref()) {
        Ok(mic) => mic,
        Err(error) => {
            println!("open failed: {error}");
            return;
        }
    };
    println!("opened in {:?}", opened.elapsed());

    let started = Instant::now();
    mic.start();
    let first = mic.audio().recv_timeout(Duration::from_secs(2));
    println!("first chunk after {:?}", started.elapsed());

    let mut samples = first.map(|chunk| chunk.len()).unwrap_or(0);
    let mut sum_squares = 0.0f64;
    let mut peak = 0.0f32;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Ok(chunk) = mic.audio().recv_timeout(Duration::from_millis(100)) {
            samples += chunk.len();
            for sample in chunk {
                sum_squares += f64::from(sample * sample);
                peak = peak.max(sample.abs());
            }
        }
    }
    mic.stop();
    let rms = (sum_squares / samples.max(1) as f64).sqrt();
    println!("{samples} samples in ~2 s (expect ~32000), rms {rms:.4}, peak {peak:.3}");
    println!("alive after stop: {}", mic.is_alive());
}
