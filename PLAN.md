# Islet — Dynamic Island for Windows
### Implementation plan (hand-off to Sonnet 5.5)

> Working name: **Islet** (rename freely — every crate/binary/pipe/path uses the `islet` prefix so one find/replace does it).

---

## 0. Product summary

A black, pill-shaped "Dynamic Island" that lives at **bottom-center** of the screen (above the taskbar; top-center selectable in settings). It morphs with spring physics between states:

| # | Feature | What the user sees |
|---|---|---|
| 1 | **Local voice dictation** (Wispr Flow parity, free, offline) | Hold hotkey → island shows live waveform + timer → release → text appears at cursor in any app |
| 2 | **Coding-agent notifications** | Claude Code / Codex / Gemini CLI / Cursor / any script → island expands: "Claude finished · refactor auth.rs" |
| 3 | **Custom keyboard sounds** | Every keypress plays a chosen mechanical-switch sound pack (Mechvibes-compatible) |
| 4 | **Now playing** (iOS parity) | Compact: album art left, animated bars right. Expanded: art, title, artist, scrubber, prev/play/next |
| 5 | **Settings** | Separate lightweight window: hotkeys, model, sound packs, integrations, position, appearance |

**Non-negotiables:** buttery-smooth animation at the display's native refresh (60/120/144/165 Hz), ~0% CPU when idle, tiny RAM when idle, small install.

---

## 1. Stack decision (final)

**Rust, fully native. No WebView, no Electron, no Tauri.**

| Layer | Choice | Why |
|---|---|---|
| Language | Rust (stable, MSVC toolchain) | Zero-GC, predictable frame times, small static binaries |
| Win32 / WinRT | `windows` crate (+ `windows-sys` on hot paths) | Direct access to DComp, D2D, GSMTC, WASAPI, hooks |
| Island rendering | **DirectComposition + Direct2D + DirectWrite** on a flip-model swap chain (`CreateSwapChainForComposition`, premultiplied alpha) | GPU-composited, per-pixel transparency with no layered-window CPU blits, native ClearType-quality text, ~15–25 MB RSS |
| Animation | Own analytic damped-spring solver, render thread paced to compositor vblank | iOS-like motion, frame-rate independent, renders **only while something is moving** |
| Settings UI | `egui`/`eframe` in a **separate process** (`islet-settings.exe`) launched on demand | Easy to build, zero cost when closed (process exits, memory freed) |
| ASR runtime | ONNX Runtime via `ort` crate (CPU EP, INT8) — decided by P0 spike vs `sherpa-onnx` | Parakeet v3 INT8 runs ~80× real-time on CPU |
| ASR model | **NVIDIA Parakeet TDT 0.6B v3**, INT8 ONNX (`istupakov/parakeet-tdt-0.6b-v3-onnx`), ~670 MB, downloaded on first run | Same model family Coucou-style local apps use; 25 languages, built-in punctuation + capitalization. Optional: v2 (English-only, slightly more accurate) |
| VAD | Silero VAD (ONNX, ~2 MB) | Chunk long dictations, trim silence |
| Audio I/O | `cpal` (WASAPI) for mic + keysound output; raw WASAPI loopback for visualizer bars | Low latency, shared mode |
| Decode / resample | `symphonia` (ogg/wav/mp3 packs), `rubato` (mic → 16 kHz) | Pure Rust |
| Media | `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager` (GSMTC) | Works with Spotify desktop, browsers, Apple Music, VLC — no Spotify API key needed |
| IPC | Windows **named pipe** `\\.\pipe\islet-events` | Same pattern Coucou uses; fast, local-only, ACL-restricted to current user |
| Config | TOML at `%APPDATA%\Islet\config.toml`, hot-reloaded (`notify` crate) | Settings process writes, island process reloads |
| Async | **None.** Plain threads + `crossbeam-channel`. No tokio. | Smaller binary, fewer moving parts |
| Logging | `tracing` + rolling file in `%LOCALAPPDATA%\Islet\logs` | **Never log keystrokes or transcripts** (opt-in transcript history only) |
| Packaging | `cargo-packager` (NSIS) or Inno Setup; per-user install, no admin | |

**Rejected:** Tauri/WebView2 (idle ~100 MB+ across WebView processes, CSS animation timing outside our control), Electron (worse), egui for the island itself (immediate-mode redraw loop, weaker text/shape fidelity), Slint (fine, but D2D gives better control over shape morphing and transparency).

