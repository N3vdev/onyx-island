//! `islet-asr-worker.exe`: runs a speech model for the island.
//!
//! Protocol (see `islet-asr/src/protocol.rs`): requests on stdin, replies on
//! stdout. Nothing else is ever written to stdout. A reader thread feeds a
//! queue so the pipe never fills while a segment is being recognised.
//!
//! One dictation: `Begin`, a stream of `Audio`, then `End`. Voice-activity
//! detection finds the pauses. Every few seconds the model hears the unkept
//! audio together with a few seconds of what it already kept, and keeps the
//! words before the last pause that has enough speech after it (reported as
//! a `Segment`). So every word is recognised with the words on both sides of
//! it (a pause in the middle of a sentence never becomes a full stop), and
//! audio is only ever cut in a silence, so no word is lost or kept twice.
//! Only the last few seconds are left at `End`, so the text comes quickly.

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

/// Silero settings tuned for dictation. The detector only finds the pauses:
/// the model always hears the microphone's own audio, pauses and all, since
/// the detector's pieces clip soft word starts ("start" heard as "chart").
const VAD_THRESHOLD: f32 = 0.5;
const VAD_MIN_SILENCE_S: f32 = 0.25;
const VAD_MIN_SPEECH_S: f32 = 0.1;
const VAD_MAX_SPEECH_S: f32 = 12.0;
const VAD_WINDOW: i32 = 512;
const VAD_BUFFER_S: f32 = 60.0;

/// A pause this long is a moment to recognise what was said so far (see
/// `keep_finished`).
const SENTENCE_PAUSE_S: f32 = 1.0;
/// Once there is this much unkept speech, a shorter pause will do, so long
/// dictation is recognised while you are still talking and the wait after
/// release stays short.
const LONG_SENTENCE_S: f32 = 3.0;
const LONG_SENTENCE_PAUSE_S: f32 = 0.35;
/// Without a pause, the audio is still looked at this often, once there
/// is at least `UNKEPT_S` of it, so little is left at `End`.
const EVERY_S: f32 = 2.5;
const UNKEPT_S: f32 = 4.5;
/// Audio kept after the last speech when recognising at a pause.
const TAIL_S: f32 = 0.3;
/// A word is only kept once this much audio follows it, so the model knew
/// how the sentence went on.
const RIGHT_CONTEXT_S: f32 = 2.0;
/// Already kept audio heard again before the new, so the model knows how
/// the sentence began.
const LEFT_CONTEXT_S: f32 = 2.5;
/// A word's timing may move this much between two recognitions; cuts are
/// in silences longer than twice this.
const BOUNDARY_SLOP_S: f32 = 0.08;
/// Unkept audio with no usable pause is cut between words past this length.
const MAX_UNKEPT_S: f32 = 20.0;

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
    /// The microphone's audio from `start` on: the last few seconds of what
    /// was kept, then what was not.
    audio: Vec<f32>,
    /// Stream sample where `audio` begins.
    start: usize,
    /// Stream sample where the unkept audio begins.
    kept_to: usize,
    /// Stream sample where the last speech ended; 0 before any speech.
    speech_end: usize,
    /// Where the audio was last recognised up to, in stream samples.
    checked_at: usize,
    /// The middles of the silences between pieces of speech, in stream
    /// samples: the only places audio is cut.
    pauses: Vec<usize>,
}

impl Dictation {
    /// The audio up to stream sample `end`.
    fn upto(&self, end: usize) -> &[f32] {
        &self.audio[..end.saturating_sub(self.start).min(self.audio.len())]
    }

    /// Drops the audio before stream sample `at` (silence, not speech).
    fn skip_to(&mut self, at: usize) {
        let drop = at.saturating_sub(self.start).min(self.audio.len());
        self.audio.drain(..drop);
        self.start += drop;
        self.kept_to = self.kept_to.max(self.start);
    }

    /// Keeps the audio up to stream sample `at`, holding on to the last
    /// `LEFT_CONTEXT_S` of it.
    fn keep_to(&mut self, at: usize) {
        self.kept_to = at.max(self.kept_to);
        let hold = (LEFT_CONTEXT_S * SAMPLE_RATE as f32) as usize;
        let drop = self.kept_to.saturating_sub(hold).saturating_sub(self.start).min(self.audio.len());
        self.audio.drain(..drop);
        self.start += drop;
    }
}

fn main() {
    // Development: `--replay <wav> <model dir> <vad model> [--tokens]` runs a
    // recording through the same path as a dictation and prints the text.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--replay") {
        replay(&args[2..]);
        return;
    }
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
            dictation.audio.extend_from_slice(&samples);
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
                text: join_kept(&std::mem::take(&mut dictation.texts)),
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

