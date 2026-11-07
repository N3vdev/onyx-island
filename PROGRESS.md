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

### Music-only filter (user request)

The island shows music only, never videos. GSMTC cannot tell: Firefox
reported an X video as `PlaybackType = Music` with title "Home / X" and no
URL. So:

- **Apps** (Spotify, Apple Music, foobar2000, ...) are trusted unless they
  set `PlaybackType = Video` themselves.
- **Browsers** (any whose process is in `BROWSER_PROCESSES`: Chrome, Edge,
  Firefox, Brave, Opera, Vivaldi, Arc, Zen, ...): `islet-media::tabs` reads
  every tab title through UI Automation (background tabs included; titles
  stay in memory, never logged). The tab whose title contains the track
  names the site. Listed music sites count; youtube.com counts only when
  `looks_like_music` says so ("- Topic"/VEVO channels, Official
  Audio/Video/Visualizer, Lyrics, feat./ft., "Artist - Song" matching the
  channel; rejects trailers, podcasts, vlogs, reactions, #shorts, ...);
  everything else is hidden. If no tab names the track (sites that keep
  their own name as the title, e.g. Deezer), a session counts when a listed
  music site tab is open.
- Config: `media.music_only` (default true) and `media.music_sites`
  (defaults: YouTube Music, SoundCloud, Deezer, Spotify, Apple Music, TIDAL,
  Bandcamp, Amazon Music, Audiomack, Mixcloud, JioSaavn, Gaana, Pandora,
  Qobuz, Napster). Entries may be names or URLs; URLs reduce to the site
  name ("https://www.jiosaavn.com/x" -> "jiosaavn", "music.apple.com" ->
  "apple music") and match on word boundaries in tab titles. Listing
  "youtube" trusts all of youtube.com. Hot-reloaded.
- Classification runs at session pick time, which now also happens on any
  session's track change, so YouTube autoplaying into a non-music video
  hides it. A rejected browser track is re-checked once after 1.5 s in case
  the tab title lagged the track change.
- Verified live: X video playing in Firefox is rejected; the island falls
  back to paused Spotify. 11 classifier tests include the real tab titles
  from that session.

Limits: matching relies on the site name appearing in the tab title (true
for the listed sites). The YouTube heuristic will misjudge some videos.
Chrome/Edge tab reading is implemented the same way but was only exercised
with Firefox here.

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

## Pause behaviour, close animation, and settings app (user requests, 2026-10-22)

Pulled forward from P6 at the user's request; the rest of P6 (dictation,
agents, keyboard sounds pages; control frames) still follows its phase.

- **Last played stays on pause.** `SessionCandidate::last_played`: when
  nothing plays, the session most recently seen playing wins over Spotify
  and the system's current session.
- **Close after pausing.** `media.pause_hide_seconds` (default 5, clamped
  1-300) replaces the fixed 5-minute drop-off. Hovering the island when the
  delay elapses re-arms it. Paused media that was never playing in this run
  never pops up (no stale Spotify at startup).
- **Fluid close.** Closing previously did not animate at all: `draw_pill`
  skipped the pill as soon as the target was hidden. Now hidden targets use
  `DISMISS_WIDTH` (0.34 s, zeta 0.92) and the slower `DISMISS_HEIGHT` (0.52 s,
  zeta 1.0), so the pill gathers into a droplet, while its opacity melts out
  below a 28 dp short side and the content blurs out. Verified with a frame
  burst capture: about 300 ms from full pill to gone.
- **`islet-settings`** (new crate, eframe/egui 0.32.3, glow renderer): a
  separate process, single instance (focuses the open window), dark
  island-styled UI with General, Now Playing (incl. music-site list editor),
  and About pages. Saves atomically on every change; the island hot-reloads.
  "Start with Windows" writes the HKCU Run key. `--page media|about`.
  Opened from the tray ("Settings..." and left click) and automatically on
  first run (no config file yet).

New dependency: `eframe` (default features off; `glow`, `default_fonts`).
Chosen in PLAN §1; it lives only in the settings exe.

| | |
|---|---|
| `islet.exe` | 1.81 MB |
| `islet-settings.exe` | 3.95 MB |
| settings window open (release) | 28.2 MiB private, 0 ms CPU over 5 s idle |

## Slim bar over apps (user request, 2026-10-22)

With the island at the top, it covered the browser's tab strip. Now, while
an app window overlaps where the island normally sits, it becomes a slim
300x26 dp bar docked flush at the **bottom** screen edge (one line: art,
"Title · Artist", mini bars; notifications and agents get a dot and one
line). Hover expands to the full island as usual. Dictation never slims.
When the spot is clear again it returns to its configured edge.

- Core: `IslandState::tucked`, `TUCKED`/`TUCKED_BUBBLE` sizes, and a new
  `LayoutTarget::inset` (edge gap) that `arrange` applies. The island window
  is now flush with the work area and the 8 dp gap is drawn as a spring, so
  docking slides instead of jumping.
- Detection (`islet-app::occlusion`): out-of-context WinEvent hooks
  (foreground, move/size end, minimize, show/hide/destroy, location,
  cloak) post to the island window; a 120 ms debounce then enumerates
  top-level windows and checks overlap with the strip at the configured
  edge using DWM extended frame bounds. Skips this process, the desktop,
  taskbars, minimised, cloaked, and click-through windows. Asking about the
  configured spot (not where the bar currently is) prevents top/bottom
  flapping. No polling.
- The window moves to the bottom before the snapshot is sent, so no frame
  is drawn at the old edge.
- Config `general.tuck_behind_windows` (default on) with a Settings toggle.
- Verified live: Position = Top, Firefox maximised, Spotify playing: tabs
  clear, slim bar at the bottom edge with art, title, and live bars.

## Dock over apps: the whole pill glides to the bottom edge (user request, 2026-10-22)

Replaces the slim bar above. The user asked for the pill itself to move:
while an app window sits behind the island, the pill glides down to the
bottom edge; when the spot is clear it glides back. Nothing changes shape
any more, so the island is the same pill at all times.

- Core: `arrange_at` takes a continuous edge `anchor` (0 = top, 1 = bottom)
  and places the pill anywhere along the line between the two edges;
  `arrange` is the discrete form used elsewhere. `TUCKED`, `TUCKED_BUBBLE`,
  and `LayoutTarget::docked` are gone: tucked now means "normal shape, other
  edge". Dictation still never docks.
- Renderer: a normalised `anchor` spring (`SpringParams::DOCK`, response
  0.50 s, damping 0.92) drives the position every frame, so the travel is the
  same spring-eased motion as every other transition and can be retargeted
  mid-flight without a jump. Its settle thresholds are scaled by the window
  height, so "settled" still means sub-pixel.
- Window: with `position = "top"` the island window now spans the whole work
  area, so the pill glides between the edges inside one window instead of a
  small window teleporting between them. At the bottom it stays 200 dp tall.
  Detection still asks about the configured edge, so it cannot flap.
- While travelling, the window is click-through (`WS_EX_TRANSPARENT`) and its
  region is the union box of the old and new pill positions, so no frame of the
  glide is clipped and the tall transparent window never swallows a click.
  `WM_APP_SETTLED` clears both and pins the region to the exact pill.
- Disabling the setting drops the WinEvent hooks before the reposition, so the
  last occlusion check cannot re-dock the island mid-return.
- Settings: toggle renamed to "Dock over apps" with a matching description.
- Debug: `--demo` with `ISLET_DEMO_DOCK=1` alternates the edges every step, so
  the glide can be watched without opening an app window.

Verified live (release build, `--demo` + `ISLET_DEMO_DOCK=1`, sampling the
window region, extended style, and screen pixels every 30 ms):

| | |
|---|---|
| glide | ~0.55 s edge to edge, ease-out, arrives within 1 px; tracked 13 distinct positions per travel |
| frames | one morph frame per vblank: 1436 morph frames / 30 s, median 5.6 ms, one 27.8 ms outlier at process start |
| click-through | on for the travel only, cleared the moment the shape settles |
| region | union box during travel (e.g. `-4..1084`), exact pill after (e.g. `7..45` top, `1035..1073` bottom), empty while hidden |
| window | 440x1080 at `716,0` constant throughout, so nothing moves the HWND |

## Dock trigger: the island's exact footprint, and invisible overlays (user request, 2026-10-22)

The user restated the rule: move the pill down only while an app window
actually covers the spot the island occupies at the top (about the top 3-4% of
the screen); with only the desktop showing it must stay at the top; and none of
it may cost resources.

Two defects were behind "it docks even when the desktop is showing":

- **The NVIDIA GeForce Overlay pinned it down permanently.** That window
  (`CEF-OSC-WIDGET`) spans the whole screen and the OS reports it as visible,
  yet it is fully transparent (`WS_EX_LAYERED` with alpha 0), which every
  existing filter missed. Layered windows whose `SetLayeredWindowAttributes`
  alpha is 0 are now ignored, next to the already ignored click-through,
  cloaked (other virtual desktop), minimised, desktop and taskbar windows.
  The decision is a pure function (`is_drawn_window`) with unit tests.
- **The zone was a taller strip** (screen edge through the bottom of the pill).
  It is now exactly the island's own footprint: the resting compact pill's
  rectangle, 220x36 dp inset 8 dp from the configured edge, i.e. the top ~4%
  of a 1080p screen. Everywhere else on screen, a window leaves it alone.

Verified live, music playing with the pill forced visible:

| step | window region |
|---|---|
| Firefox + Vibeforge open over the band | `109,1035,331,1073` (docked at the bottom) |
| those two minimised, desktop showing | `109,7,331,45` (glided back to the top) |
| restored | `109,1035,331,1073` (docked again) |

The probe that lists counted windows now reports exactly the two real ones;
the NVIDIA overlay, the shell's cloaked edge window, the widget board, and the
OneDrive/Start helpers no longer appear.

Cost, measured on the release build (per-second and per-thread sampling):

| state | CPU |
|---|---|
| idle, nothing playing (island hidden) | **0.000%** of one core: 0 ms over 60 s |
| music playing, compact pill with live bars | ~5% of one core = ~0.4% of the machine, the same as P2's 0.52%-of-machine figure |
| 194 window moves in 6 s (a WinEvent storm) | +0.5% of one core across those 6 s, so the detection itself is free |
| full-work-area window (docked design) vs 200 dp window at the bottom | 5.47% vs 5.16% of one core: the tall window costs nothing measurable |

Nothing polls. The hooks are out-of-context WinEvent hooks; a burst of events
is debounced by one one-shot 120 ms timer that is killed when it fires, and the
check runs only when a top-level window moves, resizes, shows, hides, cloaks or
takes the foreground. Turning the music filter's UI Automation tab reading off
did not change the numbers (6.05% vs 4.88% of one core), so the tab scans are
not a factor.

## Docking as a droplet, not a glide (user request, 2026-10-22)

The user rejected the straight top-to-bottom slide. The island must gather into
a leaf, slip through the edge it is leaving, and grow back out of the other
edge in the same shape, mirrored, and it has to feel premium without costing
anything at rest. Still the native Rust/Win32/D2D/DComp stack, no new
dependencies.

**One phase, two edges.** The travel is the existing normalized `anchor` spring
(0 = top, 1 = bottom) read as a phase rather than a position. `t < 0.5` draws
the pill at the top edge with `fullness = 1 - t/0.5`; `t > 0.5` draws it at the
bottom edge with `fullness = (t - 0.5)/0.5`. At `t = 0.5` both copies are
invisible, so the handoff cannot pop - and retargeting mid-flight simply runs
the same curve backwards, mirrored, so an interrupted dock reverses instead of
jumping.

**The shape.** Every outline point of the squircle is pulled toward the centre
line by `1 - taper * reach^1.15`, where `reach` is the distance from the edge
the island is leaving (0 at that edge, 1 at the far end). `taper` and the two
size factors come from `droplet_shape(fullness)`:

- a **gather** (`smoothstep(0, 0.42, 1 - fullness)`) narrows the width to 34%
  and the height to 78% while `taper` goes 0 -> 0.97: the pill becomes a leaf;
- a **press** (`smoothstep(0.30, 1.0, 1 - fullness)`) then flattens the height
  to 0 and narrows the width by a further 46%, so the tail disappears into the
  edge.

The power profile was chosen after comparing 1.0 / 1.15 / 1.35 / 1.6 and an
elliptical profile at real travel sizes (`droplet/profiles.png`): 1.0 reads as
a guitar pick, 1.6 as a shield with a keel, 1.15 as a plump droplet whose sides
leave the round end tangentially and keep bending to a point. The squircle's
straight sides are sampled into 12 segments before warping - warping only the
corners would leave a trapezoid - and the resulting path is cached by (size,
corner, taper, edge) with 128 entries, so a dock never evicts the resting
pills.

**Flush with the surface.** While it is being pressed through, the island sits
exactly on the screen edge; the resting 8 dp inset only returns at the very end
of the re-form:

`gap = inset * smoothstep(0.70, 1.0, fullness)`

An earlier version scaled the gap with the shape's *fraction*, which let the
drop detach from the edge mid-flight whenever the target size differed (an
expanded card docking into a compact pill). Tying it to the phase keeps the
whole flight flush and lifts the pill onto its inset as it settles; sampled
window frames show `y = 0` at the top and `y + h = 1080` at the bottom for the
entire press.

**Alpha.** The shell and the card dissolve on separate windows:

`shell_alpha = smoothstep(0.05, 0.45, fullness)`
`content_alpha = smoothstep(0.50, 0.92, fullness)`

so the card's content is gone before the leaf is fully pressed, and it comes
back only as the pill re-forms.

**Content clipped to the droplet.** A rectangular clip let album art and text
peek outside the tapered silhouette for ~100 ms per half. The content is now
drawn through a D2D layer whose geometric mask is the same path the shell is
filled with (`PushLayer` + `D2D1_LAYER_PARAMETERS1.geometricMask`, one reusable
`ID2D1Layer`, only while `taper > 0`). Window-only PrintWindow frames confirm
the artwork ends exactly on the leaf's edge (`droplet/pw-absorb.png`,
`droplet/pw-emerge.png`).

**Pacing.** `SpringParams::DOCK` moved from response 0.50 / damping 0.92 to
0.85 / 0.90: a 0.55 s straight glide had no room for a gather and a re-form.
Measured from the animator at 120 Hz:

| | |
|---|---|
| visible gather (leaf 71x25 at 117 ms) | 0 - 210 ms |
| handoff, both edges invisible | ~208 ms |
| visible re-form | 225 - 700 ms |
| geometry back to a full pill | ~825 ms |
| click-through released as the anchor settles | ~880 ms |

The asymmetry is deliberate: a quick suck into the edge, a slower and gentler
re-form. Both directions play the same shape; the 9 s demo flight
(top-to-bottom, shrinking) was captured to confirm the mirrored half.

