# Islet Progress

## P0 - Skeleton + ASR spike (complete, 2026-10-22)

**Status:** P0 acceptance checks pass. The ASR comparison is recorded below and
`sherpa-onnx` is selected for the shipping app. ASR is intentionally not wired
into the island yet; that belongs to P3.

### Environment

| | |
|---|---|
| OS | Windows 11 Pro 10.0.26300 |
| CPU | Intel Core i5-12400F, 6 physical / 12 logical cores |
| RAM | 32 GB |
| Toolchain | Rust 1.94.0, MSVC 19.44 |
| Model | NVIDIA Parakeet TDT 0.6B v3 INT8 (~670 MB) |

### What was built

- Cargo workspace with `islet-core`, `islet-app`, `islet-asr`, and the separate
  `asr-bench-sherpa` comparison binary.
- Release profile: fat LTO, one codegen unit, `opt-level = 3`, abort on panic,
  stripped symbols.
- `islet-core`: analytic damped-spring solver (overshoot, settle, retarget with
  velocity preservation, frame-rate independence) and the full TOML config
  schema with validation, atomic save, defaults, and forward-compatible
  loading. No platform code and no unsafe in this crate.
- `islet-app`: per-session single-instance mutex, per-monitor-v2 DPI awareness,
  rolling logs under `%LOCALAPPDATA%\Islet\logs`, default config creation,
  config-directory watcher with debounce, procedural tray icon, tray menu
  (config, logs, reload, quit), and `TaskbarCreated` re-add handling.
- ASR spike: two independent CLIs, `asr-bench` (`ort` + own greedy TDT decode)
  and `asr-bench-sherpa` (`sherpa-onnx` NeMo transducer). Each loads one model,
  transcribes the three fixtures in sequence, and reports load time, latency,
  speed, WER, and process peak RSS.

### ASR fixtures

Generated reproducibly by `tools/generate-test-audio.ps1` with Windows SAPI at
16 kHz mono. The manifest is `assets/audio/test/manifest.json`; reference text
is the spoken text, not a human transcript.

| clip | duration | reference words |
|---|---:|---:|
| `short-5s.wav` | 4.38 s | 13 |
| `medium-15s.wav` | 15.48 s | 40 |
| `long-60s.wav` | 55.30 s | 164 |

### Results: `ort` 2.0.0-rc.15 + custom TDT loop

Dynamic MSVC runtime build. `xRT` = audio seconds / inference seconds (higher
is better). Peak RSS is the whole process after the three-clip run.

| threads | load | short 5 s | medium 15 s | long 55 s | peak RSS |
|---:|---:|---:|---:|---:|---:|
| 1 | 2.477 s | 0.535 s / 8.19x | 1.910 s / 8.10x | 7.909 s / 6.99x | 1268.8 MiB |
| 2 | 2.480 s | 0.319 s / 13.72x | 1.105 s / 14.01x | 4.609 s / 12.00x | 1268.8 MiB |
| 4 | 2.456 s | 0.220 s / 19.88x | 0.720 s / 21.49x | 2.990 s / 18.50x | 1269.4 MiB |
| 8 | 2.414 s | 0.186 s / 23.50x | 0.604 s / 25.61x | 2.474 s / 22.36x | 1270.8 MiB |

### Results: `sherpa-onnx` 1.13.8 (`nemo_transducer`, greedy)

Built with `-C target-feature=+crt-static`. Same fixtures and method.

| threads | load | short 5 s | medium 15 s | long 55 s | peak RSS |
|---:|---:|---:|---:|---:|---:|
| 1 | 1.775 s | 0.596 s / 7.35x | 2.123 s / 7.29x | 8.574 s / 6.45x | 1224.7 MiB |
| 2 | 1.713 s | 0.379 s / 11.55x | 1.272 s / 12.17x | 4.931 s / 11.22x | 1225.5 MiB |
| 4 | 1.766 s | 0.309 s / 14.18x | 0.932 s / 16.61x | 3.444 s / 16.06x | 1226.7 MiB |
| 8 | 1.765 s | 0.915 s / 4.79x | 1.283 s / 12.06x | 3.784 s / 14.62x | 1228.2 MiB |