---

## 2. Performance budgets (acceptance gates)

| Metric | Budget |
|---|---|
| Idle CPU (island collapsed, no music) | ≤ 0.1% average, 0 wakeups/s from the render thread |
| Idle RAM (model **not** loaded) | ≤ 30 MB private working set |
| RAM with Parakeet loaded | Measure in P0. Expect several hundred MB. Model **unloads after N min idle** (default 10, user-configurable, "keep warm" toggle) |
| Animation | 0 dropped frames over a 30 s state-cycling soak at 60 and 144 Hz (verify with PresentMon) |
| Keypress → sound | ≤ 15 ms end-to-end |
| Release hotkey → text pasted (10 s utterance) | ≤ 400 ms on a mid-range 4-core CPU |
| Agent hook exe | Starts, sends, exits in ≤ 30 ms. **Never blocks the agent** (100 ms pipe timeout, always exit 0) |
| Installer (without model) | ≤ 25 MB |
| Main exe | ≤ 8 MB stripped |

Release profile:
```toml
[profile.release]
lto = "fat"
codegen-units = 1
opt-level = 3
panic = "abort"
strip = true
```
Build with `-C target-feature=+crt-static` (no VC++ redist).

---

## 3. Architecture

```
                         ┌──────────────────────────── islet.exe ─────────────────────────────┐
 Claude/Codex/Gemini/    │                                                                     │
 Cursor hooks ──► islet-hook.exe ──named pipe──► [IPC thread] ──┐                              │
                         │                                      │                              │
 Keyboard ──► [LL hook thread] ──┬─ key events ─► [Keysound audio thread] (cpal, mixer)        │
                         │       └─ hotkey FSM ─► [Dictation controller] ◄─► [ASR worker]      │
                         │                              │   ▲ mic PCM                          │
                         │                              │   └─ [Mic capture thread]            │
                         │                              └─► Text injector (clipboard+SendInput)│
 GSMTC (WinRT events) ──► [Media thread] ──┐                                                   │
 WASAPI loopback ──► [Spectrum thread] ────┤                                                   │
                         │                 ▼                                                   │
                         │        ┌── crossbeam channel: AppEvent ──┐                          │
                         │        ▼                                 │                          │
                         │  [UI thread]  Win32 window + message loop + IslandStateMachine      │
                         │        │ target layout + content                                    │
                         │        ▼                                                            │
                         │  [Render thread]  springs → D2D draw → DComp commit (vblank-paced)  │
                         └─────────────────────────────────────────────────────────────────────┘
   islet-settings.exe (egui) ── writes config.toml ── file watch ──► islet.exe hot-reloads
```

### Cargo workspace

```
islet/
├─ Cargo.toml                 # workspace, shared release profile
├─ PLAN.md  PROGRESS.md
├─ crates/
│  ├─ islet-core/             # PURE logic, no Win32: AppEvent, state machine, layout targets,
│  │                          #   spring solver, config schema, agent-event normalization. Unit-tested.
│  ├─ islet-render/           # DComp/D2D/DWrite device, swap chain, shapes (squircle), text, images, effects
│  ├─ islet-app/              # main exe: window, message loop, thread wiring, tray icon
│  ├─ islet-asr/              # AsrEngine trait, Parakeet impl, Silero VAD, model manager (download/verify/load/unload)
│  ├─ islet-audio/            # mic capture + resample, keysound mixer, loopback spectrum
│  ├─ islet-input/            # WH_KEYBOARD_LL hook, hotkey FSM, text injection, clipboard save/restore
│  ├─ islet-media/            # GSMTC wrapper: sessions, metadata, thumbnail, timeline, controls
│  ├─ islet-agents/           # pipe server, per-agent parsers, config installers (install/uninstall hooks)
│  ├─ islet-hook/             # tiny exe called by agents (<500 KB, no deps beyond std + windows-sys + serde_json)
│  └─ islet-settings/         # egui settings exe
├─ assets/
│  ├─ icons/  sounds/default-pack/  ui/  (CC0 or self-made only)
└─ installer/
```

Rule: **all `unsafe` and Win32 lives in render/audio/input/media/agents/app.** `islet-core` is safe, platform-free, and has tests.

---

## 4. The island: window, rendering, animation

