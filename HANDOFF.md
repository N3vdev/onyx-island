# Onyx handoff — 3 November 2026

## Workspace and source state

- Windows Rust app: `D:\Codes\Dynamic Island`. User-visible name **Onyx**; internal crate/executable/data names remain `islet` / `Islet`.
- Current workspace/app version: **1.0.1**. MSIX identity version: **1.0.1.0**.
- Last commit: **553bcb7**, `Checkpoint Onyx before OrbitDrop converter integration`.
- **Subsequent converter, model-download, packaging and version changes are still uncommitted**, including important untracked files under `workers/`, `tools/`, and the Rust converter modules. Preserve them and unrelated Claude/user changes. Nothing from this work was pushed by the agent.
- Detailed history: `PROGRESS.md`. Design records: `DESIGN.md`, `.impeccable/design.json`, `.impeccable/converter-extension.md`.

## Drag-to-convert feature

- Native OLE file drops expand the island into a converter. Drop directly onto a format to start, or drop onto the card and then click a format. Tab/Shift+Tab, Enter/Space and Escape are supported when focused.
- Settings > Widgets > File converter is enabled by default. Conversion exists only in the expanded state; normal compact activity and other widgets are preserved. Dictation retains priority.
- Current user-requested geometry: **544 × 184 logical units**, 38-unit shell radius. Left preview **176 × 176 at (4,4)**, 34-unit radius, matching media cover spacing. Right content starts at x=196, 16 units after the preview; title y=20 and subtitle y=41. Format controls are 36 high, up to four columns/two rows. Preserve these dimensions/spacing unless asked to change them.
- Single-file previews are bounded 256 × 256 premultiplied BGRA. PNG/JPEG/BMP/GIF/TIFF use direct decoding before Windows Shell thumbnails, fixing dependence on Explorer's thumbnail cache. Unavailable previews and grouped files use a native file/extension icon.
- Outputs use OrbitDrop collision-safe writes beside originals. Multi-page PDFs create a new page-image folder. Originals and previous outputs are preserved. Progress, cancellation, partial results, errors/retry, close and Show in folder are implemented.
- Backend: `crates/islet-app/src/file_drop.rs`, `converter.rs`, `island_window/file_converter.rs`. Model/layout: `crates/islet-core/src/converter.rs`. Drawing: `crates/islet-render/src/converter.rs`.
- Converter helper: `workers/Onyx.Converter/`, self-contained .NET 8. It links supplied **MIT OrbitDrop sources unchanged** from `OrbitDrop-main/`. Enabled providers: ImageConversion, PDF, MediaFoundation, MediaEditing. Supports PDF→PNG/JPG, image formats/PDF, audio and supported video formats including GIF.
- **Office-backed conversion is excluded**: an authored Word-automation PDF→DOCX probe stalled. Do not advertise Office/DOCX/CorelDRAW conversion as working.
- Helpers run only for catalog/preview/conversion, with private JSON IPC. No idle converter process, polling service or perpetual animation was added. Process teardown does not wait on the UI thread. Preview drawing reuses the bounded media artwork cache.

## Dictation and cleanup model

- Existing dictation pipeline includes deterministic corrections, punctuation, numbers, bullet grouping, chunking and fact-preservation checks. Rejected model rewrites fall back to rule-cleaned text. Existing clipboard/history behavior should be preserved.
- Selected model: **Gemma 3 270M, Round 5, selected training step 64**, Q8_0. Do not replace with the final step-256 export or Qwen.
- Filename: `onyx-cleanup-270m-r5-selected-q8_0.gguf`.
- Exact size: **291,545,536 bytes** (278.04 MiB).
- SHA256: `760e148a14c0057a086f47efbec76f738ce817901141e45a58d9566dee67cf8f`.
- **Current host is GitHub Releases, not R2**:
  `https://github.com/N3vdev/onyx-cleanup/releases/download/cleanup-r5/onyx-cleanup-270m-r5-selected-q8_0.gguf`
- Public download repository: `https://github.com/N3vdev/onyx-cleanup`; release `cleanup-r5`. Only model hosting was connected; app source was not uploaded. Preserve the asset/tag because released apps pin this URL and checksum.
- Settings > Dictation installs the model on demand. Downloader supports retries, cancellation, HTTP Range resume and SHA256 validation. Once installed/enabled, inference works offline. Existing installed models are recognized. Speech-recognition models remain separate optional downloads; current default registry uses Parakeet v3 int8.
- Model/runtime registry: `crates/islet-asr/src/registry.rs`. Download/install behavior: `manager.rs`; model prompt/inference: `polish.rs`; deterministic cleanup/safety: `crates/islet-core/src/transcript.rs`.
- CPU runtime remains bundled in the installer under `models/ai-cleanup/runtime`; executable code is not fetched during the verified packaged-model installation. Runtime archive: `llama-b11927-bin-win-cpu-x64.zip`, SHA256 `f70b7fe800e683f58fed3199fcf9c961932307f30916498a7758dce5f1d76ce9`.
- Training/evaluation workspace: `D:\Codes\onyx-cleanup-model`; Python: `venv\Scripts\python.exe` there. See `ROUND5-RESULTS.md` there for measured quality and limitations. No new training was performed during converter/hosting work. Public model/dataset redistribution licence/provenance review remains an open item.

## Packaging and Store version repair

