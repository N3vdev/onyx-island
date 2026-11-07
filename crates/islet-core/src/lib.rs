//! Platform-free domain logic for Islet.
//!
//! Everything in this crate must be safe, testable, and independent of Win32. The
//! renderer, input hooks, and other platform layers are consumers of these types.

pub mod command;
pub mod config;
pub mod converter;
pub mod geometry;
pub mod handoff;
pub mod hotkey;
pub mod learn;
pub mod island;
pub mod media;
pub mod music;
pub mod spring;
pub mod transcript;
pub mod tween;
pub mod widgets;

pub use config::Config;
pub use geometry::Rect;
pub use island::{ContentCard, ContentSnapshot, IslandEvent, IslandState, LayoutTarget, PillSize};
pub use spring::{Spring, SpringParams};
pub use tween::{Ease, Tween};
