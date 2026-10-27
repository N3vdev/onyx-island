---
version: 1
slug: "crates-islet-render-src-widgets-rs"
primary_target: "crates/islet-render/src/widgets.rs"
related_targets: ["crates/islet-settings/src/ui.rs","crates/islet-core/src/widgets.rs","crates/islet-app/src/island_window.rs"]
---

# Scrollable island widgets

Mode: Operate. Local extension of the shipped native island and settings world.
Keep Claude's dual-player, condensed pill, sizing, and shared-element motion.
The user requested scroll widgets in the hovered/expanded island, first a
calculator, plus a Widgets settings pane to toggle widgets without extra idle work.

Use the existing native Direct2D renderer and its finite cross-fade for page
changes. Wheel navigation accumulates precision deltas and ignores burst repeats.
Per the user's latest instruction, scrolling is the only navigation mechanism;
non-clickable dots indicate the current page. Stack the dots vertically, centered
at the right edge of the expanded card, with even spacing and no Scroll text.
Calculator input remains
inside the island, with clear/delete, percent, sign, arithmetic, and error recovery.
No keyboard hook, web view, dependency, worker, or polling loop for calculator work.
Settings inherits Nunito, graphite groups, compact icon navigation, and saved
feedback. Calculator is opt-in and exists exclusively in expanded mode. Never
show widget names, icons, or cards in the collapsed island. Preserve its music,
activity, or configured idle appearance; enabling a widget must not create a
compact card or force an otherwise hidden island to appear.
Urgent notices and dictation keep priority. Compact sizes and expanded scaling
retain their existing behavior. The preview grows only when the tool is enabled.

Finish with native capture evidence, functional/performance checks, a fresh finish
review and updated design documentation. No HTML/CSS detector on native Windows.
