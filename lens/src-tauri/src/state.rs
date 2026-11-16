//! AppState and the cached capture for one overlay session.
//! See CLAUDE.md for the full state machine diagram.

use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Armed,
}

/// The captured frame is kept only for the active overlay. OCR is deliberately
/// not cached or started here: it runs only after the user chooses Select Text.
#[derive(Clone)]
pub struct FrameCache {
    // Region OCR, QR detection and snapping share the compressed frame.
    // Cloning a worker snapshot must not copy the whole screenshot.
    pub png_bytes: Arc<Vec<u8>>,
    pub dpi_scale: f64,
    /// The captured monitor's top-left corner, in virtual-desktop physical
    /// pixels (`MonitorInfo::bounds`). `pin.rs`'s `create_pin` adds a
    /// selection's frame-relative x/y to this to get a real screen
    /// coordinate for the pin window. Can be negative on a monitor
    /// above/left of the primary one.
    pub origin_x: i32,
    pub origin_y: i32,
}

struct Session {
    state: SessionState,
    /// Bumped on every activation so the web UI can ignore an older result.
    token: u64,
    frame_cache: Option<FrameCache>,
}

pub struct AppState {
    session: Mutex<Session>,
    settings: Mutex<crate::settings::Settings>,
    // A trial starting or a purchase completing changes this *during* a
    // running session now (see license.rs), so unlike Phase A this can no
    // longer be a plain write-once field. The invariant that actually
    // matters — the WebView can never set this itself — is preserved a
    // different way: `set_entitlements` below is a plain Rust method, not a
    // `#[tauri::command]`, so there is still no IPC path to it. Only
    // license.rs (after verifying a signed token) and this module's own
    // `.setup()`-time initialization call it.
    entitlements: Mutex<crate::entitlements::Entitlements>,
    /// The email `begin_purchase` most recently started a checkout for,
    /// polled by `poll_purchase_status` and cleared once that purchase
    /// resolves. Lives here rather than as a Rust-side timer so the
    /// Settings window's own polling interval is what's actually alive —
    /// this is just the one piece of state it needs across calls.
    pub pending_purchase_email: Mutex<Option<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            session: Mutex::new(Session { state: SessionState::Idle, token: 0, frame_cache: None }),
            // Overwritten with the real persisted value during .setup(),
            // before the hotkey is registered — see lib.rs. Seeded with
            // defaults here only because AppHandle (needed to resolve the
            // config/cache paths) doesn't exist yet at `.manage()` time.
            settings: Mutex::new(crate::settings::Settings::default()),
            entitlements: Mutex::new(crate::entitlements::Entitlements::default()),
            pending_purchase_email: Mutex::new(None),
        }
    }
}

impl AppState {
    /// Transitions IDLE -> ARMED, returning the new session's token on
    /// success. Returns `None` (no-op) if already armed, so a hotkey press
    /// while the overlay is up is ignored rather than starting a second
    /// capture.
    pub fn try_arm(&self) -> Option<u64> {
        let mut guard = self.session.lock().unwrap();
        if guard.state == SessionState::Idle {
            guard.state = SessionState::Armed;
            guard.token += 1;
            Some(guard.token)
        } else {
            None
        }
    }

    pub fn reset(&self) {
        let mut session = self.session.lock().unwrap();
        session.state = SessionState::Idle;
        session.frame_cache = None;
    }

    pub fn is_armed(&self) -> bool {
        self.session.lock().unwrap().state == SessionState::Armed
    }

    pub fn is_active(&self, token: u64) -> bool {
        let session = self.session.lock().unwrap();
        session.state == SessionState::Armed && session.token == token
    }

    /// An old worker must not clear a newer session when it fails.
    pub fn reset_if_active(&self, token: u64) {
        let mut session = self.session.lock().unwrap();
        if session.token == token {
            session.state = SessionState::Idle;
            session.frame_cache = None;
        }
    }

    pub fn set_frame_cache(&self, token: u64, png_bytes: Vec<u8>, dpi_scale: f64, origin_x: i32, origin_y: i32) -> bool {
        let mut session = self.session.lock().unwrap();
        if session.state != SessionState::Armed || session.token != token {
            return false;
        }
        session.frame_cache = Some(FrameCache { png_bytes: Arc::new(png_bytes), dpi_scale, origin_x, origin_y });
        true
    }

    pub fn frame_cache(&self) -> Option<FrameCache> {
        self.session.lock().unwrap().frame_cache.clone()
    }

    pub fn settings(&self) -> crate::settings::Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn set_settings(&self, settings: crate::settings::Settings) {
        *self.settings.lock().unwrap() = settings;
    }

    pub fn entitlements(&self) -> crate::entitlements::Entitlements {
        self.entitlements.lock().unwrap().clone()
    }

    /// Deliberately not a `#[tauri::command]` — see the field's doc comment.
    pub fn set_entitlements(&self, entitlements: crate::entitlements::Entitlements) {
        *self.entitlements.lock().unwrap() = entitlements;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_arm_returns_none_when_already_armed() {
        let state = AppState::default();

        let first = state.try_arm();
        let second = state.try_arm();

        assert!(first.is_some());
        assert!(second.is_none(), "a hotkey press while already armed should be ignored");
    }

    #[test]
    fn try_arm_after_reset_issues_a_new_token() {
        let state = AppState::default();

        let first = state.try_arm().unwrap();
        state.reset();
        let second = state.try_arm().unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn reset_releases_the_captured_frame() {
        let state = AppState::default();
        let token = state.try_arm().unwrap();
        assert!(state.is_armed());
        state.set_frame_cache(token, vec![1, 2, 3], 1.0, 0, 0);
        state.reset();
        assert!(!state.is_armed());
        assert!(state.frame_cache().is_none());
    }

    #[test]
    fn dismissed_capture_cannot_repopulate_the_cache() {
        let state = AppState::default();
        let token = state.try_arm().unwrap();
        state.reset();
        assert!(!state.set_frame_cache(token, vec![1, 2, 3], 1.0, 0, 0));
        assert!(!state.is_active(token));
        assert!(state.frame_cache().is_none());
    }

    #[test]
    fn old_capture_cannot_overwrite_or_cancel_a_new_session() {
        let state = AppState::default();
        let old = state.try_arm().unwrap();
        state.reset();
        let current = state.try_arm().unwrap();
        assert!(state.set_frame_cache(current, vec![4, 5], 1.5, -1920, 0));
        assert!(!state.set_frame_cache(old, vec![1, 2, 3], 1.0, 0, 0));
        state.reset_if_active(old);
        assert!(state.is_active(current));
        let frame = state.frame_cache().unwrap();
        assert_eq!(*frame.png_bytes, vec![4, 5]);
        assert_eq!(frame.origin_x, -1920);
    }

    #[test]
    fn worker_snapshots_share_the_screenshot_and_outlive_dismissal() {
        let state = AppState::default();
        let token = state.try_arm().unwrap();
        state.set_frame_cache(token, vec![1, 2, 3], 1.0, 0, 0);
        let first = state.frame_cache().unwrap();
        let second = state.frame_cache().unwrap();
        assert!(Arc::ptr_eq(&first.png_bytes, &second.png_bytes));
        state.reset();
        assert_eq!(*first.png_bytes, vec![1, 2, 3]);
        assert!(state.frame_cache().is_none());
    }
}
