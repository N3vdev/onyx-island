//! P0 ASR spike: load Parakeet TDT once, transcribe the bundled fixtures, and
//! report model load time, latency, real-time speed, WER, and peak RSS.
//!
//! Usage:
//!   cargo run --release -p islet-asr --bin asr-bench
//!   cargo run --release -p islet-asr --bin asr-bench -- --wav path/to.wav --expected "text"
//!
//! Model files are read from `%LOCALAPPDATA%\Islet\models\parakeet-tdt-0.6b-v3-int8`
//! unless `--models` is supplied.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use islet_asr::{Engine, ParakeetTdt, SAMPLE_RATE};
use serde::Deserialize;
use serde_json::json;

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

struct BenchResult {
    name: String,
    audio_seconds: f64,
    inference_seconds: f64,
    speedup: f64,
    wer: Option<f64>,
    text: String,
}

struct Args {
    models: PathBuf,
    manifest: Option<PathBuf>,
    wav: Option<PathBuf>,
    expected: Option<String>,
    threads: usize,
    json: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("asr-bench: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;

    let manifest_path = args
        .manifest
        .clone()
        .unwrap_or_else(|| PathBuf::from("assets/audio/test/manifest.json"));

    let fixtures = if let Some(wav) = &args.wav {
        vec![ManifestEntry {
            file: wav.display().to_string(),
            text: args.expected.clone().unwrap_or_default(),
            seconds: 0.0,
        }]
    } else {
        load_manifest(&manifest_path)?
    };

    if fixtures.is_empty() {
        return Err("no test fixtures found".into());
    }

    eprintln!(
        "loading Parakeet TDT from {} with {} threads...",
        args.models.display(),
        args.threads
    );
    let load_started = Instant::now();
    let mut engine = ParakeetTdt::load(&args.models, args.threads)?;
    let load_seconds = load_started.elapsed().as_secs_f64();
    let after_load = memory_mib();
    eprintln!(
        "model loaded in {load_seconds:.3}s; working set {:.0} MiB, peak {:.0} MiB",
        after_load.0, after_load.1
    );

    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut results = Vec::with_capacity(fixtures.len());

    for fixture in fixtures {
        let wav_path = PathBuf::from(&fixture.file);
        let path = if wav_path.is_absolute() || args.wav.is_some() {
            wav_path
        } else {
            manifest_dir.join(wav_path)
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
        let expected = if fixture.text.trim().is_empty() {
            None
        } else {
            Some(fixture.text.as_str())
        };

        let started = Instant::now();
        let transcript = engine.transcribe(&audio.samples)?;
        let inference_seconds = started.elapsed().as_secs_f64();
        let speedup = audio.seconds / inference_seconds.max(f64::EPSILON);
        let wer = expected.map(|reference| word_error_rate(reference, &transcript.text));

        results.push(BenchResult {
            name: Path::new(&fixture.file).file_stem().map_or_else(
                || fixture.file.clone(),
                |stem| stem.to_string_lossy().into_owned(),
            ),
            audio_seconds: audio.seconds,
            inference_seconds,
            speedup,
            wer,
            text: transcript.text,
        });
    }

    let (working_set_mib, peak_rss_mib) = memory_mib();
    if args.json {
        let payload = json!({
            "engine": "ort 2.0.0-rc.15 + Parakeet TDT v3 INT8 (greedy TDT)",
            "model_dir": args.models.display().to_string(),
            "threads": args.threads,
            "load_seconds": round3(load_seconds),
            "working_set_mib": round1(working_set_mib),
            "peak_rss_mib": round1(peak_rss_mib),
            "results": results.iter().map(|result| json!({
                "file": result.name,
                "audio_seconds": round3(result.audio_seconds),
                "inference_seconds": round3(result.inference_seconds),
                "rtf": round3(result.inference_seconds / result.audio_seconds.max(f64::EPSILON)),
                "speedup": round2(result.speedup),
                "wer": result.wer.map(round4),
                "text": result.text,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!();
        println!(
            "{:<14} {:>7} {:>9} {:>7} {:>7}  transcript",
            "fixture", "audio", "infer", "xRT", "WER"
        );
        for result in &results {
            println!(
                "{:<14} {:>6.2}s {:>8.3}s {:>6.2}x {:>6.1}%  {}",
                result.name,
                result.audio_seconds,
                result.inference_seconds,
                result.speedup,
                result.wer.map_or(f64::NAN, |wer| wer * 100.0),
                result.text
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
    let mut models = default_model_dir();
    let mut manifest = None;
    let mut wav = None;
    let mut expected = None;
    let mut json = false;
    let mut threads = std::thread::available_parallelism().map_or(4, |count| count.get().min(4));

    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--models" => {
                models = PathBuf::from(args.next().ok_or("--models needs a directory argument")?);
            }
            "--manifest" => {
                manifest = Some(PathBuf::from(
                    args.next().ok_or("--manifest needs a file argument")?,
                ));
            }
            "--wav" => {
                wav = Some(PathBuf::from(
                    args.next().ok_or("--wav needs a file argument")?,
                ));
            }
            "--expected" => {
                expected = Some(args.next().ok_or("--expected needs text")?);
            }
            "--threads" => {
                threads = args
                    .next()
                    .ok_or("--threads needs a number")?
                    .parse::<usize>()?
                    .clamp(1, 32);
            }
            "--json" => json = true,
            "--help" | "-h" => {
                println!(
                    "asr-bench [--models DIR] [--manifest FILE] [--wav FILE] [--expected TEXT] [--threads N] [--json]"
                );
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

fn default_model_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Islet")
        .join("models")
        .join("parakeet-tdt-0.6b-v3-int8")
}

fn load_manifest(path: &Path) -> Result<Vec<ManifestEntry>, Box<dyn Error>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    // Windows PowerShell writes UTF-8 files with a BOM by default.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Ok(serde_json::from_str(text)?)
}

fn read_wav(path: &Path) -> Result<Audio, Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let spec = reader.spec();

    if spec.channels != 1 {
        return Err(format!("{} is not mono", path.display()).into());
    }
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err(format!("{} is not 16-bit PCM", path.display()).into());
    }

    let samples: Result<Vec<f32>, _> = reader
        .samples::<i16>()
        .map(|sample| sample.map(|value| f32::from(value) / 32768.0))
        .collect();
    let mut samples =
        samples.map_err(|error| format!("could not read {}: {error}", path.display()))?;

    if spec.sample_rate != SAMPLE_RATE {
        samples = resample_linear(&samples, spec.sample_rate);
    }

    let seconds = samples.len() as f64 / f64::from(SAMPLE_RATE);
    Ok(Audio { samples, seconds })
}

fn resample_linear(samples: &[f32], source_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == SAMPLE_RATE {
        return samples.to_vec();
    }

    let ratio = f64::from(SAMPLE_RATE) / f64::from(source_rate);
    let output_len = (samples.len() as f64 * ratio).round().max(1.0) as usize;
    let mut output = Vec::with_capacity(output_len);
    let last = samples.len() - 1;

    for index in 0..output_len {
        let source_index = index as f64 / ratio;
        let left = source_index.floor() as usize;
        let right = (left + 1).min(last);
        let fraction = (source_index - left as f64) as f32;
        output.push(samples[left] * (1.0 - fraction) + samples[right] * fraction);
    }

    output
}

fn memory_mib() -> (f64, f64) {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: the counters struct is initialized and the size is passed to the
    // API exactly as required by the Windows contract.
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
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wer_is_zero_for_identical_text() {
        assert_eq!(word_error_rate("Hello, world!", "hello world"), 0.0);
    }

    #[test]
    fn wer_counts_one_word_error() {
        assert!((word_error_rate("Aunt Em", "Aunt M") - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn resampling_preserves_duration() {
        let input = vec![0.0_f32; 22_050];
        let output = resample_linear(&input, 22_050);
        assert!((output.len() as i64 - 16_000).abs() <= 1);
    }
}