### 4.1 Window
- Styles: `WS_POPUP`, ex: `WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP`. Never steals focus. Not in Alt-Tab or the taskbar.
- Fixed-size window sized to the **largest expanded state**, positioned once per monitor or DPI change. Never resize per frame (resizing causes jank).
- **Hit-testing:** `WM_NCHITTEST` returns `HTTRANSPARENT` outside the current animated pill shape, so clicks pass through to apps beneath.
- Per-monitor DPI v2 (`SetProcessDpiAwarenessContext`). Handle `WM_DPICHANGED` and `WM_DISPLAYCHANGE`.
- Position: bottom-center of the **work area** (`SystemParametersInfo(SPI_GETWORKAREA)` / `MONITORINFO.rcWork`) with an 8 dp gap above the taskbar. Monitor: primary by default; "follow cursor" option.
- Auto-hide while a fullscreen app or game is focused (`SHQueryUserNotificationState` → `QUNS_RUNNING_D3D_FULL_SCREEN` / `QUNS_BUSY`, plus a foreground-window-covers-monitor check). Re-show on exit.
- Re-assert topmost on `WM_WINDOWPOSCHANGING` only when needed (no polling).

### 4.2 Rendering
- D3D11 device (BGRA support) → DXGI device → D2D1 device context → DComp device/target/visual.
- Swap chain: `DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL`, `DXGI_ALPHA_MODE_PREMULTIPLIED`, 2 buffers, frame latency waitable object.
- Pill shape: **continuous-corner squircle** (superellipse-ish cubic path, like iOS), not a plain rounded rect. Cache `ID2D1PathGeometry` per size bucket; rebuild only when size changes.
- Fill: pure black `#000000` (true iOS look) with optional 1 px 6%-white inner hairline (toggle). Optional soft shadow via D2D shadow effect, cached.
- Text: DirectWrite, Segoe UI Variable (fallback Segoe UI), cached `IDWriteTextLayout`s. Marquee for long song titles.
- Album art: decode once via WIC → `ID2D1Bitmap1`, rounded-clipped.
- Content transitions: outgoing content fades out + scales to 0.92 + Gaussian blur 0→8 px. Incoming does the reverse with a 40 ms stagger (iOS morph feel). Blur effect is only active during transitions.

### 4.3 Animation engine (`islet-core::spring`)
- **Analytic damped harmonic oscillator** per animated scalar (width, height, corner radius, y-offset, content opacity, scale, blur). Closed-form position(t) from (from, to, initial velocity, ω, ζ). Exact at any frame rate. Retargeting mid-flight preserves current velocity.
- Presets:
  - `expand`: response 0.42 s, damping ratio 0.78 (slight overshoot)
  - `collapse`: response 0.36 s, damping ratio 0.90
  - `pulse` (notification arrival): response 0.25 s, damping ratio 0.55
- "Settled" when |Δ| < 0.25 px and |v| < 1 px/s → the render thread **parks** (blocks on a condvar). Idle = zero frames.
- Frame pacing: Windows 11 → `DCompositionWaitForCompositorClock`. Windows 10 → swap-chain waitable object + `DwmFlush` fallback. Time source: QPC.
- Render thread owns all D2D resources. UI thread only sends `LayoutTarget` and `ContentSnapshot` structs over a channel.

### 4.4 Sizes (logical dp, scaled by DPI)
| State | W × H | Corner |
|---|---|---|
| Hidden / minimal (nothing active) | 0 or 120 × 6 sliver (setting) | full |
| Compact | 220 × 36 | 18 |
| Compact + split bubble | 220 × 36 + 36 × 36 bubble, 6 dp gap | 18 |
| Expanded notification | 380 × 76 | 30 |
| Expanded media | 400 × 168 | 38 |
| Dictation | 260 × 44 | 22 |

### 4.5 State machine (`islet-core::island`)
Activities, highest priority first:
1. **Dictation** (takes over while active)
2. **Agent alert, needs input** (permission/idle prompt). Sticky until clicked or dismissed.
3. **Agent alert, done/info.** Transient: expand 4 s, then collapse back. Queued, max 3 shown sequentially, rest merged ("+2 more").
4. **Agent running** (spinner dot in the split bubble with agent icon)
5. **Media playing**
6. **Idle**