**Verification.** 13 renderer unit tests (including the mirror and the flush
edge), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt
--check`, and window-only PrintWindow captures of four demo flights.

Cost, release build on a 12-core machine: the demo cycle - live bars plus four
flights in 12 s - averages 10.2% of one core (0.85% of the machine); the same
state without flights is 3.7% of one core, so a flight is roughly 0.2 s of CPU
spread over 0.9 s (and the demo's flights also relayout a card that changes
size). Idle remains 0.000% of one core, and the mask layer only exists while
the shell is tapered.

## A Helvetica-like voice

All island text now uses **Inter**, a Helvetica-style grotesque with the full
weight range, so the title/sub/tiny hierarchy is preserved. Segoe UI remains
the system fallback when the family is absent.

The compact media tile is now a **circle**: its radius is `art * 0.5` (12 dp on
the 24 dp tile), which is the pill's 18 dp corner clamped to the tile, so the
art sits concentric with the pill's rounded ends. Window-only captures of the
compact pill and the expanded card confirm the new face and the circular art.

## The expanded player

The hovered player is now a full 400 x 168 card built around a 120 dp cover
(18, 24, radius 26) and a single text column at x = 154. Title (Inter 17 dp
demi-bold), artist (12 dp), a source chip, a hairline timeline and one control
row replace the old sparse layout: nothing floats in dead space, and the
timeline and volume track share the same 378 dp right edge.

**Source chip.** The app name sits in a soft pill with a small north-east
arrow. Pressing the cover, the title block or the chip calls
`open_media_source`, which activates the running window first, then tries the
Spotify URI, the `shell:AppsFolder` AUMID, the executable and finally the AUMID
again. The chip makes that affordance visible instead of hiding it in the art.

**Volume.** A speaker glyph and a 68 dp slider sit at the right end of the
control row, aligned with the transport's centre line. `volume_fraction`
mirrors `scrub_fraction`: a press on the row starts a capture drag, the slider
follows the cursor and clamps at both ends, and clicking the speaker mutes.
`islet-audio::VolumeHandle` owns a named worker thread with its own COM
apartment and `IAudioEndpointVolume`; the UI only sends `Set`/`Refresh` over a
channel and reads an atomic, so hovering never blocks on Core Audio and an idle
island wakes no threads. The slider is hidden when the endpoint cannot be read.
While the player is visible the hover poll refreshes the value, so hardware
volume keys move the knob too; the drag itself skips that sync.

**Verification.** The full workspace test suite (including new chip and volume
hit tests), clippy `-D warnings` and `cargo fmt --check` are green.
Window-only captures of the demo state show the new card; a scripted mouse drag
from 25% to 75% moved the knob with the cursor, and a probe read the endpoint,
wrote the same value back and read it again (0.9676).


## Reference-led horizontal hover player (2026-10-23)

The expanded media card is now a 448 x 112 black capsule with an 84 dp cover,
compact Inter title and artist, small filled transport icons, and a white
hairline seek track. A quiet vertical divider separates the fixed 280 dp music
pane from two charcoal pills containing the existing source shortcut and volume
slider. Source activation, seeking, mute, and captured volume dragging are
preserved. The window grows to 488 dp to retain 20 dp of animation clearance.

Playback layout and its hit zones stay fixed when the capsule widens. The first
feature column also stays anchored beside the player, leaving future columns
to grow on its right. The shared geometry module owns drawing and hit testing.

Verification: 90 tests across islet-core, islet-render, and islet-app pass,
including a regression check for extending the feature area and separate
transport/seek/volume hit zones. Scoped Clippy with warnings denied, formatting,
diff whitespace checks, and the release build pass. Native screenshot inspection
was unavailable: the Computer Use helper's native pipe failed to connect after
retry and session reset. No rendered visual approval is claimed.

## Expanded control hover feedback (2026-10-23)

Every actionable part of the expanded music player now gets a neutral grey
hover tint: album art, title/artist, previous, play/pause, next, seek bar, source
shortcut, and volume. The seek thumb also appears on hover. Artwork retains its
rounded mask, transport uses circular backgrounds, and the two existing feature
pills brighten in place. Captured slider drags keep their highlight until release.

Hover targets use the same shared hit zones and mouse-down precedence as clicks.
Missing timeline, volume, or source controls do not show feedback. The UI sends
only hover-target changes over a separate render command, avoiding track
transitions and continuous animation while paused. Mouse leave, collapse, and
the existing stationary-pointer poll reconcile the highlight.

Verification: 92 tests pass across islet-core, islet-render, and islet-app,
including unavailable controls and the transport/seek boundary. Scoped Clippy
with warnings denied and formatting pass.

## Hover feedback correction (2026-10-23)

Per the user's correction, hover feedback now changes foreground only. The
white title and source label shift to light grey; transport and volume icons
soften their opacity. Artist text and the seek fill get a small opacity change,
and artwork dims slightly. Removed the hover backgrounds, pill brightening,
and hover-only seek thumb. Existing static pill fills retain their resting color.

## Mixed hover treatment (2026-10-23)

Restored the original grey hover backgrounds for previous, play/pause, next,
seek, volume, and the source shortcut above volume, per the user's latest
preference. These controls retain their normal foreground opacity. The seek
thumb appears on hover again. Title and artist keep the light text-only effect;
artwork keeps its subtle opacity feedback. Scoped Clippy and formatting pass.

### Settings redesign and native preview handoff (2026-10-23)
- Rebuilt all three settings pages around the supplied black-sidebar / purple-workspace reference: rounded main surface, static atmospheric gradient, authored navigation icons, compact sidebar footer, softer controls, independent page scroll state, and short page fades.
- Reserved the upper workspace for the existing live island. Settings publishes an invisible geometry anchor; the main app retracts through its current edge, relocates while hidden, and emerges from the preview top. Closing restores desktop placement.
- Preview remains an independent native window, aligned in normal z-order. Cross-process ownership was rejected after a forced-close probe showed Windows destroys owned windows. The final implementation survives settings process termination and restores the same island HWND to the desktop.
- Motion is finite and respects Windows client-area animation settings. No second media/audio/render pipeline; settings uses no permanent preview timer. Idle settings CPU measured 0 seconds over a three-second settled sample (not an FPS/GPU benchmark).
- Added named/checked metadata and keyboard-focus outlines to switches. Startup-registration failures now remain visible and roll back the requested startup toggle rather than falsely reporting success.
- Validation: 94 core/render/app tests passed; settings built; scoped Clippy with warnings denied passed. Native probes verified matching preview/anchor rectangles, move/resize, minimize/restore, graceful close, and forced-close recovery. All three pages captured, including About at minimum size.
- Finish review: reference styling matched; startup error fix resolved; switch labels/checked metadata/focus-outline source verified. Actual focused-switch screenshot remains unverified because native keyboard-focus injection was refused; no further source change was indicated.
- Launched the app with cargo run -p islet-app and reopened the redesigned settings. Native integration captures/probe scripts are under ignored target/.

### Matching title bar and rounded settings typography (2026-10-23)
- Set the native settings caption and border to the app frame's #030304, with light caption text and dark native controls through DWM attributes. Native drag, resize, minimize/maximize, and close controls are retained.
- Bundled SIL Open Font License Nunito at regular weight 400 for all proportional settings typography. The upstream variable font defaults to weight 200, so a static 400 instance was generated using fontTools for clear labels and descriptions. Font and license ship in crates/islet-settings/assets/fonts/.
- Updated DESIGN.md and design sidecar typography. Built settings, passed scoped Clippy with warnings denied, and visually checked the final window. Reopened updated settings for the user.

### Bold headings and animated sidebar (2026-10-23)
- Added a bundled Nunito Bold 700 face for page/section headings, branding, and selected navigation labels. Regular text remains weight 400.
- Rebuilt the sidebar with a clearer brand lockup, icon/label/description navigation rows, a shared sliding selection surface, subtle hover and icon transitions, visible keyboard focus, and compact connection/version footer.
- Selection moves over 220ms, responds immediately with Windows reduced motion, and stops repainting when settled. Existing egui animation manager is reused.
- Built settings, passed scoped Clippy with warnings denied, visually checked General and Now Playing, and verified page navigation by clicking the running app. Settled settings CPU measured zero seconds over three seconds. Updated design records; updated app remains open.

### Dark context menu and Restart app (2026-10-23)
- Styled the native tray/island context menu with dark backgrounds, light text, muted separators, and a dark hover state. Owner drawing retains native menu tracking, item captions, keyboard navigation, and dismissal; GDI/menu resources are released after closing.
- Added Restart app above Quit. The replacement preserves launch arguments, waits on the old process handle before initialization, and starts after singleton/logging cleanup. Failure to spawn keeps the existing instance running.
- New app instances reconnect to an existing settings preview anchor without permanent polling. Right-clicking the island forwards to the same menu as the tray icon.
- Validation: app built; three app tests and scoped Clippy with warnings denied passed; formatting and diff checks passed. Captured the dark menu. Native menu-item hit testing selected Restart app, confirmed a new PID with exactly one running main instance, and verified the existing settings preview reconnects with matching anchor/island rectangles.

### Compact rounded context menu (2026-10-23)
- Reduced the menu from 232 x 228 to 184 x 182 physical pixels at 100% scaling (about 20% smaller in each dimension), with 27-point rows, 13-point text, narrower padding, and tighter separators.
- Added 10-point outer corner rounding and inset rounded hover surfaces. Rounding uses a process/thread-scoped popup-start hook installed only while the menu is open; its region is owned by Windows and the hook is released on close.
- Built and relaunched the app; scoped Clippy with warnings denied passed. Screenshot and native window-region probe confirmed the compact size and actual corner clipping (corner outside the region, center inside). Existing menu actions are unchanged.

### Simplified context menu (2026-10-23)
- Removed Open config, Reload config, and Open logs, along with their obsolete command handlers. The compact rounded dark menu now contains Settings, Restart app, and Quit Islet.
- Built and relaunched the app; scoped Clippy with warnings denied passed. Settings was closed during rebuilding to prevent its preview recovery from relaunching the old binary, then reopened afterward.

### Borderless native context menu (2026-10-23)
- Removed the stock light outline by clipping the two-point system frame inside the existing rounded window region. Native item layout, hover behavior, hit testing, and scoped hook lifetime remain intact; no extra paint loop or subclass is needed.
- Built, passed scoped Clippy with warnings denied, and relaunched app/settings. Verified the finished popup visually and sampled edge pixels to confirm the light outline is gone; visible menu edges use #120f15 and clipped pixels expose the background.

## P3 - Dictation (implemented, 2026-10-23; real-speech checks pending)

**Status:** the whole pipeline works end to end on this machine: hotkey ->
microphone -> Parakeet -> text cleanup -> inserted at the cursor, with the
clipboard restored. Verified with a fake microphone playing the P0 test clips
(so no one has to talk while a script drives the keys), with the real keyboard
hook, and with real text insertion. Not yet verified with a human voice on the
real microphone, nor in Notepad/VS Code/Chrome/Windows Terminal (see
"Pending").

### Decisions (deviations from PLAN, all small)

1. **ASR runs in `islet-asr-worker.exe`, not a thread in `islet.exe`.** The
   sherpa + ONNX Runtime static link is 19 MB, which would take `islet.exe`
   from 2 MB to ~20 MB (budget: 8 MB). A child process also makes "unload the
   model" mean "the process exits" (every byte returned; P0 showed ONNX Runtime
   keeps memory after long clips) and isolates ASR crashes from the island.
   Same stack (sherpa-onnx); only the process boundary differs. Protocol:
   length-prefixed frames over stdin/stdout, JSON control + raw f32 audio
   (`islet-asr/src/protocol.rs`).
2. **No `cpal` or `rubato`.** WASAPI shared mode with `AUTOCONVERTPCM` delivers
   16 kHz mono 16-bit directly (Windows' own converter), so mic capture is ~150
   lines on the `windows` crate we already use and needs no resampler. `cpal`
   will still be wanted for P5 (key sounds).
3. **The `ort` spike moved to its own crate** (`asr-bench-ort`) so the shipping
   `islet-asr` has no ONNX Runtime and the two static copies can never meet.
4. **Model registry** (`islet-asr/src/registry.rs`): Parakeet TDT v3 (default,
   multilingual) and v2 (English). Adding a model is one entry with its files,
   sizes and SHA-256. Hashes of the large files are Hugging Face's published
   LFS ids; the existing local v3 files were checked against them. The VAD
   model and v2's `tokens.txt` have no published hash, so theirs are pinned from
   the first download over HTTPS (trust on first use).
5. **No regex crate** for the personal dictionary yet (PLAN says "regex
   allowed"): dictionary and snippets match whole words/phrases.

### What was built

- `islet-core::hotkey`: `Combo` parsing ("Ctrl+Win", "F9", ...) and an
  allocation-free `HotkeyFsm` that runs *inside* the hook callback (it must
  decide synchronously whether to swallow a key). Hold = push to talk; taps
  under 250 ms are discarded; double-tap or Ctrl+Win+Space = hands-free; Esc
  cancels; any other key during a hold cancels and passes through (Ctrl+Win+
  Left still switches desktops); extra modifiers or typing first do not
  trigger. 19 tests.
- `islet-core::transcript`: filler removal ("um", "uh", clause-initial "you
  know"), spoken punctuation ("new line", "new paragraph", "comma", "question
  mark", ...), personal dictionary, snippets; unicode-safe. 11 tests.
- `islet-asr`: registry, `ModelManager` (resumable HTTPS download with `Range`,
  SHA-256 verify, delete, status; tested against a local HTTP server for
  resume, dropped connections, servers that ignore ranges, corrupt files and
  cancel), protocol, and `AsrClient` (spawns and supervises the worker).
- `islet-asr-worker`: Parakeet via sherpa `nemo_transducer` + Silero VAD
  (a 0.5 s pause closes an utterance, 12 s cap), recognising each utterance as
  it closes so only the last one is pending at release.
- `islet-audio`: `Microphone` (opened warm, started on demand, 16 kHz mono),
  device list for the picker, `Tone` (generated start/stop/done chimes via
  `PlaySound`), and an `ISLET_FAKE_MIC=<wav>` test hook.
- `islet-input`: `KeyboardHook` (`WH_KEYBOARD_LL` thread; ignores its own
  injected keys) and `insert_text`: clipboard paste (all formats saved and
  restored, only if the user did not copy something meanwhile; flagged out of
  clipboard history and cloud sync) or Unicode typing. It waits for the
  hotkey's modifiers to be released first (a synthetic Ctrl+V with Win still
  down becomes Win+V), detects elevated target windows (UIPI), and injects a
  dummy key so releasing Win does not open the Start menu.
- `islet-app::dictation`: one controller thread owns hook, mic and worker.
  The model loads lazily on the first press (the pill says "Warming up", audio
  is held and replayed), unloads after `unload_after_minutes` (which also
  closes the mic), or stays loaded with `keep_warm`. Tray menu: Pause/Resume
  dictation.
- Island pill: pulsing dot (ring when hands-free), 24-bar live waveform, timer;
  spinner while warming up or transcribing; green check when done. Notices
  (model missing, mic problem, admin window) use the compact pill.
- Settings: **Dictation page** with hotkey capture (reads key state directly,
  so the Win key works; the island ignores dictation while Settings has focus),
  microphone picker, filler/punctuation/spacing/insert-method/tones toggles,
  model list with Download/Resume/Cancel/Verify/Delete and progress, keep-warm
  and unload timer, personal dictionary and snippets. First run (no model)
  shows a notice and opens this page. About > Privacy now describes dictation.

### Bugs found by testing the real flow

- **Audio buffered while the model loads lost its opening words** ("The quick
  brown fox jumps" vanished). Silero VAD in sherpa mishandles one large
  `accept_waveform` call; the worker now feeds it 512-sample windows. Burst
  sizes 0-4000 ms all return the full sentence now.
- The message loop in my own test harness dropped queued keystrokes (posted
  messages overtake input); harness fixed, product unaffected.
- A standalone dash in a transcript was dropped by the text cleaner (found by a
  unit test); fixed.

### Measurements (release, static CRT, i5-12400F)

| | |
|---|---|
| `islet.exe` | 2.18 MB (budget 8 MB) |
| `islet-asr-worker.exe` | 18.84 MB (only running while in use) |
| `islet-settings.exe` | 5.63 MB |
| island idle, no model | 17.3 MiB private, 15 ms CPU / 10 s |
| island after a dictation | 26.1 MiB private |
| worker with model loaded | 712 MiB private, 0 ms CPU idle |
| after the idle timeout | worker process gone; island 12 MiB private |
| model load | 1.8 s |
| mic open / first audio after `start()` | 19 ms / 17 ms |
| key release -> text ready (15.5 s clip) | 156-182 ms (finishing the last utterance) |
| key release -> inserted | ~430 ms, of which 250 ms is the deliberate wait before restoring the clipboard; the text is pasted ~180 ms after release |
| 15.5 s dictation, cold start (model loaded mid-recording) | complete and correct |

### Pending / open issues

- **Real microphone + real voice not tested.** The system default input on this
  PC is the *Voicemeeter* virtual device (silent); the FIFINE mic works when
  selected explicitly (Settings > Dictation > Microphone). WER with a human
  voice is untested (the clips are SAPI speech).
- **The latency budget (400 ms for a 10 s utterance) is not proven.** The clips
  end in silence, so the last utterance had already closed. With unbroken
  speech the final utterance is still decoding at release: about 0.3-0.75 s
  measured for 5-12 s segments. If real speech misses the budget: lower the
  12 s cap, give the tail decode more threads, or shorten the pause threshold.
- Not tested in Notepad, VS Code, Chrome or Windows Terminal. Per-app
  insertion overrides (terminals that dislike Ctrl+V) are not implemented.
- Not implemented: transcript history, the "record 5 s and transcribe" test
  button, hook watchdog/re-install, GPU provider, models other than Parakeet.
- The old `%LOCALAPPDATA%\Islet\models\parakeet-tdt-0.6b-v3-int8` (the `ort`
  export, ~640 MB) is no longer used and can be deleted.
- The hotkey capture button could not be clicked by automation; its logic is
  unit-tested (6 tests) but the button itself was only seen, not pressed.
- Hands-free recordings stop themselves after 10 minutes.

### P3 acceptance

- [x] Hold / double-tap / toggle / Esc through the real keyboard hook.
- [x] Text inserted at the cursor with the clipboard preserved (own text box;
  paste and type methods; emoji, accents, CJK, line breaks).
- [x] The model unloads after the timeout and memory is returned.
- [x] 15 s hands-free and push-to-talk dictations, cold and warm.
- [ ] 10 s utterance in Notepad / VS Code / Chrome / Windows Terminal within
  400 ms.
- [ ] 3-minute hands-free dictation without memory growth.

## Hotkeys: left/right modifiers, single keys, remove button (user request, 2026-10-23)

- **Sides.** Modifiers are now `Option<Side>` per family (`Any`/`Left`/`Right`)
  matched against one bit per physical key, so "Right Ctrl" and "Right Alt"
  are distinct from their left twins. Config text: `Right Ctrl`, `Left Alt`,
  `Ctrl+Right Alt+Space`; aliases `RCtrl`, `right-alt`, `AltGr`. Plain `Ctrl`
  still means either side, so the default `Ctrl+Win` works with either hand.
- **Single-key hotkeys.** A lone modifier must name a side ("Right Ctrl"; bare
  "Ctrl" would fire on every shortcut). A lone ordinary key must be one nobody
  types with: F1-F24, CapsLock, Pause, ScrollLock, PrintScreen, Insert, Menu
  (letters, Space, Enter, arrows, Home/End... need a modifier). Such keys are
  swallowed so they do not also act (CapsLock does not toggle caps). A
  modifier-only hands-free toggle also works.
- **Setting one.** Settings captures *physical* keys. A lone modifier keeps its
  side; in a combination, left-side keys mean "either side" and right-side keys
  stay right-only. Each hotkey row has a drawn "x" button that removes it
  ("Not set"); an unparsable or empty hotkey is simply disabled, and Settings
  warns when neither is set.
- **No false starts for shared keys.** A single-key hotkey that is also part of
  other shortcuts (Right Ctrl+C) used to flash the pill, play the tone and start
  loading the 700 MiB model. The controller now waits 120 ms of holding before
  showing the pill, playing the tone, or loading the model; shorter presses are
  silent.
- **AltGr.** On layouts with AltGr, Windows sends a fake Left Ctrl (scan code
  0x21D) before Right Alt; the hook ignores it so a Right Alt hotkey can match.
- Tests: hotkey 34 (sides, aliases, single-key rules, exact side matching,
  Right Ctrl hold/shortcut/double-tap/Space, F9/CapsLock swallowing,
  modifier-only toggle, removed hotkeys), settings capture 8.
- Verified through the real keyboard hook (release build, fake mic): with
  `Right Ctrl` as the only hotkey, holding Left Ctrl did nothing, a Right Ctrl
  tap and Right Ctrl+C neither loaded the model nor inserted anything, and a
  real Right Ctrl hold loaded the model once and inserted once.
- Not verified: AltGr layouts (none available), and the capture button itself
  could not be clicked by automation (its logic is unit-tested). A later
  automated run was disrupted when focus moved to another window while keys
  were being sent, so it did not re-confirm the final text in the test box.

## Simpler hotkey model (user request, 2026-10-23)

- **One hotkey, hold only.** The main hotkey is now just "Hotkey": hold to
  speak, release to insert. The tap-twice-to-lock path is gone (two quick taps
  are just two discarded taps). Config key stays `dictation.push_to_talk`.
- **Hands-free is a toggle.** `dictation.hands_free_enabled` (default off). When
  on, Settings shows a second "Hands-free hotkey" (press to start, press again
  to stop); when off, that hotkey is not even installed in the keyboard hook.
  Existing configs get the toggle off. The hold-then-press-the-hands-free-key
  upgrade still works while it is on.
- **Esc cancels with a message.** Esc while recording (including "Warming up")
  discards the audio and shows a "Stopped" pill. It has its own action
  (`Action::Cancel`); accidental taps and "this was another shortcut" still
  discard silently.
- **Notices take over the pill.** A transient notice used to be squeezed into
  the small satellite bubble while music played (an unreadable "i"). It now
  replaces the pill for its few seconds, and info-level notices ("Stopped")
  last 2.5 s instead of 4.
- Verified with the real keyboard hook in an isolated config directory (the
  user's config was untouched): hold shows "Warming up..." then the recording
  pill, Esc swaps it for "Stopped / Islet", nothing is inserted, and the music
  pill returns. Tests: hotkey 35, island 127 (core).
- Note: Esc is only intercepted while recording, as before; pressing it during
  "transcribing" does nothing.

## Sleek dictation pill (user request, 2026-10-23)

- The dictation pill is now small: 92x32 dp (was 260x44), **six bars and
  nothing else**: no timer, no dot, no text. With music playing the music pill
  shrinks into it with the existing spring morph (content blurs out, bars
  fade in; first movement ~170 ms after the press, small pill by ~300 ms), and
  grows back to the music pill when dictation ends. With no music it grows
  from nothing and hides again afterwards.
- **Colour.** `IslandState::last_music_accent` remembers the bar colour of the
  most recent music pill (the art colour, or the default blue when the art gave
  none) even after the music stops; dictation bars use it, white if there has
  never been music. Unit-tested.
- **Looks.** Recording: each bar averages four recent mic readings, newest on
  the right (calm flow, no flicker). Warming up: bars breathe. Transcribing:
  bars sweep. Done: a small check mark. Hands-free: a faint pulsing dot at the
  left edge (the only extra mark; it is how a locked-on recording is told
  apart from a held one).
- `--demo` now cycles 13 looks (added: warming up, transcribing, done,
  hands-free with a music colour, dictation over music).
- Verified on a real run with music playing (isolated config, fake mic): music
  pill -> shrink -> six orange bars -> Esc -> "Stopped" -> music pill back.
  Not visually verified: the no-music hide-after-done path (covered by a
  layout test); it needs a real insertion into a window.

- Island outline: the pill and bubble outline is now 1 dip wide at 17% white (was 0.75 dip at 7.5%), so the black island stays visible on dark windows. Checked magnified on a dark backdrop.

## Droplet show and hide (user request, 2026-10-23)

- Appearing and disappearing are now the same droplet motion the island uses
  to travel between edges, at **both the top and the bottom**. A new
  `presence` spring (0 = drawn into the edge, 1 = resting pill) scales the
  droplet's fullness together with the travel phase.
- **Show:** the pill keeps its final size from the first frame (no growing
  from zero); it wells out of its edge as a flush leaf, rounds into the pill
  and swells ~3% away from the edge before settling (`SpringParams::EMERGE`,
  0.60 s / 0.72). Content un-blurs as it fills.
- **Hide:** the exact reverse. The shape stays frozen while the content blurs,
  the pill gathers into a leaf and sinks into its edge (`RECEDE`, 0.50 s,
  critically damped, aiming at -0.12 so it slips through instead of creeping
  the last pixels). Gone in ~0.2 s after it starts gathering.
- An interrupted hide or show reverses from where it is (one spring, no
  jump). Size changes while visible (music -> dictation, expand) are the usual
  morph. Replaced `DISMISS_WIDTH`/`DISMISS_HEIGHT`.
- Debug: `ISLET_DEMO_STEPS=0,1` with `--demo` cycles only the listed looks
  (here hidden <-> music every 3 s) to watch the motion.
- Verified with ~30 fps screen captures at both edges; 4 new renderer tests
  (emerge at both edges, hide is a draw-in at both edges, interruption has no
  jump, resizes still morph).

## Stutter-proof dictation, no check mark (user request, 2026-10-23)

- **No check mark.** `DictationPhase::Done` and the controller's Done phase
  are gone: after inserting, the pill leaves at once (droplet back to the
  music pill, or hides). The done tone still plays if tones are on.
- **The model is not retrained** (Parakeet ships pre-trained; there is no
  on-device training). Stutters are handled in two places instead:
  - **Worker: sentences survive pauses.** Silero now cuts at 0.25 s, and the
    worker glues pieces into one sentence unless the pause is >= 1.0 s
    (>= 0.45 s once the sentence is 6 s long, so long dictation is still
    recognised while you talk). Glued pauses are squeezed to 0.15 s of
    silence, because Parakeet reads a long pause as a full stop ("I want to.
    I want to finish ..."). Measured with TTS clips containing 0.7-0.8 s
    stutter blocks: no false full stops; release-to-text 0.22-0.37 s on all
    clips (was ~0.3-1.2 s). Long clip: no mid-sentence seam, two small word
    slips ("Praies", "chair").
  - **Clean-up: `remove_stutters`** (runs with filler removal): repeated
    words ("I I I want"), restarted phrases up to four words ("I want to I
    want to go"), broken-off starts ("th the", "wed- Wednesday",
    "b-b-but"). Keeps deliberate doubles (very very, no no, had had), real
    hyphenated words (re-read, co-op) and short real words (in inside).
- `dictate_wav` now also prints the cleaned text (`typed:`).

## Performance audit (2026-10-25)
- Audited release builds across hidden idle, static notifications, compact/expanded music, settings idle/minimized, normal user configuration, and loaded ASR worker idle/decode.
- Fixed controller spin on disconnected microphone/hook channels and stopped microphone capture on speech-worker failure. Added three synthetic-input regressions; removed per-iteration deadline allocation and reused outgoing audio buffers.
- Cached the static settings background by rectangle and skipped configuration cloning on unchanged redraws. Isolated egui replay: 109.8 ms -> 4.3 ms for 1,000 warmed background paint passes (~25x for that function).
- Measured hidden/settings idle at no observable CPU over 5-second samples. Two warmed compact-music runs: ~30 FPS, p95 34.12-34.24 ms, ~0.36% total CPU on 12 logical processors. Normal configured app: ~82 MiB working set and ~0.5% total CPU, with no ASR worker loaded. Loaded ASR worker: no observed idle CPU over 5 seconds, ~691 MiB private memory. Local speech fixture: ~266 ms release-to-final; cold model load ~2.3 s.
- 188 tests and broad all-targets Clippy with warnings denied passed. Release app/settings/worker rebuilt. Optimized release app is running with the original user configuration; audit sessions used isolated temporary configuration and were stopped.
- Full findings and scope are in PERFORMANCE.md. Native cursor movement was denied, so live hover/drag timing was not validated; raw hover rows are annotated invalid. No GPU-power trace, physical hot-unplug, or extended leak soak was performed.

## High-refresh animation (2026-10-26)
- Verified the primary display runs at 180 Hz. Shape transitions already followed that cadence; removed the 30 FPS content-animation cap so all visible animation follows the compositor clock.
- Added allocation-free, time-based audio-bar interpolation without increasing audio capture frequency, and limited DXGI queued frames to one. Settled rendering still blocks without a busy wait.
- Release demo frame intervals: transitions median 5.5605 ms / p95 5.956 ms; content median 5.563 ms / p95 5.867 ms. Content previously measured median 33.337 ms.
- Warmed eight-second samples: compact music 1.69% total CPU and 57.5 MiB working set on 12 logical processors; hidden idle no measurable CPU and 18.6 MiB. GPU power and physical scanout were not measured.
- 156 relevant tests, scoped all-targets Clippy with warnings denied, and release build passed. Updated app relaunched with the user's original configuration. Details in PERFORMANCE.md.

## Pause media during dictation + frame skipping (2026-10-26)
- New `dictation.pause_media` setting (Settings > Dictation), off by default and for existing configs. Sends a pause once recording is confirmed, so short accidental taps never interrupt playback. Works with the music widget disabled.
- Renderer skips settled content-only vblanks that would look identical, using a hash quantized to half a device pixel. Expanded music now measures 0.026% total CPU; it previously drew every vblank and was not measured at 180 Hz; compact procedural bars fell from 1.69% to 1.33%. Still 180 Hz pacing. Details in PERFORMANCE.md.

## Two players at once (2026-10-26)
- The media service tracks up to two music players in the order they started playing (`MAX_PLAYERS`). The first-started stays in the main pill; the second shows as a round bubble beside it: circular album art inside a thin accent ring showing track progress (dims when paused).
- Clicking the bubble expands the island with that player's full controls; the first player moves into a slightly larger bubble (`SWITCH_BUBBLE`, 44 dp) beside the expanded card, so another click swaps back. The choice sticks: collapsing keeps the opened player in the pill and the other in the bubble, until the user switches again or that player goes away.
- A paused player keeps its place for `media.pause_hide_seconds`, and indefinitely while the pointer is on the island, so nothing swaps out under the cursor. A lone paused player behaves exactly as before.
- Alerts and agents still take the bubble ahead of a second player. Transport commands and "open app" go to whichever player is shown. Island window widened 488 -> 520 dp (settings preview slot to match).
- Demo looks 13 (two players) and 14 (expanded with switcher). Verified by posting clicks to the demo island: bubble opens the second player, clicking again swaps back. Not yet verified with two real GSMTC players at once.
- Clicks wait for the island to finish moving: a press or release while the shape is still morphing (`morph_box` set, or traveling between edges) is ignored, so a click aimed at the small pill never lands on an expanded control. Measured expand morph ~0.8 s; verified a mid-expansion click on play/pause is ignored and the same click after settling pauses.
- New `general.expand_on` setting (Settings > General > Behavior > "Open the island": Hover / Click). Default Hover, unchanged for existing configs. In Click mode hovering never opens the island; a click does, and moving away still closes it after the usual delay. Preview caption follows the choice. Verified live: in Click mode a 0.9 s hover left the pill collapsed and a click expanded it.
- Music filter: YouTube shows with guests were shown as music ("INDIA'S GOT LATENT S2 EP8 ft. ..." matched the bare "ft." marker; "EP8" escaped the "ep." check). Episode numbering (EP8, Ep 12, S2E5, Season 3) and stand-up/sketch/roast/prank/challenge now mean not music; "ft./feat." only counts bracketed as a song credit or with the "Artist - Song" shape. Verified against the live session.
- YouTube videos whose title does not look like a song are still music when the video's channel shows YouTube's "Official Artist Channel" badge. The tab reader finds that tab's page through UI Automation and accepts only the owner's badge (an Image styled ytBadgeShape); sidebar recommendations carry the same label on Group elements and are ignored. Checked at most twice per tab title (once more if the page was loading), ~120 ms each on the media thread, only for title-rejected YouTube tracks. Verified live: Samay Raina (Verified, not artist) -> no; Emiway Bantai official video -> yes. Background tabs in browsers that expose only the active page fall back to the title heuristic.

## Enter / exit / edge-travel animation redesign (2026-10-26)
- Show, hide and top<->bottom travel are now designed tweens (`islet_core::tween`: cubic-bezier `Ease` + `Tween`, closed-form like `Spring`, so identical at any refresh rate). Retargeting mid-flight carries the current speed via a cubic Hermite, so interrupted shows and reversed trips turn around without a jump. Size morphs while visible keep their springs.
- Travel: one symmetric ease-in-out (0.84 s top to bottom): drawn into the leaving edge as a droplet, fastest at the invisible midpoint, welling out of the other edge as the mirror image. Before, a spring left the top edge in ~0.2 s and arrived over ~0.6 s.
- Show: eases in from the first frame (no ~60 ms dead start) and lands softly in 0.56 s with no overshoot swell (removed the >1 presence "bloom"). Hide: soft start, accelerates into the edge (0.36 s).
- Leaf gather spread over more of the motion (smoothstep 0..0.62).
- Measured at 180 Hz (before -> after): max width step per frame show 9.7 -> 7.5 px, travel 14.2 -> 7.5 px, hide 14.6 -> 7.8 px; largest frame-to-frame change in that step 2.3-3.6 -> 1.2-1.3 px; show settles 0.76 -> 0.49 s. Live burst captures of the demo confirmed the top-edge droplet, invisible handoff, and bottom-edge emergence.

## Stay over empty space (2026-10-26)
- New `general.stay_over_empty_space` (default on; Settings > General > Behavior, shown under "Dock over apps"). When an app window is behind the island's home spot, the island samples the composed screen there (a tight strip: the compact pill's footprint plus 10 dp left/right and 2 dp top/bottom, never the expanded size; its own drawn pixels plus 1.5 dp excluded) and stays at its edge if it is one flat colour (per-channel tolerance 14, every 3rd pixel). Busy content (text, tabs, a search box) docks it as before.
- Cost: one BitBlt of a ~240 x 40 dp strip per check: ~0.23 ms CPU, ~5.5 ms wall (GPU readback wait) on the UI thread. Runs on window events (already debounced) and every 1.5 s only while an app is behind the island; nothing on an empty desktop. Pixels are compared in memory and discarded.
- Hysteresis: busy docks immediately; returning to the top needs two empty readings in a row. Never re-evaluated while expanded or under the pointer.
- Verified with an isolated instance (sliver idle) and a test window: plain -> stays on top; text added (no window event) -> docked via the re-check; text removed -> returned to top.
- Verified after tightening: text right under the pill docks it; the same text ~20 dp below the pill leaves it on top.

## Dictation always at the bottom (2026-10-26)
- `effective_anchor_top` is false while dictation is active, so a top island travels to the bottom edge as the droplet trip when dictation starts and back to the top when it ends (the existing travel machinery: click-through while moving, region, tween).
- Renderer defers a new layout received while the island is still leaving its edge (`Renderer::deferred`), and applies it instantly at the hidden midpoint (`apply_deferred` each frame): the departing droplet keeps its old look and the new one wells out of the other edge, with no morph at either edge. A trip reversed before the midpoint applies it as the normal morph instead. Benefits any layout change that coincides with docking.
- Verified with demo burst captures (music <-> dictation): music pill drawn into the top, dictation pill emerging at the bottom, and the reverse. New renderer test pins the deferral.

## Position: Dynamic / Top / Bottom (2026-10-26)
- `general.position` is now `dynamic` | `top` | `bottom`. Dynamic is the previous behaviour with docking on (rests at the top, glides to the bottom while an app is behind the island, unless that space is empty); Top and Bottom are fixed and never dock (no window hooks, no screen reads).
- The separate "Dock over apps" switch is gone. Old configs migrate on load: `position = "top"` with `tuck_behind_windows = true` becomes `dynamic`; `top` with it false stays `top`; `bottom` stays `bottom`. The legacy key is read only and never written back, so a later choice of Top is not re-migrated. "Stay over empty space" is shown under Position only in Dynamic.
- Dictation still always shows at the bottom, in every mode except where it already is.
- Verified with isolated instances and a test window: Dynamic docks only while text is under the island and returns; Top never docks; Bottom is always at the bottom.

## Music pill shrinks after 3 s (2026-10-26)
- A track that has played for 3 s (`MEDIA_CONDENSE_MS`) shrinks the main music pill to `MEDIA_CONDENSED` (= the dictation pill, 92 x 32) showing only the art and the bars. The timer is armed when a track starts playing or changes, not on timeline/volume/scrub updates of the same track; a pause, a new track or nothing playing restores the full pill with its title (and the timer restarts when playback resumes). Hover/click expansion still opens the full player and collapses back to the shrunk pill.
- Applies to the music pill wherever it is the main pill (single or split with an agent/second-player bubble). Notifications and dictation keep their sizes. `IslandState::resting_size` is also what the docking checks use as the island's footprint.
- Renderer: `draw_media_compact` fades the title/artist out between 200 and 150 dp wide, moves the art to a 4 dp concentric inset and centres the bars in the remaining space, all following the spring's width. Demo look 15 shows the shrunk pill.
- Tried smaller bars in the shrunk pill and reverted: they keep the full pill's size.

## Shared-element expand / collapse (2026-10-26)
- Modelled on the "One Shape" web animation (D:\Codes\Animated Website). Previously a card switched from its small to its big layout in a single frame when the springing pill crossed 60 dp tall, so the art jumped and text/controls popped.
- Now a card between its looks is composed in passes (`Gfx::draw_morph`): the small look and the big look are each laid out at their final size and centred in the moving shape (revealed by it, never reflowed), rendered into the existing offscreen content layers and blurred/faded on the GPU. They take turns: small fades out over 0-35% open, big fades in over 50-95% (`morph_fades`), with a 0.965 -> 1 scale. A music card's album art is drawn once, sharp, interpolating position, size and corner radius from the compact disc (`compact_art`) to the player cover.
- "Open" is measured from the size spring's height (not the droplet-squeezed shape), between the card's small size (`media_rest`: compact or shrunk) and its big one (MEDIA, or NOTIFICATION for notifications/agents). Not used during content cross-fades or droplet travel. At rest nothing changes: the extra passes run only while a card is opening or closing.
- Verified with burst captures of expand, collapse and a notification opening; unit test pins no-overlap of the looks, the ends, and the art's start position.
- Shrunk music pill revised: 104 x 32 (wider than the dictation pill), art at the left, three bars at their normal size and 8 dp spacing at the right with a 14 dp margin (`SHRUNK_BARS`, `SHRUNK_BARS_RIGHT`), leaving clear space after the art. Bars 4 and 5 fade out as the pill shrinks.
- Revised again: the shrunk pill keeps all 5 bars (same size and spacing), right-aligned with a 14 dp margin, and is 108 x 32 so there is clear space between the art and the bars.
- Shrunk pill bars peak lower: travel 15 -> 12 dp (`SHRUNK_BAR_REACH`, eased with the shrink), max height 18 -> 15 dp. Full pill unchanged.
- Expanded player enlarged ~8%: MEDIA 448 x 112 -> 484 x 120 (corner 38). Layout table in `media::expanded` rescaled (art 88 at 16 dp, text column at 120 / 164 wide, controls at y 74 spaced 44, scrubber y 96, times y 102, player column 300, side panel up to 156 wide, chip 38 tall, volume row at 64, track 86). New const test keeps the play-button hit circle meeting the seekbar slop exactly and everything inside the card. The island window was already 680 dp wide (changed by another session together with the settings preview), so no window change was needed.
- Capsule ends: the outline's corner handle was a fixed squircle (0.48 x radius), which flattened the ends of fully round pills. `corner_control` now blends to the true-circle handle (0.5523) as the corners become fully round (radius -> half the short side), so pills have real half-circle ends while the expanded card keeps its softened corners; blended, so resizing never jumps. Also: fixed the morph midpoint test for the new player height, and switched result checks to case-sensitive matching with --no-fail-fast (an earlier filter could hide a failing crate).
- Music pill art centred in the round end: `compact_art` uses the same inset from the end as from the top and bottom ((height - 24) / 2: 6 dp in the full pill, 4 dp in the shrunk one) instead of a fixed 8 dp from the left. Used by both the resting pill and the moving art in the expand animation.
- One art gap everywhere: `expanded::ART_INSET` = 4 dp between album art and the edge of its pill, with the art's corners following the pill's (concentric). Full pill art 24 -> 28 dp, shrunk 24 (unchanged gap), expanded cover 88 -> 112 dp at (4, 4) with radius = card corner - 4 (34), second-player bubble art inset 6 -> 4. Expanded text column now starts after the cover (TEXT_X derived), PLAYER_WIDTH 312, and the card widened 484 -> 496 so the side panel keeps its full 156 dp.
- Made the customize test's size round-trip comparisons tolerant to float rounding (exact == failed for 496 at scale 0.8); the behaviour it checks is unchanged.
- Album art is now filled with the island's own outline geometry (`GeometryCache` squircle, true half-circles when fully round) instead of a plain rounded rectangle. The expanded card's corners are squircles, so a circular-cornered cover inset 4 dp left an uneven, wider gap around each corner; with the same curve family and radius = card corner - inset, the gap runs parallel all the way round. Verified at 6x zoom; test pins that card and cover use the same corner handle.

## Settings workspace and sizing (2026-10-26)
- Replaced the settings shell using the user's dark canvas and narrow sidebar references: compact 164 dp navigation, Nunito headings, graphite panels, matching native caption, and a static dotted island preview. Existing settings and Claude's media, dictation, and motion changes are retained.
- General > Position now has Customize sizing directly below it. The full-screen editor previews the real island and keeps draft preferences for each edge: 0–5% added vertical spacing and 80–125% uniform expanded size. Apply copies only these four preferences; Cancel/Escape restores the window without saving the draft. Existing configs retain 0% added spacing and 100% size.
- Preview attachment retries recover from startup delays and island restarts; changed anchors refresh their geometry. The embedded slot is 720 x 168 dp to accommodate the current 496 x 120 player and its second-player bubble at 125%. Expanded content and hit coordinates scale together; compact and condensed pill dimensions remain unchanged.
- Integrated sizing with Claude's shared-element morph by measuring opening progress in unscaled content coordinates. This prevents an 80% expanded card from remaining partially faded. Existing morph timing, art paths, and blur passes are preserved.
- Validation: 193 tests across app/core/render/settings and scoped all-target Clippy passed; render tests additionally cover scaled morph endpoints. Native probes verified attachment and rectangle alignment, window resizing, recovery after an island restart, keyboard entry into customization, and Escape restoring the window without saving. Captures cover all settings pages, minimum window size, and both full-screen edges. Captures combine native PrintWindow with actual GPU pixels at measured window coordinates; desktop z-order and mouse-driven Apply were not visually verified. Temporary capture instrumentation was removed from shipping code.

## Sizing editor interaction verification (2026-10-27)
- Added an isolated egui event test that drives the actual editor widgets through keyboard focus. It verifies Reset edge stays a draft; Cancel and Escape discard changes; Apply preserves the other edge and unrelated settings; and values survive atomic save/reload. Slider input reaches, but cannot exceed, 5% spacing and 125% expanded scale.
- The test disables native preview attachment and uses a temporary config file, so it cannot alter the user's running island or preferences. The new interaction test and settings all-target Clippy pass. No production behavior or visual design changed in this continuation; native mouse automation remains unavailable.

## Scrollable calculator widget (2026-10-27)
- Added an opt-in Calculator under Settings > Widgets. It stays available without music and supports arithmetic precedence, percentage, sign, clear/delete, result chaining, and error recovery. Dictation and urgent notices retain priority.
- Scroll over the expanded island to switch between the current activity and Calculator. Following the user's instruction, navigation arrows and their click targets are removed; the Scroll label and page dots are informational only. Precision-wheel deltas accumulate with burst gating.
- Calculator input follows its editable tail; long results use a smaller font. Its shared geometry keeps rendering and click coordinates aligned at 80–125% expanded size. The settings preview grows only when Calculator is enabled, and its existing health repaint restores native stacking after focus changes.
- Preserved Claude's dual-player, condensed pill, sizing, and shared-element motion changes. Calculator uses the existing native renderer and event loop, adds no worker or polling timer, and settled identical snapshots skip redraws. See PERFORMANCE.md for measured costs and limits.
- Validation: 201 tests passed across app/core/render/settings; scoped all-targets Clippy with warnings denied and the release build passed. Native wheel messages verified agent → calculator → agent; real native button messages verified long entry and scientific results. Final raw desktop captures cover all three sizes, the Widgets pane, and minimum settings size in `.impeccable/review/widgets/`.

## Music title visibility (2026-10-27)
- Increased the music condensation delay from three to five seconds so the title and artist stay visible longer on playback start, resume, and track changes. This uses the existing one-shot timer.
- Updated the existing state-machine test to assert the five-second timer explicitly; all 37 island tests pass.

## Right-side widget indicator (2026-10-27)
- Removed both Scroll labels inside the island. The two non-clickable page dots now form a centered vertical stack on the expanded card's right edge, with 12 logical units between centers and an 8-unit edge inset. Removed the old label's source-text width restriction.
- Navigation stays scroll-only. Native wheel input verified agent → calculator → agent; raw captures cover agent, media, and calculator at 80%, 100%, and 125% without overlapping controls.
- All 21 renderer tests, renderer all-targets Clippy, formatting check, and the release build passed. The updated normal app was relaunched; captures are in `.impeccable/review/widget-dots/`.

## Calculator keyboard input (2026-10-27)
- Added native character input for number-row and numpad digits, decimal point/comma, arithmetic, percent, Enter/equals, and Backspace. Delete/Escape clears; F9 changes sign. Windows handles keyboard layouts and normal key repeat; coalesced repeats share one render snapshot.
- The expanded calculator can take focus on a click. Wheel selection requests focus where Windows permits it; clicking is the reliable fallback under foreground restrictions. Focus is leased from the previous window and returned on mouse leave, widget changes, priority content, hiding, travel, or shutdown. Selecting another app discards the lease without reclaiming focus. Input requires both calculator visibility and native foreground/keyboard focus; Ctrl/Alt/Windows shortcuts are not calculator characters.
- No global keyboard hook, polling loop, thread, or dependency was added. The ordinary island retains its no-activation style outside the calculator page.
- Validation: 171 app/core tests passed, including typed arithmetic/editing; scoped all-targets Clippy, formatting check, and release build passed. Native SendInput verified number-row `12.5+7.5` gives `20` and numpad `2+3*4` gives `14`; a posted digit while another window had focus was ignored. The external test text box received `427`, and mouse-leave focus restoration passed using the native activation handler. Raw captures and results are in `target/calculator-keyboard-test/`. Synthetic wheel-focus attempts were denied by Windows; click activation was verified. The updated app was relaunched with normal preferences.

## Widgets only in expanded mode (2026-10-27)
- Corrected the earlier compact-widget assumption following the user's explicit instruction. Calculator is now eligible only while expanded. Enabling it preserves the configured Hidden/Sliver idle appearance; collapsing restores music or agent content even if Calculator remains the selected widget.
- Removed compact calculator lettering, icon drawing, and the bubble fallback so transitions cannot expose a widget name. Expanded layout, keyboard input, right-side vertical dots, and scroll switching are retained. Settings copy now describes expanded-only widgets.
- Validation: all 204 app/core/render/settings tests passed, including two new regression cases; scoped all-targets Clippy, formatting check, and release builds passed. Native captures verified expanded Calculator -> compact music/agent, plus unchanged idle sliver and an empty Hidden region. Evidence: `.impeccable/review/expanded-widgets-only/`. The normal app was relaunched.

## Dictation clean-up: spoken corrections and bullets
- transcript.rs: "I mean / no wait / or rather / actually no" swap the last 1-2 words for the corrected ones, "scratch that" drops the clause, stray mid-sentence "oh" is a filler, "bullet point"/"new bullet" commands, and a "here are a few points" intro turns the following sentences into a bullet list. Weak cue "I mean" only fires mid-clause.
- Restate cue: "sorry my bad / my bad / my mistake / let me rephrase" + a redo sentence replaces the earlier try (matched on its first two words).
- More built-in cleanup: filler padding (basically / let's say after a linking word), restart rule (can I just can't we just), more cues (no I meant, strike/forget/ignore that, oops, let me start over), more fillers (er, ehm, mmm). All shipped in the app, nothing for the user to enter.
- Automatic lists: announcements ("I want three things", "some ideas for X:", "here are a few points") with 2+ sentences after, or spoken markers (first/second/number one/step two/next/finally; needs a first and a second) become "- " bullets. Grammar models tested (grammar-synthesis-small: mangles tech words; t5-base: safe but ~348 MB / ~600 MB RAM / 0.2-0.6 s per sentence for few fixes) and not shipped.

## Optional AI cleanup (on-device LLM)
- Tested: grammar-synthesis-small (mangles tech words), t5-base (safe, few fixes, ~348 MB/600 MB RAM), Qwen2.5 0.5B/1.5B via llama.cpp (plain prompt answered questions). Shipped Qwen2.5-1.5B-Instruct Q4_K_M + llama.cpp b11927 CPU server as an opt-in download (Settings > Dictation > AI cleanup, ~1.14 GB, SHA-256 pinned; zip unpacked to models/ai-cleanup/runtime).
- islet-asr/polish.rs: llama-server on a loopback port, hidden window, few-shot prompt with <dictation> tags; started while the user speaks, unloaded with the speech model.
- dictation.rs: rule-cleaned text goes to the model on a helper thread with a budget (2.5 s + 60 ms/word, max 15 s); transcript::accept_rewrite refuses answers, new content, lost question marks; any failure types the rule-cleaned text.
- Known: llama.cpp release needs the VC++ runtime; a missing DLL shows an "AI cleanup off" notice. Few-shot prompt not yet verified live (auto mode blocked running llama-server from the scratchpad).

## System stats widget
- WidgetConfig.system_stats (off by default). Widget pages generalised: IslandState.widget: Option<WidgetKind> replaces calculator_selected; the wheel steps activity -> calculator -> stats; dots show page count.
- islet-app/sysstats.rs: GetSystemTimes (CPU), GlobalMemoryStatusEx, GetIfTable2 (physical adapters up), PDH GPU Engine 3D utilisation + Thermal Zone temperature. TIMER_STATS: 1 s full readings while the card is on screen, 2 s CPU-only otherwise (for the glow); no timer when the widget is off.
- Hot glow: 3 readings >= 90% light a static orange halo on the main pill (draw_heat_glow), off below 80%.
- Not visually verified (screenshot script blocked by auto mode).

## Always-on island
- general.always_on (default true) + general.glance: ResumeOrClock (default) | Clock | Stats. IslandState.glance/glance_info, ContentCard::Glance; GLANCE_CLOCK 108x32, GLANCE_STATS 164x32; opened, the glance uses the notification size (big time + long date, or CPU/Memory tiles).
- ResumeOrClock: paused music is never dismissed; after the pause delay it shrinks to cover + play triangle (right half of the pill resumes on click). With no music, the clock.
- clock.rs: GetTimeFormatEx/GetDateFormatEx in the user locale; TIMER_CLOCK fires on each minute boundary only while a clock glance is in use. Stats glance reuses the sampler (CPU + memory every 2 s).
- Not visually verified.
- Last music persists: last_music.rs saves the playing track (text + raw cover) under LOCALAPPDATA\Islet once per track/cover; with Resume + clock and no player open, it is shown paused and small at once. Play relaunches the player (open_media_source) and presses play when its session appears (30 s window). Paused live sessions are no longer dropped in this mode. Widget page choice stays in memory only, so a restart opens on the music again.
- Correction: the remembered song is for the opened island only (IslandState.remembered, SetRemembered). The collapsed pill keeps the time; stale paused sessions are dropped again as before. Opening with no player open shows the last song first, then the widgets.
- Opened island always starts on a music player (live > remembered > empty "Nothing playing" card). Pages: player, then the running agent (WidgetKind::Agent), calculator, stats. Collapsed behaviour unchanged (agent/music/time on the pill; urgent notices and dictation still take over).
- Removed the paused cover + play pill and the Resume + clock mode: paused music leaves after the pause delay as before and the glance (Clock default, or Stats) shows; hovering opens the player. Config value resume_or_clock is read as clock.
- Fix: play on the remembered song did nothing when its player was still open but paused (dismissed from the island). remembered_action now sends the command to the live session; a closed app is relaunched and resumed; a closed browser opens a YouTube Music search for the song (no tab address is available).
- Fix: play on the remembered song no longer falls back to any open session (it started WhatsApp audio). Only the remembered app is controlled; if it was closed it is relaunched and, once its session appears, played; if none appears within 4 s, WM_APPCOMMAND play/pause is posted to that app's own window. Chat apps (WhatsApp, Telegram, Discord, Teams...) are never remembered as the last music.
- YouTube music video missing: the filter accepted it, but the WhatsApp app session also counted as a music player and, with paused Spotify, filled both player slots. Chat apps are now never music (music::is_chat_app, SessionFacts.app), and a playing session always takes a slot before a paused one. New example media_why prints each session and the filter verdict (console only).
- Dynamic mode bouncing: the empty-space check now judges only a 2 dp ring beyond the small pill's 1 dp edge (5 dp with the heat glow), reads every pixel, never samples while the island travels or morphs, excludes the resting footprint (not the last drawn target), and needs two agreeing readings before moving either way during re-checks (a window newly behind still acts at once).
- Firefox reports one media session (the last tab to play), so a YouTube Music tab playing behind a newer video tab was invisible. tabs.rs now reads which tabs are audible (Firefox "Mute tab" button / Chromium "playing audio"), YouTube Music's player bar (title, byline) from a background tab, and presses its player-bar buttons via UIA Invoke. The media service adds such a tab as a player (HiddenTab, id "browser#tab", polled every 2 s only while it exists) when the browser session itself is a rejected non-music track. Live-tested reading the bar; the hidden-player path itself was not live-tested (the tab was closed by then). media_why now folds the artist badge into its verdict and shows playback state, music tabs and audible tabs.

## Rename to Onyx + app icon

- User-visible name is now **Onyx** (store listing: "Onyx Island"): settings title "Onyx Settings", sidebar, About, tray tooltip, "Quit Onyx", dictation notices, startup entry.
- Internal ids stay "Islet" on purpose (crate names, window classes, props, mutexes, %APPDATA%\Islet and %LOCALAPPDATA%\Islet) so existing settings, models and the remembered song carry over.
- Startup: the Run value is now "Onyx"; the old "Islet" value is deleted whenever the startup setting changes.
- Icon: `assets/onyx.ico` (polished onyx pill with a live dot on a black tile; 16-256px, small sizes drawn with a heavier rim), generated by `assets/make_icon.py` (Pillow). Embedded with `embed-resource` from each exe's build.rs, along with VERSIONINFO (FileDescription "Onyx" / "Onyx Settings"). The settings window uses `assets/onyx-64.rgba`. The tray keeps its procedural white glyph.

## v1.0.0 packaging

- Tray icon is now the Onyx app icon (resource 1 from `assets/onyx.ico`, loaded at SM_CXSMICON so the heavy-rim small frame is used). The procedural glyph in `icon.rs` is the fallback.
- "Start with Windows" uses the MSIX startup task (`OnyxStartup`, declared in the manifest, off by default) when running packaged; the Run key otherwise. A packaged app's HKCU Run writes are virtualized and never read.
- Workspace version 1.0.0.
- `packaging/build.ps1` builds `dist/Onyx_<ver>.0_x64.msix` (unsigned; Store signs) and `dist/Onyx-Setup-<ver>.exe` (NSIS, per-user, no admin). Pass `-IdentityName/-Publisher/-PublisherDisplayName` from Partner Center for a Store-ready MSIX.
- Both ship vcruntime140.dll + vcruntime140_1.dll app-local (the exes import them; the UCRT is part of Windows 10+).
- MSIX logos are generated by `packaging/make_assets.py` (scale + targetsize/unplated variants) and indexed with makepri.
- Store risk: AI cleanup downloads llama-server.exe (native code) at runtime, which Store policy 10.2.2 may reject. Bundle the runtime in the package or hide AI cleanup in the Store build if certification flags it.


## Dictation corrections and rewrite checks - 30 October 2026

- Added bounded weekday/time, quantity, and recipient corrections; preserve the existing time for weekday-only repairs and avoid consuming the next instruction.
- Rewrite checks protect observable numbers/units, calendar terms, named recipients, negation and selected action direction. Truncated model replies are refused. Rejection reasons contain no transcripts.
- 193 core and 17 ASR tests pass in target/cal-fast; Clippy is clean; static-runtime release build succeeds.
- Actual Round 3 Q8 comparison: 6/6 new complex passages satisfy targeted fact checks (baseline 1/6); 28/28 unsafe mutations rejected (baseline 0/28); 6/6 correct references accepted. One model output uses rule-cleaned fallback.
- The filter remains conservative: reference compatibility on the broader corpus is 997/1200 versus 1183/1200 before; unresolved ambiguous corrections can cause extra fallbacks and retain rough grammar. This is not semantic verification.
- Experimental tester defaults to the complete rule/model/filter path, with optional raw mode. App model selection and Qwen prompt are unchanged. Detailed evidence is in D:/Codes/onyx-cleanup-model/PIPELINE-RESULTS-2026-10-30.md.


## Experimental Gemma long-input testing

- Added bounded streaming chunk processing to the standalone tester with per-piece and combined fact checks, repetition cancellation, and local fallbacks.
- Corrected negation-scope scanning so a previous negative clause does not absorb an independent later command. All 210 Rust tests and seven chunk tests pass.
- Seven real inference passages preserve checked facts. The long example keeps 4/4 rewritten chunks; three warm runs average 2.162 s versus 3.668 s for monolithic requests that all hit their output limit. Small single-PC timing sample; model and grammar limits remain.
- Experimental evidence: D:/Codes/onyx-cleanup-model/CHUNKING-RESULTS.md. Native model selection remains unchanged.


## Native Onyx Cleanup integration - 30 October 2026

- Replaced the Qwen registry entry and few-shot prompt with Onyx Cleanup / Gemma 3 270M Round 3 Q8, using the exact training prompt. Internal ai-cleanup id and settings are preserved.
- Ported bounded streaming chunks into Rust: per-chunk checks, repetition/output limits, rule-cleaned fallbacks, and a final combined check. No Python process is needed for app dictation.
- Settings > Dictation > AI cleanup identifies the experimental Gemma model and local installation. Installed and enabled on this PC; defaults remain off on other installations.
- Model is supplied locally beside the release executables, checksum verified on installation. Stage it with tools/install-cleanup-model.ps1 -ModelPath <GGUF>. No public model upload or installer/Store package was produced.
- Model SHA-256: 6597d3a39bb64f9ac86f055fa82c9546bad618010ec71f2a806cebef93c2dd06. Installed size 291,545,504 bytes (278.04 MiB). Old Qwen model files removed from this PC's ai-cleanup folder; CPU runtime retained.
- Validation: 193 core + 21 ASR tests passed; core/ASR Clippy with warnings denied passed; static-runtime release builds of app and settings passed; diff whitespace check passed.
- Actual native Polisher: 7/7 synthetic complex passages preserve the targeted facts and pass the guard. Warm debug runs: 904/768/629/700/600/618 ms, long passage 2723 ms, four CPU inference threads. These are single local text-inference measurements, not microphone-to-insertion latency or proof of semantic equivalence. Runtime shutdown verified.
- Live microphone-to-insertion behavior and other languages were not tested. Conservative fallbacks and grammar limitations remain. Full local record: D:/Codes/onyx-cleanup-model/NATIVE-INTEGRATION.md.


## Spoken-count correction regression - 30 October 2026

- A real dictation exposed a missing punctuated "No, I'm sorry" cue and incomplete English cardinal parsing. The guard rejected the model's corrected count and retained rough text. Prior passage checks did not cover this form.
- Added straight/curly-apostrophe cues, full cardinal quantities through billions, conservative counted-noun repairs, and canonical word/digit comparisons. Actual number changes and independent counts remain protected; ordinary apologies remain intact.
- Logs now distinguish unchanged rule-cleaned fallback from an applied rewrite and include data-free chunk rejection reasons. No dictated content is logged.
- 195 core + 21 ASR tests pass; Clippy passes. The reported text and an additional unseen spoken-count passage pass actual native inference checks (307/251 ms in one warm debug run). All seven earlier native passages still pass. These check explicit counts, filler/correction removal, and selected facts, not arbitrary grammar or speech-recognition quality.
- Corpus compatibility remains 997/1200 references accepted and 150/150 unchanged inputs retained. No model retraining or relaxation allowing changed quantities was used.


## Live long-dictation follow-up - 30 October 2026

- User confirmed reading the supplied correction cues aloud, but the pasted final text lacked them and retained both recipients/counts. The original live raw ASR text/audio was not retained, so the stage that lost those cues cannot be established from that final text alone.
- Fixed a reproducible guard/formatting defect: spoken clock minutes and explicit spoken decimal/version points now compare with their written forms. Spoken punctuation formats these before filler removal, preserving zero/oh inside decimals. Missing version separators or digits are not invented. Filename spelling remains a limitation.
- Added data-free recognized/remaining candidate correction-phrase counts in the app. These are presence diagnostics, not semantic accuracy measurements. No raw transcripts or audio are logged.
- Controlled Microsoft David synthetic speech probe preserved all three apology cues plus actually/I mean with both whole-clip and actual VAD-streamed Parakeet recognition. The streamed audio -> native rules -> Gemma -> guard check passed recipient, weekday/time, count, duration assertions (556 ms warm debug cleanup in one run). This is synthetic speech evidence, not a reproduction of the user's microphone recording.
- 197 core + 21 ASR tests pass, Clippy passes, corpus reference compatibility remains 997/1200 and unchanged cases 150/150. Rebuilt and relaunched development app with cargo run -p islet-app --offline. No speech model/decoder was changed. Live cue-loss remains to isolate with a short user test.


## Numeric dictation output - 31 October 2026

- Added deterministic final number presentation immediately before insertion, after model/fallback selection. Counts and amounts use digits, including 210 checks, 24500 rupees and 6 compatibility tests, even when Gemma returns number words.
- Reuses the cardinal/decimal/time readers and verifies unchanged quantities. Preserves line breaks, surrounding whitespace, quoted wording, likely proper names, pronoun one, filenames and existing identifiers. No extra inference or model request is involved.
- 199 core + 21 ASR tests pass; Clippy with warnings denied passes. Two actual native Gemma-to-final-format cases pass numeric output and rewrite guard assertions (164/324 ms in one local warm debug run). Live microphone insertion remains user-tested separately.
- Development app rebuilt and relaunched with cargo run -p islet-app --offline. No settings or unrelated website changes made for this fix.


## Explicit denial/replacement correction - 31 October 2026

- Added bounded "No, it is not X, it is Y" / contracted / "not X but Y" quantity corrections. The denied value must match the preceding quantity, with compatible stated units and repeated counted nouns. Unrelated facts, mismatched values/units, ordinary negations and quoted values remain unchanged.
- Preserves inherited suffix units and following clauses; does not carry an obsolete sentence end into the middle of a continuing correction. Candidate correction counts include these phrases.
- 201 core + 21 ASR tests and Clippy pass. Three actual native model cases pass: reported count contrast (189 ms), spelled count plus deadline/negation (220 ms), unrelated denial retaining all counts (331 ms). One warm debug run; no microphone latency claim. Corpus compatibility remains 997/1200 references accepted, 150/150 unchanged inputs retained.
- Development app rebuilt and restarted. Extra name fragment remains unresolved: requested clarification whether the speaker hesitated/restarted or clearly said the name once. No global removal of the legitimate name particle De was introduced. No transcript logging or user-specific dictionary override was added.


## Recipient and qualified-denial contrasts - 31 October 2026

- Extended verified denial/replacement handling to explicitly addressed recipients, including full names and repeated address prefixes. The entire denied recipient must match the preceding addressed name; partial or unrelated names stay unchanged. Legitimate particles such as de/van are retained.
- Shared bounded bridge/modifier parsing now permits actually/really/exactly before the denied or replacement value. Quoted modifiers, unrelated facts, prohibitions and mismatched amounts are preserved.
- Reported combined recipient/count passage now resolves to Maya and 210 checks in native inference. Three actual native cases pass (182/235/155 ms warm debug in one run): combined correction, full-name/deadline/negation case, and mismatched denial preserving all names.
- 203 core + 21 ASR tests and Clippy pass. Corpus reference compatibility remains 997/1200; unchanged inputs remain 150/150. Development app rebuilt and restarted via cargo run -p islet-app --offline. Live microphone behavior remains user-tested; no transcripts are logged.


## Spoken pointers and bullet layout - 31 October 2026

- Added pointers as an announced-list noun and generalized named ordinal scaffolding, including "the first one is", "and the second feature will be to", and "third feature will be to". Opening report sentences remain before the list, related body sentences stay with their item, and ordinary ordinal descriptions such as first/second car remain prose.
- Rewrite checks now require an existing bullet list's item count to survive inference. Flattened or merged lists retain the rule-formatted text instead. Numeric formatting already preserves line breaks. This adds no model request.
- 206 core + 21 ASR tests and Clippy pass. Three actual native cases pass item-count/fact checks: reported three pointers (315 ms), related negative instruction (181 ms), and announced unranked pointers (127 ms), one warm debug run. Corpus compatibility remains 997/1200 and unchanged inputs 150/150. Native text-path evidence does not establish live microphone accuracy or general grammar quality.
- Rebuilt and restarted the development app with cargo run -p islet-app --offline. No changes to the unrelated website or settings design.


## Settings preview visibility - 31 October 2026

- Minimizing or hiding Settings now detaches the live island to its normal topmost desktop position instead of suspending/hiding it. Attachment requires a visible, non-minimized, non-cloaked settings root; invisible child anchors are still supported.
- Settings suppresses attachment while hidden and retries when restored. Native attachment rechecks visibility at transfer completion. Duplicate detach requests are idempotent, rapid restore can reconnect the current anchor, and cancellation of a pending attachment clears its acknowledgement property.
- Uses existing WinEvent hooks and handoff animation; no new polling timer or renderer was added.
- Verified using actual native settings/island windows: visible attachment, minimized desktop return/topmost, ignored stale hidden attachment, restoration, rapid minimize/restore, hidden-window return, show/reattach, minimizing during pending attachment, and final minimized/visible-desktop state. All nine checks pass. Reproduce with tools/check-preview-visibility.ps1 while app and Settings are open; it leaves Settings minimized. Virtual-desktop cloaking is gated in code but was not exercised.
- 13 app + 10 settings tests pass in target/cal-fast; debug builds succeed. Both development executables rebuilt; Onyx is running with Settings minimized. No transcript content is logged and unrelated app/website changes are preserved.


## Copy completed dictation to clipboard - 31 October 2026

- Added Dictation > Text > Copy dictation to clipboard, on by default. Missing keys in existing configs resolve to on; explicit off choices persist through TOML save/load. Stored as dictation.copy_to_clipboard.
- Final, number-formatted text is retained after Paste insertion and copied after Type insertion. Off keeps prior behavior: Paste snapshots/restores the prior clipboard and Type leaves it untouched. Paste retention skips the old snapshot/restore delay. Clipboard history/cloud exclusion flags remain in place; no transcripts or clipboard contents are logged.
- Clipboard copy failure after successful typing is reported separately so successfully inserted text is not treated as a failed insertion.
- 207 core + 5 input + 13 app + 10 settings tests pass (235 total); core/input Clippy including all targets passes. Native synthetic edit-control probe verified actual inserted text plus clipboard contents for Paste/Type with retention on and off. Original clipboard was preserved in memory and restored after verification. Live microphone-to-clipboard behavior remains user-tested separately.
- Updated the existing inject_probe example with --keep-clipboard verification and data-free failure output. Rebuilt and relaunched both development executables and opened Dictation settings at the Text controls.


## Windows clipboard history correction - 31 October 2026

- User confirmed that Ctrl+V retention was not the complaint: dictation was missing from Win+V. Runtime config was on and current clipboard metadata still had ExcludeClipboardContentFromMonitorProcessing=1 and CanIncludeInClipboardHistory=0. The initial implementation retained temporary-paste exclusion flags even for saved copies.
- Retained copies now omit the monitoring exclusion format and request local Windows history with CanIncludeInClipboardHistory=1. Temporary/off-mode paste remains excluded; CanUploadToCloudClipboard stays zero. Settings help explicitly describes Ctrl+V and Win+V when Windows history is enabled. Earlier notes about history exclusion describe the pre-fix behavior.
- Extended inject_probe --keep-clipboard to verify current text plus actual history eligibility/cloud flags for both methods. --history-marker adds an owned unique fixture. Verified that fixture appears via the real Windows GetHistoryItemsAsync/GetTextAsync API. Removed only owned synthetic history entries and restored the original clipboard after the test; unrelated history was preserved. No clipboard/transcript text is printed.
- 235 core/input/app/settings tests pass; core/input all-target Clippy passes. Rebuilt both development executables and restarted Onyx; copy_to_clipboard remains enabled in saved settings. Fresh live dictation is ready for user verification.
- Windows format contract: https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats#cloud-clipboard-and-clipboard-history-formats


## Conversational pointer grouping - 31 October 2026

- Recognizes bounded leading and/so wrappers around list markers and bare ordinal copulas such as "the third is". Explicit first/second/third sequences now keep unmarked continuation sentences (including an RGB alternative beginning Or) with their existing item instead of falling back to one bullet per sentence.
- Declared separate alternatives such as "here are two options" remain separate bullets. Ordinary ordinal prose and a lone first marker remain prose. Existing structural rewrite checks continue to protect item counts.
- 209 core + 21 ASR tests and Clippy pass. Native verification now checks per-item term membership, not just total bullet count. Three real Gemma cases pass: reported RGB alternative grouped with custom lighting (470 ms), same grouping with a negative instruction (230 ms), separate declared options (143 ms), one warm debug run. Corpus reference compatibility remains 997/1200 and unchanged inputs 150/150.
- Rebuilt and restarted development Onyx. Live microphone recognition remains user-tested separately; no new model calls, transcript logging, or changes to clipboard/settings behavior were introduced.

## Round 4 pipeline repair and refined model work - 31 October 2026

- Added verified full-clause restatements for explicit correction cues. Only one addressed recipient or quantity may differ; surrounding words must match. Direct denied-count restatements require the same old value and repeated noun on both sides. Mismatches, unrelated clauses, quoted instructions and prohibitions remain protected. All 38 development repair references now pass the native guard after deterministic cleanup, versus zero before this work.
- Literal multiline input retains paragraphs, bullet symbols and indentation. Explicit announced first/second/third lists also work when recognition omits all punctuation; alternatives remain with their item. The 16 selected formatting development cases retain correct bullet membership and numbers through actual native inference with both existing models.
- Extended addressed-name checks to ask/give and lowercase surname continuations. Filename/path/address spelling and case are protected literally. Bullet safety checks compare each item's facts, scope and retained content, beyond checking only the item count. A new authored 158-word regression caught a dropped calibration instruction in the pilot; both current model versions now pass that regression through the guarded native pipeline. These are synthetic text checks, not live microphone accuracy.
- 214 core + 21 ASR + 13 app + 10 settings tests pass; core/ASR all-target Clippy and app/settings build/check pass. An additional 410 authored unsafe fact mutations are all rejected. No new AI requests or idle timers were introduced. Rebuilt the development executables and relaunched with cargo run -p islet-app --offline; the installed model remains Round 3.
- External research workspace: D:/Codes/onyx-cleanup-model. A real native harness now exercises Rust clean, Polisher, outer guard and number formatting with app budgets. An isolated evaluation environment runs ERRANT 3.0.2 with spaCy en_core_web_sm 3.8.0; evaluation persists scores only. On 128 grammar development cases before the final list safeguards, Round 3 F0.5 was 0.36984 and the pilot 0.36300. The pilot was therefore not promoted despite improved repairs/preservation.
- Reviewed public grammar labels and excluded non-minimal content changes from the next training set. The refined set has 28,102 rows: 10,492 grammar, 4,006 repairs, 1,056 formatting and 12,548 preservation. Added corrected grammar identity examples and current-rule list inputs; verified no conflicting identity labels. This heuristic label audit is not semantic verification or distribution approval.
- Refined training is detached and ongoing: 384 steps from the pilot at LR 2e-5; answer-token mix 55/20/10/15 for grammar/repairs/formatting/preservation. See training-r4-refined-status.json and its timestamp, not only the log tail. Checkpoints include optimizer state. The supervisor exports Q8, measures native development quality and the long regression, and uses the untouched test split only if development gates pass. No candidate is installed automatically, and the PC stays on. Further training results and live-app candidate verification remain pending.

## Refined Round 4 final evaluation - 31 October 2026

- All 384 refined steps completed, but the first final save failed when D: ran out of space. Resumed the intact step-320 optimizer/sample-order/RNG checkpoint, recomputed only 64 updates, saved the complete final model on C:, exported GGUF, then restored the D: HF folder with checksum verification. No existing model was replaced. The final GGUF is 291,545,536 bytes, SHA256 79c81ab5f09540370223c8995c14ed8fbacf7029d196ef312333bd0b9457c587.
- Native development grammar F0.5 improved 0.36984 to 0.42739 with precision 0.47531 to 0.50739, passing the development gates and the 158-word complex regression. The independent 369-case test then failed grammar qualification: on 135 grammar cases, precision fell 0.45876 to 0.40848 and F0.5 was 0.32951 versus 0.32868. These are ERRANT single-reference edit scores, not general dictation accuracy.
- Test preservation improved 117/145 to 137/145; repair exact matches 5/57 to 16/57; formatting bullet membership 0/20 to 9/20; joined exact matches 2/12 to 9/12. The test also exposes unresolved runtime correction/list forms; it is now a regression set, not an untouched set for another qualification claim.
- Decision: do not promote. The development app remains running on Round 3 with the tested deterministic pipeline fixes. GPU training and evaluation are finished; no further run starts automatically. Full metrics and limitations: D:/Codes/onyx-cleanup-model/ROUND4-REFINED-RESULTS.md. The PC stays on, and live microphone candidate verification was not performed.

## Round 5 selected and integrated locally - 31 October 2026

- Audited public grammar failures and quarantined 17 legacy repair inputs already damaged by older preprocessing; historical results remain unchanged. Built 25,076 training pairs using current native cleanup and actual chunk boundaries, with grammar, preservation, repair and formatting examples. No larger architecture or tokenizer was introduced.
- Completed 256 training steps from Round 3 at LR 1e-5. Selected the better step-64 checkpoint on development data according to the frozen selection contract before opening the reserved final test. The final step-256 export was not installed.
- On 160 fresh four-reference JFLEG grammar cases, native-pipeline ERRANT precision improved 51.13% to 57.82%, recall 30.54% to 33.15%, and F0.5 45.06 to 50.33. These are grammar-edit metrics, not overall dictation accuracy, and are not comparable to previous single-reference CoEdIT absolute scores. Authored repairs matched exactly 14/16 versus 10/16; bullet membership passed 8/8; already-correct text stayed unchanged 16/16 versus 10/16. All 40 authored protected-fact checks passed. Median cleanup 221.5 ms versus 224 ms; p95 592 ms versus 445 ms on four CPU threads.
- Added bounded punctuation-free repeated-clause corrections, verified reverse recipient replacements, comma-separated explicit list markers, literal balanced-quote preservation, and question punctuation for direct requests recognized with a period. No new model calls or idle timers were added.
- Synthetic David/Zira speech probes passed 7/8. The remaining David case lacked the intended name in recognizer output; cleanup cannot safely invent it. Natural microphone validation remains separate. No raw transcripts, audio or clipboard contents are logged.
- Installed and checksum-verified onyx-cleanup-270m-r5-selected-q8_0.gguf (291,545,536 bytes, 278.04 MiB), SHA256 760e148a14c0057a086f47efbec76f738ce817901141e45a58d9566dee67cf8f. Registry/settings model name is Round 5; installer pins selected weights. Old Round 3 remains available. Five actual installed-path native checks passed, including the long complex regression.
- 262 core/ASR/app/settings tests passed; core/ASR all-target Clippy with warnings denied passed. Rebuilt development app/settings offline and relaunched Onyx via cargo run -p islet-app --offline --quiet. Saved AI cleanup is enabled. Unrelated Claude/app/website changes were preserved. PC stays on.
- Full report: D:/Codes/onyx-cleanup-model/ROUND5-RESULTS.md. User microphone validation and distribution provenance/license review remain pending; no Store package was built.

## Dictation punctuation and numeric formatting regression - 31 October 2026

- User testing exposed retained deadlines/count corrections and non-numeric output. Fixed an actual whole-pass formatting defect: spoken integer version/build/code/pin values now have the same fact type as their written forms, so they no longer cancel numeric formatting elsewhere. Numeric identifiers retain strict rewrite checks; separated version fragments are not joined or changed to guess the intended version.
- Short Sorry/Actually corrections can now cross an ASR-inserted period only for matching typed values or the same repeated counted noun. Questions, standalone No, unrelated units/nouns, quotes and ordinary predicate continuations remain intact. Weekday plus explicit at supports an hour without inventing AM/PM; a redundant at before the replacement weekday preserves the original deadline relation. An independent next clause retains its sentence boundary/capitalization.
- A bounded interrupted-cardinal span can be discarded for an explicit repeated-noun correction only if it contains a scale and a known broken numeral prefix, with no ordinary words. The old value is never guessed; the replacement must parse completely. Other malformed phrases remain intact.
- 221 core + 21 ASR + 13 app + 10 settings tests pass (265). Core/ASR all-target Clippy with warnings denied passes. Eight installed Round 5 native checks pass, including the prior long regression, unrelated facts, identifier formatting and a new combined punctuation/interrupted-count/list case. Native checks use authored fixtures, not saved user dictation. Whole-workspace rustfmt check reports existing unrelated formatting differences; only the two edited helper modules were formatted.
- Rebuilt development app/settings and restarted Onyx via cargo run -p islet-app --offline --quiet. Model weights, architecture and inference request count are unchanged. The historical Round 5 benchmark scores precede this follow-up; these targeted checks are not a new independent grammar score. A version already split into ambiguous numeric fragments remains a recognition limitation. Natural microphone retesting is pending.

## Accepted local release finalized - 31 October 2026

- User accepted the current behavior and requested finalization. Built optimized release islet.exe, islet-settings.exe and islet-asr-worker.exe offline from the current workspace, preserving unrelated changes. No new training or model architecture changes were made.
- Final checks: 270 core/ASR/input/app/settings tests pass. All eight installed-model checks also pass through the optimized release cleanup example, including the long passage, interrupted-count/deadline/list case, identifier formatting, literal quotes and preservation of unrelated facts. These repeat regression checks, not a fresh grammar benchmark.
- Staged the selected step-64 Round 5 GGUF beside the release app and verified both staged and installed hashes: 760e148a14c0057a086f47efbec76f738ce817901141e45a58d9566dee67cf8f, 291,545,536 bytes. Installer now skips identical verified copies, allowing an already loaded model to remain mapped without a failed overwrite. Round 3 remains available for rollback.
- Launched target/release/islet.exe. Finalized for local use; no Store submission, public package or distribution license approval is implied. Existing ambiguous ASR version fragments remain a documented limitation. PC stays on.

## MSIX refreshed with bundled cleanup - 31 October 2026

- Rebuilt dist/Onyx_1.0.0.0_x64.msix using packaging/build.ps1 -SkipBuild -MsixOnly after rebuilding the release app/settings/worker. Package size 306,435,426 bytes (292.2 MiB); SHA256 a9ccf710bd2b18d06d1f4940c8efff73cf53ee20d219c72cbf378826a5a4efe6. Identity remains Nevdev.OnyxIsland, version 1.0.0.0, x64, existing publisher.
- Fixed missing cleanup payload in packaging: selected Round 5 GGUF plus pinned CPU server/DLLs and app-local VC++ DLLs are included. Payload hashes are verified before staging. Runtime executes inside the package; only model data is copied to writable storage. ModelManager caches the bundle root and prefers the bundled runtime; verified local files are also preferred over matching network downloads. No new polling timer or inference request was added.
- Packaging workspace deletion is guarded within target and refuses a linked workspace. Added -MsixOnly and recursive payload staging. NSIS source now includes/removes its owned model/runtime payload, but the setup EXE was not regenerated or install-tested in this MSIX-only run.
- 272 core/ASR/input/app/settings tests pass (including two new packaged-runtime tests); ASR all-target Clippy passes. SDK packaging/unpacking succeeded. All 39 staged files matched package hashes; model and root/runtime CRT DLL presence verified. Fresh isolated install from the unpacked package passed all eight real cleanup checks. No runtime was copied into the isolated model directory or downloaded. This validates unpacked payload behavior, not signed MSIX installation/container behavior or Store certification.
- MSIX is unsigned, not installed and not submitted. Dataset provenance/license review remains open before public distribution. Verification report: target/release/onyx-cleanup-msix-verification.json. Rebuilt release app relaunched; PC stays on.
# OrbitDrop drag-to-convert integration - 1 November 2026

- Committed the pre-integration workspace as `553bcb7` (Checkpoint Onyx before OrbitDrop converter integration). Preserved prior dictation/model work and supplied OrbitDrop sources.
- Added native OLE file-drop support on the island. Dragging opens an expanded-only conversion card; drop on a format to convert, or drop on the card and choose with mouse or Tab/Enter. Compact island behavior and scroll widgets remain separate. Settings > Widgets has an enabled-by-default File converter toggle and reserves sufficient embedded-preview space at supported scales.
- Reuses OrbitDrop's unmodified ImageConversion, PDF, MediaFoundation and MediaEditing providers through a self-contained .NET 8 helper. PDF to PNG/JPG, image formats/PDF, audio and supported video formats come from the actual provider catalog. Office-backed conversions are excluded because the authored Word-automation probe stalled; they are not advertised as working.
- Outputs use the original collision-safe write logic beside source files. Multi-page PDFs create a new page-image folder. Added progress, cancel, unsupported/error recovery and show-in-folder. No idle converter process, polling timer or perpetual animation was introduced. Helper teardown does not wait on the UI thread; cancellation retains partial completion counts.
- Verification: 298 normal Rust tests passed across core/ASR/input/render/app/settings; final app/settings rerun passed 24 tests. Final all-target core/render/app/settings Clippy passed with warnings denied. A real-helper lifecycle test passed three conversions with the cancellation pipe open, verifying nonblocking UI teardown. All 23 authored helper checks passed, covering PDF pages, image/audio/video conversions, rejection, cancellation, collision safety and unchanged originals.
- Initial cross-process native OLE direct-drop and drop-then-keyboard flows passed with authored fixtures and actual converted files. Later desktop automation reruns were unreliable at the source-drag step and are not counted as new passes. Actual native GPU state captures cover five states at 80/100/125%; independent finish review returned SHIP with no material fixes. Installed MSIX validation is separate.
- Built and launched the optimized release app. Rebuilt unsigned `dist/Onyx_1.0.0.0_x64.msix` (420,104,248 bytes, 400.6 MiB); SHA256 `ffd86191273a45717d5608a5dc1b74aeaa4e4f6b8b69bf5eef0c9a5e44a933c7`. Windows SDK unpack passed; all 530 staged files matched package/extracted hashes. The extracted helper passed the same 23 fresh conversion checks, and the selected cleanup model hash remained unchanged. No signed installation or Store submission was performed. Verification: `target/release/converter-msix-verification.json`. No converter helper remains running at idle.

## Converter selection and result refinement - 1 November 2026

- Refined format choices with stronger contrast and taller single-row controls. Result states now have stage-aware subtitles, a drawn success check only on complete success, aligned status text and centered full-width recovery/reveal buttons. Partial and canceled outcomes retain accurate messaging and access to saved results.
- Native GPU state captures and core converter tests passed; app/render compile checks passed. Updated release app; the earlier MSIX still represents the previous converter appearance.

## Media-shaped converter layout - 1 November 2026

- Converter now matches the expanded media shell at496x120 logical units. A112square preview at4,4 uses the media cover corner radius34; filename, status, format controls and actions occupy the right column. Non-previewable files use a native file/extension icon in the same tile.
- On-demand converter catalog obtains a128square Windows Shell thumbnail for single files, passes premultiplied BGRA over private IPC, and reuses the bounded album-art GPU cache. Multi-file selection retains the icon. No idle service or animation was added; thumbnail failure falls back to the icon.
- Four core converter tests, five-state native GPU captures at three scales, all23 conversion checks, and a real128square image-thumbnail check passed. The prior MSIX retains the earlier layout. Release app rebuilt for local testing.

## Roomier converter - 1 November 2026

- Increased converter height from120 to160 logical units at user request, retained496width and112square preview centered vertically. Added vertical spacing between filename, subtitle, formats/status and actions; format targets now32high and outcome actions26high. Updated native hit geometry to match.
- Four core converter tests and native five-state GPU captures at80/100/125percent passed. Release app rebuilt for local use; prior MSIX is unchanged.

## Converter proportions and thumbnail correction - 1 November 2026

- Rebalanced the converter to544x184 logical units. Content preview now144square inset16 horizontally and20 vertically, with20-unit corners instead of the strongly rounded media cover. Right column starts at180;36-high format targets have10-unit row gaps. Action geometry remains synchronized.
- Raised on-demand Shell thumbnails to256square. Verified actual authored image thumbnail extraction and rendering, fallback state captures at three scales, and four core converter tests. Added optional authored-preview input to the GPU capture fixture and an eight-format fixture to cover the reported crowded state. No idle work was added. The previous MSIX remains unchanged.

## Converter media spacing restored - 1 November 2026

- Kept requested544x184 outer dimensions. Applied the actual media spacing:4-unit cover inset,176square cover,34-unit radius,16-unit content gap, titley20 and subtitley41. Right-column controls and hitboxes moved together.
- PNG/JPEG/BMP/GIF/TIFF previews now try a bounded direct image decode before Shell thumbnails, avoiding dependence on Explorer thumbnail cache. Authored PNG preview extraction passed; four core converter tests and native five-state captures at three scales passed. The MSIX remains the previous build.

## Latest MSIX packaged - 1 November 2026

- Packaged current media-spacing app and direct-image-preview helper into unsigned Onyx_1.0.0.0_x64.msix (420,106,103 bytes). SHA256 1c9fa42191ab8dfb32c212a9d9eda2e2807b2633566c20f7bfb240c268e6a808. Windows SDK unpack passed; all 530 staged files matched archive/extracted hashes. The packaged helper passed23 conversion checks and the256-square PNG preview check. Selected cleanup model hash unchanged. No signed install or Store submission performed.

## Converter package size reduction - 1 November 2026

- Excluded native debug PDBs and unused non-English satellite resource assemblies in the helper publish. Added cleanup of only those stale artifact files within the verified converter output, since dotnet publish does not remove obsolete files. Kept self-contained .NET runtime and licenses. No optional download or separate runtime installation was added.
- Rebuilt MSIX from420,106,103 to386,067,365 bytes, saving34,038,738 bytes. SHA256 8aa2351550626bd2c196cd4a8a5c3e9c2ee261097503d4311dce08c66358c6d6. SDK unpack, hashes of all307 staged files,23 packaged converter checks and PNG preview passed. Cleanup model hash preserved. Package remains unsigned; no signed installation or Store submission performed.

## R2 optional cleanup model - 1 November 2026

- Connected the selected Round 5 model to the supplied R2 HTTPS object URL. Downloaded and verified all291,545,536 bytes against the existing SHA256 pin. The endpoint supports HTTP Range requests; downloader uses an Onyx User-Agent, existing retries, partial-file resume, cancellation and final hash validation. R2 development URL is for testing; a production custom domain remains recommended.
- MSIX and NSIS staging now include only the pinned CPU runtime, excluding the GGUF. Existing installed models remain recognized. Settings describes the one-time offline model download; size/progress and initial Missing status exclude an already bundled runtime. Rule-based cleanup and safety checks remain bundled. All23 ASR and10 settings tests and Clippy passed.
- Rebuilt unsigned MSIX: 113,872,116 bytes (108.6 MiB), SHA256 907adf6b023bc71d185d12e1c8c36e68a9838e6e1b1dd43208f1a6b7bafb600e. SDK unpack and all306 staged-file hashes passed; archive contains no GGUF. Actual Rust ModelManager installed from R2 into a fresh isolated model root, verified checksum, and ran three authored native cleanup checks successfully using the packaged runtime without copying/downloading executable code. No signed installation or Store submission performed. Verification: target/release/r2-msix-verification.json.

## GitHub cleanup download host - 1 November 2026

- Replaced R2 development URL with the cleanup-r5 GitHub release asset in N3vdev/onyx-cleanup. Public asset size and GitHub SHA256 match the pinned selected weights. HTTP Range request verified. Only the model is hosted; runtime and rule-based checks stay packaged.
- All23 ASR tests passed. A fresh isolated native ModelManager install downloaded the full model from GitHub, verified SHA256, and passed three authored cleanup cases using the bundled runtime without copying/downloading executable code.
- Updated unsigned MSIX: 113,871,989 bytes (108.6 MiB), SHA256 cae3a1b19189e431657bbb5bbe346d33e622b6f0d493a7e4ebde9f87b7e3369e. SDK unpack and all306 staged-file hashes passed; no GGUF in package. No signed install or Store submission performed. Verification: target/release/github-msix-verification.json.

## Store duplicate package version repair - 1 November 2026

- Incremented workspace/app version to1.0.1 and MSIX identity to1.0.1.0, retaining the existing Store Name, Publisher and x64 architecture. Rebuilt app, settings and speech worker; app/settings PE versions verified as1.0.1. Added a packaging guard against overwriting an existing package version and verified that guard rejects reuse before mutating staging.
- Created Onyx_1.0.1.0_x64.msix: 113,872,173 bytes, SHA256 a4993cba1c9b651f85cbebfe8eda07243121cfcd2ab135b822f126c11ce3b57c. SDK unpack, manifest version/identity and all306 staged-file hashes passed. Model remains an optional GitHub download; no GGUF is included. No signed installation or Store submission performed. The rejected1.0.0.0 draft upload should be replaced with this new package.

## PDF tools in the island (converter)

- Dropping PDFs on the island now offers PDF tools before format conversions (max 8 buttons, `PdfMenu.cs`): Merge (several PDFs), Compress, Split, Rotate (90° right), Page numbers, Watermark, Protect, then PNG/JPG. Images offer "One PDF". A password-locked PDF offers only Unlock.
- Merge/Compress/Split/Rotate are OrbitDrop's existing `PdfToolsProvider` (now registered; Tools mode queried). OrbitDrop sources stay unchanged.
- New `OnyxPdfTools.cs`: page numbers ("3 / 12", bottom centre, upright on 0/90/180/270 rotated pages), diagonal text watermark sized to 70% of the page diagonal, Protect (AES-256, PDFsharp `SetEncryptionToV5`), Unlock (opens with the password, saves without encryption). Segoe UI is supplied through a small `IFontResolver` (PDFsharp's default finds no Windows fonts).
- `InputWindow.cs`: the one question a tool needs (watermark text, password typed twice, unlock password) in a small dark WPF window; Enter/Esc; nothing typed is logged.
- Outputs are new files beside the original ("x (numbered).pdf", "x (protected).pdf" ...); originals untouched.
- Converter now strips a leading UTF-8 BOM from the request line, and reports Skipped outcomes' messages (e.g. Compress "already as small as it can get").
- Verified through the JSON IPC: catalog for one/two/locked PDFs, Merge, Split, Rotate, Compress skip message, Page numbers on all rotations (rendered and checked), Watermark (rendered), Protect -> locked -> wrong password refused -> right password unlocks. Not yet tried by dropping files on the live island.
- Page numbers removed at the user's request (menu entry, operation and drawing code). One PDF now offers: Compress, Split, Rotate, Watermark, Protect, PNG, JPG.
- Fixed: the converter card's open-folder button opened Documents instead of the output. Command::arg quoted the whole `/select,<path>` argument, which Explorer can't parse for paths with spaces (every output name has one, e.g. "(watermarked)"). Now passes `/select,"<path>"` via raw_arg (file_converter.rs, with a test); checked that Explorer opens the output's folder with the file selected.

## Settings compact island dock (2026-11-05)
Removed the dedicated preview card/grid/captions. Settings reserve 64 logical pixels for the native pill and retain a 280-pixel transparent native expansion surface over the controls, with no reflow. Sidebar reduced to 150 pixels; unified caption/background, rounded quieter groups, stronger group headings. Existing native preview ownership/minimize handling and all settings behavior retained. cargo check and all 10 settings tests pass. Live hover/minimize visual verification pending.


## Collapsed island drag gestures (2026-11-05)
Added captured screen-coordinate drag handling with 8-DIP slop, directional axis lock, 48-DIP horizontal and 64-DIP vertical commit thresholds. Carousel cycles normal content, clock, and CPU/RAM; urgent content retains priority and expanded widgets are unchanged. Vertical gestures persist Top/Bottom only when direction changes the edge. Analytic render springs return pointer-following offsets/stretch to rest; updates use the existing compositor-paced loop. No idle gesture timer. Mouse capture loss cancels; media bubble clicks remain intact. Preview and auto-tucked states currently exclude gesture initiation. Automated app/core/render tests pass (17/226/21, three ignored platform/helper captures). Physical touchscreen and live 180-Hz feel/performance are not verified.


### Mouse gesture follow-up
Removed gesture eligibility exclusions for auto-tucked pills and settings previews (preview vertical gestures do not change desktop position). Start capture can now tolerate compact morphs. Added 8-ms mouse-button fallback sampling only while held for non-activating window capture; unchanged coordinates produce no render commands. Cancel/release kills this timer. Vertical direction uses actual visible edge. Empty normal page is skipped so idle carousel cannot disappear. cargo check and gesture tests passed. Native mouse smoke check pending.


Native mouse smoke verification: real mouse down/move/up against demo fixture successfully enlarged the drag region and cycled stats -> media (222-pixel region) -> clock (110-pixel region), including bottom-docked placement. Restarted normal app after test. Pointer automation helper is in target only.


### Directional pull refinement
Visual feedback begins before the 8-DIP gesture classification threshold. Pointer targets use a 90-ms critically damped tracking spring; release uses a 320-ms spring with 0.82 damping. Pill width/height increase toward the dragged edge with smooth near-zero magnitude and bounded growth (76/44 DIPs). Existing rubber resistance, navigation thresholds and capture behavior retained. No new timers/workers; stationary holds do not submit new render commands. Focused renderer geometry and gesture tests passed.


### Pull content separation and fade
Shell uses elastic bounds; content retains unstretched dimensions centered inside it. Prevents vertical pull accidentally invoking expanded media layout above 60 DIPs and keeps cover/text/bars proportional. Content opacity fades smoothly from full toward 18% with increasing pull, returning with the existing spring. 23 renderer tests passed; native GPU rest/horizontal/vertical captures generated and visually checked in target/pull-content-captures/comparison.png. No additional layers or background work.


## Dictation: English v2 by default, uninstalled models greyed out

- Default speech model is now Parakeet TDT 0.6B v2 (English) everywhere: `DEFAULT_MODEL_ID`, the config default, and `find_or_default` (which was hard-coded to v3; now follows the default). v2 is listed first; summaries read "Recommended for English" / "For other languages". Existing configs keep their chosen model.
- Why: the same 7 dictated sentences scored 4/7 on v3 and 6/7 on v2 (v3 misheard "Xcode" as "X Code", dropped "check the", and heard "old one" as "old enough").
- Settings > Dictation: a model that isn't downloaded is drawn at 40% opacity and its selector is disabled (hover: "Download this model to use it."); the Download/Resume button stays at full strength.
- "Rewrite with AI" (dictation.ai_cleanup) is now on by default. It only runs once the AI cleanup model is installed (`ai_ready` checks both); until then the rule-based cleanup is used. Deleting the model no longer switches it off.
- Settings scrolling: egui scrolls 40 points per wheel notch, about 2.5x slower than native Windows apps. `ui::install_scrolling` now sets `line_scroll_speed` to the user's Windows 'lines to scroll' (SPI_GETWHEELSCROLLLINES, default 3) x 33 points, about 100 points a notch.
- Default dictation hotkey is now Right Ctrl (was Ctrl+Win); hands-free stays Ctrl+Win+Space and off. Existing configs keep their hotkey.
- Dictation cleanup: new `resolve_quick_swaps` pass (after resolve_corrections) for cue-less self-corrections: 'enter not enter but enable' -> 'enable' (needs the same word on both sides of 'not'), and adjacent question words 'know how when' -> 'know when' (no punctuation between). Ordinary 'not red but blue' and 'what, when and where' stay; tests cover both.
- Investigating: the dictation pill sometimes doesn't appear although dictation works. Each dictation now logs one 'dictation pill requested' line (window visible/rect, fullscreen_hidden, preview attached/pending, edge, tucked, traveling, morphing); no dictated text. Leading suspect: the surface left 'departed' after a Settings-preview handoff (window visible, nothing drawn).
- Pulling the island to the other edge now glides: the window spans the full work area at both edges (`compute_window_layout` no longer makes a compact bottom window), and `reposition_layout` treats a home-edge change with the same geometry as no window change (`WindowLayout::same_geometry`). Releasing a pull saves the new edge and the renderer plays the same droplet travel as Dynamic docking (drawn into the edge it leaves, welling out of the other) instead of springing back and teleporting. `WINDOW_LOGICAL_HEIGHT` removed.

- Music swipes on the pill (gestures.rs): while the collapsed pill shows the song, a sideways swipe of 48 px changes track (left = next, right = previous), and a push into the edge the pill sits on plays or pauses. A push away from the edge still moves the island to the other edge. The cursor stops at the screen edge, so the push threshold is 60% of the room between the press and the monitor edge, clamped to 4–24 px, and the edge gives a little (0.45) under the push instead of almost nothing. With the clock or stats glance showing, sideways swipes still page between glances.

- Startup greeting (islet-render/src/intro.rs, `ContentCard::Intro`, `IslandState::intro`, `INTRO` 150x38). On every launch (not the demo) the pill wells out of its edge whatever the idle style. The Onyx ring pops in with a lavender halo, a spark runs once around it, and the core fills. "Onyx" slides out from behind the mark, then one soft light band sweeps across the pill and its rim catches it. After INTRO_MS (2.6 s) the app clears `intro` and the island cross-fades into its real state, or sinks back into the edge when idle is hidden. The renderer starts the greeting's clock on its first frame after the device exists (`intro_since`), so a slow cold start doesn't skip beats. Dictation, a needs-input notice, or opening the island take over at once. Verified by filming the bottom strip at startup.
- Startup greeting redrawn as the app icon (no "Onyx" text). The pill (INTRO, now 112x40, the icon's proportions) stays pitch black. A silver rim traces itself in from the middle of the right end both ways round, each trace led by a bright head, and meets at the left end. The lavender dot rides in along the upper orbit line, which is revealed behind it with a short glowing tail. It lands at the icon's dot position (85%, 50%), pops, and sends out one ring. Then the lower orbit line and the top-left gloss mark fade in, a soft sheen sweeps across, and the dot glows gently until the greeting ends (INTRO_MS 2.9 s). The outline and orbit paths are sampled in intro.rs (`outline_point`, `orbit_point`, with tests). Verified by filming the bottom strip.
- Dictation pill bars (islet-render/src/voice.rs, dictation.rs `Loudness`):
  - **Live while loading.** The bars show the live mic from the first frame, including while the speech model loads (about 2 s cold). The audio is already buffered then. Before, they showed a canned breathing pulse that looked stuck.
  - **No more maxed-out bars.** The meter learns the noise floor (drops at once, creeps up about 1 dB/s) and the loudness of recent speech (jumps to peaks, eases back about 3 dB/s). It places each 33 ms reading between them with a 1.5 power, so steady speech sits mid-range and only stressed syllables reach the top. Release is faster (0.42) so syllable dips show.
  - **Each bar moves on its own.** Each bar reads a different delay of the level history (middle newest, edges older) and has its own reach (edges shorter), so speech rolls outward instead of six equal bars.
  - **Smooth motion.** Each bar is an under-damped spring (k 520, damping ratio 0.52) stepped every frame, so motion is smooth at the display rate with a small wobble.
  - **Opening animation.** When the pill opens, the bars ripple up from the middle out (crest 0.62, 0.3 s, staggered 50 ms) and settle into the voice. A faint shimmer keeps a silent pill alive.
  - Verified by filming a held push-to-talk (cancelled with Esc).

## Voice assistant (phase 1: fixed commands, offline)
- **Hotkey.** A third hold-to-speak hotkey, `dictation.assistant_hotkey`, default "Right Alt", on by default (`assistant_enabled`). HotkeyFsm `set_command` / `Action::StartCommand`; `Phase::Ptt` carries `command`. A lone Alt or Win sends the harmless key so menus and Start don't open. A tap, or Right Alt used with another key (AltGr typing), discards as usual. Hook: `KeyboardHook::set_command`.
- **Parsing.** `islet-core/src/command.rs` turns the cleaned transcript into a `Command`, with tests:
  - Open, switch to or launch; close; minimize or maximize; show desktop.
  - Music: play, pause, next, previous; "play X (on Spotify)".
  - Volume: up, down, set N%, mute.
  - "type ..." (keeps the words as spoken); web or site search; screenshot; lock.
  - It strips politeness and fillers ("can you", "please", "hey Onyx"). Anything else returns `None` and the island says so.
- **Execution.** `islet-app/src/assistant.rs`:
  - **App list.** Read from `shell:AppsFolder` (desktop and Store apps, 196 here), listed in the background when the assistant turns on, and refreshed once on a miss.
  - **Name matching.** Exact, then joined ("whats app"), prefix, all words, then one or two letters off. Aliases cover vs code, files, edge, store and similar. Uninstallers and readmes are pushed down; ties go to the shorter name.
  - **Open windows.** Matched by program name (plus a few known exe names) or the title ending in the app name. The desktop and taskbar are excluded, so "close" can never hit Program Manager. Opening an app that is already open brings it to the front (an empty mouse input to pass the foreground lock). Close sends WM_CLOSE, so apps can still ask to save.
  - **Websites.** Well-known names, or anything with a dot, open in the browser.
  - **Play X.** Spotify search if it is installed or named, otherwise YouTube Music search. "Play my music on Spotify" starts or focuses the player and presses play after 3.5 s.
  - **Volume.** Up and down use the volume keys (10%, Windows shows its display). Set and mute go through `islet_audio::volume::set_master_once`.
  - **Media.** Goes to the island (`UiUpdate::Media`, `voice_media`), where "play" and "pause" act only when they would change something.
  - **Typing.** Uses the dictation insertion method.
- **Feedback.** The listening pill's bars turn lavender (`DictationCard.command`, INTRO_ACCENT). Results show as a short island notice ("Opening Spotify", "Couldn't find X"); music and volume changes speak for themselves. The log records only the command kind and whether it worked, never the words.
- **Settings.** A "Voice assistant" card on the Dictation page: an on/off toggle, the hotkey, and example commands.
- Assistant follow-up: "open Steam and type something" showed "Couldn't find Steam And Type Something" because the whole tail was taken as the app name.
  - `command::parse_all` now splits at "and" / "then" / a sentence end when a command verb follows, but never inside "type ..." text. It runs only if every step is understood.
  - The controller runs steps in order and stops at the first failure, so typing never lands in the wrong window.
  - "Open X" then "type" waits up to 8 s for X's window (`Assistant::wait_for_window`), focuses it, then types.
  - `open` also tries shorter and shorter versions of the name ("steam app for me" → "steam").
  - Failure notices now have a short title and "Heard “…”" underneath.
- Assistant notices show the app, not "Onyx".
  - `NotificationCard.icon: Option<ArtImage>`. The assistant fetches the app's Start-menu icon (IShellItemImageFactory on `shell:AppsFolder\<target>`, 96 px, straight alpha premultiplied if needed). Icons are cached per app and attached to "Opening / Switched to / Closing X" and "Finding … on Spotify".
  - Onyx's own notices (dictation and assistant) have an empty source. The card then shows a tiny Onyx mark (black pill, silver rim, lavender dot) instead of an "O". The compact card centres a lone title or shows the body ("Heard …") as its second line; the large card lifts the title and body into the space.
  - Verified by temporarily pointing the demo notice at Spotify's icon (reverted).
- Dictation cleanup: `abandoned_start` (transcript.rs) drops a sentence start given up with "sorry" / "I'm sorry" ("This search for a certain mus I'm sorry. If I open Spotify…"). It applies only when the apology closes its phrase, the start is at most 10 words, and the next sentence reuses at least 2 of its content words (a cut-off word counts towards the whole one). Real apologies stay; there are tests for both. Before this, the rules saw the cue but left it, and Gemma 270M returned the text unchanged ("retained").
- Assistant, search inside an app: "open X and search for Y" searches in X, not on the web (`Assistant::search_in`).
  - Apps with a search link open it directly: Spotify (`spotify:search:`), YouTube, YouTube Music, Netflix, Amazon, Wikipedia, GitHub, Reddit, Microsoft Store, File Explorer (`search-ms:`), and browsers (Google).
  - Any other app: wait for its window, find a field whose UI Automation name, ID or help text says search or find (edit or combo box, on screen, enabled), focus it, paste the query and press Enter. The lookup has a 3 s limit. Nothing is typed when no such field exists.
  - Verified the finder locates the search box in Spotify and File Explorer (dry run, no typing).
- abandoned_start also handles a start that speech recognition ended with a full stop (…should follow the system. I'm sorry. Can the dark mode toggle follow the system theme…). Because that looks like a finished sentence, the next sentence must reuse at least 3 of its content words and at least 60% of them. Real apologies (The quarterly report is late. I'm sorry. The quarterly report will be ready tomorrow., I broke the build. I'm sorry. I'll fix the build tonight.) stay, with tests.
- Assistant: searching an installed app now types into its own search box, the way a person would (`search_in` → `type_in_search_box`). Steps:
  - Wait for the window and keep looking for the box for up to 5 s while a fresh app builds its window.
  - UI Automation SetFocus on the box. Type only if that app's window is really in front.
  - Ctrl+A to replace any old search, paste the words, Enter.
  - The search link is the fallback; websites and browsers use their link directly.
  - "Search X on Spotify" (site = an installed non-browser app) opens the app first, then searches the same way.
- Box matching: Edit, then ComboBox fields that are on screen, enabled and have a real size. The label must read like a search box: "search", "find", "what do you want" (Spotify's "What do you want to play?" combo box), "look up / look for", and not "address". The first dry run had falsely matched Spotify's hidden 0×0 Chromium address bar; this now picks the real 474×48 search box. File Explorer's box is found too.
- Fix: a notice could stay on the island forever (for example "Opening Defragment" after the app had opened). The expiry timer is single-shot. When WM_TIMER fired a few ms before `done_deadline` (Windows rounds timers to its tick), the Tick saw "not yet", did nothing, and no timer was left. Tick now clears a notice within 50 ms of its deadline, and otherwise re-arms the timer for the remaining time plus a tick. There is a test for a timer 8 ms early.
- Assistant: "search for GTA 5" came out as "gitif". That is the speech model mishearing a short acronym (Parakeet TDT here decodes with greedy search only, so sherpa-onnx hotword biasing isn't available). Fixes:
  - `command::join_spelled`: letters spelled one by one ("G T A five", "G-T-A 5", "G.T.A. five") become one word ("gta 5"), and a number word right after them becomes a digit. A lone letter and ordinary number words stay. Tests included.
  - Steam gets a store-search link fallback (`steam://openurl/…/search/?term=`).
  - The personal Dictionary (Settings → Dictation) already applies to commands, so a recurring mishearing can be mapped ("gitif" → "GTA 5").
