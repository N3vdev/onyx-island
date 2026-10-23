//! Wire format between the island and `islet-asr-worker.exe`.
//!
//! A frame is `tag: u8`, `len: u32` (little endian), then `len` payload
//! bytes. Control messages carry JSON; audio carries raw little-endian `f32`
//! samples at 16 kHz mono. Frames flow over the worker's stdin/stdout.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Largest accepted payload. Audio is sent in small chunks; this only bounds
/// a corrupt length so a bad frame cannot make a reader allocate gigabytes.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// Sample rate of all audio on the wire.
pub const SAMPLE_RATE: u32 = 16_000;

/// Parameters for loading a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadRequest {
    /// `EngineKind::wire_name`.
    pub engine: String,
    /// Directory holding the model's files.
    pub dir: String,
    /// Path to `silero_vad.onnx`.
    pub vad: String,
    pub threads: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loaded {
    pub load_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    /// Length of the segment's audio.
    pub audio_ms: u64,
    /// Time spent recognising it.
    pub decode_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Final {
    /// Everything recognised since `Begin`, segments joined with spaces.
    pub text: String,
    /// Recognition time for what was still pending at `End`.
    pub tail_decode_ms: u64,
    /// Audio recognised at `End` (the part not already sent as segments).
    pub tail_audio_ms: u64,
}

/// Messages from the island to the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Load(LoadRequest),
    /// Start a dictation: reset voice-activity state.
    Begin,
    /// More microphone audio for the current dictation.
    Audio(Vec<f32>),
    /// The dictation ended: finish what is pending and reply with `Final`.
    End,
    /// Throw the current dictation away.
    Cancel,
    /// Recognise one clip without voice-activity detection.
    Transcribe(Vec<f32>),
    Quit,
}

/// Messages from the worker to the island.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Loaded(Loaded),
    /// A segment finished during the dictation.
    Segment(Segment),
    Final(Final),
    Error(String),
}

const TAG_LOAD: u8 = 1;
const TAG_BEGIN: u8 = 2;
const TAG_AUDIO: u8 = 3;
const TAG_END: u8 = 4;
const TAG_CANCEL: u8 = 5;
const TAG_TRANSCRIBE: u8 = 6;
const TAG_QUIT: u8 = 7;

const TAG_LOADED: u8 = 101;
const TAG_SEGMENT: u8 = 102;
const TAG_FINAL: u8 = 103;
const TAG_ERROR: u8 = 104;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn samples_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn bytes_to_samples(bytes: &[u8]) -> io::Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(invalid("audio frame is not a whole number of samples"));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn json<T: Serialize>(value: &T) -> Vec<u8> {
    // Serialising these plain structs cannot fail.
    serde_json::to_vec(value).unwrap_or_default()
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))
}

fn write_frame(writer: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(invalid("frame too large"));
    }
    // One write per frame keeps frames contiguous on a shared pipe.
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(tag);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    writer.write_all(&frame)?;
    writer.flush()
}

/// Reads one frame. `Ok(None)` is a clean end of stream between frames.
fn read_frame(reader: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 5];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(invalid("frame too large"));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(Some((header[0], payload)))
}

impl Request {
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Load(request) => write_frame(writer, TAG_LOAD, &json(request)),
            Self::Begin => write_frame(writer, TAG_BEGIN, &[]),
            Self::Audio(samples) => write_frame(writer, TAG_AUDIO, &samples_to_bytes(samples)),
            Self::End => write_frame(writer, TAG_END, &[]),
            Self::Cancel => write_frame(writer, TAG_CANCEL, &[]),
            Self::Transcribe(samples) => {
                write_frame(writer, TAG_TRANSCRIBE, &samples_to_bytes(samples))
            }
            Self::Quit => write_frame(writer, TAG_QUIT, &[]),
        }
    }

    /// `Ok(None)` when the stream ended cleanly.
    pub fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>> {
        let Some((tag, payload)) = read_frame(reader)? else {
            return Ok(None);
        };
        Ok(Some(match tag {
            TAG_LOAD => Self::Load(parse(&payload)?),
            TAG_BEGIN => Self::Begin,
            TAG_AUDIO => Self::Audio(bytes_to_samples(&payload)?),
            TAG_END => Self::End,
            TAG_CANCEL => Self::Cancel,
            TAG_TRANSCRIBE => Self::Transcribe(bytes_to_samples(&payload)?),
            TAG_QUIT => Self::Quit,
            other => return Err(invalid(format!("unknown request tag {other}"))),
        }))
    }
}

