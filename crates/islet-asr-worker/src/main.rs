//! `islet-asr-worker.exe`: runs a speech model for the island.
//!
//! Protocol (see `islet-asr/src/protocol.rs`): requests on stdin, replies on
//! stdout. Nothing else is ever written to stdout. A reader thread feeds a
//! queue so the pipe never fills while a segment is being recognised.
//!
//! One dictation: `Begin`, a stream of `Audio`, then `End`. Voice-activity
//! detection splits the audio into utterances, each recognised as soon as it
//! closes (reported as a `Segment`), so only the last one is left at `End`.

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use islet_asr::protocol::{Final, LoadRequest, Loaded, Reply, Request, Segment, SAMPLE_RATE};
use islet_asr::EngineKind;
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, SileroVadModelConfig,
    VadModelConfig, VoiceActivityDetector,
};

/// Silero settings tuned for dictation. The detector cuts speech into small
/// pieces at every short pause; `drain` then glues pieces back together into
/// sentences (see `SENTENCE_PAUSE_S`).
const VAD_THRESHOLD: f32 = 0.5;
const VAD_MIN_SILENCE_S: f32 = 0.25;
const VAD_MIN_SPEECH_S: f32 = 0.1;
const VAD_MAX_SPEECH_S: f32 = 12.0;
const VAD_WINDOW: i32 = 512;
const VAD_BUFFER_S: f32 = 60.0;

/// Only a pause this long ends a sentence. Shorter ones (a stutter, a breath,
/// searching for a word) stay inside it, so the model hears the whole
/// sentence at once.
const SENTENCE_PAUSE_S: f32 = 1.0;
/// ...and the model hears those short pauses as this much silence. Parakeet
/// reads a long pause as the end of a sentence and puts a full stop there,
/// which is wrong when the pause was a stutter.
const JOIN_GAP_S: f32 = 0.15;
/// Once a sentence is this long, a natural pause of `LONG_SENTENCE_PAUSE_S`
/// ends it, so long dictation is recognised while you are still talking and
/// the wait after release stays short.
const LONG_SENTENCE_S: f32 = 6.0;
const LONG_SENTENCE_PAUSE_S: f32 = 0.45;
/// A monologue with no pause at all is still recognised in pieces.
const MAX_SENTENCE_S: f32 = 15.0;

/// A loaded speech model plus the voice-activity detector that feeds it.
struct Model {
    recognizer: OfflineRecognizer,
    vad: VoiceActivityDetector,
}

#[derive(Default)]
struct Dictation {
    texts: Vec<String>,
    /// Audio recognised since the last `Begin`, in milliseconds.
    recognised_ms: u64,
    /// Speech of the sentence in progress, short pauses squeezed.
    sentence: Vec<f32>,
    /// Where the last piece of that sentence ended, in stream samples.
    sentence_end: i32,
}

fn main() {
    let (tx, rx) = mpsc::channel::<io::Result<Option<Request>>>();
    std::thread::spawn(move || {
        let mut stdin = io::BufReader::new(io::stdin().lock());
        loop {
            let message = Request::read_from(&mut stdin);
            let finished = matches!(message, Ok(None) | Err(_) | Ok(Some(Request::Quit)));
            if tx.send(message).is_err() || finished {
                return;
            }
        }
    });

    let mut out = BufWriter::new(io::stdout().lock());
    let mut loaded: Option<Model> = None;
    let mut dictation = Dictation::default();

    for message in rx {
        let request = match message {
            Ok(Some(Request::Quit)) | Ok(None) | Err(_) => break,
            Ok(Some(request)) => request,
        };
        let reply = handle(request, &mut loaded, &mut dictation, &mut out);
        let sent = match reply {
            Some(reply) => reply.write_to(&mut out),
            None => Ok(()),
        };
        if sent.and_then(|()| out.flush()).is_err() {
            break; // The island is gone.
        }
    }
}

fn handle(
    request: Request,
    loaded: &mut Option<Model>,
    dictation: &mut Dictation,
    out: &mut impl Write,
) -> Option<Reply> {
    match request {
        Request::Load(request) => {
            *loaded = None;
            let started = Instant::now();
            match load(&request) {
                Ok(model) => {
                    *loaded = Some(model);
                    Some(Reply::Loaded(Loaded {
                        load_ms: started.elapsed().as_millis() as u64,
                    }))
                }
                Err(message) => Some(Reply::Error(message)),
            }
        }
        Request::Begin => {
            let Some(model) = loaded.as_ref() else {
                return Some(Reply::Error("no model is loaded".to_owned()));
            };
            model.vad.reset();
            *dictation = Dictation::default();
            None
        }
        Request::Audio(samples) => {
            let model = loaded.as_ref()?;
            // The detector is built for window-sized input. A big buffer in
            // one call (audio held back while the model loaded) makes it miss
            // the start of the speech, so always feed it window by window.
            for window in samples.chunks(VAD_WINDOW as usize) {
                model.vad.accept_waveform(window);
            }
            drain(model, dictation, out, false);
            None
        }
        Request::End => {
            let Some(model) = loaded.as_ref() else {
                return Some(Reply::Error("no model is loaded".to_owned()));
            };
            model.vad.flush();
            let before = dictation.recognised_ms;
            let started = Instant::now();
            drain(model, dictation, out, true);
            let done = Final {
                text: std::mem::take(&mut dictation.texts).join(" "),
                tail_decode_ms: started.elapsed().as_millis() as u64,
                tail_audio_ms: dictation.recognised_ms - before,
            };
            *dictation = Dictation::default();
            Some(Reply::Final(done))
        }
        Request::Cancel => {
            if let Some(model) = loaded.as_ref() {
                model.vad.reset();
            }
            *dictation = Dictation::default();
            None
        }
        Request::Transcribe(samples) => {
            let Some(model) = loaded.as_ref() else {
                return Some(Reply::Error("no model is loaded".to_owned()));
            };
            let started = Instant::now();
            let text = recognise(&model.recognizer, &samples);
            Some(Reply::Final(Final {
                text,
                tail_decode_ms: started.elapsed().as_millis() as u64,
                tail_audio_ms: samples_to_ms(samples.len()),
            }))
        }
        Request::Quit => None,
    }
}