- Assistant: search and play queries keep only the name (`command::tidy_query`). "a channel which is gmat prep" → "gmat prep", "the channel gmat prep" → "gmat prep", "a song called blinding lights" → "blinding lights", "a song by adele" → "adele", "something called lo-fi" → "lo fi". It applies only when the query opens with an article (or "something") plus a kind of thing (channel, group, song, video, game…), followed by an optional "which is / called / named / titled / by". Ordinary searches ("a word that is spelled wrong", "song lyrics", "the best pizza near me") stay whole. Tests included.
- Lists: (1) split_lead_in - a first marker after a lead-in in the same sentence ('To set it up, step one, download... Step two, ...') becomes a heading plus bullets, only when a second marker follows. (2) inline_list - an explicit request ('make a list', 'bullet points', 'as bullets') ending in a colon, followed by 3+ short comma items as the last sentence, becomes one bullet per item. Unrequested comma lists stay prose. A 15-sentence probe of list styles was run through the rules first: 13 passed already, these 2 were the gaps. Tests included.
- Dictation list test round 2 (user dictated 12 list styles; 6 were laid out wrong). Fixes in transcript.rs, each with the user's actual speech-model output as a test (`dictated_list_tests`):
  - **AI guard.** `rewrite_rejection` refuses a model rewrite that has fewer line breaks than the rules' text. The model had flattened "new paragraph / new line" and dropped "Best regards", and was accepted. Adding breaks, to make a list, stays allowed. A stricter "no dropped words" check broke legitimate rewrites (number words to digits, "alternative" to "instead") and was removed.
  - **Markers.** Digit markers ("Number 1.", "Step 1:"). A stop right after a marker is skipped ("the third one is. Improving…" had left "- . Improving").
  - **Comma splits.** Markers after commas ("…is broken, lastly, the settings…") are split before list detection, but only at ordinals and "finally / lastly", so "cook the rice, then add salt" stays one item.
  - **Announced lists.** Clauses after a comma that open with the same two words ("We need X, we need Y") become separate items.
  - **Colon-less requested lists.** "Make a list of things to pack for a passport, charger, …": the first item starts after the last linking word before the first comma.
  - **Stops.** `even_stops`: when some bullets end with a stop, bare ones and ones ending in a comma get a full stop.
