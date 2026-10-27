//! The last song played, kept on disk so an always-on island can show it
//! (cover, title, a play button) after Islet or the PC restarts.
//!
//! Two files under the local data folder: a small text file with the track
//! and the app it came from, and the cover's raw pixels. Written once per
//! track (and once more when its cover arrives), never per tick.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use islet_core::island::MediaCard;
use islet_core::media::ArtImage;

const INFO_FILE: &str = "last-music.txt";
const ART_FILE: &str = "last-music.art";
/// Marks art loaded from disk, so it never matches a live art id.
const RESTORED_ART_ID: u64 = 0x1A57_0000_0000_0001;

#[derive(Clone, Debug, PartialEq)]
pub struct Remembered {
    /// The media session's app id, used to relaunch the player.
    pub app_id: String,
    /// Always paused, with no timeline or volume.
    pub card: MediaCard,
}

impl Remembered {
    pub fn from_live(app_id: &str, card: &MediaCard) -> Self {
        Self {
            app_id: app_id.to_owned(),
            card: MediaCard {
                playing: false,
                timeline: None,
                scrub: None,
                volume: None,
                ..card.clone()
            },
        }
    }

    /// Same track and cover: nothing new to write.
    pub fn same_as(&self, other: &Self) -> bool {
        self.app_id == other.app_id
            && self.card.same_track(&other.card)
            && self.card.art.as_ref().map(|art| art.id) == other.card.art.as_ref().map(|art| art.id)
    }
}

fn clean(value: &str) -> String {
    value.replace(['\n', '\r', '\t'], " ")
}

pub fn save(dir: &Path, remembered: &Remembered) {
    let card = &remembered.card;
    let mut info = format!(
        "v1\napp_id\t{}\nsource\t{}\ntitle\t{}\nartist\t{}\n",
        clean(&remembered.app_id),
        clean(&card.source),
        clean(&card.title),
        clean(&card.artist)
    );
    if let Some([r, g, b]) = card.accent {
        info.push_str(&format!("accent\t{r},{g},{b}\n"));
    }
    if let Some(art) = &card.art {
        info.push_str(&format!("art\t{},{}\n", art.width, art.height));
    }
    let art = card.art.as_ref().map(|art| Arc::clone(&art.pixels));
    let dir: PathBuf = dir.to_owned();
    // Off the UI thread: a cover is a few hundred kilobytes.
    std::thread::spawn(move || {
        let written = fs::create_dir_all(&dir)
            .and_then(|()| match &art {
                Some(pixels) => fs::write(dir.join(ART_FILE), pixels),
                None => match fs::remove_file(dir.join(ART_FILE)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                },
            })
            .and_then(|()| fs::write(dir.join(INFO_FILE), info));
        if let Err(error) = written {
            tracing::warn!(%error, "could not remember the last song");
        }
    });
}

pub fn load(dir: &Path) -> Option<Remembered> {
    let info = fs::read_to_string(dir.join(INFO_FILE)).ok()?;
    let mut lines = info.lines();
    if lines.next()? != "v1" {
        return None;
    }
    let mut remembered = Remembered {
        app_id: String::new(),
        card: MediaCard {
            title: String::new(),
            artist: String::new(),
            source: String::new(),
            playing: false,
            timeline: None,
            art: None,
            accent: None,
            scrub: None,
            volume: None,
        },
    };
    let mut art_size = None;
    for line in lines {
        let Some((key, value)) = line.split_once('\t') else {
            continue;
        };
        let numbers = || -> Vec<u32> { value.split(',').filter_map(|n| n.parse().ok()).collect() };
        match key {
            "app_id" => remembered.app_id = value.to_owned(),
            "source" => remembered.card.source = value.to_owned(),
            "title" => remembered.card.title = value.to_owned(),
            "artist" => remembered.card.artist = value.to_owned(),
            "accent" => {
                if let [r, g, b] = numbers()[..] {
                    remembered.card.accent = Some([r as u8, g as u8, b as u8]);
                }
            }
            "art" => {
                if let [width, height] = numbers()[..] {
                    art_size = Some((width, height));
                }
            }
            _ => {}
        }
    }
    if remembered.card.title.is_empty() {
        return None;
    }
    if let Some((width, height)) = art_size {
        let pixels = fs::read(dir.join(ART_FILE)).ok();
        if let Some(pixels) = pixels.filter(|p| p.len() == width as usize * height as usize * 4) {
            remembered.card.art = Some(ArtImage {
                id: RESTORED_ART_ID,
                width,
                height,
                pixels: pixels.into(),
            });
        }
    }
    Some(remembered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("islet-last-music-{}", std::process::id()));
        let card = MediaCard {
            title: "Midnight\tCity".to_owned(),
            artist: "M83".to_owned(),
            source: "Spotify".to_owned(),
            playing: true,
            timeline: None,
            art: Some(ArtImage {
                id: 7,
                width: 2,
                height: 1,
                pixels: vec![1, 2, 3, 4, 5, 6, 7, 8].into(),
            }),
            accent: Some([10, 20, 30]),
            scrub: None,
            volume: Some(0.5),
        };
        let remembered = Remembered::from_live("Spotify.exe", &card);
        assert!(!remembered.card.playing && remembered.card.volume.is_none());
        save(&dir, &remembered);
        let mut loaded = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            loaded = load(&dir);
            if loaded.as_ref().is_some_and(|r| r.card.art.is_some()) {
                break;
            }
        }
        let loaded = loaded.expect("saved");
        assert_eq!(loaded.app_id, "Spotify.exe");
        assert_eq!(loaded.card.title, "Midnight City");
        assert_eq!(loaded.card.accent, Some([10, 20, 30]));
        assert_eq!(&loaded.card.art.unwrap().pixels[..], &[1, 2, 3, 4, 5, 6, 7, 8]);
        let _ = fs::remove_dir_all(&dir);
    }
}