The 8-thread short-clip number is a repeatable outlier (thread-pool overhead on
the smallest input); 4 threads is the stable setting. Per-clip memory at 4
threads:

| after | `ort` working / peak | `sherpa` working / peak |
|---|---:|---:|
| model load | 698 / 717 MiB | 699 / 716 MiB |
| 5 s clip | 732 / 732 MiB | 730 / 730 MiB |
| 15 s clip | 817 / 818 MiB | 805 / 807 MiB |
| 55 s clip | 1263 / 1269 MiB | 1218 / 1227 MiB |

Both engines retain ONNX Runtime arena memory after long audio; the 15 s
working set is the realistic dictation footprint. The model-unload-after-idle
policy in P3 remains necessary.

### Accuracy

Both engines produced **identical transcripts** on all three fixtures.

| clip | WER | notes |
|---|---:|---|
| short 5 s | 0.0% | exact |
| medium 15 s | 0.0% | exact |
| long 55 s | 4.27% | 7 word edits / 164; SAPI itself says "Aunt M" for two "Aunt Em" references, plus `cookstove` -> `cook stove`, `chairs` -> `chair`, `trap door` -> `trapdoor` |

The long-clip WER is dominated by fixture/compound-word normalization, not
missing content. All sentences are present and readable.

### Build finding: static CRT

- `islet.exe` and the sherpa benchmark link successfully with
  `-C target-feature=+crt-static`. The resulting `islet.exe` is 1.47 MB and
  imports no `MSVCP*.dll` / `VCRUNTIME*.dll`.
- The prebuilt static ONNX Runtime used by `ort` is compiled against the
  dynamic MSVC runtime. Building `asr-bench` with `+crt-static` fails at link
  time with 71 unresolved `__imp_*` symbols. Using `ort` in the shipping app
  would therefore require either an app-local VC++ runtime (contradicting the
  "no VC++ redist" rule) or a custom ONNX Runtime build.
- `tools/build-release.ps1` builds the app with the static runtime.

### Decision

**Selected engine: `sherpa-onnx` 1.13.8**, offline recognizer with
`model_type = "nemo_transducer"`, `decoding_method = "greedy_search"`,
`feature_dim = 128`, and 4 threads.

Reasons:

1. It satisfies the static-CRT / no-VC++-redist requirement out of the box.
   `ort`'s prebuilt static runtime does not.
2. It loads faster (1.71-1.78 s vs 2.46-2.48 s) and has a slightly lower
   long-clip footprint.
3. It matches `ort`'s accuracy exactly on the fixtures.
4. At 4 threads it runs at 14-16x real time. With the planned VAD segmentation
   (segments <= 1.5 s, hard cap 15 s), the last segment transcribes in roughly
   0.1-0.25 s, inside the 400 ms release budget. A full unsegmented 10 s clip
   would take ~0.6 s, so VAD is required as designed.

Tradeoff: `ort` is materially faster (20-26x at 4-8 threads). If latency ever
becomes more important than redist-free packaging, revisit this decision; the
`ort` spike remains in `islet-asr` and `artifacts/asr/` for comparison. The
shipping app must not link both static ONNX Runtime copies; P3 will implement
the sherpa engine in `islet-asr` and remove the `ort` dependency from the
shipping path.

### App shell acceptance

- First run created `%APPDATA%\Islet\config.toml` (625 bytes) with defaults and
  installed the tray icon.
- A second process exited with code 0 and logged
  `another Islet instance is already running`.
- Atomic valid config replace reloaded and logged `config reloaded`.
- Invalid TOML was rejected with a parse error; the previous config stayed
  active.
- `WM_CLOSE` on `IsletTrayWindow` exited the message loop and logged
  `Islet stopped`.
- Idle after 10 s: **14.1 MiB working set**, 0.000 s CPU over 5 s, 7 threads.
  This is the no-model shell and is within the P1 30 MB idle budget.

### Quality gates

