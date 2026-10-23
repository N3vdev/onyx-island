//! The `ort` ASR spike from P0 (kept for comparison; never linked into the app).
//!
//! **P0 status:** this crate currently contains the `ort` spike backend. The
//! P0 comparison selected `sherpa-onnx` for the shipping app (see
//! `PROGRESS.md`); P3 replaces this backend with the sherpa engine. The
//! `asr-bench-sherpa` crate holds the other half of the comparison so both
//! static ONNX Runtime copies never link into one executable.
//!
//! P0 exposes a single Parakeet TDT engine. The ONNX graph is the
//! `istupakov/parakeet-tdt-0.6b-v3-onnx` INT8 export, decoded with the same
//! greedy TDT loop as `onnx-asr`:
//!
//! 1. `nemo128.onnx` turns 16 kHz f32 samples into 128-bin log-mel features.
//! 2. `encoder-model.int8.onnx` produces `[1, 1024, frames]`.
//! 3. `decoder_joint-model.int8.onnx` predicts a vocabulary token and a
//!    duration step for one encoder frame at a time.
//!
//! The decoder state is only committed when the token is not blank; a duration
//! step greater than zero advances the encoder frame cursor.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{Tensor, ValueType};

pub const SAMPLE_RATE: u32 = 16_000;
const MAX_TOKENS_PER_STEP: usize = 10;
const FEATURE_BINS: usize = 128;
const ENCODER_HIDDEN: usize = 1024;

pub type Result<T> = std::result::Result<T, AsrError>;

/// A finished transcription.
#[derive(Clone, Debug)]
pub struct Transcript {
    /// Text with normalized inter-word spacing.
    pub text: String,
    /// Token ids in emission order (before vocabulary text joining).
    pub tokens: Vec<u32>,
    /// Number of encoder frames decoded.
    pub frames: usize,
}

/// The engine contract used by the island and the benchmark.
pub trait Engine {
    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript>;
}

/// File names in the P0 model directory.
#[derive(Clone, Debug)]
pub struct ModelFiles {
    pub preprocessor: PathBuf,
    pub encoder: PathBuf,
    pub decoder_joint: PathBuf,
    pub vocab: PathBuf,
}

impl ModelFiles {
    #[must_use]
    pub fn in_directory(directory: &Path) -> Self {
        Self {
            preprocessor: directory.join("nemo128.onnx"),
            encoder: directory.join("encoder-model.int8.onnx"),
            decoder_joint: directory.join("decoder_joint-model.int8.onnx"),
            vocab: directory.join("vocab.txt"),
        }
    }