- Not fixable by rules: "itinerary" misheard as "iterny. Iternary" (speech model).
- spoken_list ends at a sentence that asks for a new list ('Also make a list of things to check: ...'). The rest is laid out on its own, as an inline list, instead of being appended to the last step. Test included. Large numbers stay unseparated (30000) by design: an existing test expects that, and the fact guard compares digits.
- Dictation test round 3 (3 mixed sentences with deliberate mistakes). New pre-passes in `clean` (with filler removal on) and a final pass, each tested with the user's speech-model output:
  - `resolve_denied_value`: "around 25,000 dollars. No, no, not $25,000. Sorry, $30,000." becomes "around $30,000.". "No (no) not X" plus sorry / I mean / actually / it's plus Y replaces the earlier mention of X (matched by digits), taking Y's currency form. Before, "sorry, Y" was applied to the "not X" itself.
  - `continue_sentences`: a "." or "?" followed by a lowercase word was only a pause. The stop is dropped, and a "?" moves to the sentence end ("Daniel? before 5pm." becomes "Daniel before 5pm?"). Abbreviations and numbers are exempt.
  - `drop_noun_restarts`: "The new contract the contract should…" becomes "The contract should…" (a determiner phrase restarted, shorter, on the same noun).
  - `inline_list`: a list request ending in "." instead of ":" still lays out its comma items.
  - `finish_layout`: applied to all typed text after format_numbers. It capitalizes each bullet and spaces currency signs ("not$25,000" from an AI rewrite becomes "not $25,000"; "$ 40" becomes "$40").