- Converter runtime bundling initially increased the model-inclusive MSIX to 400.6 MiB. Removed native PDB symbols and unused non-English satellite resources, reducing it to 368.2 MiB. Then moved model weights to optional download, reducing the package to **108.6 MiB**. Required runtime libraries and licence notices remain bundled; no separate .NET install is needed.
- `tools/build-converter.ps1` publishes the helper and cleans stale symbol/resource files. Its project also excludes these from publishing. `packaging/build.ps1` includes converter + CPU runtime but **excludes GGUF weights** from MSIX/NSIS staging.
- Store rejected different builds sharing identity `Nevdev.OnyxIsland_1.0.0.0_x64_…`. Fixed by bumping workspace/app to 1.0.1 and MSIX to **1.0.1.0**, keeping Store Name/Publisher/architecture unchanged.
- Latest package: **`dist\Onyx_1.0.1.0_x64.msix`**.
- Exact size: **113,872,173 bytes**.
- SHA256: `a4993cba1c9b651f85cbebfe8eda07243121cfcd2ab135b822f126c11ce3b57c`.
- Identity: Name `Nevdev.OnyxIsland`, Publisher `CN=7BE60B1B-C205-4C0E-AB26-9C89190ED9A1`, x64.
- Package is **unsigned** for Store submission. Agent did not sign/install/submit it. Current Partner Center outcome is unverified; user was told to replace the rejected 1.0.0.0 draft upload with 1.0.1.0.
- Builder now refuses to overwrite an existing versioned MSIX. **Increment `[workspace.package].version` in Cargo.toml before packaging changed contents again** (next unused version, normally 1.0.2). Keep the fourth Store version component zero. Existing output is not evidence that a rebuild ran.
- `dist\Onyx-Setup-1.0.0.exe` is OLD (28 October) and does not represent current work. NSIS source is updated, but that EXE was not regenerated/install-tested.

## Verification completed

- At converter integration: 298 normal Rust tests passed across core/ASR/input/render/app/settings, plus appropriate Clippy. Later targeted converter/ASR/settings checks passed after their respective changes; this is not a claim that the full suite was rerun after every visual adjustment.
- 23 authored converter integration checks passed, including multi-page PDF, image/audio/video, collision safety, unsupported/damaged files, cancellation and unchanged originals. Packaged helper and real PNG previews were also tested.
- Actual helper lifecycle test passed with stdin held open, verifying nonblocking teardown. Initial native OLE direct-drop and drop-then-keyboard paths passed; later desktop source-drag automation reruns were unreliable and are not counted as new passes.
- Native GPU captures covered five converter states at 80/100/125% scales, including eight-format and image-preview fixtures. Earlier layouts received independent finish review. Latest small spacing changes were inspected locally; do not imply a new independent review where none ran.
- **GitHub fresh install verified through actual Rust ModelManager**, into an isolated empty model directory, from an unpacked model-free package. Full download/hash passed; three authored recipient/number/prohibition cleanup cases passed using the packaged CPU runtime. No runtime executable was downloaded/copied into that model directory.
- For 1.0.1.0: SDK pack/unpack, manifest identity/version, app/settings PE version, all **306 staged-file hashes**, no GGUF in archive, and version-reuse guard passed. Functional reference is the prior GitHub build with the same behavior; signed installation/container behavior and Store certification remain unverified.
- Reports: `target/release/onyx-1.0.1-msix-verification.json`, `github-msix-verification.json`, `github-native-install-verification.json`, `onyx-cleanup-local-release.json`. Reports about older versions are historical.

## Useful commands and working conventions

```powershell
# Publish helper when converter code/dependencies change:
.\tools\build-converter.ps1

# Rebuild current Rust executables:
cargo build --release -p islet-app -p islet-settings -p islet-asr-worker --offline -j 2

# After bumping the workspace version, package the rebuilt binaries:
.\packaging\build.ps1 -SkipBuild -MsixOnly

# Authored converter integration checks:
D:\Codes\onyx-cleanup-model\venv\Scripts\python.exe tools\check-converter.py

# Actual helper teardown check:
cargo test -p islet-app completed_helpers_never_block --offline -- --ignored
```

- Stop only workspace-owned executable processes before rebuilding release binaries. Resolve/normalize paths before comparing process paths; avoid killing unrelated installed Onyx copies or other apps.
- Relaunch `target\release\islet.exe` after app changes. It was last relaunched successfully as version 1.0.1; current running state has not been rechecked for this handoff.
- **Keep the PC on. Never shut it down.**
- Never log user transcripts, clipboard contents, tab/window titles, or selected-file contents/names. Tests use authored fixtures and print metadata only.
- Preserve unrelated user/Claude work. Do not reset/clean the dirty tree. Commit the current feature work before another large change if asked/appropriate.
- Report only measured results; do not claim Store approval, live microphone accuracy, or untested conversions.
- RGBA preview images sometimes appeared to lose content in the inspection tool. Use a single opaque RGB export at original detail before diagnosing a rendering defect from a batched preview.

## Next steps

1. User uploads `Onyx_1.0.1.0_x64.msix` and reports any remaining Partner Center errors; inspect the actual error rather than guessing approval.
2. Preserve/commit the substantial uncommitted implementation before further feature work.
3. Complete pending model/dataset licence/provenance review before treating redistribution as approved.
4. Signed MSIX install/upgrade and natural microphone checks remain distinct from the successful unpacked-payload and authored-text tests.
