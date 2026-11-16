# Onyx Lens — CLAUDE.md

Onyx's screen search (formerly the standalone Trace Lens / Circle Search).
Rust owns capture, OCR, geometry, and state. The WebView only renders.

## Part of Onyx

- This is `onyx-lens.exe`, a helper next to `islet.exe`. It has its own
  Cargo workspace (Tauri, `windows` 0.58, unwinding panics), separate from
  Onyx's. Build it with `tools/build-lens.ps1` from the repo root.
- Onyx starts it (when Settings > Lens is on), keeps it in its kill-on-close
  job, and owns the shortcut (`[lens] hotkey` in Onyx's config.toml). A press
  sets the named event `Local\Onyx.Lens.Capture`; `hotkey.rs` waits on it.
  `Local\Onyx.Lens.Settings` opens this app's settings window.
- Onyx's island is the overlay's toolbar when it is on the captured screen
  (`island.rs`): it glides to the top and shows the tools. The overlay's own
  `#tool-bar` stays in the DOM, hidden by `body.island-tools`, as the state
  the island mirrors (`island_tools` posts a registered message with the
  visible/pressed masks); clicks come back to the trigger thread as a thread
  message and on as the `island-tool` event. Onyx hides the island from
  screen capture for the press, so the frozen frame doesn't show it.
- The overlay's bars wear the island's look (`overlay-controls.css` tokens):
  black, a 0.17 white hairline, pill shapes, Inter, the island's lavender
  for pressed and primary buttons. With Onyx's Glass look (read from its
  config.toml on each capture, `island::look`), they get its tint, sheen and
  top rim, frosted with `backdrop-filter` over the frozen frame while
  Windows' transparency effects are on.
- No tray icon, autostart or global shortcut here. The settings window hides
  its Shortcut, Start with Windows and Plan sections.
- Every feature is unlocked: `entitlements::resolve` returns Pro until Onyx
  Pro decides it. `license.rs` and the plan UI are kept for that.
- Data stays under the old identifier `com.circlesearch.app`, so existing
  settings and downloaded models carry over.

## Non-negotiables

| Metric | Target | How to verify |
|---|---|---|
| Idle RAM | < 50 MB | Task Manager, app in tray 10 min |
| Idle CPU | ~0% | No timers, no polling loops |
| Hotkey → overlay visible | < 100 ms | `tracing` span, logged every run |
| OCR after Select Text | < 300 ms for a typical lasso | `tracing` span |
| Lasso release → action choices | < 100 ms | `tracing` span |

Rules:
- Rust owns capture, OCR, geometry, and state. The WebView renders and hosts
  native text selection only.
- No React, no UI framework, no CSS framework. Vanilla TS + canvas + DOM.
- The overlay window is created at startup and kept hidden. Never created on hotkey.
- Capture completes **before** the overlay is shown. Non-negotiable ordering.
- Capture only the monitor under the cursor.
- Every timing target above is instrumented from day one, not retrofitted.

## Stack

| Concern | Choice |
|---|---|
| Shell | Tauri 2.0 |
| Core | Rust |
| Overlay | Tauri WebView, vanilla TS |
| Capture | GDI copy of the monitor (~20 ms); `windows-capture` (Windows Graphics Capture) as fallback when that comes back black |
| OCR | `Windows.Media.Ocr` via `windows` crate |
| Hotkey | Onyx (`RegisterHotKey`), signalled through a named event |
| Geometry | `geo` crate |
| Logging | `tracing` + `tracing-subscriber` |

## State machine

Both Rust and TS follow this.

```
IDLE
  └─ hotkey ──► CAPTURING
                  │  (monitor lookup → WGC single frame)
                  ▼
                ARMED ──────────────► overlay shown, frozen frame painted
                  │                   no OCR or timers run
                  ▼
                DRAWING ────────────► user drags; lasso path accumulates
                  │
                  ▼
                CHOOSING ───────────► Select Text · Translate · Google Lens · Image to AI · Save
                  │
                  ▼
                TEXT MODE ──────────► selected-region OCR on a background worker
                  │                   spans materialized; no text actions yet
                  ▼
                TEXT SELECTED ──────► native drag selects text; Copy · Translate here · Search · Ask AI appear
                  │
                  ├─ Esc / click-away ──► IDLE (drop frame, clear OCR results)
                  └─ action ────────────► execute, then IDLE
```

OCR never runs while idle. Once the overlay is visible, the whole frame is
read once in the background ("live text": drag across text anywhere to
select it, no lasso first) alongside a QR scan (`screenlinks.rs`); the
overlay never waits for either, and a lasso started before they finish takes
over. **Select Text** still runs its own region OCR for a drawn selection.

Translation (Translate / Translate here) is on-device via `fxtranslate`
(Firefox Translations models, downloaded once per language into the app's
local data folder). The model is loaded on first use in a session and
dropped when the overlay is dismissed — never held while idle.

Plans: `entitlements.rs` is the only source of Free/Pro/Team and has no
`#[tauri::command]` setter; never store plan state in `Settings`. Paid
features (custom actions, visual pack, JPEG/WebP export, Redact, Designer
toolkit, Edit / snap-to-edges) fall back to free behavior on Free without
deleting the user's data. Any paid feature with a Rust command must also
refuse on Free in that command, not only hide its button. Plan changes
reach the overlay via the `entitlements-changed` event
(`entitlements::refresh`). Debug builds honor `CIRCLE_SEARCH_DEV_PLAN=pro`
for testing.

Pro trial/purchase: a 7-day trial, then a one-time lifetime purchase via
Stripe, gates the same Pro features above. `license.rs` verifies a signed
Ed25519 token fully offline against an embedded public key — the private
key never touches this repo, only a Supabase edge function does (see
`supabase/`, not yet deployed; `supabase/README.md` has the exact setup
steps). The trial is anchored server-side (idempotent per email), not on
local state, specifically so wiping `%LOCALAPPDATA%` can't grant a second
one. `AppState.entitlements` is a `Mutex` (not a plain field) for this
reason — trial-starting/purchasing changes it mid-session — but the
invariant that matters is unchanged: `set_entitlements` is a plain Rust
method, never a command, so there is still no IPC path to it.

Edit mode's object network (`objectnet.rs`): U²-Net-P (4.6 MB, Apache-2.0,
by Qin et al.) runs through Windows' built-in Windows ML on the CPU, so
nothing is bundled. It downloads on first Edit-mode use into the app's local
data folder, is checked against a pinned SHA-256, and only loads during a
snap. It is a *prior* for `snap.rs`; with no model (offline, failure) the
colour-only snapping runs unchanged.
Before that cut-out wins, `rectsnap::frame_in_loop` looks for a frame in the
loop (a window, card or panel: straight sides that stop inside the loop and
meet at real corners); `snap::snap_outline` takes the frame unless the
cut-out found an object sitting free inside it. The colour model alone cuts
dark windows on dark desktops to a title-bar strip. `MODEL_URL` is a third-party release
asset — mirror it on the project's own storage before shipping widely.

## Build order

See `../circle-to-search-plan.md` in the repo root for full phase details,
acceptance criteria, and gotchas. Work one phase at a time; do not skip ahead.

1. Shell + capture + overlay (no OCR)
2. OCR + selected-region dispatch
3. Selection intelligence (layout, hit-test, reading order, test harness)
4. Selectable text (span layer, native selection, Copy action)
5. Action system (select text → copy/translate/ask AI; Lens; image to ChatGPT)
