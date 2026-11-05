---
name: Islet Settings
description: Native Windows settings in a compact graphite workspace with the real island.
colors:
  background: "#101113"
  sidebar: "#151618"
  card: "#1a1b1e"
  card-line: "#2f3135"
  text: "#ecedf2"
  muted: "#a6aab2"
  accent: "#a4ccb4"
  selection: "#303337"
  control: "#202026"
  control-hover: "#2c2c34"
  control-active: "#363640"
  input: "#0e0e11"
  dots: "#26282b"
  danger: "#ff6e6e"
typography:
  headline:
    fontFamily: "Nunito, sans-serif"
    fontSize: "25px"
    fontWeight: 700
  body:
    fontFamily: "Nunito, sans-serif"
    fontSize: "14px"
    fontWeight: 400
  navigation:
    fontFamily: "Nunito, sans-serif"
    fontSize: "13px"
  label:
    fontFamily: "Nunito, sans-serif"
    fontSize: "12px"
  metadata:
    fontFamily: "Nunito, sans-serif"
    fontSize: "11px"
rounded:
  control: "8px"
  card: "10px"
  navigation: "6px"
  capsule: "14px"
  island-shell: "38px"
spacing:
  item: "10px"
  button: "6px 12px"
  card: "16px 18px"
components:
  button:
    backgroundColor: "{colors.control}"
    textColor: "{colors.text}"
    rounded: "{rounded.control}"
    padding: "{spacing.button}"
    typography: "{typography.body}"
  button-hover:
    backgroundColor: "{colors.control-hover}"
  button-active:
    backgroundColor: "{colors.control-active}"
  card:
    backgroundColor: "{colors.card}"
    textColor: "{colors.text}"
    rounded: "{rounded.card}"
    padding: "{spacing.card}"
  selected-option:
    backgroundColor: "{colors.selection}"
    textColor: "{colors.text}"
    rounded: "{rounded.control}"
---

# Design System: Islet Settings

## Overview

**Creative North Star: "A compact desktop control workspace with the real island"**

Near-black canvas, charcoal navigation, softly outlined graphite groups, and rounded Nunito text make a restrained native control workspace. Fine static dots identify the island preview and full-screen sizing canvas. The actual island supplies the live content; settings supplies clear controls and feedback.

This refresh supersedes the former purple atmosphere and wide navigation rail. It preserves the bundled regular and bold Nunito faces, matching native Windows titlebar, right-aligned controls, visible focus, and real island preview. Source authority is `crates/islet-settings/src/ui.rs`, `customize.rs`, `preview.rs`, and `main.rs`, with sizing behavior in core configuration, island layout, app window integration, and renderer. Measurements are egui logical points, expressed as px in portable tokens; they are not physical screenshot pixels. This record governs settings and the shipped widget and converter extensions; the island retains its independent visual language. Widget source authority is `crates/islet-render/src/widgets.rs`, `crates/islet-core/src/widgets.rs`, `crates/islet-core/src/island.rs`, and `crates/islet-app/src/island_window.rs`. Converter source authority is `crates/islet-render/src/converter.rs`, `crates/islet-core/src/converter.rs`, and `crates/islet-app/src/island_window/file_converter.rs`.

**Key Characteristics:**
- Narrow navigation with authored line icons and restrained active rows.
- Graphite groups, fine outlines, and a static dotted canvas.
- Compact Nunito hierarchy with dedicated bold headings.
- A native island preview and draft-only full-screen sizing.
- Native tools with scroll-only widget navigation, expanded-only drag conversion, and finite state changes.

## Colors

Graphite neutrals carry the interface; a pale cool green identifies enabled state, links, and selection outlines.

### Primary
- **Accent:** enabled switches, links, and selection strokes; it does not wash the workspace.
- **Selection:** neutral dark fill behind selected choices.

### Neutral
- **Background / Sidebar:** near-black workspace and slightly lighter navigation, with native caption and border matching the sidebar.
- **Card / Card line:** grouped settings and fine separators.
- **Text / Muted:** labels and controls versus descriptions and metadata.
- **Control / Control hover / Control active:** progressively lighter interaction surfaces.
- **Input:** recessed text-entry background.
- **Dots:** low-contrast canvas texture.

Save failures use the danger color. The full-screen editor's Apply changes action uses a light neutral fill with dark text; this is a deliberate action emphasis, not a global accent palette.

