//! Audio capture and playback for Islet.
//!
//! P2 provides the WASAPI loopback visualizer. Mic capture (P3) and the
//! keysound mixer (P5) will live here too.

pub mod loopback;
pub mod volume;

pub use loopback::Loopback;
pub use volume::VolumeHandle;
