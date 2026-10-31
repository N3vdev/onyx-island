---
name: Onyx website music island
description: Native music player geometry within the incumbent Onyx marketing page.
colors:
  island-ground: "#000"
  control-ink: "#f0f0f2"
  focus: "#b3b9cc"
  chip-ground: "rgb(255 255 255 / .115)"
  control-hover: "rgb(255 255 255 / .14)"
  source-hover: "rgb(255 255 255 / .22)"
  rail: "rgb(255 255 255 / .16)"
  progress: "rgb(255 255 255 / .78)"
typography:
  title:
    fontFamily: "Inter, Segoe UI, sans-serif"
    fontSize: "13px"
    fontWeight: 600
  artist:
    fontFamily: "Inter, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: "16px"
  compact-artist:
    fontFamily: "Inter, Segoe UI, sans-serif"
    fontSize: "10px"
    fontWeight: 400
    lineHeight: "13px"
  source:
    fontFamily: "Inter, Segoe UI, sans-serif"
    fontSize: "11px"
    fontWeight: 500
  clock:
    fontFamily: "Inter, Segoe UI, sans-serif"
    fontSize: "9px"
    fontWeight: 400
    lineHeight: "12px"
rounded:
  control: "50%"
  chip: "19px"
spacing:
  artwork-inset: "4px"
  transport-gap: "12px"
  chip-horizontal: "16px"
components:
  transport:
    backgroundColor: "transparent"
    textColor: "{colors.control-ink}"
    rounded: "{rounded.control}"
    width: "32px"
    height: "32px"
    padding: "0"
  transport-hover:
    backgroundColor: "{colors.control-hover}"
  source-chip:
    backgroundColor: "{colors.chip-ground}"
    textColor: "{colors.control-ink}"
    typography: "{typography.source}"
    rounded: "{rounded.chip}"
    width: "156px"
    height: "38px"
    padding: "0 16px"
  source-chip-hover:
    backgroundColor: "{colors.source-hover}"
  volume-chip:
    backgroundColor: "{colors.chip-ground}"
    rounded: "{rounded.chip}"
    width: "156px"
    height: "38px"
---

# Design System: Onyx website music island

## Overview

**Creative North Star: "The Native Music Island"**

This record governs the live music component, not the surrounding landing page
or the native settings application. The incumbent website retains its near-black
ground and Geist page typography. The component uses the native player's compact,
precise visual language and Inter typography.

The native authority is `../crates/islet-core/src/media.rs`, `island.rs`, and
`../crates/islet-render/src/lib.rs`, together with the shipped player captures in
`public/assets/shots/`. The built web implementation is recorded here; the
website's native app design records remain separate.

**Key Characteristics:**
- Solid black island with translucent neutral controls.
- Fixed native coordinates with uniform viewport scaling.
- Shared sharp artwork through a finite geometry morph.
- Actual media state, explicit source identity, and visible keyboard focus.

## Colors

The player uses neutral tonal layering over a solid black ground; artwork and
track-specific equalizer bars supply the changing color.

### Neutral
- **Island Ground:** The opaque player silhouette, isolated above the page grain.
- **Control Ink:** Transport controls, source label, and status text.
- **Chip Ground:** Shared translucent fill for source and volume capsules.
- **Control Hover / Source Hover:** Local interactive feedback on dark surfaces.
- **Rail / Progress:** Subordinate track and brighter elapsed portion.
- **Focus:** Visible outline for keyboard interaction across links and controls.

**The Clean Ground Rule.** Keep the island solid black. The page grain sits below
it (grain layer 79, island layer 80); page texture is not a player material.

## Typography

**Component Font:** Inter, with Segoe UI and sans-serif fallbacks. The loaded Inter
variable font supplies weights 400 through 600; synthetic weights are disabled.
**Page Font:** Geist remains the surrounding page's incumbent font, outside this
component record.

### Hierarchy
- **Title:** Shared compact and expanded title role; compact line height is 15px,
  expanded line height is 19px. Single-line ellipsis preserves fixed geometry.
- **Artist:** Expanded secondary metadata; the compact variant uses the smaller
  role in frontmatter, also with single-line ellipsis.
- **Source:** Medium-weight provider label inside the source capsule.
- **Clock:** Tabular numerals for elapsed and remaining time.

**The Native Type Rule.** Preserve the native component's small, clear type ramp
and real font weights. Scale the complete player to fit rather than independently
resizing its labels.

## Layout