- Two concurrent persistent activities → **split mode** (main pill + bubble), exactly like iOS.
- Hover 250 ms → expand the current top activity. Mouse leave 400 ms → collapse. Click → action (open/expand/focus terminal).
- Pure function: `fn reduce(state, event, now) -> (state, Vec<Effect>)`. Fully unit-tested.

---

## 5. Feature 1 — Local dictation (Wispr Flow parity)

### 5.1 UX
- **Push-to-talk:** hold `Ctrl+Win` (default; rebindable), speak, release → text inserted at cursor.
- **Hands-free:** `Ctrl+Win+Space` toggles start/stop. Also double-tap the push-to-talk combo.
- **Cancel:** `Esc` while recording discards.
- Island: red dot + live waveform (RMS bars from mic, 30 Hz) + elapsed timer. On release: brief "transcribing" shimmer, then ✓ pulse, then collapse.
- Start/stop tones (soft, short, toggleable).
- Ignore presses shorter than 250 ms (accidental taps).

### 5.2 Pipeline
1. **Mic thread** (`cpal`, default input device, user-selectable). Native rate → `rubato` → 16 kHz mono f32 into a ring buffer. Pre-roll 300 ms kept while armed so first syllables aren't clipped.
2. **Silero VAD** segments speech while recording. Each closed segment (≥ 1.5 s or pause ≥ 600 ms; hard cap 15 s) goes immediately to the ASR worker, so long dictations are mostly transcribed **before** release. After release only the last segment remains.
3. **ASR worker:** `AsrEngine::transcribe(&[f32]) -> Transcript`. Parakeet TDT greedy decode. Output already has punctuation and casing.
4. **Post-processing** (pure, in `islet-core`, unit-tested):
   - Filler removal (`um`, `uh`, `erm`, `you know` at clause starts) — toggle
   - Personal dictionary / replacements (`"claude code" → "Claude Code"`, regex allowed)
   - Snippets: spoken trigger → expansion ("insert my email" → `…`)
   - Spoken punctuation commands: "new line", "new paragraph", "comma" (toggle)
   - Smart spacing/capitalization joining segments
5. **Text injection** (`islet-input::inject`):
   - Default: save clipboard (text + common formats), set text, `SendInput` Ctrl+V, restore clipboard after 250 ms, skip the clipboard-history entry (`ExcludeClipboardContentFromMonitorProcessing` format).
   - Fallback (toggle, or auto for terminals that dislike paste): `SendInput` with `KEYEVENTF_UNICODE`, chunked.
   - Per-app override list (exe name → method).
6. **History** (opt-in, off by default): last 50 transcripts in settings, copyable.

