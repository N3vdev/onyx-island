//! Runs the loopback visualizer briefly and prints the bar levels, to check
//! WASAPI initialisation on the current default output device.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use islet_audio::Loopback;
use islet_core::media::BarLevels;

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

fn main() {
    let levels = Arc::new(BarLevels::default());
    let _loopback = Loopback::start(Arc::clone(&levels), now_ms).expect("start loopback");
    let mut fresh = 0;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        match levels.load_fresh(now_ms(), 250) {
            Some(bars) => {
                fresh += 1;
                println!("{bars:.2?}");
            }
            None => println!("(no levels)"),
        }
    }
    println!("fresh samples: {fresh}/20");
}
