# Local packages

Build the MSIX from the current optimized binaries:

```powershell
.\packaging\build.ps1 -SkipBuild -MsixOnly
```

Omit `-SkipBuild` to rebuild the app, settings and speech worker first. Omit
`-MsixOnly` to also build the NSIS installer. Windows SDK, Visual C++
redistributable files and Python with Pillow are required; NSIS is needed only
for the setup executable.

The full build also publishes the self-contained .NET 8 converter helper using
`tools/build-converter.ps1`. When using `-SkipBuild`, run that script first if
the converter has changed. Staging requires `target/release/converter` and
includes its runtime and OrbitDrop license notices. The installed app does not
need a separate .NET installation for conversion.
The converter publish excludes native PDB debug symbols and non-English
satellite resources. `tools/build-converter.ps1` removes stale copies of these
files from earlier publishes before packaging; required runtime libraries and
license notices remain bundled.

File conversion is enabled under Settings > Widgets. Drag local files onto the
island and drop onto a format, or drop onto the card and choose a format. The
helper reuses OrbitDrop's PDF, image, and Windows media providers. Office-backed
document conversion is excluded. Outputs are saved beside the originals with
collision-safe names; the original files remain unchanged. The converter adds
package disk space but has no idle helper process.

The pinned CPU runtime archive must be staged under `target/release/models`:
`llama-b11927-bin-win-cpu-x64.zip`. The builder verifies its checksum, extracts
its server and DLLs into `models/ai-cleanup/runtime`, and includes app-local
Visual C++ DLLs. Runtime executable code stays inside the package.

The Round 5 cleanup GGUF is excluded from both installers. Settings > Dictation
installs it on demand from the HTTPS URL in `crates/islet-asr/src/registry.rs`,
checks the pinned SHA-256, and stores it under the user's models directory.
Downloads support cancellation, retries and resuming; cleanup then works offline.
Existing installed models remain available. Rule-based formatting and safety
checks are still bundled. Speech-recognition models also remain optional downloads.

The model download is pinned to the `cleanup-r5` asset in
`https://github.com/N3vdev/onyx-cleanup/releases`. The SHA-256 is verified
independently of the hosting service. Keep released model files and version tags
stable so existing app versions continue to download their expected weights.
`tools/install-cleanup-model.ps1` remains available for local staging/testing,
but staging a GGUF does not add it to either installer.

Increment `[workspace.package].version` in `Cargo.toml` for each changed release.
The builder refuses to overwrite an existing package version so different
contents cannot accidentally reuse an uploaded MSIX identity. The fourth MSIX
version component stays zero for the Store.

Output: `dist/Onyx_1.0.1.0_x64.msix` for the current workspace version. The
MSIX is unsigned; this workflow does not install it, sign it or submit it to
the Store. Unpacked-payload tests are separate from signed MSIX installation
and Store certification. The cleanup dataset provenance/license review remains
open before public distribution; see the external `ROUND5-RESULTS.md`.
