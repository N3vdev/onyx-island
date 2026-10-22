//! Audio capture and playback for Islet.
//!
//! P2 provides the WASAPI loopback visualizer. Mic capture (P3) and the
//! keysound mixer (P5) will live here too.

pub mod loopback;

pub use loopback::Loopback;