**The State Accent Rule.** Reserve the cool accent for state and links; keep navigation and setting groups graphite.

## Typography

Bundled Nunito Regular and Nunito Bold provide the proportional interface family, with egui fallback glyphs. The headline role is used for page and sizing-editor titles. Body serves settings labels, controls, and explanatory paragraphs. Navigation uses the navigation role, with the selected label in the dedicated bold face. Label serves descriptions and functional group names; group names use the bold face. Metadata serves save status, version, and preview guidance.

The hierarchy is compact rather than theatrical. The application name and Settings heading are intermediate bold sizes specific to the shell; they do not establish additional reusable display roles. Line heights remain toolkit/font-derived.

**The Utility Type Rule.** Preserve the page-title, setting-label, description, and metadata hierarchy with real bundled bold faces for headings.

## Layout

The native window opens at 1040 × 820 logical points, with a 940 × 720 minimum. A fixed 164-point sidebar has 12-point horizontal and 18-point vertical margins. Navigation targets are 36 points high. General, Now Playing, Dictation, and Widgets sit above; About and version metadata sit near the bottom.

The workspace has 28-point horizontal and 20-point vertical margins. Its header precedes a full-width preview stage: 304 points high when File converter is enabled; otherwise 264 with Calculator enabled, or 216 with both disabled. The native geometry anchor is centered 9 points below the stage center, up to 720 points wide and 268 points high with File converter enabled; otherwise 232 with Calculator enabled, or 168 with both disabled. At narrower supported widths the anchor width follows the stage. A 24-point gap precedes the page body. Each page has a separate vertical scroll identity; the preview stays above the scroll area. The implemented body fills the available workspace width rather than imposing a separate centered maximum-width column.

Rows pair left-hand labels and descriptions with right-aligned controls. The text column uses 53% of available row width, with a 180-point minimum. Music-site tags wrap. At the supported minimum size, pages scroll and navigation stays present; no mobile breakpoint is defined.

Full-screen customization replaces the shell with a dotted monitor-sized canvas, the actual island at the selected edge, and a centered editor with 390-point content width and 26-point padding. A horizontal guide marks the 5% vertical-spacing limit for the selected edge.

## Elevation & Depth

Settings uses tonal layering and thin outlines rather than custom drop shadows. The preview stage and full-screen editor are outlined surfaces over static dots. There is no purple glow or atmospheric gradient in the current build. The island retains its own renderer and depth treatment.

**The Static Canvas Rule.** Keep the dot grid still; animate interaction state rather than the workspace texture.

## Shapes

Controls use the control radius, cards the card radius, navigation the smaller navigation radius, and music-site tags the capsule radius. Switches are full capsules with circular thumbs. The preview stage has a 12-point radius; the full-screen editor has a 14-point radius. One-point outlines and dividers establish grouping without heavy borders.

## Components

### Buttons and fields

Compact dark buttons use shared body type, padding, and control radius. Hover lightens their surface. The sizing editor uses a light Apply changes action alongside dark Cancel and Reset edge actions. Music-site entry pairs a single-line field with Add site and a frameless Reset action. Preserve toolkit keyboard and focus behavior. Sidecar HTML/CSS snippets are portable visual translations of native egui components, not executable application UI.

### Cards / Containers

Graphite groups use the card tokens, a one-point card-line outline, and separators between related rows. Each group leaves 14 points after the container. Functional group titles identify their contents. These labels are not decorative eyebrows.

### Navigation

Small authored line icons precede labels. Selected rows have a subtle white-alpha fill and card-line outline; hover adds a weaker white-alpha fill. The active label uses bold Nunito. Keyboard focus adds a light inner outline. Current navigation animates individual hover and selected fills; the previous sliding shared selection plate is retired.

### Switches and segmented controls

Switches measure 44 × 24 points. A white thumb moves across the capsule while its fill interpolates from dark grey to accent. Focus adds a light outline outside the capsule. Semantic checkbox metadata retains the label and selected state. Segmented choices use adjacent selectable controls separated by four points, aligned to the right of the row.

### Music-site tags

Wrapping dark capsules identify configured sites, with a separate removal action. The literal `x` in the current build is not an approved icon convention. The sidecar represents removal with an inline vector cross.

### Live island preview and feedback

The stage contains the existing native island through a geometry anchor, not a second renderer. Its caption reports the configured expansion trigger or connection state. Changes in ordinary settings save automatically; save failures replace the success status and appear below the page.

