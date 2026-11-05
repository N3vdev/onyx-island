# Drag-to-convert island

Mode: Operate. Extend the existing native black island, Nunito typography,
38-unit shell corners and uniformly scaled expanded content. Preserve compact
music/activity/glance behavior and existing scroll widgets.

Dragging real local files onto the island opens a roomier 544 x 184 logical-unit
layout retaining the expanded media composition. The left 176-unit square centered at (4, 4), with 34-unit
corners, shows a native Shell thumbnail when available or an authored file icon
and extension. The right column begins at x=196 and spans 324 units. Filename
and stage-aware supporting text lead the column; up to eight formats occupy
four columns and two rows at y=70 and y=114. Formats are 36 units high with
10-unit corners, 8-unit horizontal and vertical gaps. White fill
rises from 11% to 20% on hover or keyboard focus. A native stroked close icon
dismisses the card.

Dropping onto a ready format starts conversion. Dropping elsewhere retains the
files so a format can be clicked. Original files remain unchanged; outputs use
OrbitDrop's collision-safe writes beside the originals. Running, canceled,
successful, unsupported and failed states are explicit. Show-in-folder and
retry actions stay in the same card. Dictation and urgent notices retain priority.

OLE drag callbacks and posted worker messages drive updates. Converter processes
start only on a drag/catalog or explicit conversion and exit afterwards. No idle
service, global drag scanner, continuous animation or polling timer is added.

## Shipped surface record

THESIS: Drag real local files to the existing island and choose a compatible local conversion in place.
OWN-WORLD: Existing black native island, Nunito type, 38-unit shell corners, translucent-white format controls and restrained pale green progress. Settings retains its graphite workspace.
STORY: Drag files, inspect actual formats, drop on a format or retain the drop and choose; read progress and outcome, then show the output or retry.
FIRST VIEWPORT: Expanded-only 544 x 184 logical-unit composition with vertically centered media-sized artwork: left file preview, right filename/count and stage-aware subtitle, then one to eight formats in up to four columns and two rows. Interactive outcome/footer actions occupy the bottom of the right column; close occupies the upper right.
FORM: Code-led native Direct2D/DirectComposition surface, uniformly scaled with the existing expanded-size preference. No raster assets or alternate web renderer.

File converter is enabled by default in Settings > Widgets. Its row uses the existing right-aligned switch and explanatory copy. The preview uses a 304-point stage and 268-point native anchor when enabled; otherwise the existing Calculator 264/232 and ordinary 216/168 dimensions remain.

Loading, Ready, Running, Complete and Error reuse the same roomier card with stage-aware supporting text. Full success uses explicit completion text; partial or canceled outcomes with saved results use `fully_succeeded=false` and distinct stopped-with-results text. There is no success-check badge. Interactive footers occupy (196, 146), measure 324 x 28 units, and use 10-unit corners and centered labels: Complete uses 18% green fill, 27% on hover/focus; cancel and retry actions use 8% white, 16% on hover/focus. Loading and Ready do not render footer actions or passive footer guidance.

`workers/Onyx.Converter/Preview.cs` requests a Shell thumbnail only for a single file, fits it into a 256 x 256 premultiplied BGRA image, and returns it as `catalog.preview` from the short-lived helper. Missing thumbnails and grouped files use the native extension/file-icon fallback. Preview retrieval adds no idle service.

Native Tab/Shift+Tab, Enter/Space and Escape support format selection, footer actions and close/cancel. Title and supporting text begin at y=20 and y=41; outcome headings at y=70, messages at y=108 with 24-unit height, and progress at y=140. The close target begins at y=16. The converter has no compact identity, page dots, wheel page switching or continuous animation. Dictation and urgent notices retain priority. Running conversion rejects another start; cancel preserves originals and completed results. An unsupported catalog offers choosing other files; errors with retained formats offer another format.

Actual conversion reuses OrbitDrop image, PDF, MediaFoundation and MediaEditing providers through a short-lived self-contained helper. Provider availability determines the formats offered. Office conversion is excluded after the blocked Word automation test; do not present Office formats as supported. Native OLE drag and keyboard paths were exercised. This record does not establish installed-MSIX validation.

Evidence: `target/converter-roomier/converter-{Loading,Ready,Running,Complete,Error}-{80,100,125}.png` records the current native roomier-layout state/scale coverage. Earlier captures under `target/converter-media-layout/`, `target/converter-render-review/` and `.impeccable/review/converter-grid-complete-proof.png` document historical compositions and interaction checks. Earlier SHIP verdicts apply to those revisions; the roomier-layout revision has a separate finish review. These captures are evidence, not shipping image assets.

Not canonized: converter-specific geometry, type sizes and alpha fills remain component details; they do not replace settings tokens. The existing music-tag removal glyph remains a carried defect, not an icon rule.

Image files supported by the native WPF decoder are loaded directly at bounded resolution before trying the Shell thumbnail provider; this avoids reliance on Explorer thumbnail caching for PNG previews.