impl Reply {
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Loaded(loaded) => write_frame(writer, TAG_LOADED, &json(loaded)),
            Self::Segment(segment) => write_frame(writer, TAG_SEGMENT, &json(segment)),
            Self::Final(done) => write_frame(writer, TAG_FINAL, &json(done)),
            Self::Error(message) => write_frame(writer, TAG_ERROR, message.as_bytes()),
        }
    }

    pub fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>> {
        let Some((tag, payload)) = read_frame(reader)? else {
            return Ok(None);
        };
        Ok(Some(match tag {
            TAG_LOADED => Self::Loaded(parse(&payload)?),
            TAG_SEGMENT => Self::Segment(parse(&payload)?),
            TAG_FINAL => Self::Final(parse(&payload)?),
            TAG_ERROR => Self::Error(String::from_utf8_lossy(&payload).into_owned()),
            other => return Err(invalid(format!("unknown reply tag {other}"))),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_request(request: Request) {
        let mut bytes = Vec::new();
        request.write_to(&mut bytes).unwrap();
        let back = Request::read_from(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(back, request);
    }

    fn round_trip_reply(reply: Reply) {
        let mut bytes = Vec::new();
        reply.write_to(&mut bytes).unwrap();
        let back = Reply::read_from(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(back, reply);
    }

    #[test]
    fn requests_round_trip() {
        round_trip_request(Request::Load(LoadRequest {
            engine: "nemo_transducer".to_owned(),
            dir: "C:\\Users\\a b\\models".to_owned(),
            vad: "C:\\vad.onnx".to_owned(),
            threads: 4,
        }));
        round_trip_request(Request::Begin);
        round_trip_request(Request::Audio(vec![0.0, 0.5, -0.25, 1.0e-7]));
        round_trip_request(Request::Audio(Vec::new()));
        round_trip_request(Request::End);
        round_trip_request(Request::Cancel);
        round_trip_request(Request::Transcribe(vec![0.1; 1600]));
        round_trip_request(Request::Quit);
    }

    #[test]
    fn replies_round_trip() {
        round_trip_reply(Reply::Loaded(Loaded { load_ms: 1750 }));
        round_trip_reply(Reply::Segment(Segment {
            text: "Hello, world — naïve café 你好".to_owned(),
            audio_ms: 2400,
            decode_ms: 130,
        }));
        round_trip_reply(Reply::Final(Final {
            text: String::new(),
            tail_decode_ms: 0,
            tail_audio_ms: 0,
        }));
        round_trip_reply(Reply::Error("model files are missing".to_owned()));
    }

    #[test]
    fn frames_stream_back_to_back_and_end_cleanly() {
        let mut bytes = Vec::new();
        Request::Begin.write_to(&mut bytes).unwrap();
        Request::Audio(vec![1.0, 2.0]).write_to(&mut bytes).unwrap();
        Request::Quit.write_to(&mut bytes).unwrap();

        let mut reader = bytes.as_slice();
        assert_eq!(
            Request::read_from(&mut reader).unwrap(),
            Some(Request::Begin)
        );
        assert_eq!(
            Request::read_from(&mut reader).unwrap(),
            Some(Request::Audio(vec![1.0, 2.0]))
        );
        assert_eq!(
            Request::read_from(&mut reader).unwrap(),
            Some(Request::Quit)
        );
        assert_eq!(Request::read_from(&mut reader).unwrap(), None);
    }

    #[test]
    fn corrupt_frames_are_errors_not_panics() {
        // Unknown tag.
        let mut bytes = vec![250u8, 0, 0, 0, 0];
        assert!(Request::read_from(&mut bytes.as_slice()).is_err());
        // Oversized length.
        bytes = vec![TAG_AUDIO, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(Request::read_from(&mut bytes.as_slice()).is_err());
        // Audio payload that is not a multiple of four bytes.
        bytes = vec![TAG_AUDIO, 3, 0, 0, 0, 1, 2, 3];
        assert!(Request::read_from(&mut bytes.as_slice()).is_err());
        // Truncated payload.
        bytes = vec![TAG_AUDIO, 8, 0, 0, 0, 1, 2];
        assert!(Request::read_from(&mut bytes.as_slice()).is_err());
        // Invalid JSON.
        bytes = vec![TAG_LOADED, 2, 0, 0, 0, b'{', b'x'];
        assert!(Reply::read_from(&mut bytes.as_slice()).is_err());
    }
}
