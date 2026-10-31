# Local packages

Build the MSIX from the current optimized binaries:

```powershell
.\packaging\build.ps1 -SkipBuild -MsixOnly
```

Omit `-SkipBuild` to rebuild the app, settings and speech worker first. Omit
`-MsixOnly` to also build the NSIS installer. Windows SDK, Visual C++
redistributable files and Python with Pillow are required; NSIS is needed only
for the setup executable.

The finalized cleanup payload must be staged under `target/release/models`:

- `onyx-cleanup-270m-r5-selected-q8_0.gguf` — selected Round 5 step-64 weights.
- `llama-b11927-bin-win-cpu-x64.zip` — the pinned CPU runtime archive from the
  URL in `crates/islet-asr/src/registry.rs`.

`tools/install-cleanup-model.ps1 -ModelPath <selected-gguf>` stages the model
and verifies its hash. The package builder verifies both payload hashes before
staging. It includes the model, extracts the runtime DLLs/server into
`models/ai-cleanup/runtime`, and adds app-local Visual C++ DLLs. The app runs the
packaged server in place and copies the model to writable storage when AI
cleanup is installed. A packaged installation does not need to download the
cleanup executable. Speech-recognition models remain optional downloads.

Output: `dist/Onyx_1.0.0.0_x64.msix` for the current workspace version. The
MSIX is unsigned; this workflow does not install it, sign it or submit it to
the Store. Unpacked-payload tests are separate from signed MSIX installation
and Store certification. The cleanup dataset provenance/license review remains
open before public distribution; see the external `ROUND5-RESULTS.md`.
