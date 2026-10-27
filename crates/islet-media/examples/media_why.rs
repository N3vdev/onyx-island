//! Prints every media session and why the music filter shows or hides it.
//! Run while the track in question is playing:
//! `cargo run -p islet-media --example media_why`

fn main() {
    let config = islet_core::Config::default();
    let filter = islet_core::music::MusicFilter::new(config.media.music_only, &config.media.music_sites);
    for line in islet_media::explain_sessions(&filter) {
        println!("{line}\n");
    }
}