/// Looks at the pauses the detector found, and at a long enough one
/// recognises what was said so far (see `keep_finished`). At `End`, all of it.
fn drain(model: &Model, dictation: &mut Dictation, out: &mut impl Write, ending: bool) {
    let rate = SAMPLE_RATE as f32;
    let tail = (TAIL_S * rate) as usize;
    while let Some(piece) = model.vad.front() {
        let start = piece.start().max(0) as usize;
        // Silence before the first word (holding the key, thinking) is
        // never worth hearing.
        if dictation.speech_end == 0 && dictation.texts.is_empty() {
            dictation.skip_to(start.saturating_sub(tail));
        }
        let spoken = dictation.speech_end > dictation.kept_to;
        let pause = start.saturating_sub(dictation.speech_end);
        if dictation.speech_end > 0 && pause > 0 {
            dictation.pauses.push(dictation.speech_end + pause / 2);
        }
        let unkept = dictation.speech_end.saturating_sub(dictation.kept_to);
        let needed = if unkept as f32 >= LONG_SENTENCE_S * rate { LONG_SENTENCE_PAUSE_S } else { SENTENCE_PAUSE_S };
        if spoken && pause as f32 >= needed * rate {
            let upto = dictation.speech_end + tail;
            keep_finished(model, dictation, out, upto, false);
        }
        dictation.speech_end = start + piece.n() as usize;
        model.vad.pop();
    }
    // Speech without a pause long enough: look at it every few seconds.
    let end = dictation.start + dictation.audio.len();
    let unkept = end.saturating_sub(dictation.kept_to);
    // Speaking now, or spoke since the last look.
    let speaking = model.vad.detected() || dictation.speech_end > dictation.kept_to;
    if !ending
        && speaking
        && unkept as f32 >= UNKEPT_S * rate
        && end as f32 >= dictation.checked_at as f32 + EVERY_S * rate
    {
        keep_finished(model, dictation, out, end, false);
    }
    if ending {
        let upto = dictation.start + dictation.audio.len();
        keep_finished(model, dictation, out, upto, true);
    }
}

/// Recognises the unkept audio (after a few seconds of the kept, for
/// context) and keeps the words before the last pause with enough audio
/// after it: at `End`, all of them.
fn keep_finished(model: &Model, dictation: &mut Dictation, out: &mut impl Write, upto: usize, ending: bool) {
    // Callers check there is speech to hear.
    dictation.checked_at = upto;
    let rate = SAMPLE_RATE as f32;
    // Only a pause with enough audio after it can be cut at; without one
    // (and not too much unkept), hearing it now would keep nothing.
    let right = (RIGHT_CONTEXT_S * rate) as usize;
    let pause_at = dictation.pauses.iter().rev().copied().find(|&at| at > dictation.kept_to && at + right <= upto);
    let overdue = upto.saturating_sub(dictation.kept_to) as f32 >= MAX_UNKEPT_S * rate;
    if !ending && pause_at.is_none() && !overdue {
        return;
    }
    let audio = dictation.upto(upto);
    let left_s = (dictation.kept_to - dictation.start) as f32 / rate;
    let started = Instant::now();
    let (tokens, times) = recognise_tokens(&model.recognizer, audio);
    let decode_ms = started.elapsed().as_millis() as u64;
    let audio_s = audio.len() as f32 / rate;
    let words = words_of(&tokens, &times);
    let cut_s = if ending {
        audio_s
    } else {
        let pause = pause_at.map(|at| (at - dictation.start) as f32 / rate);
        match pause.or_else(|| between_words(&words, audio_s)) {
            Some(cut) => cut,
            None => return,
        }
    };
    let text: String = words
        .iter()
        .filter(|(_, at)| *at >= left_s - BOUNDARY_SLOP_S && *at < cut_s)
        .map(|(word, _)| word.as_str())
        .collect::<String>()
        .trim()
        .to_owned();
    let at = dictation.start + (cut_s * rate) as usize;
    let audio_ms = samples_to_ms(at.saturating_sub(dictation.kept_to));
    dictation.recognised_ms += audio_ms;
    dictation.keep_to(at);
    dictation.pauses.retain(|&pause| pause > at);
    if text.is_empty() {
        return;
    }
    dictation.texts.push(text.clone());
    if !ending {
        let reply = Reply::Segment(Segment { text, audio_ms, decode_ms });
        // A closed pipe is noticed by the main loop.
        let _ = reply.write_to(out).and_then(|()| out.flush());
    }
}

/// The kept pieces as one text. A piece after a finished sentence starts
/// with a capital (it was heard mid-sentence, with the words before it).
fn join_kept(pieces: &[String]) -> String {
    let mut text = String::new();
    for piece in pieces {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let sentence_start = text.is_empty() || text.ends_with(['.', '?', '!']);
        if !text.is_empty() {
            text.push(' ');
        }
        let mut chars = piece.chars();
        match chars.next() {
            Some(first) if sentence_start => {
                text.extend(first.to_uppercase());
                text.push_str(chars.as_str());
            }
            _ => text.push_str(piece),
        }
    }
    text
}

