//! Trial + purchased-license state: a signed token issued by a Supabase edge
//! function (after `start-trial` or a completed Stripe checkout), verified
//! **offline** here against an embedded Ed25519 public key. The private
//! signing key never touches this binary or this repo — only the edge
//! function that issues tokens holds it (see `supabase/functions/`).
//!
//! Token shape: `base64(payload_json).base64(signature)`, verified with
//! `VerifyingKey::verify_strict` (the malleability-safe variant). Not JWT:
//! both ends of this token are code we write, so JWT's interop value is
//! moot — skipping it avoids picking up algorithm-negotiation machinery
//! (RSA/ECDSA/HMAC parsers) this app will never use, for a token whose Rust
//! side only ever knows how to verify one thing.
//!
//! Email normalization/hashing is deliberately NOT duplicated here — it
//! lives server-side (Deno), the single source of truth for the
//! trials/licenses tables, so there's no way for a Rust-side copy to drift
//! from what the backend actually enforces.
//!
//! Cached at `%LOCALAPPDATA%\com.circlesearch.app\license.token` — a
//! dedicated file, not inside `settings.json`, for the same reason
//! `entitlements.rs` keeps plan state out of `Settings`: that file is
//! WebView-writable and hand-editable JSON, and must never be a path to
//! forging Pro.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::entitlements::Entitlements;
use crate::state::AppState;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The license-signing public key. Its private half lives outside the repo
/// (the owner's `%USERPROFILE%\.circlesearch\license-signing-key.txt`) and
/// is the key to give the Supabase edge functions as `LICENSE_SIGNING_KEY`
/// — generating a new pair would invalidate every token already issued.
/// An all-zero key still fails closed (see `is_placeholder_key`).
const PUBLIC_KEY: [u8; 32] = [
    97, 133, 118, 154, 209, 33, 187, 125, 33, 253, 71, 76, 204, 94, 29, 27, 227, 31, 197, 164, 181, 21, 123, 192, 183,
    177, 61, 37, 19, 107, 120, 247,
];

fn is_placeholder_key() -> bool {
    PUBLIC_KEY == [0; 32]
}

/// Base URL for the Supabase edge functions, e.g.
/// `https://xxxxxxxx.supabase.co/functions/v1`. Overridable via
/// `CIRCLE_SEARCH_LICENSE_API` so the real project URL can be wired in
/// without a rebuild. TODO(you): set the env var, or hardcode the real URL
/// here once the Supabase project exists.
fn api_base() -> String {
    std::env::var("CIRCLE_SEARCH_LICENSE_API")
        .unwrap_or_else(|_| "https://YOUR-PROJECT.supabase.co/functions/v1".to_string())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Trial,
    License,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub kind: Kind,
    /// Opaque, server-computed — not recomputed or compared against
    /// anything locally; kept only in case a future support/debug view
    /// wants to show which account a token belongs to.
    pub email_hash: String,
    pub issued_at: i64,
    /// Trials always have one (issued_at + 7d). Licenses carry a modest
    /// soft-refresh window too, even though the purchase itself is
    /// lifetime — silently refreshed from the backend while online, so a
    /// refund/dispute can actually lapse a cached token without the app
    /// ever needing to be online at time of use. See the plan for why.
    pub expires_at: i64,
}

impl Payload {
    pub fn is_expired(&self) -> bool {
        now() >= self.expires_at
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Verifies a token's signature and structure. Returns `None` on any
/// failure — malformed, wrong signature, or the public key is still the
/// unset placeholder — which callers treat exactly like "no token" (Free).
/// Does not check expiry itself; callers decide what "expired" means for
/// their purpose (entitlements.rs treats it as Free, the Settings UI shows
/// a different message for "expired" than "never had one").
pub fn verify(token: &str) -> Option<Payload> {
    if is_placeholder_key() {
        return None;
    }
    let (payload_b64, sig_b64) = token.trim().split_once('.')?;
    let payload_bytes = B64.decode(payload_b64).ok()?;
    let sig_bytes: [u8; 64] = B64.decode(sig_b64).ok()?.try_into().ok()?;
    let key = VerifyingKey::from_bytes(&PUBLIC_KEY).ok()?;
    key.verify_strict(&payload_bytes, &Signature::from_bytes(&sig_bytes)).ok()?;
    serde_json::from_slice(&payload_bytes).ok()
}

fn token_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|err| format!("No local data folder: {err}"))?;
    std::fs::create_dir_all(&dir).map_err(|err| format!("Couldn't create the app data folder: {err}"))?;
    Ok(dir.join("license.token"))
}

/// Loads and verifies the cached token, if any. Never fails outward — a
/// missing or corrupt cache just means Free, the same as a fresh install.
pub fn load_cached(app: &AppHandle) -> Option<Payload> {
    let path = token_path(app).ok()?;
    let token = std::fs::read_to_string(path).ok()?;
    verify(&token)
}

fn save_cached(app: &AppHandle, token: &str) -> Result<(), String> {
    std::fs::write(token_path(app)?, token).map_err(|err| format!("Couldn't save the license: {err}"))
}