/// Collects the pieces the detector has closed into sentences and recognises
/// each finished sentence. Segments are reported as they finish, except at
/// `End` where the final reply carries the text.
fn drain(model: &Model, dictation: &mut Dictation, out: &mut impl Write, ending: bool) {
    let rate = SAMPLE_RATE as f32;
    while let Some(piece) = model.vad.front() {
        let start = piece.start();
        let pause = start - dictation.sentence_end;
        let needed = if dictation.sentence.len() as f32 >= LONG_SENTENCE_S * rate {
            LONG_SENTENCE_PAUSE_S
        } else {
            SENTENCE_PAUSE_S
        };
        if !dictation.sentence.is_empty() && pause as f32 >= needed * rate {
            finish_sentence(model, dictation, out, ending);
        }
        if !dictation.sentence.is_empty() {
            let gap = (pause.max(0) as usize).min((JOIN_GAP_S * rate) as usize);
            dictation.sentence.extend(std::iter::repeat_n(0.0, gap));
        }
        dictation.sentence.extend_from_slice(piece.samples());
        dictation.sentence_end = start + piece.n();
        model.vad.pop();
        if dictation.sentence.len() as f32 >= MAX_SENTENCE_S * rate {
            finish_sentence(model, dictation, out, ending);
        }
    }
    if ending && !dictation.sentence.is_empty() {
        finish_sentence(model, dictation, out, ending);
    }
}

fn finish_sentence(model: &Model, dictation: &mut Dictation, out: &mut impl Write, ending: bool) {
    let samples = std::mem::take(&mut dictation.sentence);
    let audio_ms = samples_to_ms(samples.len());
    let started = Instant::now();
    let text = recognise(&model.recognizer, &samples);
    let decode_ms = started.elapsed().as_millis() as u64;

    dictation.recognised_ms += audio_ms;
    if text.is_empty() {
        return;
    }
    dictation.texts.push(text.clone());
    if !ending {
        let reply = Reply::Segment(Segment {
            text,
            audio_ms,
            decode_ms,
        });
        // A closed pipe is noticed by the main loop.
        let _ = reply.write_to(out).and_then(|()| out.flush());
    }
}

fn recognise(recognizer: &OfflineRecognizer, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE as i32, samples);
    recognizer.decode(&stream);
    stream
        .get_result()
        .map(|result| result.text.trim().to_owned())
        .unwrap_or_default()
}

fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1000 / u64::from(SAMPLE_RATE)
}

fn load(request: &LoadRequest) -> Result<Model, String> {
    let kind = EngineKind::from_wire_name(&request.engine)
        .ok_or_else(|| format!("unsupported model type `{}`", request.engine))?;
    let dir = Path::new(&request.dir);
    let threads = request.threads.clamp(1, 16) as i32;

    let recognizer = match kind {
        EngineKind::NemoTransducer => {
            let file = |name: &str| -> Result<String, String> {
                let path = dir.join(name);
                if path.is_file() {
                    Ok(path.display().to_string())
                } else {
                    Err(format!("model file {name} is missing"))
                }
            };
            let mut config = OfflineRecognizerConfig::default();
            config.model_config.transducer = OfflineTransducerModelConfig {
                encoder: Some(file("encoder.int8.onnx")?),
                decoder: Some(file("decoder.int8.onnx")?),
                joiner: Some(file("joiner.int8.onnx")?),
            };
            config.model_config.tokens = Some(file("tokens.txt")?);
            config.model_config.num_threads = threads;
            config.model_config.provider = Some("cpu".to_owned());
            config.model_config.model_type = Some("nemo_transducer".to_owned());
            config.feat_config.feature_dim = 128;
            config.decoding_method = Some("greedy_search".to_owned());
            OfflineRecognizer::create(&config)
                .ok_or_else(|| "the speech model could not be loaded".to_owned())?
        }
    };

    if !Path::new(&request.vad).is_file() {
        return Err("the voice detection model is missing".to_owned());
    }
    let vad_config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(request.vad.clone()),
            threshold: VAD_THRESHOLD,
            min_silence_duration: VAD_MIN_SILENCE_S,
            min_speech_duration: VAD_MIN_SPEECH_S,
            window_size: VAD_WINDOW,
            max_speech_duration: VAD_MAX_SPEECH_S,
        },
        sample_rate: SAMPLE_RATE as i32,
        num_threads: 1,
        provider: Some("cpu".to_owned()),
        ..VadModelConfig::default()
    };
    let vad = VoiceActivityDetector::create(&vad_config, VAD_BUFFER_S)
        .ok_or_else(|| "the voice detection model could not be loaded".to_owned())?;

    Ok(Model { recognizer, vad })
}
