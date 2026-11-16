//! For each media session: which app, whether it hands Windows a thumbnail,
//! how big, and whether Onyx can decode it. Then what Onyx's own service
//! makes of the players (art or not, and by which path). Prints no titles.
//! `cargo run -p islet-media --example art_probe`

use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;
use windows::Storage::Streams::{DataReader, InputStreamOptions};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

fn main() -> windows::core::Result<()> {
    // SAFETY: this thread's apartment, for WinRT and WIC.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let manager = Manager::RequestAsync()?.join()?;
    let sessions = manager.GetSessions()?;
    for index in 0..sessions.Size()? {
        let session = sessions.GetAt(index)?;
        let app = session.SourceAppUserModelId()?.to_string_lossy();
        let properties = session.TryGetMediaPropertiesAsync()?.join()?;
        let has_title = !properties.Title()?.is_empty();
        let thumbnail = match properties.Thumbnail() {
            Err(error) => format!("no thumbnail ({error})"),
            Ok(reference) => match reference.OpenReadAsync().and_then(|op| op.join()) {
                Err(error) => format!("thumbnail won't open ({error})"),
                Ok(stream) => {
                    let size = stream.Size().unwrap_or(0) as u32;
                    let reader = DataReader::CreateDataReader(&stream)?;
                    reader.SetInputStreamOptions(InputStreamOptions::ReadAhead)?;
                    let loaded = reader.LoadAsync(size)?.join()?;
                    let mut bytes = vec![0u8; loaded as usize];
                    reader.ReadBytes(&mut bytes)?;
                    let decoded = islet_media::decode_image(&bytes)
                        .map_or("does NOT decode".to_owned(), |art| format!("decodes to {}x{}", art.width, art.height));
                    format!("thumbnail {size} bytes, {decoded}")
                }
            },
        };
        println!("session {app} | title reported: {has_title} | {thumbnail}");
    }

    // What Onyx's service shows, with the default music filter.
    let config = islet_core::Config::default();
    let filter = islet_core::music::MusicFilter::new(config.media.music_only, &config.media.music_sites);
    for line in islet_media::explain_sessions_privately(&filter) {
        println!("{line}");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let _service = islet_media::MediaService::start(filter, move |players| {
        let lines: Vec<String> = players
            .iter()
            .map(|info| {
                let path = if info.app_id.contains('#') { "found as a browser tab" } else { "from its media session" };
                let art = info.art.as_ref().map_or("NO art".to_owned(), |art| format!("art {}x{}", art.width, art.height));
                format!("shown: {} ({path}) | {art}", info.source)
            })
            .collect();
        let _ = tx.send(lines);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    let mut last = Vec::new();
    while let Ok(lines) = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
        last = lines;
    }
    if last.is_empty() {
        println!("shown: nothing");
    }
    for line in last {
        println!("{line}");
    }
    Ok(())
}
