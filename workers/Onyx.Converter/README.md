# Onyx file converter bridge

This self-contained .NET 8 Windows helper compiles the supplied MIT OrbitDrop
source directly, without running the OrbitDrop application or altering its UI.
Providers currently enabled: `ImageConversionProvider`, `PdfProvider` and
`MediaFoundationProvider`, plus `MediaEditingProvider` for video-to-GIF. The same `FileSet`, operation registry, STA imaging
worker, codecs and collision-safe atomic `SafeOutput` implementation are reused.

Office-dependent `DocumentProvider` is intentionally not exposed: an authored
PDF-to-DOCX test blocked in Word automation on this PC. It is not counted as a
working conversion. No CorelDRAW reader is bundled. Unsupported selections
return an empty catalog, rather than invented conversion options.

Build using `tools/build-converter.ps1`. Output is `target/release/converter`.
The runtime is self-contained; users do not need a separate .NET installation.
The MSIX/NSIS builders include this directory and its source/license notices.
Publish excludes native debug symbols and unused non-English satellite resources.
The build script also removes these artifacts left by previous publishes.
Neutral English resources, conversion libraries, and the self-contained runtime
remain included; this does not add a separate .NET installation requirement.

The Rust app launches a process only for an OLE drag catalog or an explicit
conversion. JSON request/event lines use redirected standard input/output; no
file names, contents or converter outcomes are written to application logs.
Cancellation uses the input pipe with a bounded termination fallback. Completed
helpers release the input pipe and are waited for outside shared locks; the UI
does not wait for process termination. No idle converter process, timer or
global drag hook is used. Format buttons are supplied by the real providers.

Outputs appear beside originals. Multi-page PDFs produce a new page-image
folder. Existing outputs receive unique names; originals are never overwritten.
Protected, invalid, inaccessible and unsupported files produce recovery states.

Checks: `python tools/check-converter.py`; opt-in actual Rust/helper lifecycle
test `cargo test -p islet-app completed_helpers_never_block -- --ignored`.
`tools/check-converter-drag.ps1` drives an authored file through native OLE and
requires an interactive desktop; its owned source process has a hard timeout.
Renderer preview tests use authored data and a Windows GPU. Preview exports
are flattened to opaque RGB for review because RGBA previews can omit pixels
in the inspection pipeline; source captures remain unchanged.
