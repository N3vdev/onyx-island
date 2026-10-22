//! Prints now-playing updates until Enter is pressed. Used to verify GSMTC
//! against real apps (Spotify, browsers, Media Player).

use islet_media::{MediaCommand, MediaService};

fn main() {
    let service = MediaService::start(|info| match info {
        Some(info) => println!(
            "[{}] {} - {} | playing={} {:.1}/{:.1}s rate={} art={} accent={:?}",
            info.source,
            info.title,
            info.artist,
            info.playing,
            info.position,
            info.duration,
            info.rate,
            info.art.as_ref().map_or("none".to_owned(), |art| format!(
                "{}x{} #{}",
                art.width, art.height, art.id
            )),
            info.accent,
        ),
        None => println!("(nothing playing)"),
    })
    .expect("start media thread");

    // `--for <seconds>`: just watch, for scripted runs without a console.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--for") {
        let seconds = args
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(5);
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        return;
    }

    println!("commands: p = play/pause, n = next, b = previous, s<sec> = seek, empty = quit");
    let mut line = String::new();
    loop {
        line.clear();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let command = match line.trim() {
            "" => break,
            "p" => MediaCommand::TogglePlayPause,
            "n" => MediaCommand::Next,
            "b" => MediaCommand::Previous,
            other => match other.strip_prefix('s').and_then(|s| s.parse().ok()) {
                Some(seconds) => MediaCommand::Seek(seconds),
                None => continue,
            },
        };
        service.send(command);
    }
}