- Not fixable by rules: speech-model mishearings ("set" for "send", "Tanks", "launch. Friday" with "to" dropped).
- AI cleanup Round 6 (teaching the model to fix speech mishearings) was trained and rejected by its gates. The app keeps Round 5. Details are in `D:\Codes\onyx-cleanup-model\ROUND6-RESULTS.md`.
  - **Data.** Authored sentences → Windows TTS → Parakeet v2 (normal and harsh passes) → inferable-swap targets checked by the guard, plus mined confusions.
  - **Dev result.** Repairs went from 6 to 12 of 66 (gate 13), but natural mishearings stayed at 1 of 14. Correct text was changed more often (preservation 8/8 → 4/8, identity changes 7 → 18).
  - **Tooling.** Added `crates/islet-asr/examples/native_pipeline.rs`, the app cleanup path for training and evaluation scripts (full, or `--rules-only` with a guard check).
- Rule `misheard_thanks`: "Tanks." / "Tanks for / a lot / again…" at a sentence start becomes "Thanks". "the tanks" and "Tanks need water" stay. Tests included.
- Learn from corrections (opt-in, `dictation.learn_corrections`, off by default; turned on for this user at their request):
  - After a dictation is typed, `islet-app/src/learn.rs` reads the focused text box through UI Automation at about 12 s and 40 s. It only reads while the same window is in front, and never reads password fields.
  - `islet_core::learn::corrected` finds the dictated passage (best-matching stretch, ties to the closest length) and accepts it only when a few words changed (at most 40% and 8 words). `word_swaps` lists lone single-word swaps.
  - Records `{heard, rule, typed, corrected, swaps}` are appended to `%LOCALAPPDATA%\Islet\corrections.jsonl`. They are never logged; the log records only the swap count.
  - Settings → Dictation → "Learn from corrections" has the toggle, the count with Show file / Delete all, and "Words you often correct" (swaps made at least twice) with Add to dictionary.
  - The pairs are real-microphone training data for a future cleanup round.
  - The config was backed up to `config.toml.before-learning` before enabling.