    fn validate(&self) -> Result<()> {
        for path in [
            &self.preprocessor,
            &self.encoder,
            &self.decoder_joint,
            &self.vocab,
        ] {
            if !path.is_file() {
                return Err(AsrError::Model(format!(
                    "missing model file: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

/// Parakeet TDT v3 INT8 with an `ort`/ONNX Runtime backend.
pub struct ParakeetTdt {
    // Sessions borrow the environment. Keeping it in the struct documents the
    // ownership relationship even though the sessions also retain it.
    _environment: Environment,
    preprocessor: Session,
    encoder: Session,
    decoder: Session,
    vocab: Vec<String>,
    blank_id: usize,
    state_width: usize,
}

impl ParakeetTdt {
    /// Loads all four model files.
    ///
    /// `threads` applies to the encoder and decoder; the small mel
    /// preprocessor runs single-threaded.
    pub fn load(directory: &Path, threads: usize) -> Result<Self> {
        let files = ModelFiles::in_directory(directory);
        files.validate()?;

        let environment = ort::init().build()?;
        let preprocessor = build_session(&environment, &files.preprocessor, 1)?;
        let encoder = build_session(&environment, &files.encoder, threads.max(1))?;
        let decoder = build_session(&environment, &files.decoder_joint, threads.max(1))?;

        let vocab = load_vocab(&files.vocab)?;
        let blank_id = vocab
            .iter()
            .position(|token| token == "<blk>")
            .ok_or_else(|| AsrError::Model("vocab.txt has no <blk> token".to_owned()))?;
        let state_width = decoder_state_width(&decoder)?;

        Ok(Self {
            _environment: environment,
            preprocessor,
            encoder,
            decoder,
            vocab,
            blank_id,
            state_width,
        })
    }

    #[must_use]
    pub fn vocab(&self) -> &[String] {
        &self.vocab
    }

    #[must_use]
    pub fn blank_id(&self) -> usize {
        self.blank_id
    }

    fn preprocess(&mut self, samples: &[f32]) -> Result<(Vec<f32>, usize, usize)> {
        let sample_count = samples.len();
        let waveforms = Tensor::from_array((vec![1_i64, sample_count as i64], samples.to_vec()))?;
        let waveforms_lens = Tensor::from_array((vec![1_i64], vec![sample_count as i64]))?;

        let outputs = self.preprocessor.run(ort::inputs![
            "waveforms" => waveforms,
            "waveforms_lens" => waveforms_lens,
        ])?;

        let (shape, features) = extract_f32(&outputs, "features")?;
        let feature_lengths = extract_i64(&outputs, "features_lens")?;
        if shape.len() != 3 || shape[0] != 1 || shape[1] != FEATURE_BINS as i64 {
            return Err(AsrError::Model(format!(
                "unexpected preprocessor output shape: {shape:?}"
            )));
        }
        let frames = usize::try_from(shape[2])
            .map_err(|_| AsrError::Model(format!("invalid feature frames: {shape:?}")))?;
        let valid_frames = feature_lengths
            .first()
            .copied()
            .unwrap_or(frames as i64)
            .clamp(0, frames as i64) as usize;

        Ok((features, valid_frames, frames))
    }

    fn encode(&mut self, features: Vec<f32>, feature_frames: usize) -> Result<(Vec<f32>, usize)> {
        let audio_signal = Tensor::from_array((
            vec![1_i64, FEATURE_BINS as i64, feature_frames as i64],
            features,
        ))?;
        let length = Tensor::from_array((vec![1_i64], vec![feature_frames as i64]))?;

        let outputs = self.encoder.run(ort::inputs![
            "audio_signal" => audio_signal,
            "length" => length,
        ])?;

        let (shape, encoded) = extract_f32(&outputs, "outputs")?;
        let encoded_lengths = extract_i64(&outputs, "encoded_lengths")?;
        if shape.len() != 3 || shape[0] != 1 {
            return Err(AsrError::Model(format!(
                "unexpected encoder output shape: {shape:?}"
            )));
        }

        // The export is [batch, hidden, frames]. Some NeMo exports use
        // [batch, frames, hidden]; accept either layout by checking the
        // static hidden dimension.
        let mut encoded = encoded;
        let mut frames = if shape[1] == ENCODER_HIDDEN as i64 {
            shape[2] as usize
        } else if shape[2] == ENCODER_HIDDEN as i64 {
            encoded = transpose_time_major(&encoded, shape[1] as usize, shape[2] as usize);
            shape[1] as usize
        } else {
            return Err(AsrError::Model(format!(
                "encoder output has no {ENCODER_HIDDEN}-wide axis: {shape:?}"
            )));
        };

        if let Some(length) = encoded_lengths.first() {
            frames = frames.min((*length).max(0) as usize);
        }

        Ok((encoded, frames))
    }

    fn decode(&mut self, encoded: &[f32], frames: usize) -> Result<Transcript> {
        let state_len = 2 * self.state_width;
        let mut state_1 = vec![0.0_f32; state_len];
        let mut state_2 = vec![0.0_f32; state_len];

        let mut tokens: Vec<u32> = Vec::new();
        let mut last_token = self.blank_id as i32;
        let mut emitted = 0_usize;
        let mut frame = 0_usize;
        let mut steps = 0_usize;
        let max_steps = frames.saturating_mul(32).max(1);

        while frame < frames && steps < max_steps {
            steps += 1;

            // One encoder column, laid out [1, 1024, 1].
            let mut column = Vec::with_capacity(ENCODER_HIDDEN);
            for hidden in 0..ENCODER_HIDDEN {
                column.push(encoded[hidden * frames + frame]);
            }

            let encoder_outputs =
                Tensor::from_array((vec![1_i64, ENCODER_HIDDEN as i64, 1_i64], column))?;
            let targets = Tensor::from_array((vec![1_i64, 1_i64], vec![last_token]))?;
            let target_length = Tensor::from_array((vec![1_i64], vec![1_i32]))?;
            let input_states_1 =
                Tensor::from_array((vec![2_i64, 1_i64, self.state_width as i64], state_1.clone()))?;
            let input_states_2 =
                Tensor::from_array((vec![2_i64, 1_i64, self.state_width as i64], state_2.clone()))?;

            let outputs = self.decoder.run(ort::inputs![
                "encoder_outputs" => encoder_outputs,
                "targets" => targets,
                "target_length" => target_length,
                "input_states_1" => input_states_1,
                "input_states_2" => input_states_2,
            ])?;

            let (_, logits) = extract_f32(&outputs, "outputs")?;
            let (_, next_state_1) = extract_f32(&outputs, "output_states_1")?;
            let (_, next_state_2) = extract_f32(&outputs, "output_states_2")?;

            if logits.len() < self.vocab.len() + 1 {
                return Err(AsrError::Model(format!(
                    "decoder produced {} logits for {} vocabulary tokens",
                    logits.len(),
                    self.vocab.len()
                )));
            }

            let token = argmax(&logits[..self.vocab.len()]);
            let duration = argmax(&logits[self.vocab.len()..]);

            if token != self.blank_id {
                state_1 = next_state_1;
                state_2 = next_state_2;
                last_token = token as i32;
                tokens.push(token as u32);
                emitted += 1;
            }

            if duration > 0 {
                frame += duration;
                emitted = 0;
            } else if token == self.blank_id || emitted == MAX_TOKENS_PER_STEP {
                frame += 1;
                emitted = 0;
            }
        }

        let text = join_tokens(tokens.iter().map(|&id| self.vocab[id as usize].as_str()));
        Ok(Transcript {
            text,
            tokens,
            frames,
        })
    }
}

impl Engine for ParakeetTdt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript> {
        if samples.is_empty() {
            return Ok(Transcript {
                text: String::new(),
                tokens: Vec::new(),
                frames: 0,
            });
        }

        let (features, feature_frames, _padded_frames) = self.preprocess(samples)?;
        if feature_frames == 0 {
            return Ok(Transcript {
                text: String::new(),
                tokens: Vec::new(),
                frames: 0,
            });
        }

        let (encoded, encoded_frames) = self.encode(features, feature_frames)?;
        self.decode(&encoded, encoded_frames)
    }
}

fn build_session(environment: &Environment, path: &Path, threads: usize) -> Result<Session> {
    let mut builder = Session::builder(environment)?;
    builder = builder.with_intra_threads(threads).map_err(builder_error)?;
    builder = builder.with_inter_threads(1).map_err(builder_error)?;
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(builder_error)?;
    Ok(builder.commit_from_file(path)?)
}

fn builder_error(error: ort::Error<ort::session::builder::SessionBuilder>) -> AsrError {
    AsrError::Ort(error.into())
}

fn decoder_state_width(decoder: &Session) -> Result<usize> {
    let outlet = decoder
        .inputs()
        .iter()
        .find(|input| input.name() == "input_states_1")
        .ok_or_else(|| AsrError::Model("decoder has no input_states_1".to_owned()))?;

    let ValueType::Tensor { shape, .. } = outlet.dtype() else {
        return Err(AsrError::Model(
            "decoder state input is not a tensor".to_owned(),
        ));
    };

    if shape.len() != 3 || shape[2] <= 0 {
        return Err(AsrError::Model(format!(
            "unexpected decoder state shape: {shape:?}"
        )));
    }
    Ok(shape[2] as usize)
}

fn load_vocab(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let mut entries: Vec<(usize, String)> = Vec::new();

    for line in text.lines() {
        let Some((token, index)) = line.rsplit_once(' ') else {
            continue;
        };
        let Ok(index) = index.trim().parse::<usize>() else {
            continue;
        };
        entries.push((index, token.replace('\u{2581}', " ")));
    }

    if entries.is_empty() {
        return Err(AsrError::Model("vocab.txt is empty".to_owned()));
    }

    let size = entries.iter().map(|(index, _)| *index).max().unwrap_or(0) + 1;
    let mut vocab = vec![String::new(); size];
    for (index, token) in entries {
        vocab[index] = token;
    }
    Ok(vocab)
}

fn extract_f32(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<(Vec<i64>, Vec<f32>)> {
    let (shape, data) = outputs[name].try_extract_tensor::<f32>()?;
    let shape = shape.iter().copied().collect();
    Ok((shape, data.to_vec()))
}

fn extract_i64(outputs: &ort::session::SessionOutputs<'_>, name: &str) -> Result<Vec<i64>> {
    let (_, data) = outputs[name].try_extract_tensor::<i64>()?;
    Ok(data.to_vec())
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    let mut best_value = f32::NEG_INFINITY;
    for (index, &value) in values.iter().enumerate() {
        if value > best_value {
            best_value = value;
            best = index;
        }
    }
    best
}

fn transpose_time_major(data: &[f32], frames: usize, hidden: usize) -> Vec<f32> {
    let mut output = vec![0.0; data.len()];
    for t in 0..frames {
        for h in 0..hidden {
            output[h * frames + t] = data[t * hidden + h];
        }
    }
    output
}

fn join_tokens<'a>(tokens: impl Iterator<Item = &'a str>) -> String {
    let mut output = String::new();
    let mut pending_space = false;

    for token in tokens {
        for character in token.chars() {
            if character.is_whitespace() {
                if !output.is_empty() {
                    pending_space = true;
                }
            } else {
                if pending_space && !output.is_empty() && is_word_start(character) {
                    output.push(' ');
                }
                pending_space = false;
                output.push(character);
            }
        }
    }

    output.trim().to_owned()
}

fn is_word_start(character: char) -> bool {
    character.is_alphanumeric() || character == '\'' || character == '"' || character == '('
}

#[derive(Debug)]
pub enum AsrError {
    Ort(ort::Error),
    Io(io::Error),
    Model(String),
    Audio(String),
}

impl fmt::Display for AsrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ort(error) => write!(f, "ONNX Runtime error: {error}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Model(message) => write!(f, "model error: {message}"),
            Self::Audio(message) => write!(f, "audio error: {message}"),
        }
    }
}

impl std::error::Error for AsrError {}

impl From<ort::Error> for AsrError {
    fn from(error: ort::Error) -> Self {
        Self::Ort(error)
    }
}

impl From<io::Error> for AsrError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joining_tokens_keeps_punctuation_attached() {
        let text = join_tokens(
            [
                " The", " quick", " brown", " fox", " jumps", " over", " the", " lazy", " dog",
                " near", " the", " river", " bank", ".",
            ]
            .into_iter(),
        );
        assert_eq!(
            text,
            "The quick brown fox jumps over the lazy dog near the river bank."
        );
    }

    #[test]
    fn joining_tokens_handles_spaced_punctuation() {
        let text = join_tokens(["Hello", " ,", " world", " !"].into_iter());
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn argmax_picks_the_highest_logit() {
        assert_eq!(argmax(&[0.1, 0.9, 0.2]), 1);
        assert_eq!(argmax(&[]), 0);
    }

    #[test]
    fn transpose_converts_time_major_to_hidden_major() {
        // frames = 2, hidden = 3
        let time_major = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let hidden_major = transpose_time_major(&time_major, 2, 3);
        assert_eq!(hidden_major, [1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
    }
}
