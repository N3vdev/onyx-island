//! Saved dictation recordings: where they go, what they are called, writing
//! them, and removing old ones. Only the audio is kept, never what was said.
//!
//! Recordings may live in a folder the user picked (and shares with other
//! files), so cleanup only ever touches files named the way Onyx names them.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const PREFIX: &str = "Dictation ";
const SAMPLE_RATE: u32 = 16_000;
const HEADER_LEN: u32 = 44;

/// Onyx's own folder for recordings, used until the user picks another.
#[must_use]
pub fn default_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Islet")
        .join("Recordings")
}

/// The folder recordings go to: `folder`, or the default when it is empty.
#[must_use]
pub fn dir(folder: &str) -> PathBuf {
    let folder = folder.trim();
    if folder.is_empty() {
        default_dir()
    } else {
        PathBuf::from(folder)
    }
}

/// "Dictation 2026-11-12 13-05-09.wav" for a local time given as
/// `[year, month, day, hour, minute, second]`.
#[must_use]
pub fn file_name(time: [u16; 6]) -> String {
    let [year, month, day, hour, minute, second] = time;
    format!("{PREFIX}{year:04}-{month:02}-{day:02} {hour:02}-{minute:02}-{second:02}.wav")
}

/// Whether `name` is a recording Onyx made: "Dictation <date> <time>.wav",
/// optionally with a " (2)" style suffix.
#[must_use]
pub fn is_recording_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(PREFIX) else {
        return false;
    };
    let Some(stem) = rest
        .strip_suffix(".wav")
        .or_else(|| rest.strip_suffix(".WAV"))
    else {
        return false;
    };
    let (stamp, suffix) = stem.split_at(stem.len().min(19));
    let shape_ok = stamp.len() == 19
        && stamp.char_indices().all(|(index, ch)| match index {
            4 | 7 | 13 | 16 => ch == '-',
            10 => ch == ' ',
            _ => ch.is_ascii_digit(),
        });
    let suffix_ok = suffix.is_empty()
        || suffix
            .strip_prefix(" (")
            .and_then(|s| s.strip_suffix(')'))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|ch| ch.is_ascii_digit()));
    shape_ok && suffix_ok
}

fn recordings(dir: &Path) -> impl Iterator<Item = (PathBuf, fs::Metadata)> {
    fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|entry| {
        let name = entry.file_name();
        if !is_recording_name(&name.to_string_lossy()) {
            return None;
        }
        let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
        Some((entry.path(), meta))
    })
}

/// Deletes recordings in `dir` last written more than `days` days before
/// `now`. `days == 0` keeps everything. Returns how many were deleted.
pub fn prune(dir: &Path, days: u32, now: SystemTime) -> usize {
    if days == 0 {
        return 0;
    }
    let Some(cutoff) = now.checked_sub(Duration::from_secs(u64::from(days) * 24 * 60 * 60)) else {
        return 0;
    };
    recordings(dir)
        .filter(|(_, meta)| meta.modified().is_ok_and(|modified| modified < cutoff))
        .filter(|(path, _)| fs::remove_file(path).is_ok())
        .count()
}

/// How many recordings `dir` holds and their total size in bytes.
#[must_use]
pub fn usage(dir: &Path) -> (usize, u64) {
    recordings(dir).fold((0, 0), |(count, bytes), (_, meta)| (count + 1, bytes + meta.len()))
}

/// Writes 16 kHz mono audio to a 16-bit WAV file as it arrives. A writer
/// dropped before [`WavWriter::finish`] deletes its file, so a discarded
/// dictation leaves nothing behind.
pub struct WavWriter {
    file: Option<BufWriter<File>>,
    path: PathBuf,
    data_bytes: u32,
    failed: bool,
}

