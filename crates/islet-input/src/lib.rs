//! Keyboard input for Islet: the global hotkey hook and text insertion.
//!
//! Everything here is Windows-only. The decisions (which combo means what)
//! live in `islet-core::hotkey`; this crate only talks to the OS.

pub mod hook;
pub mod inject;

pub use hook::{HookEvent, KeyboardHook};
pub use inject::{
    foreground_exe, foreground_is_blocked_by_elevation, insert_text, suppress_start_menu,
    InjectError, InsertReport, Method,
};

/// Marks every key event Islet injects (`dwExtraInfo`), so the hook ignores
/// them and the paste never triggers its own hotkeys.
pub const EXTRA_INFO: usize = 0x1514_E700;