- Learn-from-corrections testing:
  - **Live test** (`learn::live_tests`, ignored; it uses its own WinForms windows, never the user's apps). A real UI Automation read-back, the real watcher at 12 and 28 s with one word fixed (saved `tanks -> thanks`), an unchanged dictation (nothing saved), and another window in front (box not read). It passes.
  - **First version used Notepad.** Win11 Notepad reopens the user's unsaved tabs on launch, which risks keystrokes reaching them, so the test was rewritten to use its own windows.
  - **Bug found and fixed.** Text typed straight after the dictation without a space ("Tanks.Can") counted as a correction. Edge words with text glued on are now ignored, with a unit test.
  - **Settings.** The "→" glyph is missing from the Settings font (it rendered as a box), so the row now reads "'tanks' corrected to 'thanks', 2 times". `ONYX_CORRECTIONS_FILE` lets tests use their own file in the app and in Settings.
- Complex dictation round 4: six long paragraphs mixing stutters, fillers, corrections, false starts, several list styles, paragraphs and speech-model punctuation, run through the full native pipeline (rules, Round 5 model, guard). All six now come out clean. Fixes, each tested in `dictated_list_tests::complex_dictations`:
  - **Lost word.** `drop_noun_restarts` treated "the hotel near / the near" as a restart and dropped "hotel". Prepositions are never the repeated noun now.
  - **Spoken numbers.** `resolve_denied_value` reads values as spoken words via `number_span` ("not twenty two thousand, sorry, twenty five thousand dollars"), and the correction's own sentence end wins.
  - **Question mark moved.** `continue_sentences` treats a stop before a spoken command ("Monday? new paragraph") as a real end; it had moved the "?" to the next sentence.
  - **Lists inside paragraphs.** `bulletize` now handles each line of a multi-paragraph dictation.
  - **Announced counts.** "I want three things" takes exactly three items (`announced_count`); later sentences stay text.
  - **Clause restarts.** New `drop_repeated_runs`: "this settings page should be the settings page should be cleaner" becomes "the settings page should be cleaner".
  - **Stutters and fillers.** "we we're" / "I I'm" are contraction stutters. ", you know," and ", like," between commas are fillers. "is is" is a stutter except after "it" / "what".
  - **Restart rule.** The older `drop_restarts` rule no longer takes "it" as a restart opener ("I like it like that" had become "It like that").
  - **Bullet stops.** `finish_layout` evens out stops per bullet list after the model (majority rule).