impl WavWriter {
    /// Creates `name` in `dir` (making the folder if needed), adding " (2)",
    /// " (3)", ... if a file of that name already exists.
    pub fn create(dir: &Path, name: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let stem = name.strip_suffix(".wav").unwrap_or(name);
        let mut attempt = 1;
        let (file, path) = loop {
            let candidate = if attempt == 1 {
                dir.join(format!("{stem}.wav"))
            } else {
                dir.join(format!("{stem} ({attempt}).wav"))
            };
            match OpenOptions::new().write(true).create_new(true).open(&candidate) {
                Ok(file) => break (file, candidate),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        };
        let mut writer = Self {
            file: Some(BufWriter::with_capacity(64 * 1024, file)),
            path,
            data_bytes: 0,
            failed: false,
        };
        writer.write_header()?;
        Ok(writer)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_header(&mut self) -> io::Result<()> {
        let Some(file) = &mut self.file else {
            return Ok(());
        };
        let mut header = Vec::with_capacity(HEADER_LEN as usize);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(HEADER_LEN - 8 + self.data_bytes).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes()); // PCM
        header.extend_from_slice(&1u16.to_le_bytes()); // mono
        header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        header.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&self.data_bytes.to_le_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// Appends samples in -1..1. After a write error the rest is ignored and
    /// [`WavWriter::finish`] reports it.
    pub fn write(&mut self, samples: &[f32]) {
        if self.failed {
            return;
        }
        let Some(file) = &mut self.file else {
            return;
        };
        let room = (u32::MAX - HEADER_LEN - self.data_bytes) as usize / 2;
        let samples = &samples[..samples.len().min(room)];
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        if file.write_all(&bytes).is_err() {
            self.failed = true;
        } else {
            self.data_bytes += bytes.len() as u32;
        }
    }

    /// Completes the header and closes the file, returning its path.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        if self.failed {
            return Err(io::Error::other("could not write the recording"));
        }
        self.write_header()?;
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        Ok(std::mem::take(&mut self.path))
    }
}

impl Drop for WavWriter {
    fn drop(&mut self) {
        // Not finished (or failed): remove the partial file.
        if let Some(file) = self.file.take() {
            drop(file);
            let _ = fs::remove_file(&self.path);
        } else if self.failed && !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onyx-recordings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_are_recognised_and_other_files_are_not() {
        let name = file_name([2026, 11, 12, 13, 5, 9]);
        assert_eq!(name, "Dictation 2026-11-12 13-05-09.wav");
        assert!(is_recording_name(&name));
        assert!(is_recording_name("Dictation 2026-11-12 13-05-09 (2).wav"));
        for other in [
            "Dictation notes.wav",
            "Dictation 2026-11-12 13-05-09.mp3",
            "My song.wav",
            "Dictation 2026-11-12 13-05-09 copy.wav",
            "Dictation 2026-11-12 13-05-09 ().wav",
        ] {
            assert!(!is_recording_name(other), "{other}");
        }
    }

    #[test]
    fn a_finished_recording_is_a_valid_wav() {
        let dir = temp_dir("write");
        let mut writer = WavWriter::create(&dir, "Dictation 2026-11-12 13-05-09.wav").unwrap();
        writer.write(&[0.0, 0.5, -0.5, 1.5]);
        writer.write(&[0.25; 100]);
        let path = writer.finish().unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 104 * 2);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 208);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 208);
        // 1.5 is clipped to full scale rather than wrapping.
        assert_eq!(i16::from_le_bytes([bytes[50], bytes[51]]), i16::MAX);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dropped_recording_leaves_no_file_and_names_do_not_collide() {
        let dir = temp_dir("drop");
        let name = "Dictation 2026-11-12 13-05-09.wav";
        let first = WavWriter::create(&dir, name).unwrap().finish().unwrap();
        let mut second = WavWriter::create(&dir, name).unwrap();
        assert_eq!(second.path().file_name().unwrap(), "Dictation 2026-11-12 13-05-09 (2).wav");
        second.write(&[0.1; 10]);
        let second_path = second.path().to_owned();
        drop(second);
        assert!(first.exists());
        assert!(!second_path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_old_recordings_are_pruned_and_other_files_are_kept() {
        let dir = temp_dir("prune");
        let old = dir.join("Dictation 2026-11-01 09-00-00.wav");
        let recent = dir.join("Dictation 2026-11-12 09-00-00.wav");
        let theirs = dir.join("Holiday.wav");
        for path in [&old, &recent, &theirs] {
            fs::write(path, b"x").unwrap();
        }
        let week = Duration::from_secs(7 * 24 * 60 * 60);
        let now = SystemTime::now() + week + Duration::from_secs(60);
        // Everything is "old" by then; touch `recent` to make it newer.
        File::options()
            .write(true)
            .open(&recent)
            .unwrap()
            .set_modified(now - Duration::from_secs(60))
            .unwrap();
        assert_eq!(usage(&dir), (2, 2));
        assert_eq!(prune(&dir, 0, now), 0);
        assert_eq!(prune(&dir, 7, now), 1);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(theirs.exists(), "files Onyx did not make are never deleted");
        let _ = fs::remove_dir_all(&dir);
    }
}