Control transitions use 160ms when Windows client-area animations are enabled. Page changes fade from 75% to full opacity over 160ms with cubic ease-out; disabled system animation skips these transitions. This is a transition contract, not a measured frame-rate or resource-use claim.

### Full-screen sizing editor

Customize sizing follows Position. Top and bottom have separate draft spacing and expanded-scale settings. Spacing ranges from 0% to 5% of screen height, in addition to the island's base edge inset. Expanded size ranges from 80% to 125%, uniformly scaling text, artwork, controls, associated bubble, and gap. Compact dimensions remain unchanged; no independent width/height controls are exposed.

Apply changes commits both edge drafts. Cancel or Escape discards them and restores the previous window geometry. Reset edge restores only the selected draft edge to zero added spacing and 100% expanded scale.

**The Proportional Expansion Rule.** Scale expanded content and its container together; preserve compact dimensions.

### Widgets settings and native calculator

Widgets uses the existing authored line-icon navigation, Nunito hierarchy, outlined settings group, right-aligned labeled switch, saved feedback, and vertically scrolling page body. Calculator is opt-in through Settings > Widgets and appears only in the expanded island. Enabling it preserves the configured Hidden or Sliver idle appearance. Collapsing Calculator restores music, agent activity, or the configured idle appearance while remembering the selected widget for later expansion. The preview reserves room for the largest enabled tool: File converter takes precedence over Calculator, including at the supported minimum settings size.

**The Expanded-Only Widgets Rule.** Show widget names, icons, and cards only in the expanded island. Enabling or selecting a widget must preserve compact activity and the configured Hidden or Sliver idle appearance; collapse remembers the selection for later expansion.

The expanded calculator is a 496 x 168 logical-unit card with the existing island silhouette (38-unit corner radius). Its expression and display occupy the left column; twenty native keys form a four-column, five-row grid on the right. Keys are 52 x 24 logical units, with 6-unit gaps and the existing 8-unit control radius. Numeric and utility keys use translucent white; arithmetic keys use a pale green tint; Equals uses solid pale green and dark text. Hover changes key fill. These are calculator component details, not replacements for settings palette or typography tokens. Arithmetic symbols are functional key labels, not decorative icon conventions.

The previous expression is a separate muted line. Active input is right-aligned and clipped to follow its newest digits; a leading ellipsis indicates hidden earlier content after thirteen characters. Results longer than twelve characters use the smaller result format; large and small magnitudes use bounded scientific notation. Errors use a warm warning tint and recover through subsequent input or Clear. Preserve the readable result and active-input tail throughout the existing uniform expanded-size range (80% to 125%).

**The Scroll Navigation Rule.** Switch widget pages by scrolling over the settled expanded island. When another activity page is available, show two noninteractive dots stacked vertically at the right edge, centered on the current expanded card. The upper dot identifies the activity page and the lower dot Calculator. Keep Scroll text out of the island; do not add navigation arrows or make the dots clickable.

Dot centers sit 8 logical units from the right edge and 12 logical units apart, at 6 units above and below the card midpoint; each dot has a 2-unit radius. The current page uses 90% text-color opacity and the other 25%, multiplied by the card alpha. These are native indicator geometry and state details, uniformly scaled with the expanded island, not new shared tokens. Both the former top Scroll label and calculator scroll-switching hint are removed; settings retains its scroll usage guidance.

Dictation and urgent notices take priority over optional tools and suppress widget navigation. Without another activity, Calculator has no page indicator and uses its ready-state guidance. Precision wheel deltas accumulate, with burst gating to prevent repeated page flips; media scrubbing and volume dragging retain their interactions.

**The Settled Widget Rule.** Reuse the existing finite island morph and cross-fade for page changes; unchanged settled content snapshots must not trigger redraw. Calculator input and hover are event-driven and add no polling worker or background animation loop.

Native calculator and settings capture evidence remains indexed in `.impeccable/review/widgets/README.md`. The shipped right-edge vertical indicator is recorded in `.impeccable/review/widget-dots/README.md`, with Agent, Media, and Calculator at 80%, 100%, and 125%; these captures supersede the earlier horizontal Scroll/dots indicator evidence. These are native Direct2D/DirectComposition and egui surfaces; portable sidecar snippets illustrate appearance only.

### Native file converter