- `cargo test --workspace`: 25 tests pass (18 core, 4 ASR helpers, 3 benchmark
  helpers).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- Release `islet.exe` built with static CRT; sherpa benchmark built with static
  CRT; `ort` benchmark built with the dynamic runtime only for the spike.

### Open issues / next

- ASR integration, model download/verify, unload-after-idle, and VAD are P3.
  P3 should use sherpa's Silero VAD support or a separate compatible path.
- `islet-asr` currently contains the `ort` spike backend; replace it with the
  chosen sherpa engine in P3 and keep the ort spike out of the shipping binary.
- Sherpa's >4-thread short-clip behavior needs a documented thread cap in the
  model manager; default to `min(physical_cores, 4)`.
- Multilingual accuracy, punctuation, and real microphone input are not tested
  yet; the fixtures are English SAPI output.
- Packaging, code signing, installer, and model-download UI are later phases.
- No code signing or update channel yet.

### P0 acceptance

- [x] Workspace, release profile, tracing, config load/save/watch, single
  instance, tray icon with Quit.
- [x] `islet-core` spring tests: overshoot, settle, retarget preserves velocity,
  frame-rate independence.
- [x] ASR spike run through both `ort` and `sherpa-onnx`; load time, peak RSS,
  real-time factor, and WER recorded.
- [x] Chosen engine transcribes the 5 s, 15 s, and 55 s fixtures correctly.
- [x] Tests green, clippy `-D warnings` clean, `PROGRESS.md` updated.
- [x] One `P0:` commit.

## P1 - Island shell (complete, 2026-10-22)

**Status:** P1 acceptance checks pass on the 60 Hz and 144 Hz modes of the
development display. Started by the previous agent (renderer, state machine,
window, demo driver); finished and measured in this pass.

### What was built

- `islet-core::island`: priority-ordered activities (dictation > needs-input >
  done > running agent > media > idle), split mode for two persistent
  activities, hover/click/done-expiry reducer with timers as `Effect`s, the
  §4.4 size table, and `arrange()` which places the main pill and bubble in
  the window. Shared by the renderer and the UI thread's window region.
- `islet-render`: D3D11 -> DXGI -> D2D device context -> DComp visual on a
  premultiplied flip-sequential swap chain. Squircle path geometry cached per
  quantized size, cached DirectWrite layouts, pure black pill with 6% white
  hairline, content morph (fade + scale 0.945 + Gaussian blur 0..6 px) through
  two offscreen layers that exist only while transitioning, 7 spring-driven
  scalars (main w/h/corner, bubble w/h/corner, gap).
- `islet-app::island_window`: `WS_POPUP` + `TOPMOST | TOOLWINDOW | NOACTIVATE |
  NOREDIRECTIONBITMAP`, fixed 440x200 dp window at the bottom (or top) centre
  of the work area, per-monitor DPI, follow-cursor monitor option, hover
  expand (250 ms) / collapse (400 ms), click to toggle, `--demo` (cycles all 8
  states every 3 s) and `--demo-state=N`.

### Changes made while finishing P1

1. **Click-through was broken.** `WM_NCHITTEST -> HTTRANSPARENT` only forwards
   to windows on the same thread, so the transparent part of the 440x200
   window swallowed clicks meant for other apps. Fixed with a window region:
   before each snapshot the UI thread grows the region to the union of the
   old and new shapes (+12 dp overshoot margin); when the render thread parks
   it posts `WM_APP_SETTLED` with the snapshot generation and the UI thread
   shrinks the region to the exact rounded pill + bubble (1 px larger than the
   squircle, so AA edges are never trimmed). Stale settle notices are ignored
   by generation.
2. **Idle wakeups.** A 100 ms UI poll timer ran forever (10 wakeups/s).
   Full-screen detection is now event driven (`ABM_NEW` appbar registration
   for `ABN_FULLSCREENAPP`, confirmed against foreground-covers-monitor +
   `SHQueryUserNotificationState`). The poll timer only runs while the pointer
   is over the pill (100 ms) or in follow-cursor mode (500 ms).