Native logical dimensions are 220 x 36 compact, 108 x 32 condensed, and 496 x 120
expanded. The website's outer boundary adds one pixel per side, producing
222 x 38, 110 x 34, and 498 x 122 outer bounds. Initial placement is horizontally
centered, 7px below the viewport top; incumbent page docking behavior remains.

Artwork has the inset in frontmatter, with 28px compact, 24px condensed, and
112px expanded sizes. The web artwork begins at outer coordinate (5,5), accounting
for the boundary. Expanded coordinates below refer to the native content plane:

| Element | Native geometry |
| --- | --- |
| Title / artist | x132; title y20, artist y41; width164 |
| Transport centers | (148,74), (192,74), (236,74) |
| Seek rail | x132, y96; 164 x 2 |
| Times | x132, y102; width164 |
| Divider | x312, y20; 1 x 80 |
| Source chip | x326, y18; 156 x 38 |
| Volume chip | x326, y64; 156 x 38 |
| Volume track | x378, y83; 86 x 3 |

**The Native Fidelity Rule.** Preserve the native geometry and scale the complete
player uniformly on narrow viewports. The scale is min(1, max(0.1,
(viewport width - 24) / outer player width)); no separate mobile composition is
introduced. These coordinates are component measurements, not website-wide tokens.

## Elevation & Depth

The island itself has no shadow. Its opaque ground, subtle white boundary,
translucent capsule fills, and divider establish depth. Hover feedback remains
local to the interactive control; text links change color without a new fill.
Artwork stays sharp while the two text/control planes crossfade with transient
blur during the finite morph. The landing page's existing glows are outside this
component system.

## Shapes

The native silhouette uses cubic squircle handles (0.48), blending toward
0.5522848 as the ends become fully round. Compact, condensed, and expanded native
outer radii are 18px, 16px, and 38px; the web clip adds the boundary allowance.
Expanded artwork uses the same squircle construction with a 34px radius. One
shared artwork element persists through all three geometries. Circular transport
controls and round-ended source/volume capsules share the compact form language.

## Components

### Transport controls

Quiet circular buttons use inline SVG icons, transparent resting fills, and
subtle hover fills. Pressing scales a control to 0.92; keyboard focus has a visible
outline. The central icon and accessible label follow real play, loading, and
pause state. Loading pulses the central control; its action can cancel loading.

### Source and volume capsules

The source capsule uses the source role in frontmatter, an inline external-link
SVG, and a brighter hover/focus fill. Apple Music describes the actual streamed
iTunes preview provider. Artwork and title links retain Spotify track identity;
Strangers retains the exact user-selected release and unchanged cover. The volume
capsule contains a real mute button and native range input, with both affecting
the active media element. Playback defaults to 30% across the playlist and keeps
the listener's chosen volume when switching tracks. Source attribution remains
outside player geometry.

### Seek and time

The seek rail sits within an 18px-high interaction area. Hover or keyboard focus
reveals its white handle and subtle background. Elapsed and remaining labels use
actual media time. The volume range has a 22px-high input area and visible focus.

**The Truthful Playback Rule.** Use actual audio time and events. Official catalog
excerpts are accepted by the user; do not promise full-song openings. One active
audio element and a lazy silent next-clip preloader serve the playlist. Playback
is capped at 30 seconds; skips preserve pause intent and the latest selection.
A bounded loading state leads to an explicit retry message on failure.

### State transitions

**The Finite Motion Rule.** Mouse hover opens after 250ms; leaving closes after
400ms. Title and artist remain visible for five seconds of playback before
condensing. Keyboard focus and touch can open the player. The geometry morph
lasts 0.55s; reduced motion applies final geometry immediately. Paused or hidden
pages do not paint the audio clock. Preserve the incumbent page outside this
component correction.

## Do's and Don'ts

### Do:
- **Do** preserve native coordinates, artwork shape, and uniform scaling.
- **Do** use Inter with real weights and preserve visible keyboard focus.
- **Do** reflect actual media time, volume, mute, and recoverable failure state.
- **Do** identify the preview provider separately from Spotify track identity.

### Don't:
- **Don't** apply the page grain or glow treatments to the player ground.
- **Don't** replace shared artwork with blurred duplicate artwork planes.
- **Don't** invent a mobile arrangement or promise full-song opening clips.

Not canonized: legacy landing-page kickers and decorative glows are outside this
native component scope; they are not rules for future player surfaces to inherit.
