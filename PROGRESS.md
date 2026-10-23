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
