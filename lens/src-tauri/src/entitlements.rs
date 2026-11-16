//! Which plan this install is on, and therefore which paid features it may
//! use. Deliberately separate from `Settings`: `set_settings` accepts any
//! settings blob from a WebView and settings.json is hand-editable, so a plan
//! stored there could be granted by anyone. There is no command that writes
//! `Entitlements` — it is only ever derived on the Rust side, from either the
//! debug-only env override below or a verified signed token (see
//! `license.rs`).

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    #[default]
    Free,
    Pro,
    // Not issued by anything yet (trial/license tokens only ever carry
    // Trial/License, mapped to Pro below) — kept for a future team plan
    // without another enum migration.
    #[allow(dead_code)]
    Team,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Entitlements {
    pub plan: Plan,
}

/// What the Plan section of Settings actually renders — richer than
/// `Entitlements` (which is just "what does the gating code check"), since
/// the UI needs to distinguish "never started a trial" from "trial expired"
/// even though both currently mean `Plan::Free`.
// Only `Licensed` is used inside Onyx for now; the rest stay for Onyx Pro.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum LicenseStatus {
    NoTrial,
    Trialing { days_left: i64 },
    TrialExpired,
    Licensed,
    /// A previously-licensed token's soft-refresh window lapsed (device was
    /// offline too long) — distinct from `TrialExpired` so the UI doesn't
    /// show a paying customer "start your trial" instead of "reconnect to
    /// refresh your license".
    NeedsRefresh,
}

/// Debug builds accept `CIRCLE_SEARCH_DEV_PLAN=pro|team` so paid features can
/// be exercised without a real trial/purchase. Release builds ignore it
/// entirely; anything else falls through to the cached, verified token.
pub fn resolve(_app: &AppHandle) -> Entitlements {
    // Inside Onyx every feature is unlocked for now. Onyx Pro will decide
    // this later; this function stays the single place it is decided.
    Entitlements { plan: Plan::Pro }
}

/// Re-derives the plan from disk and, if it changed, stores it and tells
/// every window via `entitlements-changed` — the overlay otherwise only
/// reads it once at startup, so a trial started (or expired) mid-session
/// would never reach it until a restart.
pub fn refresh(app: &AppHandle) -> Entitlements {
    let fresh = resolve(app);
    let state = app.state::<AppState>();
    if state.entitlements().plan != fresh.plan {
        state.set_entitlements(fresh.clone());
        if let Err(err) = app.emit("entitlements-changed", &fresh) {
            tracing::warn!(%err, "could not broadcast entitlements change");
        }
    }
    fresh
}

pub fn license_status(_app: &AppHandle) -> LicenseStatus {
    // See `resolve`: no trial or purchase inside Onyx yet.
    LicenseStatus::Licensed
}

#[tauri::command]
pub fn get_entitlements(state: tauri::State<AppState>) -> Entitlements {
    state.entitlements()
}

#[tauri::command]
pub fn get_license_status(app: AppHandle) -> LicenseStatus {
    license_status(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_serializes_as_a_lowercase_string() {
        let json = serde_json::to_string(&Entitlements { plan: Plan::Pro }).unwrap();
        assert_eq!(json, r#"{"plan":"pro"}"#);
    }
}
