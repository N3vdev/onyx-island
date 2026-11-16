//! Finds a song's lyrics on LRCLIB (lrclib.net), the free and open lyrics
//! database, for the island's lyrics view.
//!
//! The song playing is looked up when it starts, so the music card shows its
//! lyrics button only for songs that have words: its title, artist and
//! length go to LRCLIB, nothing else. A lookup that could not reach LRCLIB
//! is tried again a little later.
//! Answers are kept in memory for the session (never on disk, never in the
//! log), so going back to a song shows its words at once.
//!
//! One thread does the asking. It starts with the first lookup and then
//! waits, costing nothing, for the next.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Sender};
use islet_core::lyrics::{parse_lrc, plain_lines, query, LyricsBody};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the island when a lookup finished.
pub const WM_APP_LYRICS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 33;

const API: &str = "https://lrclib.net/api";
/// Songs remembered for the session.
const KEEP: usize = 40;
/// A lookup that could not reach LRCLIB is asked again after this.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// A song as the player names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Song {
    pub title: String,
    pub artist: String,
}

struct Request {
    song: Song,
    /// Seconds; zero when unknown.
    duration: f64,
}

pub struct Lyrics {
    island: isize,
    answers: Arc<Mutex<Answers>>,
    tx: Option<Sender<Request>>,
}

#[derive(Default)]
struct Answers {
    found: HashMap<Song, LyricsBody>,
    /// Oldest first, to forget beyond `KEEP`.
    order: Vec<Song>,
    /// When a song's lookup could not reach LRCLIB.
    failed_at: HashMap<Song, Instant>,
}

impl Lyrics {
    pub fn new(island: HWND) -> Self {
        Self { island: island.0 as isize, answers: Arc::default(), tx: None }
    }

    /// The words for `song`: found, not found, or being looked up (asked
    /// for now if they were not already).
    pub fn get(&mut self, song: &Song, duration: f64) -> LyricsBody {
        let known = self.answers.lock().ok().and_then(|answers| {
            let body = answers.found.get(song).cloned()?;
            // Offline then: ask again once a while has passed.
            let stale = body == LyricsBody::Unavailable
                && answers.failed_at.get(song).is_none_or(|at| at.elapsed() >= RETRY_AFTER);
            (!stale).then_some(body)
        });
        if let Some(body) = known {
            return body;
        }
        if let Ok(mut answers) = self.answers.lock() {
            answers.failed_at.remove(song);
            if answers.found.insert(song.clone(), LyricsBody::Loading).is_none() {
                answers.order.push(song.clone());
            }
            while answers.order.len() > KEEP {
                let old = answers.order.remove(0);
                answers.found.remove(&old);
                answers.failed_at.remove(&old);
            }
        }
        let tx = self.tx.get_or_insert_with(|| spawn(self.island, self.answers.clone()));
        let _ = tx.send(Request { song: song.clone(), duration });
        LyricsBody::Loading
    }

    /// Asks again for a song whose lookup failed (back online).
    pub fn retry(&mut self, song: &Song, duration: f64) {
        let failed = self
            .answers
            .lock()
            .is_ok_and(|answers| answers.found.get(song) == Some(&LyricsBody::Unavailable));
        if failed {
            if let Ok(mut answers) = self.answers.lock() {
                answers.failed_at.remove(song);
            }
            let _ = self.get(song, duration);
        }
    }
}

fn spawn(island: isize, answers: Arc<Mutex<Answers>>) -> Sender<Request> {
    let (tx, rx) = unbounded::<Request>();
    let _ = std::thread::Builder::new().name("lyrics".to_owned()).spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .user_agent(concat!("Onyx/", env!("CARGO_PKG_VERSION"), " (desktop island, lyrics view)"))
            .timeout_connect(Duration::from_secs(8))
            .timeout_read(Duration::from_secs(10))
            .build();
        while let Ok(mut request) = rx.recv() {
            // Only the newest matters if several queued (skipping songs):
            // the skipped ones are forgotten, to be asked for if they play.
            while let Ok(newer) = rx.try_recv() {
                if let Ok(mut answers) = answers.lock() {
                    if answers.found.get(&request.song) == Some(&LyricsBody::Loading) {
                        answers.found.remove(&request.song);
                        answers.order.retain(|song| *song != request.song);
                    }
                }
                request = newer;
            }
            let body = look_up(&agent, &request.song, request.duration);
            // Which song, never in the log; only how it went.
            tracing::info!(outcome = outcome(&body), "lyrics lookup finished");
            if let Ok(mut answers) = answers.lock() {
                if answers.found.contains_key(&request.song) {
                    if body == LyricsBody::Unavailable {
                        answers.failed_at.insert(request.song.clone(), Instant::now());
                    }
                    answers.found.insert(request.song, body);
                }
            }
            // SAFETY: a non-blocking post to the island window.
            unsafe {
                let _ = PostMessageW(Some(HWND(island as *mut _)), WM_APP_LYRICS, WPARAM(0), LPARAM(0));
            }
        }
    });
    tx
}