/// Whether the "your trial ended" paywall has already been auto-opened once.
/// An empty marker file next to the token cache — existence is the whole
/// signal, so a purchase (which makes the trial irrelevant) or a deleted
/// `%LOCALAPPDATA%` naturally resets it rather than needing separate
/// cleanup logic. Used by `lib.rs`'s `.setup()` so expiry opens Settings
/// once, not on every subsequent launch.
pub fn expiry_popup_already_shown(app: &AppHandle) -> bool {
    token_path(app).map(|path| path.with_file_name("trial-expiry-shown")).is_ok_and(|marker| marker.exists())
}

pub fn mark_expiry_popup_shown(app: &AppHandle) {
    if let Ok(path) = token_path(app) {
        let _ = std::fs::write(path.with_file_name("trial-expiry-shown"), "");
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    /// `None` (or the field simply absent) means the server positively
    /// confirmed there is nothing for this email — not a network/server
    /// failure, which is a distinct `Err` from this function entirely. That
    /// distinction matters: callers must NOT overwrite a perfectly good
    /// cached token just because a *different* (e.g. mistyped) email came
    /// back empty.
    token: Option<String>,
}

#[derive(Deserialize)]
struct CheckoutResponse {
    #[serde(rename = "checkoutUrl")]
    checkout_url: String,
}

/// Blocking; every call site wraps this in `spawn_blocking`, matching the
/// existing pattern in `translate.rs` for the same reason — the WebView
/// only ever talks to Rust, never the network directly.
fn call_for_token(path: &str, email: &str) -> Result<Option<String>, String> {
    let response: TokenResponse = ureq::post(&format!("{}/{path}", api_base()))
        .set("Content-Type", "application/json")
        .send_json(ureq::json!({ "email": email }))
        .map_err(|err| format!("Could not reach the license server: {err}"))?
        .into_json()
        .map_err(|err| format!("Unexpected response from the license server: {err}"))?;
    Ok(response.token)
}

/// So the UI reflects a trial start/purchase immediately, without needing
/// an app restart.
fn refresh_state(app: &AppHandle, _state: &AppState) -> Entitlements {
    crate::entitlements::refresh(app)
}

/// Starts (or, for a returning email, re-confirms) the 7-day trial.
/// Idempotent server-side: a second call with the same email gets back the
/// *original* `issuedAt`, not a fresh one — that idempotency, not the token
/// signing, is what actually makes wiping `%APPDATA%` not grant a new trial.
#[tauri::command]
pub async fn start_trial(app: AppHandle, state: tauri::State<'_, AppState>, email: String) -> Result<Entitlements, String> {
    let app_for_worker = app.clone();
    let token = tokio::task::spawn_blocking(move || call_for_token("start-trial", &email))
        .await
        .map_err(|err| err.to_string())??
        .ok_or_else(|| "The license server didn't start a trial. Please try again.".to_string())?;
    save_cached(&app_for_worker, &token)?;
    Ok(refresh_state(&app_for_worker, &state))
}

/// Creates a Stripe Checkout Session for `email` and remembers it as the
/// pending purchase `poll_purchase_status` checks. Returns the checkout URL
/// for the Settings window to open (via `@tauri-apps/plugin-opener`, same
/// as every other external link this app opens) — deliberately not opened
/// from Rust, so the WebView controls exactly when the user's browser gets
/// focus-stolen.
#[tauri::command]
pub async fn begin_purchase(state: tauri::State<'_, AppState>, email: String) -> Result<String, String> {
    *state.pending_purchase_email.lock().unwrap() = Some(email.clone());
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        let response: CheckoutResponse = ureq::post(&format!("{}/create-checkout-session", api_base()))
            .set("Content-Type", "application/json")
            .send_json(ureq::json!({ "email": email }))
            .map_err(|err| format!("Could not reach the checkout server: {err}"))?
            .into_json()
            .map_err(|err| format!("Unexpected response from the checkout server: {err}"))?;
        Ok(response.checkout_url)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// One bounded poll round against `check-license`, using whatever email
/// `begin_purchase` last remembered. The Settings window owns the interval
/// and its lifetime (cleared when the Plan section is left or the window
/// closes) — there's deliberately no Rust-side timer that could outlive the
/// paywall UI and violate the idle-CPU non-negotiable. A call with no
/// purchase in progress is a harmless no-op, not an error, so the UI's
/// interval never needs special-case handling.
#[tauri::command]
pub async fn poll_purchase_status(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<Entitlements, String> {
    let Some(email) = state.pending_purchase_email.lock().unwrap().clone() else {
        return Ok(state.entitlements());
    };
    let app_for_worker = app.clone();
    let token = tokio::task::spawn_blocking(move || call_for_token("check-license", &email)).await.map_err(|err| err.to_string())??;
    // No token yet just means checkout hasn't completed — leave the cache
    // (and current entitlements) exactly as they were and let the next
    // poll try again, rather than treating "not yet" as "nothing".
    let Some(token) = token else { return Ok(state.entitlements()) };
    save_cached(&app_for_worker, &token)?;
    let entitlements = refresh_state(&app_for_worker, &state);
    if entitlements.plan != crate::entitlements::Plan::Free {
        *state.pending_purchase_email.lock().unwrap() = None;
    }
    Ok(entitlements)
}

/// Recovers a trial or a purchased license for `email` on this device —
/// used both by "Restore purchase" after a reinstall and as the manual
/// fallback if purchase polling was closed before it caught completion.
/// Same endpoint as polling; the server, not the client, decides what
/// state that email is actually in.
///
/// Deliberately leaves any existing cached token untouched when the server
/// has nothing for this email — a mistyped email here must never be able
/// to clobber a perfectly good trial/license already cached for the
/// account the user actually meant.
#[tauri::command]
pub async fn restore_purchase(app: AppHandle, state: tauri::State<'_, AppState>, email: String) -> Result<Entitlements, String> {
    let app_for_worker = app.clone();
    let token = tokio::task::spawn_blocking(move || call_for_token("check-license", &email)).await.map_err(|err| err.to_string())??;
    let Some(token) = token else { return Ok(state.entitlements()) };
    save_cached(&app_for_worker, &token)?;
    Ok(refresh_state(&app_for_worker, &state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    fn sign(signing: &SigningKey, payload: &Payload) -> String {
        let json = serde_json::to_vec(payload).unwrap();
        let signature = signing.sign(&json);
        format!("{}.{}", B64.encode(&json), B64.encode(signature.to_bytes()))
    }

    fn test_payload(expires_in_secs: i64) -> Payload {
        Payload { kind: Kind::Trial, email_hash: "abc".into(), issued_at: now(), expires_at: now() + expires_in_secs }
    }

    // These tests exercise the decode + signature check against a LOCAL
    // test keypair (the real private key never touches the repo), proving
    // the token format and signature logic independently of PUBLIC_KEY.
    #[test]
    fn accepts_a_correctly_signed_token() {
        let signing = SigningKey::generate(&mut OsRng);
        let payload = test_payload(3600);
        let token = sign(&signing, &payload);

        // verify() checks against the embedded PUBLIC_KEY, so exercise the
        // decode+signature-check path it uses against our test key.
        let (payload_b64, sig_b64) = token.split_once('.').unwrap();
        let payload_bytes = B64.decode(payload_b64).unwrap();
        let sig_bytes: [u8; 64] = B64.decode(sig_b64).unwrap().try_into().unwrap();
        let verifying = signing.verifying_key();
        assert!(verifying.verify_strict(&payload_bytes, &Signature::from_bytes(&sig_bytes)).is_ok());
        let decoded: Payload = serde_json::from_slice(&payload_bytes).unwrap();
        assert_eq!(decoded.kind, Kind::Trial);
    }

    #[test]
    fn rejects_a_tampered_payload() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = sign(&signing, &test_payload(3600));
        let (_, sig_b64) = token.split_once('.').unwrap();
        let mut tampered_payload = test_payload(3600);
        tampered_payload.kind = Kind::License; // attacker tries to upgrade a trial token
        let tampered = format!("{}.{}", B64.encode(serde_json::to_vec(&tampered_payload).unwrap()), sig_b64);

        let (payload_b64, sig_b64) = tampered.split_once('.').unwrap();
        let payload_bytes = B64.decode(payload_b64).unwrap();
        let sig_bytes: [u8; 64] = B64.decode(sig_b64).unwrap().try_into().unwrap();
        let verifying = signing.verifying_key();
        assert!(verifying.verify_strict(&payload_bytes, &Signature::from_bytes(&sig_bytes)).is_err());
    }

    #[test]
    fn verify_rejects_a_token_signed_by_any_other_key() {
        let signing = SigningKey::generate(&mut OsRng);
        let token = sign(&signing, &test_payload(3600));
        // Well-formed and correctly signed — by a key that isn't ours.
        assert!(verify(&token).is_none());
    }

    #[test]
    fn malformed_tokens_are_rejected_without_panicking() {
        for bad in ["", "no-dot-here", "abc.", ".abc", "!!!.!!!"] {
            assert!(verify(bad).is_none(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn is_expired_reflects_expires_at() {
        assert!(!test_payload(3600).is_expired());
        assert!(test_payload(-1).is_expired());
    }

    /// Checks the license installed on this machine against the embedded
    /// key: `cargo test --lib -- --ignored installed_license`.
    #[test]
    #[ignore = "reads this machine's installed license"]
    fn installed_license_verifies() {
        let Some(dir) = std::env::var_os("LOCALAPPDATA") else { return };
        let path = std::path::Path::new(&dir).join("com.circlesearch.app").join("license.token");
        let token = std::fs::read_to_string(path).expect("an installed license");
        let payload = verify(&token).expect("signed by our key");
        assert_eq!(payload.kind, Kind::License);
        assert!(!payload.is_expired());
    }
}
