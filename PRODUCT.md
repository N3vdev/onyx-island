# Islet

## Product
Islet is a native Windows dynamic island for music playback, local dictation,
notifications, and agent activity. A separate settings process edits a shared,
atomically saved configuration. The island renders through Direct2D and
DirectComposition; settings uses Rust and egui.

## Platform
Native Windows desktop. This is not a web or mobile surface.

## User and task
The current user customizes their personal desktop on a 180 Hz display and
expects fluid motion with low idle resource use. Broader audiences are not
established by this brief.

## Confirmed constraints
- Keep the dark theme, rounded typography, bold headings, and matching native
  titlebar requested in this conversation.
- Replace the large settings sidebar with the user's compact sidebar reference:
  narrow charcoal rail, small line icons, restrained active rows.
- The user's dark workflow-canvas reference pins the workspace: near-black,
  subtle dot grid, soft panel outlines, and restrained state accents.
- Give the actual island a reliable, dedicated preview inside settings.
- Preserve Dynamic / Top / Bottom positioning, dual-player support, condensed
  music, hover/click expansion, dictation, and Claude's existing motion changes.
- Under Position, provide Customize sizing. Its full-screen overlay edits only
  vertical edge spacing (at most 5%) and proportional expanded-state size.
  Do not expose independent width and height controls or resize the compact pill.
- No background animation loops for static settings; respect reduced motion.
- Add scrollable widgets to the expanded island, beginning with a calculator,
  and a Widgets settings pane for enabling or disabling individual widgets.
- Widget work must remain event-driven and preserve the existing media,
  dictation, and motion behavior.
- Switch widget pages by scrolling only; navigation arrows are removed.
- Expanded widget indicators are vertically stacked dots on the right, without
  a Scroll label inside the island.
- Calculator accepts keyboard number entry while it is the active widget.
- Widgets exist only in expanded mode. The collapsed island must never show a
  widget name or replace music, activity, or the configured idle appearance.

## Current implementation assumptions
The user supplied and confirmed two visual references on 26 October 2026.
Top and bottom keep separate spacing and expanded-scale preferences.
Overlay changes are a draft until Apply; Cancel and Escape restore settings.
Widgets are opt-in and appear only after expansion. Enabling them preserves the
configured idle appearance; an idle sliver can still expand to the calculator.
Wheel navigation switches between the
current activity and calculator; dictation and urgent notices retain priority.
Calculator uses native window focus for keyboard input. Clicking it begins typing;
wheel selection also requests focus when Windows permits it. Leaving the island,
switching away, hiding it, or showing a priority card returns its focus lease.
