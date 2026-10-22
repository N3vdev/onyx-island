//! P0 ASR spike, sherpa-onnx side.
//!
//! Uses sherpa's split Parakeet TDT export (encoder/decoder/joiner) and the
//! `nemo_transducer` offline recognizer. See `asr-bench` for the `ort` side.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    file: String,
    text: String,
    #[serde(default)]
    seconds: f64,
}

struct Audio {
    samples: Vec<f32>,
    seconds: f64,
}

struct Args {
    models: PathBuf,
    manifest: PathBuf,
    wav: Option<PathBuf>,
    expected: Option<String>,
    threads: usize,
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("asr-bench-sherpa: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let fixtures = if let Some(wav) = &args.wav {
        vec![ManifestEntry {
            file: wav.display().to_string(),
            text: args.expected.clone().unwrap_or_default(),
            seconds: 0.0,
        }]
    } else {
        load_manifest(&args.manifest)?
    };

    if fixtures.is_empty() {
        return Err("no test fixtures found".into());
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(args.models.join("encoder.int8.onnx").display().to_string()),
        decoder: Some(args.models.join("decoder.int8.onnx").display().to_string()),
        joiner: Some(args.models.join("joiner.int8.onnx").display().to_string()),
    };
    config.model_config.tokens = Some(args.models.join("tokens.txt").display().to_string());
    config.model_config.num_threads = args.threads as i32;
    config.model_config.provider = Some("cpu".to_owned());
    config.model_config.model_type = Some("nemo_transducer".to_owned());
    config.feat_config.feature_dim = 128;
    config.decoding_method = Some("greedy_search".to_owned());

    eprintln!(
        "loading sherpa-onnx Parakeet TDT from {} with {} threads...",
        args.models.display(),
        args.threads
    );
    let load_started = Instant::now();
    let recognizer =
        OfflineRecognizer::create(&config).ok_or("sherpa-onnx could not create the recognizer")?;
    let load_seconds = load_started.elapsed().as_secs_f64();
    let after_load = memory_mib();
    eprintln!(
        "model loaded in {load_seconds:.3}s; working set {:.0} MiB, peak {:.0} MiB",
        after_load.0, after_load.1
    );

    let manifest_dir = args.manifest.parent().unwrap_or_else(|| Path::new("."));
    let mut results = Vec::with_capacity(fixtures.len());

