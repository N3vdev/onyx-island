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