3. **Frame pacing.** Uses `DCompositionWaitForCompositorClock` (resolved at
   runtime so Windows 10 still loads; falls back to `DwmFlush`).
4. **Content-only animation capped at 30 Hz** (media bars, waveform), by
   counting compositor vblanks instead of sleeping (sleep overshot to ~21 fps
   on the 15.6 ms timer). Morphs and transitions still run every vblank.
5. **Expand stall from hidden.** Graphics were released the instant the
   island hid, and recreating the device (~150 ms) ate the start of the next
   expand. Graphics are now kept 15 s after hiding (one timed wakeup, then
   released), and if they must be recreated the springs restart from the
   post-creation time.
6. Pill fill changed from 96.5%-alpha near-black to pure black per §4.2.

### Measurements (release, static CRT, i5-12400F, 1920x1080)

Frame intervals from the in-app log (`ISLET_FRAME_LOG=<file>`), 30 s `--demo`
cycle. `m` = morph/transition frames, `c` = content-only frames.

| refresh | morph median | morph p99 | morph max | content median |
|---|---:|---:|---:|---:|
| 60 Hz | 16.69 ms | 18.77 ms | 21.99 ms | 33.34 ms |
| 144 Hz | 6.94 ms | 7.71 ms | 21.12 ms* | 34.78 ms |

\* The only morph intervals above 1.5 vblanks are the first frame after a
snapshot arrives during 30 Hz content animation (measured against a content
frame up to 33 ms earlier) and process start. No morph frame skipped a vblank.

| state | avg CPU (12 logical) | private WS | working set |
|---|---:|---:|---:|
| idle, hidden (no graphics) | 0.000% (0 ms / 15 s) | 5.4 MiB | 15.8 MiB |
| static notification shown | 0.000%, render thread parked | 19.8 MiB | 52.4 MiB |
| compact media, bars animating | 0.52% | 20.1 MiB | 53.1 MiB |
| dictation waveform | 0.35% | 19.6 MiB | 51.6 MiB |

`islet.exe` release size: 1.62 MB.

Interaction checks (automated with a topmost test form under the island):
click above the pill reaches the app below; click on the pill does not;
the foreground window is unchanged after clicking the pill; hovering the
compact pill expands it and moving away collapses it.

### Open issues

- Frame pacing was measured with the in-app present-cadence log, not
  PresentMon (not installed). It shows CPU-side cadence, not compositor
  misses. Re-check with PresentMon before release.
- `ABN_FULLSCREENAPP` hiding has not been exercised with a real exclusive
  full-screen game yet; borderless/maximised windows are filtered by design.
- Only one monitor was available: DPI change and follow-cursor across
  monitors are implemented but untested on hardware.
- Another always-on-top window can cover the island; we only re-assert
  topmost on our own z-order changes (no polling, by design).
- Hit-testing on the pill uses the enclosing rounded rect (slightly generous
  at corners), not the exact squircle.

### P1 acceptance

- [x] 0 skipped vblanks in morph frames over a 30 s `--demo` cycle at 60 Hz
  and 144 Hz (in-app log; PresentMon pending).
- [x] Idle CPU 0.000%, render thread parked, no UI-thread timers when idle.
- [x] Idle private working set 5.4 MiB; active 20 MiB (budget 30 MB).
- [x] Clicks pass through outside the pill; no focus stealing.
- [x] Tests green (36 core, 4 render, 7 ASR/bench), clippy `-D warnings`
  clean, `cargo fmt --check` clean.

## P2 - Now playing (implemented, 2026-10-22; hands-on checks pending)

**Status:** Built and verified against a live (paused) Spotify session.
The checks that need audio playing are listed under "Pending" below; they
were not run automatically because they would play sound on the user's
machine.

### What was built