/// Recognised tokens grouped into words with where each starts: a word is a
/// token beginning with a space plus the pieces and punctuation after it.
fn words_of(tokens: &[String], times: &[f32]) -> Vec<(String, f32)> {
    let mut words: Vec<(String, f32)> = Vec::new();
    for (token, &at) in tokens.iter().zip(times) {
        match words.last_mut() {
            Some((word, _)) if !token.starts_with(' ') => word.push_str(token),
            _ => words.push((token.clone(), at)),
        }
    }
    words
}

/// With no pause to cut in: just before the last word that has
/// `RIGHT_CONTEXT_S` of audio after it.
fn between_words(words: &[(String, f32)], audio_s: f32) -> Option<f32> {
    let next = (1..words.len()).rev().find(|&i| words[i].1 + RIGHT_CONTEXT_S <= audio_s)?;
    Some((words[next].1 - 0.1).max(words[next - 1].1 + 0.05).min(words[next].1))
}

fn recognise_tokens(recognizer: &OfflineRecognizer, samples: &[f32]) -> (Vec<String>, Vec<f32>) {
    let stream = recognizer.create_stream();
    stream.accept_waveform(SAMPLE_RATE as i32, samples);
    recognizer.decode(&stream);
    stream
        .get_result()
        .map(|result| {
            let times = result.timestamps.unwrap_or_default();
            (result.tokens, times)
        })
        .unwrap_or_default()
}

fn recognise(recognizer: &OfflineRecognizer, samples: &[f32]) -> String {
    let (tokens, _) = recognise_tokens(recognizer, samples);
    tokens.concat().trim().to_owned()
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

/// See `main`: a recording, fed in 20 ms chunks like the microphone does.
fn replay(args: &[String]) {
    let (wav, dir, vad) = (&args[0], &args[1], &args[2]);
    let request = LoadRequest { engine: "nemo_transducer".to_owned(), dir: dir.clone(), vad: vad.clone(), threads: 4 };
    let model = load(&request).expect("load");
    let wave = sherpa_onnx::Wave::read(wav).expect("read wav");
    let samples = wave.samples();
    if args.iter().any(|a| a == "--tokens") {
        let stream = model.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, &samples[..samples.len().min(16_000 * 6)]);
        model.recognizer.decode(&stream);
        let result = stream.get_result().unwrap();
        println!("text: {:?}", result.text);
        println!("tokens: {:?}", result.tokens);
        println!("timestamps: {:?}", result.timestamps);
        return;
    }
    let mut loaded = Some(model);
    let mut dictation = Dictation::default();
    let mut sink = io::sink();
    handle(Request::Begin, &mut loaded, &mut dictation, &mut sink);
    let started = Instant::now();
    for chunk in samples.chunks(320) {
        handle(Request::Audio(chunk.to_vec()), &mut loaded, &mut dictation, &mut sink);
    }
    let streamed = started.elapsed();
    if let Some(Reply::Final(done)) = handle(Request::End, &mut loaded, &mut dictation, &mut sink) {
        println!("audio {:.1} s, recognised while talking in {:.2} s, after release {} ms for the last {} ms of audio", samples.len() as f32 / 16_000.0, streamed.as_secs_f32(), done.tail_decode_ms, done.tail_audio_ms);
        println!("text: {}", done.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn tokens_group_into_words_with_their_punctuation() {
        let t = tokens(&[" pl", "an", "ning", ".", " Next", " 8", " p", ".", "m", "."]);
        let times = [0.1, 0.2, 0.3, 0.4, 0.9, 1.2, 1.4, 1.45, 1.5, 1.55];
        let words = words_of(&t, &times);
        let text: Vec<&str> = words.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(text, [" planning.", " Next", " 8", " p.m."]);
        assert_eq!(words[1].1, 0.9);
    }

    #[test]
    fn pieces_join_with_a_capital_after_a_finished_sentence() {
        let pieces = ["to protect our eyes.", "and I wanted to", "start after 8 p.m.", "by default"].map(String::from);
        assert_eq!(join_kept(&pieces), "To protect our eyes. And I wanted to start after 8 p.m. By default");
    }

    #[test]
    fn without_a_pause_a_long_run_is_cut_between_words() {
        let words: Vec<(String, f32)> = (0..10).map(|i| (format!(" w{i}"), i as f32 * 0.5)).collect();
        // The last word with 2 s after it starts at 2.5 s (of 4.6 s).
        let cut = between_words(&words, 4.6).unwrap();
        assert!(cut > 2.0 && cut <= 2.5, "{cut}");
        assert_eq!(between_words(&words[..2], 1.0), None);
    }
}