fn outcome(body: &LyricsBody) -> &'static str {
    match body {
        LyricsBody::Synced(_) => "synced",
        LyricsBody::Plain(_) => "plain",
        LyricsBody::Instrumental => "instrumental",
        LyricsBody::NotFound => "not found",
        LyricsBody::Unavailable => "unavailable",
        LyricsBody::Loading => "loading",
    }
}

/// The exact match first (title, artist and length), then a search.
fn look_up(agent: &ureq::Agent, song: &Song, duration: f64) -> LyricsBody {
    let (title, artist) = query(&song.title, &song.artist);
    if title.is_empty() {
        return LyricsBody::NotFound;
    }
    let mut get = agent.get(&format!("{API}/get")).query("track_name", &title).query("artist_name", &artist);
    if duration >= 1.0 {
        get = get.query("duration", &format!("{}", duration.round() as u64));
    }
    match get.call() {
        Ok(response) => {
            if let Some(body) = response.into_json::<serde_json::Value>().ok().and_then(|record| body_of(&record)) {
                return body;
            }
        }
        Err(ureq::Error::Status(_, _)) => {}
        Err(error) => {
            tracing::info!(kind = %error.kind(), "lyrics lookup failed");
            return LyricsBody::Unavailable;
        }
    }
    let search = agent.get(&format!("{API}/search")).query("track_name", &title).query("artist_name", &artist).call();
    let records = match search {
        Ok(response) => response.into_json::<Vec<serde_json::Value>>().unwrap_or_default(),
        Err(ureq::Error::Status(_, _)) => Vec::new(),
        Err(_) => return LyricsBody::Unavailable,
    };
    best(&records, duration).and_then(body_of).unwrap_or(LyricsBody::NotFound)
}

/// From search results: timed words of about the right length, then any
/// timed words, then any words at all.
fn best(records: &[serde_json::Value], duration: f64) -> Option<&serde_json::Value> {
    let synced = |record: &&serde_json::Value| record["syncedLyrics"].as_str().is_some_and(|text| !text.trim().is_empty());
    let close = |record: &&serde_json::Value| {
        duration < 1.0 || record["duration"].as_f64().is_some_and(|length| (length - duration).abs() <= 3.0)
    };
    records
        .iter()
        .find(|record| synced(record) && close(record))
        .or_else(|| records.iter().find(|record| synced(record) && duration < 1.0))
        .or_else(|| records.iter().find(|record| close(record) && body_of(record).is_some()))
}

fn body_of(record: &serde_json::Value) -> Option<LyricsBody> {
    if record["instrumental"].as_bool() == Some(true) {
        return Some(LyricsBody::Instrumental);
    }
    if let Some(text) = record["syncedLyrics"].as_str() {
        let lines = parse_lrc(text);
        if !lines.is_empty() {
            return Some(LyricsBody::Synced(Arc::new(lines)));
        }
    }
    let lines = plain_lines(record["plainLyrics"].as_str()?);
    (!lines.is_empty()).then(|| LyricsBody::Plain(Arc::new(lines)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_becomes_timed_words_plain_words_or_instrumental() {
        let synced = serde_json::json!({ "syncedLyrics": "[00:01.00]Hello", "plainLyrics": "Hello" });
        assert!(matches!(body_of(&synced), Some(LyricsBody::Synced(lines)) if lines.len() == 1));
        let plain = serde_json::json!({ "syncedLyrics": null, "plainLyrics": "Hello\nthere" });
        assert!(matches!(body_of(&plain), Some(LyricsBody::Plain(lines)) if lines.len() == 2));
        let instrumental = serde_json::json!({ "instrumental": true, "plainLyrics": null });
        assert_eq!(body_of(&instrumental), Some(LyricsBody::Instrumental));
        assert_eq!(body_of(&serde_json::json!({ "plainLyrics": "" })), None);
    }

    #[test]
    fn the_search_prefers_timed_words_of_the_right_length() {
        let records = vec![
            serde_json::json!({ "id": 1, "duration": 180.0, "syncedLyrics": "[00:01.00]Wrong cut" }),
            serde_json::json!({ "id": 2, "duration": 237.0, "syncedLyrics": null, "plainLyrics": "Plain" }),
            serde_json::json!({ "id": 3, "duration": 238.0, "syncedLyrics": "[00:01.00]Right" }),
        ];
        assert_eq!(best(&records, 237.0).unwrap()["id"], 3);
        assert_eq!(best(&records[..2], 237.0).unwrap()["id"], 2, "words of the right length beat a wrong cut");
        assert_eq!(best(&records, 0.0).unwrap()["id"], 1, "no length known: the first timed words");
    }
}
