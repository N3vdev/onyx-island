//! Platform-free domain logic for Islet.
//!
//! Everything in this crate must be safe, testable, and independent of Win32. The
//! renderer, input hooks, and other platform layers are consumers of these types.

pub mod config;
pub mod geometry;
pub mod island;
pub mod spring;

pub use config::Config;
pub use geometry::Rect;
pub use island::{ContentCard, ContentSnapshot, IslandEvent, IslandState, LayoutTarget, PillSize};
pub use spring::{Spring, SpringParams};