- `islet-core::media` (pure, 17 tests): `Timeline` extrapolation,
  `format_clock`, Spotify-first `choose_session`, `source_display_name` for
  AUMIDs/exe names, `dominant_color` (16x16 downsample, k-means k=5, scored
  by population x saturation, lifted to 52-70% lightness for contrast on
  black, `None` for greyscale art), expanded-view layout constants and
  `expanded_action_at` / `scrub_fraction` hit-testing (shared by renderer and
  UI thread), lock-free `BarLevels`, and a 512-point FFT `Spectrum` (5
  log-spaced bands 60 Hz-8 kHz, fast attack / slow release).
- `islet-core::island`: `MediaCard` now carries source, timeline, art,
  accent, and scrub. Paused media arms a 5-minute timer and then drops off
  until something plays again. `ContentSnapshot::updates_in_place` stops
  timeline ticks, play/pause, and level changes from running the content
  morph; only a new track (or new art) cross-fades.
- `islet-media` (new crate): one MTA thread. GSMTC `SessionsChanged`,
  `CurrentSessionChanged`, and per-session properties/playback/timeline
  events post refresh flags; bursts coalesce for 40 ms. Thumbnails decode via
  WIC to a 128 px centre-cropped premultiplied BGRA square, deduplicated by
  hash (browsers re-send identical art). Transport: play/pause, next,
  previous, seek. `media-probe` example for manual testing.
- `islet-audio` (new crate): WASAPI loopback on the default render device,
  30 Hz high-resolution waitable timer, f32/i16 mix formats, no allocation in
  the loop, reopens after device loss. Runs only while playing media is on
  screen and `media.real_bars` is on; otherwise bars are procedural.
  `loopback_probe` example.
- Renderer: album art through a cached `ID2D1BitmapBrush1` in a rounded
  rect (placeholder when absent), real timeline scrubber with elapsed and
  remaining time, source name in the art colour, drag knob, ellipsis
  trimming for long titles, real bar levels when fresh (<250 ms).
- App: media and loopback lifecycle follow config hot-reload
  (`enabled`, `real_bars`, `color_tint`). In the expanded media pill:
  prev / play-pause / next buttons and click-or-drag seeking (mouse capture;
  hover-collapse suppressed mid-drag). Commands update the card
  optimistically. While media plays the render thread now reports "settled"
  when the shape stops moving, not when it parks, so the window region still
  shrinks to the exact pill.

No new third-party crates: only additional `windows` features.

### Measurements (release, static CRT)

| | |
|---|---|
| `islet.exe` | 1.68 MB |
| live, Spotify paused, compact media shown, 30 s after 10 s warm-up | 0 ms CPU, 21.0 MiB private, 63.6 MiB working set |
| GSMTC read of live Spotify | title, artist, 128x128 art, accent `[188, 78, 81]`, timeline 9.3/135.6 s |
| loopback probe, nothing playing | 20/20 fresh level samples at 100 ms checks, all zero |

### Fix: browser media ignored when Spotify was paused (user report)

`choose_session` preferred Spotify whenever its session existed, so a paused
Spotify hid a playing YouTube tab. It now ranks playing > paused first, then
Spotify, then the system's current session. The media thread also watches
`PlaybackInfoChanged` on every session (not just the shown one), so starting
or pausing another app re-picks immediately. Chromium/Firefox register hashed
AUMIDs (e.g. `308046B0AF4A39CB`); the display name is now resolved through
the shell Apps folder ("Firefox"). Verified live: paused Spotify + playing
YouTube in Firefox shows the Firefox track. YouTube in Firefox reports no
track length, so the scrubber is hidden for it (controls still apply).

### Pending (needs audio; please verify)

- [ ] Spotify: play/pause, next, previous, and dragging the scrubber from the
  expanded island control playback.
- [ ] YouTube in Edge/Chrome shows title/art and responds to controls.
- [ ] Real bars move with the music; switching `media.real_bars = false`
  falls back to procedural bars live.
- [ ] Track change cross-fades the content.
- [x] Idle with music paused adds no wakeups (0 ms CPU / 30 s).

### Open issues

- No marquee for long titles yet (ellipsis instead); PLAN §4.2 lists marquee.
- The track-change "scale bounce" on the art is the generic content morph,
  not an art-specific spring.
- Source-app icon in the expanded view is text only (no icon extraction).
