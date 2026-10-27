# Performance audit — 25 October 2026

The release build is in good shape for the measured workloads. This audit fixed
two dictation failure paths that could keep background work running, and reduced
avoidable work in settings. It does not establish that every workload or device
is optimally tuned.

## Measurements

Windows, 12 logical processors; optimized release builds. CPU is process CPU
time divided by elapsed time and processor count. Working-set figures include
resident shared libraries; private memory is reported separately in the raw data.

| Workload | Result |
| --- | --- |
| Hidden island, controlled config | No measurable CPU in a 5-second sample; ~19 MiB working set |
| Compact music demo, two warmed 10-second runs | ~0.36% total CPU; ~58 MiB working set; no private-memory growth in either sample |
| Compact music frame intervals | Median 33.37–33.39 ms; p95 34.12–34.24 ms, approximately 30 FPS |
| Expanded music demo | ~0.31% total CPU; ~58 MiB working set; median 33.40 ms, p95 34.06 ms |
| Static notification, settled | No measurable CPU in a 5-second sample |
| Settings, settled / minimized | No measurable CPU in each 5-second sample; ~65 MiB working set |
| Normal user configuration, media monitoring and live bars enabled | ~0.50% total CPU over 8 seconds; ~82 MiB working set; no speech worker running |
| Loaded Parakeet speech worker, idle | No measurable CPU over 5 seconds; ~691 MiB private memory, ~703 MiB working set |
| Speech model cold load | 2.31–2.35 seconds |
| Local 4.38-second speech fixture | ~266 ms release-to-final in real-time streaming; separate full-clip decode ~273 ms |

The speech model's large allocation is isolated in its worker process. It loads
on demand with the user's current `keep_warm = false` setting, and is unloaded
after the configured idle interval. No user preference was changed.

Short CPU samples have finite accounting resolution: “no measurable CPU” is not
a claim that a process can never wake. A shorter playback sample was noisier;
the two longer warmed runs above were consistent. These numbers are not universal
hardware guarantees.

## Changes made

- Removed disconnected microphone and keyboard-hook channels from the
  dictation controller's active selection. Closed channels are always ready;
  repeatedly selecting one could otherwise spin a CPU core. A vanished
  microphone cancels an active recording and reports the failure once.
- A speech-worker error now drops the microphone and ends audio capture.
  Previously the UI could return to idle while microphone packets continued.
- Removed the temporary deadline vector from each controller iteration and
  reused the outgoing audio buffer between packets.
- Cached the static settings background mesh by its rectangle. The gradient
  and geometry are rebuilt only when that rectangle changes. An isolated egui
  replay of 1,000 warmed background paint passes fell from 109.8 ms total to
  4.3 ms total: about 25 times less CPU time for that function, not for the
  entire application or GPU renderer.
- Unchanged settings now return before cloning and normalizing configuration
  on hover and animation repaints.

## Existing safeguards checked

At the time of this audit, rendering parked when settled, content animation was
capped near 30 FPS, and geometry motion used the compositor clock. The content
cap was removed on 26 October as described below. Hidden graphics resources have a
15-second release delay. Media/config/volume workers normally block on events.
Audio loopback runs only while playing media is shown and dictation is absent.
Microphone capture and speech-model loading remain separate from the UI thread.
Renderer text, geometry, and album-art caches have size limits.

## Validation and limits

All **188 tests** across core, rendering, media, audio, input, ASR, worker, app,
and settings passed. Clippy with warnings denied passed for all targets in those
packages. Three new controller regression tests cover a dead hook, disconnected
microphone state recovery, and capture termination after a speech failure, using
synthetic input without a real microphone or text injection.

Runtime probes used temporary app-data directories and deterministic demos;
the separate normal-config sample used the user's actual configuration without
editing it. The ASR check used the repository's synthetic speech fixture.

Native cursor movement was denied in this session, so live hover/drag latency
could not be measured. The background optimization was measured with egui frame
replay instead. No GPU power trace, physical microphone unplug test, or extended
memory-leak soak was performed. Short samples showed stable resources, which
does not rule out every long-running leak.

Raw measurements and temporary profiling tools are under
`target/performance-audit/` and `target/audit-*.ps1`; they are not shipped.

## High-refresh animation update — 26 October 2026

The primary display reports 180 Hz. Before this update, shape transitions
already ran at that cadence (median 5.555 ms), but content animation was capped
at 30 FPS (median 33.337 ms). All visible animation now follows the compositor
clock. The renderer still blocks when settled; no busy wait or new timer was
added. Audio capture retains its existing update rate, with allocation-free,
time-based interpolation of the five displayed bars. The DXGI device now limits
queued frames to one to reduce latency.

In a 16-second release demo cycling hidden, compact music, and expanded music:

