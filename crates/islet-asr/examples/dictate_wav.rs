//! Streams a 16 kHz mono WAV through the worker the way the microphone will:
//! 20 ms chunks between `Begin` and `End`. Prints segments as they close and
//! how long the final reply takes after `End` (the user-visible latency).
//!
//!   cargo run --release -p islet-asr --example dictate_wav -- clip.wav [--model id] [--threads 4] [--realtime]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use islet_asr::{find_or_default, AsrClient, AsrEvent, LoadRequest, ModelManager, ModelStatus};

fn main() {
    let mut wav = None;
    let mut model_id = String::from(islet_asr::DEFAULT_MODEL_ID);
    let mut threads = 4u32;
    let mut realtime = false;
    // Send this much audio as one frame first, like audio buffered while the
    // model was still loading.
    let mut burst_ms = 0usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model_id = args.next().expect("--model needs an id"),
            "--realtime" => realtime = true,
            "--burst-ms" => {
                burst_ms = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--burst-ms N")
            }
            "--threads" => {
                threads = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--threads N")
            }
            other => wav = Some(PathBuf::from(other)),
        }
    }
    let wav = wav.expect("usage: dictate_wav <clip.wav> [--model id] [--threads N]");

    let spec = find_or_default(&model_id);
    let models = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("Islet")
        .join("models");
    let manager = ModelManager::new(&models);
    println!("model: {} ({:?})", spec.name, manager.status(spec));
    if manager.status(spec) != ModelStatus::Installed {
        let mut last = 0;
        manager
            .download(
                spec,
                &mut |p| {
                    let percent = p.done * 100 / p.total.max(1);
                    if percent != last && percent % 5 == 0 {
                        last = percent;
                        println!("  downloading {percent}%");
                    }
                },
                &AtomicBool::new(false),
            )
            .expect("download");
    }

    let samples: Vec<f32> = {
        let mut reader = hound::WavReader::open(&wav).expect("open wav");
        let spec = reader.spec();
        assert_eq!(
            (spec.sample_rate, spec.channels),
            (16_000, 1),
            "need 16 kHz mono"
        );
        match spec.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|s| f32::from(s.unwrap()) / 32768.0)
                .collect(),
            hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        }
    };
    println!("audio: {:.1} s", samples.len() as f64 / 16_000.0);

    let worker = std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("islet-asr-worker.exe");
    let mut client = AsrClient::new(worker);
    client
        .load(LoadRequest {
            engine: spec.engine.wire_name().to_owned(),
            dir: manager.model_dir(spec).display().to_string(),
            vad: manager.vad_path().display().to_string(),
            threads,
        })
        .expect("start worker");
    match client
        .events()
        .recv_timeout(Duration::from_secs(60))
        .expect("load reply")
    {
        AsrEvent::Loaded { load_ms } => println!("loaded in {load_ms} ms"),
        other => panic!("load failed: {other:?}"),
    }

    client.begin().unwrap();
    let started = Instant::now();
    let burst = (burst_ms * 16).min(samples.len());
    if burst > 0 {
        client.audio(&samples[..burst]).unwrap();
    }
    for (index, chunk) in samples[burst..].chunks(320).enumerate() {
        if realtime {
            // Deliver each 20 ms chunk when the microphone would have.
            let due = started + Duration::from_millis(index as u64 * 20);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        client.audio(chunk).unwrap();
        while let Ok(event) = client.events().try_recv() {
            if let AsrEvent::Segment(s) = event {
                println!(
                    "  segment {:>5} ms audio, {:>4} ms decode: {}",
                    s.audio_ms, s.decode_ms, s.text
                );
            }
        }
    }
    println!("streamed in {:?}", started.elapsed());
    let end = Instant::now();
    client.end().unwrap();
    loop {
        match client
            .events()
            .recv_timeout(Duration::from_secs(60))
            .expect("final reply")
        {
            AsrEvent::Segment(s) => println!("  segment {:>5} ms audio: {}", s.audio_ms, s.text),
            AsrEvent::Final(done) => {
                println!(
                    "FINAL after End: {:?} (tail {} ms audio, {} ms decode)",
                    end.elapsed(),
                    done.tail_audio_ms,
                    done.tail_decode_ms
                );
                println!("text: {}", done.text);
                break;
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    client.shutdown();
}