### 5.3 Model manager
- First run: island shows "Download voice model (670 MB)?" → settings opens to the Models page.
- Download from Hugging Face over HTTPS (`ureq`, resumable with Range requests), verify **SHA-256** (pin hashes in code), store in `%LOCALAPPDATA%\Islet\models\parakeet-tdt-0.6b-v3-int8\`.
- Models offered: Parakeet v3 INT8 (default, multilingual), Parakeet v2 INT8 (English). The registry is a static list, so adding more later is trivial.
- Load lazily on first hotkey press (show "warming up" on the island), or eagerly at startup if "keep warm" is on. Unload after idle timeout.
- ORT session: CPU EP, `intra_op_num_threads = min(physical_cores, 4)`, graph optimization level 3. GPU EP (DirectML/WinML) is a **stretch** item, behind a setting, only if P0 shows a real gain.
- Attribution: Parakeet is **CC-BY-4.0**. Credit NVIDIA and the ONNX export author in the About page.

### 5.4 Stretch (P8)
- Optional AI cleanup pass through a **local** OpenAI-compatible endpoint (Ollama/LM Studio), off by default. Wispr's "polish" equivalent, still free and offline.
- Context-aware formatting (detect code editor → no trailing period).

---

## 6. Feature 2 — Coding-agent notifications

### 6.1 `islet-hook.exe`
Tiny CLI invoked by agents. Behaviour:
- `islet-hook <source>`: reads JSON payload from **stdin** (Claude/Gemini/Cursor), or from the **last argv** (Codex `notify` passes JSON as an argument).
- Adds metadata: `source`, `cwd`, timestamp, parent-process chain (PIDs + exe names, used to find and focus the terminal window later).
- Connects to `\\.\pipe\islet-events` with a **100 ms timeout**, writes one length-prefixed JSON frame, exits **0 always**. If Islet isn't running, it exits immediately. Never blocks, never prints to stdout (stdout is parsed by some agents).
- `islet-hook notify --title "Build" --body "done" [--level info|success|error|attention]`: generic mode for any script, CI, or test runner.

### 6.2 Supported agents and events

| Agent | Config file | Events used → island meaning |
|---|---|---|
| **Claude Code** | `~/.claude/settings.json` → `hooks` | `SessionStart` → running · `UserPromptSubmit` → working · `PreToolUse` (matcher `*`) → activity text ("Editing auth.rs", "Running npm test") · `Notification` → **needs input** (sticky) · `Stop` → **done** (use `last_assistant_message` for the summary line) · `SubagentStop` → subagent done (low priority) · `SessionEnd` → remove |
| **Codex CLI** | `~/.codex/config.toml` → **top-level** `notify = ["…\\islet-hook.exe", "codex"]` | `agent-turn-complete` → done (summary from `last-assistant-message`). Approval events if present in the payload → needs input |
| **Gemini CLI** | `~/.gemini/settings.json` → `hooks` | `BeforeAgent` → working · `AfterAgent` → done · `Notification` → needs input · `SessionEnd` → remove |
| **Cursor** | `~/.cursor/hooks.json` (version 1) | `beforeSubmitPrompt` → working · `stop` → done (`status`: completed/aborted/error) |
| **Anything else** | — | `islet-hook notify …` |

Normalised internal event (`islet-core::agent`):
```rust
struct AgentEvent {
    source: AgentKind,          // Claude, Codex, Gemini, Cursor, Generic
    session_id: String,         // stable per session (fallback: cwd+pid hash)
    project: String,            // basename of cwd
    kind: AgentEventKind,       // Started, Working{activity}, NeedsInput{msg}, Done{summary}, Error{msg}, Ended
    terminal_pids: Vec<u32>,
    at: Instant,
}
```
The session registry keeps live sessions. The expanded agent view lists them: icon, project, status, elapsed time.

### 6.3 One-click integration (settings → Integrations)
- Detect installed agents (config dir exists or binary on PATH).
- **Install:** back up the file (`*.islet-backup-<timestamp>`), parse it (JSON via `serde_json` preserving order; TOML via `toml_edit` preserving comments), **merge** our hook entries (never clobber the user's hooks), write atomically (temp file + rename).
- Codex `notify` is a single command. If the user already has one, install an Islet **chain mode** (`islet-hook codex --chain "<their original command>"`) that runs theirs too.
- **Uninstall:** remove exactly our entries (identified by the `islet-hook` path).
- **Test button:** fires a sample event through the real pipe.
- Hook paths use the installed absolute path, quoted (spaces in `%LOCALAPPDATA%`).

### 6.4 Click actions
- Click "done"/"needs input" → focus the originating terminal window: walk `terminal_pids` → find the top-level window (Windows Terminal / conhost / VS Code / Cursor) → `AllowSetForegroundWindow` + `SetForegroundWindow` (use the `AttachThreadInput` fallback if refused).
- Right-click → dismiss. Middle-click → mute that session.

### 6.5 Stretch (P8)
- **Approve/deny from the island** for Claude Code permission prompts (`PermissionRequest`/`PreToolUse` hook blocks on a pipe reply with a timeout, island shows Allow/Deny/Always). Coucou proves this works. Only ship with a strict timeout that falls back to normal CLI behaviour.

---

## 7. Feature 3 — Custom keyboard sounds

### 7.1 Capture
- One **`WH_KEYBOARD_LL`** hook on a dedicated thread with its own message loop, shared with the dictation hotkey FSM.
- The hook callback must return in microseconds: push `(vk, scan, down/up, extended, timestamp)` into a lock-free SPSC ring (`rtrb`), then `CallNextHookEx`. No allocation, no locks, no I/O inside the callback.
- Ignore injected events (`LLKHF_INJECTED`) so our own paste doesn't click. Ignore auto-repeat downs (track key-down state) unless the "repeat sounds" toggle is on.
- Privacy: key events go only to the audio thread and hotkey FSM. They are **never stored or logged**. Say so in the README and settings.

### 7.2 Playback
- `cpal` output stream, WASAPI shared, smallest stable buffer (target 3–5 ms period). The stream stays open while sounds are enabled, writes silence when idle, and is **paused after 30 s without keys** to avoid CPU wakeups.
- Mixer: 24 voice slots, pre-decoded PCM at device rate (decode + resample on pack load, not at play time). Voice stealing: oldest first.
- Realism: ±3% random pitch (resample-on-play via linear interpolation) and ±1.5 dB gain variation, separate key-up samples if the pack has them, distinct samples for Space/Enter/Backspace/modifiers.
- Master volume and per-pack volume. Mute when: dictation recording, fullscreen game focused (toggle), or a specific app is focused (blocklist).

### 7.3 Sound packs
- Built-in: 3 self-made or CC0 packs (e.g. "Thock", "Clicky", "Soft").
- **Mechvibes pack import** (both formats): `config.json` + single sprite file with `[start_ms, length_ms]` per keycode, or one file per key. Map Mechvibes keycodes → Windows VK/scan codes.
- Import via drag-drop `.zip` into settings → validate → copy to `%APPDATA%\Islet\packs\<id>\`.
- Preview: on-screen keyboard in settings, click to audition.
- Don't bundle third-party packs. Users import their own.

### 7.4 Island tie-in
- Changing a pack via the tray or settings pops a compact island toast: "Sound · Thock". Optional typing-speed WPM chip in expanded idle state (stretch).

---

## 8. Feature 4 — Now playing (iOS parity)

### 8.1 Data
- `GlobalSystemMediaTransportControlsSessionManager::RequestAsync()` once, then subscribe to `CurrentSessionChanged`, `SessionsChanged`, and per-session `MediaPropertiesChanged`, `PlaybackInfoChanged`, `TimelinePropertiesChanged`. **Event-driven, no polling.**
- Metadata: title, artist, album, thumbnail (`Thumbnail().OpenReadAsync()` → bytes → WIC decode → 128 px and 48 px cached bitmaps), playback status, timeline (position, start, end).
- Progress bar interpolates locally from the last timeline sample, since GSMTC timeline updates are sparse. Redraw the progress bar only while expanded.
- Source preference: Spotify first if multiple sessions, then the most recently changed. The expanded view shows a small source-app icon.
- Controls: `TryTogglePlayPauseAsync`, `TrySkipNextAsync`, `TrySkipPreviousAsync`, `TryChangePlaybackPositionAsync` (drag scrubber).

### 8.2 Visuals
- **Compact:** album art (24 px, 6 px radius) at the left, 4–5 animated bars at the right, tinted with the album art's dominant colour (k-means on a 16×16 downsample, computed once per track).
- Bars: real audio via WASAPI **loopback** on the default render device → 5-band energy (simple biquad bands or a 256-pt FFT with `realfft`) at 30 Hz, smoothed. Runs **only** while media is playing and the island is visible. "Fake bars" option (procedural) for zero cost.
- **Expanded:** 64 px art, title (marquee if long), artist, scrubber with elapsed/remaining, prev/play-pause/next, source icon. The art morphs (springs) from its compact position to its expanded position.
- Track change: art cross-fades with a scale bounce.
- Paused > 5 min → media activity drops off the island.

---

## 9. Feature 5 — Settings (`islet-settings.exe`)

egui/eframe, dark theme matching the island (black, Segoe UI Variable, 12 px radii). Launched from the tray icon, a right-click on the island, or the first-run prompt. Single instance.

Pages:
1. **General:** launch at login (HKCU `Run` key), position (bottom/top), monitor (primary/follow cursor), idle style (hidden / sliver), hide in fullscreen, theme accents.
2. **Dictation:** hotkeys (capture widget), mic device, start/stop tones, filler removal, spoken punctuation, injection method + per-app overrides, dictionary/replacements, snippets, history toggle.
3. **Models:** list, size, status, download/delete, progress bar, keep-warm toggle, idle-unload minutes, thread count, a "Test: record 5 s and transcribe" button.
4. **Agents:** detected agents with Install/Uninstall/Test, list of live sessions, which event types to show, sticky vs transient, per-agent sound.
5. **Keyboard sounds:** enable, pack picker with preview keyboard, volume, pitch variation, key-up sounds, repeat sounds, app blocklist, import pack.
6. **Media:** enable, preferred source, real vs fake bars, colour tint.
7. **About:** version, licences (Parakeet CC-BY-4.0, ORT MIT, crates), logs folder button, "copy diagnostics".

Communication: the settings process writes `config.toml` atomically → the island process watches it and reloads diffs. For commands (test notification, test transcription, preview sound) settings sends a control frame over the same named pipe (`kind: "control"`).

---

## 10. App shell
- **Single instance:** named mutex `Local\Islet.Singleton`. A second launch sends "show settings" over the pipe and exits.
- **Tray icon** (`Shell_NotifyIconW`): Settings, Pause dictation, Mute keyboard sounds, Hide island for 1 h, Quit.
- **Crash safety:** panic hook writes to log; the LL hook is unhooked on exit. A watchdog thread checks the hook thread's heartbeat (Windows silently removes LL hooks whose callback exceeds `LowLevelHooksTimeout`) and reinstalls the hook if events stop arriving while the user is clearly typing.
- **Sleep/resume and device change:** rebuild D3D device on `DXGI_ERROR_DEVICE_REMOVED`, re-open audio streams on default-device change (`IMMNotificationClient`).
- **Windows support:** Windows 11 primary, Windows 10 22H2 supported (vblank fallback path).

### Known OS limitations (document them; don't fight them)
- **UIPI:** an unelevated Islet cannot inject text into, or hear keys from, **elevated** (admin) windows. Option: "Run as administrator" toggle in settings via a scheduled task. Default off.
- Security software may flag LL keyboard hooks + SendInput + clipboard access. **Code-sign releases** (Azure Trusted Signing or similar) before public distribution.
- Some terminals and remote-desktop apps mishandle Ctrl+V. Per-app injection override covers this.

---

## 11. Phases (Sonnet: execute strictly in order, one at a time)

Each phase ends with: all acceptance checks pass, `cargo clippy -- -D warnings` clean, `cargo test` green, `PROGRESS.md` updated (what was done, measurements, open issues), one commit per phase (`P<n>: <summary>`).

### P0 — Skeleton + ASR spike (go/no-go)
- Workspace, release profile, `tracing`, config load/save/watch, single instance, tray icon with Quit.
- `islet-core` spring solver + tests (overshoot, settle, retarget preserves velocity, frame-rate independence).
- **ASR spike** (CLI binary `asr-bench`): run Parakeet v3 INT8 on bundled 5 s / 15 s / 60 s WAVs via (a) `ort` + own TDT greedy decode (port the `onnx-asr` reference), (b) `sherpa-onnx` bindings. Record load time, peak RSS, real-time factor, WER sanity check. **Pick one, write the decision + numbers into PROGRESS.md.**
- ✅ Accept: spike numbers recorded, chosen engine transcribes the test WAVs correctly.

### P1 — Island shell (the hardest part; spend the time here)
- Window, DComp/D2D/DWrite pipeline, squircle pill, springs, render-thread parking, vblank pacing, DPI, multi-monitor, work-area positioning, fullscreen auto-hide, click-through hit-testing, hover expand/collapse.
- Debug-only mock driver: keys F13–F18 (or a `--demo` flag) cycle through every state from §4.4 with fake content (text, art, bars, split bubble).
- Content morph transitions (fade + scale + blur).
- ✅ Accept: PresentMon shows 0 dropped frames over a 30 s `--demo` cycle at 60 Hz and ≥120 Hz. Idle CPU ≤ 0.1% and render thread parked (verify with Process Explorer thread CPU). RSS ≤ 30 MB. Clicks pass through outside the pill. No focus stealing.

### P2 — Now playing
- GSMTC wrapper, event subscriptions, thumbnail decode, colour extraction, compact + expanded media views, controls, scrubber, loopback bars + fake bars.
- ✅ Accept: Spotify desktop + YouTube in Edge/Chrome both show and control correctly. Track change animates. Idle with music paused adds no wakeups.

### P3 — Dictation
- Mic capture + resample, LL hook thread + hotkey FSM (hold, double-tap, toggle, Esc), VAD segmentation, ASR worker, post-processing, injection with clipboard restore, model manager (download/verify/load/unload), dictation island states, tones.
- ✅ Accept: 10 s utterance → text in Notepad, VS Code, Chrome textarea, Windows Terminal within the §2 budget. Clipboard content preserved after insertion. 3-minute hands-free dictation works without memory growth. Model unloads after timeout (RSS drops).

### P4 — Agent notifications
- `islet-hook.exe`, pipe server (ACL: current user only), parsers for Claude/Codex/Gemini/Cursor/generic, session registry, alert queue, split-bubble running indicator, click-to-focus terminal, integration installers with backup/merge/uninstall (+ Codex chain mode).
- Test fixtures: real captured payload JSON for each agent in `crates/islet-agents/tests/fixtures/`.
- ✅ Accept: real Claude Code session shows working → activity → done. Codex `notify` fires on turn complete. Install/uninstall leaves user configs byte-identical apart from our entries. Hook exe ≤ 30 ms and exits 0 when Islet is closed.

### P5 — Keyboard sounds
- Lock-free hook → audio path, mixer, built-in packs, Mechvibes import, pitch/gain variation, key-up, blocklist, stream pause on idle.
- ✅ Accept: ≤ 15 ms latency (measure with loopback + key timestamp log in a debug build). 150 WPM burst typing has no dropouts or stuck voices. Zero CPU after 30 s idle.

### P6 — Settings app
- All pages in §9, config hot reload, control frames, hotkey capture widget, pack preview, model download UI, integration buttons.
- ✅ Accept: every setting takes effect live without restarting `islet.exe`. Settings process exits fully on close.

### P7 — Packaging and first-run
- Installer (per-user, no admin), Start menu entry, autostart option, uninstaller removes hooks from agent configs (calls the integration uninstall), first-run flow (welcome island → model download → hotkey tip → agent integrations).
- Version stamping, About page licences.
- ✅ Accept: clean Windows 11 VM: install → dictate → see agent notification → uninstall leaves no hooks behind. Installer ≤ 25 MB.

### P8 — Stretch (only after P0–P7 are solid)
Claude permission approve/deny from island · local-LLM cleanup pass · GPU EP for ASR · WPM chip · per-app keysound profiles · timers/calendar activity · auto-update.

---

## 12. Rules for the implementing agent (Sonnet 5.5)

1. **Do not change the stack** in §1 without writing the reason in PROGRESS.md and stopping for approval. In particular: no WebView, Electron, Tauri, or tokio.
2. Work **one phase at a time**. Don't start P(n+1) until P(n)'s acceptance checks are met and recorded with real numbers.
3. Keep `islet-core` platform-free and tested. Put every state transition in the pure reducer, not in Win32 callbacks.
4. Hot paths (hook callback, audio callback, render loop) must not allocate, lock a contended mutex, log, or do I/O.
5. Every Win32/COM handle is wrapped in an RAII type. No leaked hooks, handles, or COM objects.
6. Never write user config files non-atomically. Always back up before modifying agent configs.
7. Never log keystrokes, transcripts (unless history is on), clipboard contents, or agent payload bodies at info level.
8. Prefer fewer, well-known crates. Justify any new dependency over ~200 KB compiled in PROGRESS.md.
9. When an acceptance metric is missed, fix it before moving on. Don't relax the budget.
10. Ask the human only for genuinely product-level decisions. Otherwise pick the sensible default and note it.

---

## 13. Kickoff prompt (paste into Sonnet 5.5)

```
You are implementing "Islet", a native Rust Dynamic Island for Windows.
Read PLAN.md fully before writing any code. It is the source of truth.
Create PROGRESS.md and keep it updated after every phase with what you built,
measured numbers vs the budgets in §2, decisions, and open issues.
Start with Phase P0 only. Stop after P0's acceptance checks pass and report the
ASR spike results and your engine choice before continuing to P1.
Follow the rules in §12 strictly.
```

---

## 14. References
- Parakeet TDT 0.6B v3 INT8 ONNX: `huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx` (CC-BY-4.0)
- Prior art, Rust + Parakeet on Windows: OpenWritr (`github.com/trsdn/openwritr-windows`), local-stt (`github.com/FirePheonix/parakeet-tdt-v3-CPU-optimized`)
- Agent hook pattern (hook exe + named pipe on Windows): Coucou (`github.com/Louis-CFM/coucou`)
- Claude Code hooks: `docs.claude.com/en/docs/claude-code/hooks`
- Gemini CLI hooks: `geminicli.com/docs/hooks`
- Cursor hooks: `cursor.com/docs/agent/hooks`
- Codex `notify`: top-level key in `~/.codex/config.toml`