| Frame type | Samples | Median interval | p95 interval |
| --- | --- | --- | --- |
| Shape transitions | 656 | 5.5605 ms | 5.956 ms |
| Animated content | 1,205 | 5.563 ms | 5.867 ms |

Intervals of 150 ms or longer (idle gaps) were excluded. These are application
frame timestamps, not a measurement of physical display scanout or input latency.

Separate warmed eight-second samples measured hidden idle at no observable CPU
and 18.6 MiB working set, and compact music at 1.69% total machine CPU and
57.5 MiB working set on 12 logical processors. These supersede the older 30 FPS
demo figures for the current rendering policy. Higher refresh requires more
active rendering work; GPU power was not measured. The normal app was relaunched
with the original user configuration after stopping the isolated demos.

All 156 tests in core, rendering, and app passed, including three new audio-bar
smoothing tests. All-targets Clippy with warnings denied and the release build
passed. Raw timing and resource measurements are in
`target/performance-audit/refresh-*`.

## Unchanged-frame skipping — 26 October 2026

While the island's shape is settled, the renderer now hashes everything that can
change over time in the frame: bar heights and the expanded seekbar, both at
half-device-pixel resolution, plus the clock seconds, hover state and geometry.
A vblank whose hash matches the frame already on screen is skipped, with no
drawing and no Present. Shape motion, content transitions and forced redraws
always draw. Agent and dictation cards are never skipped, because their motion
is not modelled. Frame pacing remains at the compositor's 180 Hz.

Warmed 10-second release demos, 12 logical processors:

| Workload | Before | After |
| --- | --- | --- |
| Compact music (procedural bars) | 1.69% total CPU | 1.33% total CPU (15.9% of one core) |
| Expanded music | Not measured at 180 Hz; drew every vblank | 0.026% total CPU (0.31% of one core) |

The expanded player's seekbar moves about one pixel per second, so it now draws
only a few frames per second. Compact bars move on almost every vblank, so the
saving there is smaller. With real audio, frames are also skipped during quiet
passages, when the bars settle. The remaining compact cost scales with frame rate:
caching the static artwork and text was tested earlier and made no measurable
difference. Raw data: `target/performance-audit/skip-*`.

## Native widgets (2026-10-27)

Calculator state is handled by input events in the existing UI thread and drawn
by the existing Direct2D renderer. It adds no worker, web view, keyboard hook, or
polling timer. Its snapshot does not animate. Unchanged snapshots (including
background media updates while the calculator is selected) no longer force a
draw. Widget transitions reuse the existing finite shared-element/cross-fade
passes. Preview z-order recovery reuses the settings process's existing two-second
health repaint and an attachment message, with duplicate render snapshots skipped.

Short release-process samples, 12 logical processors, ten seconds each after a
three-second warm-up, with media/dictation disabled and settings closed:

| Mode | CPU seconds | Normalized CPU | Private memory | Working set |
| --- | ---: | ---: | ---: | ---: |
| Pre-widget release, hidden | 0 | 0% | 6.54 MB | 20.96 MB |
| New release, widgets disabled | 0 | 0% | 6.50 MB | 18.78 MB |
| Calculator compact, first sample | 0.265625 | 0.2212% | 81.67 MB | 61.02 MB |
| Calculator expanded, settled | 0 | 0% | 77.10 MB | 59.11 MB |

Two follow-up settled compact samples recorded zero CPU seconds over ten seconds
each; the frame log stayed at 102 initial transition rows throughout both samples.
The initial compact spike is retained above rather than treated as steady-state
cost. These are short samples at process timer resolution, not a claim of zero
resource use. A visible widget retains the renderer's graphics resources, unlike
the hidden baseline, which can release them. Interaction and transitions cost work.

Data: `target/widgets-runtime-test/performance.json`, `compact-repeat.json`, and
`agent-roundtrip.json`. Unit coverage checks that settled calculator snapshots
at 80%, 100%, and 125% request no animation frames and unchanged snapshots do not
request a redraw. Tests and capture readback helpers are excluded from production.

## Calculator keyboard input (2026-10-27)

Input uses the existing native message loop and active keyboard layout. No new
global hook, timer, worker, or dependency is introduced. Repeated characters in
one Windows message are applied as a batch with one content snapshot. Activation
style changes only when the calculator becomes available or unavailable; normal
keyboard updates reuse the static calculator rendering path. Modifier checks run
on keyboard events, not a polling loop. This extension has no new resource sample;
the earlier widget measurements above retain their original scope.

## Expanded-only widget correction (2026-10-27)

Enabling Calculator no longer creates a compact widget card. Collapsed mode uses
the configured Hidden/Sliver appearance or existing music/agent activity. The
earlier "Calculator compact" samples describe the previous implementation and
are not measurements of the current collapsed behavior. Expanded Calculator
retains the existing event-driven rendering path. This correction introduces no
new worker, hook, or polling and has no additional resource sample.