    for fixture in fixtures {
        let path = if Path::new(&fixture.file).is_absolute() || args.wav.is_some() {
            PathBuf::from(&fixture.file)
        } else {
            manifest_dir.join(&fixture.file)
        };
        let audio = read_wav(&path)?;
        if fixture.seconds > 0.0 && (audio.seconds - fixture.seconds).abs() > 0.25 {
            eprintln!(
                "warning: {} is {:.2}s; manifest declares {:.2}s",
                path.display(),
                audio.seconds,
                fixture.seconds
            );
        }

        let started = Instant::now();
        let stream = recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, &audio.samples);
        recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| format!("sherpa-onnx returned no result for {}", path.display()))?;
        let inference_seconds = started.elapsed().as_secs_f64();
        let speedup = audio.seconds / inference_seconds.max(f64::EPSILON);
        let wer = if fixture.text.trim().is_empty() {
            None
        } else {
            Some(word_error_rate(&fixture.text, &result.text))
        };

        results.push((
            Path::new(&fixture.file).file_stem().map_or_else(
                || fixture.file.clone(),
                |stem| stem.to_string_lossy().into_owned(),
            ),
            audio.seconds,
            inference_seconds,
            speedup,
            wer,
            result.text,
        ));
    }

    let (working_set_mib, peak_rss_mib) = memory_mib();
    if args.json {
        let payload = json!({
            "engine": "sherpa-onnx 1.13.8 + Parakeet TDT v3 INT8 (nemo_transducer)",
            "model_dir": args.models.display().to_string(),
            "threads": args.threads,
            "load_seconds": round3(load_seconds),
            "working_set_mib": round1(working_set_mib),
            "peak_rss_mib": round1(peak_rss_mib),
            "results": results.iter().map(|(name, audio, inference, speedup, wer, text)| json!({
                "file": name,
                "audio_seconds": round3(*audio),
                "inference_seconds": round3(*inference),
                "rtf": round3(*inference / audio.max(f64::EPSILON)),
                "speedup": round2(*speedup),
                "wer": wer.map(round4),
                "text": text,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!();
        println!(
            "{:<14} {:>7} {:>9} {:>7} {:>7}  transcript",
            "fixture", "audio", "infer", "xRT", "WER"
        );
        for (name, audio, inference, speedup, wer, text) in &results {
            println!(
                "{:<14} {:>6.2}s {:>8.3}s {:>6.2}x {:>6.1}%  {}",
                name,
                audio,
                inference,
                speedup,
                wer.map_or(f64::NAN, |value| value * 100.0),
                text
            );
        }
        println!();
        println!(
            "load {load_seconds:.3}s | peak RSS {peak_rss_mib:.0} MiB | threads {} | model {}",
            args.threads,
            args.models.display()
        );
    }

    Ok(())
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut models = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Islet")
        .join("models")
        .join("sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8");
    let mut manifest = PathBuf::from("assets/audio/test/manifest.json");
    let mut wav = None;
    let mut expected = None;
    let mut json = false;
    let mut threads = std::thread::available_parallelism().map_or(4, |count| count.get().min(4));

    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--models" => models = PathBuf::from(args.next().ok_or("--models needs a directory")?),
            "--manifest" => manifest = PathBuf::from(args.next().ok_or("--manifest needs a file")?),
            "--wav" => wav = Some(PathBuf::from(args.next().ok_or("--wav needs a file")?)),
            "--expected" => expected = Some(args.next().ok_or("--expected needs text")?),
            "--threads" => {
                threads = args
                    .next()
                    .ok_or("--threads needs a number")?
                    .parse::<usize>()?
                    .clamp(1, 32)
            }
            "--json" => json = true,
            "--help" | "-h" => {
                println!("asr-bench-sherpa [--models DIR] [--manifest FILE] [--wav FILE] [--threads N] [--json]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(Args {
        models,
        manifest,
        wav,
        expected,
        threads,
        json,
    })
}

fn load_manifest(path: &Path) -> Result<Vec<ManifestEntry>, Box<dyn Error>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Ok(serde_json::from_str(text)?)
}

fn read_wav(path: &Path) -> Result<Audio, Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 16 {
        return Err(format!("{} must be mono 16-bit PCM", path.display()).into());
    }
    let samples: Result<Vec<f32>, _> = reader
        .samples::<i16>()
        .map(|sample| sample.map(|value| f32::from(value) / 32768.0))
        .collect();
    let samples = samples?;
    if spec.sample_rate != SAMPLE_RATE {
        return Err(format!(
            "{} is {} Hz; expected {SAMPLE_RATE} Hz",
            path.display(),
            spec.sample_rate
        )
        .into());
    }
    let seconds = samples.len() as f64 / f64::from(SAMPLE_RATE);
    Ok(Audio { samples, seconds })
}

fn memory_mib() -> (f64, f64) {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
        if ok == 0 {
            return (0.0, 0.0);
        }
        (
            counters.WorkingSetSize as f64 / (1024.0 * 1024.0),
            counters.PeakWorkingSetSize as f64 / (1024.0 * 1024.0),
        )
    }
}

fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference = words(reference);
    let hypothesis = words(hypothesis);
    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }

    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0_usize; hypothesis.len() + 1];
    for (i, reference_word) in reference.iter().enumerate() {
        current[0] = i + 1;
        for (j, hypothesis_word) in hypothesis.iter().enumerate() {
            let substitution = previous[j] + usize::from(reference_word != hypothesis_word);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[hypothesis.len()] as f64 / reference.len() as f64
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '\'' {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}