File converter uses the same Widgets group, label/description row and right-aligned switch. It is enabled by default, including for older configurations without an explicit value; Calculator remains opt-in. Disabling the switch removes the file-drop entry point. Converter availability does not add a compact identity or a scroll-widget page.

The expanded converter retains the expanded media composition with additional vertical room: a 544 x 184 logical-unit black island with the shared island-shell radius, uniformly scaled through the existing 80% to 125% expanded-size preference. A 176-unit square preview is vertically centered at (4, 4) with the 34-unit corners. The content column begins at x=196 and is 324 units wide; its filename/count and stage-aware supporting line begin at y=20 and y=41. Native Nunito reuses the 13-unit semibold title, 11.5-unit supporting line, and 12-unit medium centered format and action labels. These native roles remain separate from the settings type ramp.

For a single file, the on-demand helper requests a Windows Shell thumbnail, fits it into a 128-square premultiplied BGRA image and returns it with the format catalog. The native renderer clips it into the media artwork slot. When no thumbnail is available, including grouped files, an authored file outline and short uppercase extension identify the file. This is native drawn geometry rather than a text-glyph icon or shipping raster asset.

The ready grid shows one to eight provider-supplied formats within the right column. Up to four columns share its width with 8-unit horizontal gaps. Every option is 36 units high, beginning at y=70; five to eight options wrap to a second row at y=114, leaving an 8-unit vertical gap. Format buttons use 10-unit corners and translucent white fills: 11% at rest and 20% for hover or keyboard focus. The upper-right close control remains a native stroked cross, with a 28 x 28-unit hit target at (500, 16) and a 12% white fill for hover or keyboard focus. Do not substitute a text glyph.

Loading, Ready, Running, Complete and Error share the roomier shell and centered preview slot. The supporting line follows the current stage. Full success uses explicit completion text; partial or canceled conversions with saved results have a distinct stopped-with-results status. The composition adds no success-check badge. Outcome headings begin at y=70 and messages at y=108 in a 24-unit-high box within the right column. Running adds a thin determinate progress track at y=140, driven by worker updates.

Interactive footer actions occupy (196, 146) with a 324 x 28-unit target, 10-unit corners and centered labels. Complete offers Show in folder with green fill at 18%, increasing to 27% for hover or keyboard focus. Cancel and retry/dismiss actions use white at 8%, increasing to 16%. Ready devotes the right column to formats; Loading has no footer action. Closing while running requests cancellation and keeps completed results visible in the eventual outcome. Tab/Shift+Tab traverse available formats, the state footer when actionable, and close; Enter/Space activate, and Escape closes or requests cancellation. Keyboard focus uses the same state emphasis as hover.

**The Expanded Converter Rule.** Present conversion only in the expanded island; preserve compact activity and idle appearance. Dictation and urgent notices retain priority. Suppress widget page dots and wheel page switching while the converter is present.

**The Finite Conversion Rule.** Drive conversion presentation from native drag events, input and posted worker updates. Reuse the island's finite morph; settled Loading, Ready and outcome cards add no continuous animation or polling loop.

The surface workflow, provider scope and native evidence are recorded in `.impeccable/converter-extension.md`. Current roomier-layout captures in `target/converter-roomier/` cover Loading, Ready, Running, Complete and Error at 80%, 100% and 125%; earlier layout captures are historical. Portable sidecar snippets translate appearance only; they are not the native conversion engine.

## Do's and Don'ts

### Do:
- **Do** pair setting labels and descriptions with right-aligned controls.
- **Do** preserve visible keyboard focus and meaningful control metadata.
- **Do** keep native caption controls and match their frame to the sidebar.
- **Do** use the real island for preview and per-edge sizing feedback.
- **Do** keep sizing edits as drafts until Apply changes.
- **Do** preserve the readable calculator result and active-input tail across supported expanded scales.
- **Do** keep dictation and urgent notices ahead of optional widget pages.

### Don't:
- **Don't** restore the superseded purple atmosphere or wide sidebar.
- **Don't** add independent width and height controls or resize the compact pill through expanded sizing.
- **Don't** introduce decorative background motion.
- **Don't** add widget navigation arrows or clickable page dots; scrolling is the page navigation mechanism.
- **Don't** promote decorative kickers, glyph icons, or system display faces into shared components.

Not canonized: the music-site tag's literal `x` removal glyph is a carried craft defect, not an icon rule. One-off shell and editor values remain component details rather than new shared tokens.

