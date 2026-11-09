# Rust shell headless agent verification plan

Status: Phases 0 and 1 complete on Linux x64 (hardware Vulkan and llvmpipe),
2026-11-09. The agent runtime remains pending. See the
[feasibility audit](engine/src/flutter/shell/platform/rust/docs/offscreen-feasibility.md)
for the validated ABI/layout contract, and the
[headless host notes](engine/src/flutter/shell/platform/rust/docs/headless-host.md)
for virtual-window, plugin and CLI evidence.
Base branch: `rust/flutter-rs-shell`.

## Goal and scope

Let an AI agent build an application after editing it, exercise realistic UI
scenarios, assert the outcomes, and retain enough evidence to diagnose failures.
The primary result is a reproducible scenario with explicit assertions. Images
and video explain that result; visual plausibility alone is not a passing test.

The host must run fully offscreen, with no X11, Wayland, virtual display server,
or hidden native window. Keep the implementation specific to the Rust shell and
`rlutter`. Preserve Impeller, the private C++ integration, Rust plugin registration,
and one Flutter engine/root isolate per application. Multiple application windows
remain multiple views in that engine.

Deliver Linux x64 first, including a software Vulkan CI configuration after it
passes the renderer capability probe. Design the transport and host abstractions
for Windows, but gate Windows support on independent validation. Android's
FlutterActivity path and other Flutter shells are outside this work.

The first useful release includes multi-window interaction, semantics inspection,
assertions, screenshots, and failure artifacts. Recording follows the same frame
capture path in a later phase. Rich Dart widget inspection is an optional bridge,
not a prerequisite for controlling ordinary accessible applications.

## Review of the current implementation

Paths below are relative to the repository root. Existing architectural notes in
[flutter-rs.md](flutter-rs.md) describe both implemented and intended components;
the source findings here distinguish them.

| Area | Existing implementation | Required change |
| --- | --- | --- |
| GPU device | [`GpuContext::headless`](engine/src/flutter/shell/platform/rust/crates/flutter-shell-wgpu/src/lib.rs) creates a Vulkan device without a surface; Android uses this ownership model. | Add owned offscreen view images, completion tracking and readback. The constructor alone does not make the application headless. |
| Presentation | `GpuBroker` retains a winit window and a surface. Acquisition returns swapchain images. [`RustVulkanPresentation`](engine/src/flutter/shell/platform/rust/cpp/rust_vulkan_surface.cc) transitions the output to `PRESENT_SRC_KHR`. | Introduce an explicit offscreen presentation contract. Ordinary images must never use the swapchain presentation layout. |
| Rendering | [`GPUSurfaceVulkanImpeller`](engine/src/flutter/shell/gpu/gpu_surface_vulkan_impeller.cc) wraps borrowed images, caches per-view resources and includes an adapter-specific owned-target workaround. | Audit image wrapping, initial/final layouts, cache invalidation and that workaround before reusing this path for offscreen targets. Keep any shared change opt-in. |
| Event loop | [`shell.rs`](engine/src/flutter/shell/platform/rust/crates/flutter-shell-winit/src/shell.rs) creates a winit event loop. [`HostEventSender`](engine/src/flutter/shell/platform/rust/crates/flutter-shell-winit/src/host_events.rs) wakes an `EventLoopProxy`. `TaskQueue` already tracks monotonic deadlines. | Share engine task dispatch and plugin services with a display-free loop, using a condition variable/channel wakeup and timer-driven frame requests. |
| Multi-window | The Rust host and [`RustShell`](engine/src/flutter/shell/platform/rust/cpp/rust_shell.h) support view-aware input, metrics, focus, asynchronous add/remove and parented windows. [`_window_rust.dart`](packages/flutter/lib/src/widgets/_window_rust.dart) forwards Dart window requests. | Add a virtual window registry implementing the same callbacks without native handles. Preserve removal completion and parent ownership. |
| Input | Pointer/key packets and text-input sessions already exist in the host. | Expose reusable dispatch functions; inject events on the merged UI/platform thread and retain keyboard, pointer and text-editing invariants. |
| Semantics | [`PlatformViewRust`](engine/src/flutter/shell/platform/rust/cpp/platform_view_rust.h) accepts a view-aware semantics callback. `RustShell::Create` does not install it. The engine's [`SemanticsNode`](engine/src/flutter/lib/ui/semantics/semantics_node.h) includes identifiers, transforms and traversal/hit-test children. | Wire enablement, updates and action dispatch through the private ABI. Build a revisioned per-view cache. The architecture's proposed accessibility crate is not an implemented dependency to assume. |
| CLI | [`rlutter`](engine/src/flutter/shell/platform/rust/rlutter/src/cli.rs) builds and launches desktop runners and supports debugger attachment. Desktop launch currently gives the runner no stdin control stream. | Add explicit headless/agent options, a private runner control channel and a noninteractive scenario command. Reuse SDK/build/staging logic. |
| Test evidence | [`tests`](engine/src/flutter/shell/platform/rust/tests) contain texture, text-input, multi-view, FRB and reload/restart fixtures. Native tests use desktop automation and screenshots. | Reuse fixtures and their assertions through the headless protocol; retain native tests for compositor and OS behavior. |

Recent Windows progress includes popup positioning, grabbing-popup dismissal,
composition-derived pacing and local-engine CLI validation. A virtual host must
implement the corresponding application behavior deliberately; it cannot prove
those native integrations work. Earlier progress-table ABI numbers are historical:
derive any new ABI bump from the checked-out headers and paired Rust declarations.

## Architecture

```text
AI agent or scenario runner
          |
          | versioned JSON requests, responses and events
          v
rlutter agent session
  build / process supervision / logs / artifacts / optional video encoder
          |
          | private local control channel, separate from app stdout/stderr
          v
Rust shell shared runtime
  merged UI/platform tasks / plugins / input / semantics / virtual views
          |
          +-- native winit host (existing)
          |
          +-- headless host (new)
                timer-driven vsync / no native window services
                owned per-view Vulkan images / completed-frame capture
          |
          v
Existing private C++ shell and Impeller

Optional Dart verification bridge
  widget finders / application predicates / structured framework errors
  connects through the same session, on the Dart UI isolate
```

Extract only the runtime pieces both hosts actually need. Do not first rewrite
the whole winit shell into a platform framework. Start with modules, then move
shared host-independent code into a small runtime crate when the headless path
needs a dependency boundary. Keep protocol parsing and image/video encoding off
the merged UI/platform thread.

Suggested implementation homes under `engine/src/flutter/shell/platform/rust/`:

| Location | Responsibility |
| --- | --- |
| `crates/flutter-shell-core` | Paired private ABI values and headless configuration contracts. |
| `crates/flutter-shell-wgpu` | Offscreen target ownership, synchronization and bounded readback buffers. |
| `crates/flutter-shell-winit` and proposed `crates/flutter-shell-runtime` | Extract shared scheduling, view state, plugin and input services; leave winit adapters in the existing crate. |
| Proposed `crates/flutter-shell-headless` | Display-free loop, virtual window callbacks and agent request dispatch. |
| `cpp/` | Presentation mode/layout seam, semantics callbacks/actions and frame correlation hooks. |
| `rlutter/src/commands/` | Agent launch, scenario execution, reports and recording supervision. |
| Proposed `dart/flutter_rust_agent/` | Opt-in widget and application assertion bridge. |
| `tests/agent_fixture/` and `tests/test_agent.py` | End-to-end verification and intentional failure scenarios. |

## Offscreen rendering and capture contract

Each live view owns a bounded set of render targets, a size generation and a
monotonic completed-frame number. Use one supported, explicit color format first
(RGBA8 or BGRA8 after the capability probe), documenting alpha, orientation and
color encoding. Screenshot output is normalized to PNG with a declared color
convention. Test channels and alpha rather than assuming a format conversion.

Reuse the existing shared Vulkan device and queue-lock discipline. Make the
presentation mode and final layout explicit in the private bridge. The initial
image layout passed into Impeller, Impeller's final layout, transfer access and
wgpu's tracked state must agree. Do not pass a native image through safe wgpu
operations while its state tracker believes a different layout is active.
Choose the exact HAL/raw-Vulkan boundary in the first rendering spike and record
the validated sequence before generalizing it.

Track separate milestones: engine submitted, GPU completed, readback completed,
artifact encoded. A screenshot is available only after GPU completion and copy
completion. A successful `present_image` callback alone is not GPU completion.
Reuse semaphore handoff where appropriate, add fences/completion notifications
for reuse, and never wait on the UI thread or while holding the shared queue lock.

Readback must account for padded row strides and overflow-checked allocation
sizes. Reuse a bounded staging pool. Continuous readback is disabled unless a
recording or explicit capture needs it. Resize/removal retires old targets only
after engine and capture users release them. Device loss produces a session
failure with available diagnostics, not a stale successful screenshot.

Capture requests support `latestCompleted` and `afterFrame` for a particular
view and size generation. A request made before the first frame waits with a
deadline. For unchanged UI, an explicit observation request can schedule a frame;
it must not wait forever for damage that will never occur. Return the actual
frame ID and dimensions with every image.

## Scheduling and virtual windows

Keep Flutter UI/platform execution on one owning thread, with the existing raster
and IO runners. The headless loop sleeps until a host message, Flutter task
deadline or requested frame pulse. A wakeup cannot be lost between checking the
queue and sleeping. Bound each batch so protocol requests cannot starve Dart and
Flutter tasks cannot starve shutdown.

Use a configured refresh interval, initially 60 Hz, driven by a monotonic clock.
Only arm frame pulses when requested. Preserve engine-wide vsync semantics;
multiple windows do not imply independent engines or multiple Dart clocks.
Manual frame advancement can follow later. Advancing a frame does not advance
all Dart timers, network requests or plugin work, so v1 makes no claim of fully
virtualized time or universally deterministic execution.

Each virtual window records view ID, parent, kind, title, logical bounds, pixel
ratio, physical target size, focus, visibility, modal/grab state and lifecycle
generation. IDs and node handles are scoped to a session epoch. Preserve the
implicit view's special lifetime and asynchronous explicit-view add/remove.
Do not hardcode every application to draw into view 0.

Implement the current regular/dialog/popup/tooltip/satellite callbacks, including
size constraints, anchor updates, focus transfer, owner cascade and close
requests. Keep a deterministic synthetic desktop coordinate system for parent
placement and optional overview screenshots. Per-view input uses logical pixels;
capture uses physical pixels and reports the conversion. Tests cover fractional
scale, negative desktop origins and popup offsets.

Application operations must receive real completion/error outcomes. Unsupported
native services return an explicit unsupported result; do not fabricate a
successful file dialog, monitor query or desktop activation. Supply session-local
clipboard behavior and configurable locale, scale, text scale and theme where
needed for repeatable UI tests.

## Semantics and widget interaction

Enable semantics when an agent session requests inspection. Forward immutable,
owned update data over the private boundary; never retain pointers into temporary
C++ node/string storage. Preserve view IDs, labels, values, hints, identifiers,
roles/flags, supported actions, transforms, bounds and traversal order.

Merge incremental updates into a per-view cache and prune nodes that become
unreachable from the current root. Clear caches on view removal and isolate
restart. Publish snapshot revisions and require revision/epoch checks for handles.
Keep raw local bounds/transforms as well as computed view-space bounds; clipping,
hidden nodes and overlapping windows affect whether a coordinate tap can hit.

Selectors prefer explicit semantics identifiers, then role/label combinations,
scoped to a view. Zero matches and multiple matches are distinct failures. Never
silently choose the first matching “Save” across several windows. A stale handle
returns `stale_handle`; the client may re-query by a stable selector.

Separate semantic actions from physical input. `semantics.perform(tap)` checks
support and dispatches the action. `input.tap` goes through pointer dispatch and
hit testing. Tests of geometry, overlays and focus should use physical input;
semantic activation alone does not prove an element is clickable on screen.
Provide keyboard down/up, pointer move/down/up, scroll, text replacement and
text insertion. Keep text replacement distinct from typing and IME composition;
reuse editing-client/session state, including UTF-16 selections and composing
ranges. Release held input on disconnect, focus loss and shutdown.

Semantics exposes the accessible UI, not every Dart widget or private state.
Add an optional Dart bridge for `ValueKey` lookup, widget existence/count,
selected public properties and application-registered predicates. Reuse relevant
Flutter test concepts without installing a fake test clock into a production
binding. The bridge executes on the actual UI isolate and reports view ownership.
Do not make arbitrary VM expression evaluation the core protocol: current Windows
progress reports an expression-compilation gap despite successful attach/reload.

The bridge is an explicit test dependency/entrypoint, compiled out of ordinary
application builds. Framework error hooks must chain existing handlers. Native
semantics/input/capture can operate without it; capability discovery states which
assertions and diagnostics are available in each build mode.

## Agent protocol and assertion semantics

Proposed commands, not currently available:

```sh
rlutter run -d linux --headless --size 1280x800 --pixel-ratio 1
rlutter agent --headless --stdio --artifacts build/agent/session
rlutter verify test/agent/save_dialog.json --headless --report build/agent/result.json
```

Use versioned newline-delimited JSON with request IDs. The CLI's public stdio
contains only protocol output in agent mode. Route build/app/engine output to
stderr and artifact logs; use a separate private local channel between CLI and
runner because the engine and application can write arbitrary stdout. The
transport should support Unix sockets and Windows named pipes without assuming
either in the schema. Keep the endpoint local and session-owned.

Handshake returns protocol version, session epoch, engine/SDK revision, build
mode, selected adapter/backend, window defaults and supported capabilities.
Unknown methods/parameters, excessive message sizes and invalid numbers fail
explicitly. Serialize UI mutations; read requests operate on published snapshots.
Long operations accept deadlines and cancellation. EOF/parent exit cleans up the
owned runner, recordings and temporary endpoint.

| Group | Initial operations |
| --- | --- |
| Session | capabilities, health, shutdown; fresh-process reset as the reproducible default. |
| Windows | list, inspect, focus, resize, close; subscribe to creation/removal. |
| Semantics | snapshot, find, perform supported action. |
| Input | tap, pointer sequence/drag, scroll, key events, insert/replace text. |
| Verification | assert and wait for existence, count, value, enabled/focused state, window lifecycle and explicit app predicates when available. |
| Capture | screenshot by view and frame constraint; optional overview. |
| Diagnostics | structured events, logs, errors and artifact manifest. |
| Recording | start, stop, status after the capture path is proven. |

Example wire request:

```json
{"id":17,"method":"input.tap","params":{"viewId":2,"selector":{"identifier":"save"},"timeoutMs":5000}}
```

Action acknowledgement means the input was dispatched, not that the intended
effect happened. The client follows it with an explicit assertion such as a
dialog appearing or a saved value changing. The scenario runner composes these
steps and reports their distinct timings and outcomes.

Track action sequence, semantics revision, per-view frame/size generation and
session epoch. Add UI-to-raster frame correlation where needed for observation
requests: an assertion followed by a screenshot must request a frame built after
that observation boundary, not merely the next old frame to finish on the GPU.
Do not promise the semantics tree and pixels are one atomic snapshot unless that
pairing has been established. Report their revisions separately.

Waits use monotonic deadlines and specific predicates; they may optionally
require stability across observations. Infinite animations must not block an
unrelated predicate. Timeouts include the last observed state, unmatched selector,
recent errors and best available completed frame. No unbounded `settle` command.
Do not automatically retry state-changing input after a timeout: completion may
have happened even if the response was lost.

Use explicit error categories including unsupported capability, invalid request,
not found, ambiguous selector, stale handle, view closed, timeout, app exception,
GPU failure and process exit. A run cannot pass if it crashed or lost its control
channel. Expected-error scenarios declare the expected error narrowly.

## Recordings and artifacts

Encode video outside the render/UI threads from the same completed frames used
for screenshots. Initially supervise an optional local FFmpeg process; check
available codecs at recording start and report an unsupported capability if the
encoder is absent. Screenshots and assertions must not depend on FFmpeg.

Record a chosen view at a configured output rate with monotonic timestamps.
Duplicate the last completed frame during idle intervals so video duration matches
elapsed time. Bound the encoder queue and report dropped frames; slow encoding
must not block the application. Segment recordings on resize in v1, recording
the dimensions/time range of each segment in a manifest. Closing a view ends its
segment. Test finalization and partial-artifact reporting after crashes.

Multi-window v1 recording is one stream per selected view, with a common session
timeline and window lifecycle events. A later composite recording can arrange
views on the synthetic desktop. Do not imply independently captured view frames
were simultaneous; retain their individual timestamps and frame IDs. Cursor and
action overlays are optional derived evidence, not part of raw screenshots.

Each run writes `result.json`, ordered `events.jsonl`, app/engine logs, window and
semantics snapshots, screenshots and optional recording files. The manifest
includes source/build fingerprints, dirty-tree state, scenario version, environment,
GPU/driver, viewport/font settings, action/frame IDs, assertion results and artifact
paths. Exit nonzero for failed assertions or infrastructure failures and distinguish
them in the report. Record a passing test's scope rather than declaring the whole
change correct.

Set bounded artifact retention and output size. Keep artifacts in the selected
session directory, handle disk/encoder failure explicitly and identify missing
evidence. Password text and other obscured fields must not be copied into textual
snapshots or command logs by default; screenshots still contain whatever the app
visibly renders. A session option can redact known sensitive regions.

## Delivery phases and acceptance gates

Phases 0–4 are complete on Linux x64. Phase 5 is pending. Each phase should be
independently reviewable and preserve existing native shell behavior.

### Phase 0 Rendering feasibility and contracts

- Build a minimal offscreen rendering probe using the existing headless device
  constructor and private engine. Inventory every WSI/layout assumption in the
  Rust broker, C++ presentation and Impeller wrapper.
- Prove one real Flutter frame can be rendered and read back with `DISPLAY` and
  `WAYLAND_DISPLAY` absent and no connection to a display socket. Exercise both
  a supported hardware Vulkan adapter and a software Vulkan candidate.
- Validate image orientation/color, repeated target reuse, resize, texture-plugin
  content, synchronization and destruction under Vulkan validation.
- Freeze the presentation mode/layout ownership contract, transport version,
  frame correlation approach and ABI changes before expanding the implementation.

Gate: retain pixel evidence and clean validation output. If software Vulkan lacks
required Impeller capabilities, report that limitation and use an explicit GPU CI
lane; do not replace this milestone with Xvfb or a different Flutter renderer.

### Phase 1 Display-free host and virtual views

- Extract shared task wakeup, plugin registration, input and view lifecycle logic.
  Add the headless loop and offscreen target implementation.
- Implement virtual window callbacks and thread-correct teardown. Share the GPU
  device across views and retain the one-engine/root-isolate model.
- Add `rlutter run --headless` configuration and runner selection. Ensure ordinary
  launch does not create an agent endpoint or change the native presentation path.

Gate: the same multi-view fixture creates, resizes and removes explicit windows,
including owner cascades and popup repositioning, without a display server. A
plugin registers once, plugin textures render, idle does not busy-spin, and repeated
shutdown has no leaked processes, pending callbacks or validation diagnostics.

### Phase 2 Agent observation and control

- Add private transport, handshake, process supervision and bounded dispatch.
- Wire semantics enable/update/action ABI, per-view cache and selectors.
- Implement physical input, editing, focus, window operations and PNG capture.
- Add typed action/observation events, frame correlation and failure artifacts.

Gate: a protocol client can list two views, find a control by identifier, enter
Unicode text, click using real pointer events, and capture a correctly sized,
completed frame showing the result. Ambiguous/stale targets and view removal
during capture fail predictably. App stdout cannot corrupt protocol messages.

### Phase 3 Reproducible verification workflow

- Add scenario execution with explicit assertions, deadlines and result reports.
- Add the optional Dart bridge for widget-level assertions, custom predicates and
  structured framework errors; expose capability differences clearly.
- Reuse build fingerprints; default verification to a fresh process. Add optional
  hot reload/restart only with epoch invalidation and reattachment tests.
- Package the fixture with stable selectors and document how agents choose a
  scenario that exercises their change, including a negative control.

Gate: an agent edits a fixture, builds it, enters a value, opens a second window,
asserts its content/focus, closes it and asserts the persisted result. An intentional
regression must fail the same scenario with a useful report and screenshot. A
passing report must never result from a stale bundle or swallowed Flutter error.

This is the first useful release. Recording is not a dependency of this gate.

### Phase 4 Video evidence

- Add per-view recording, bounded frame delivery, idle-frame timing, segmentation
  on resize, timeline metadata and optional failure-triggered retention.
- Handle missing encoder, encoder crash, view closure and app crash with explicit
  recording status and retained partial evidence.

Gate: decode recorded output and verify frame dimensions, timestamps, idle duration,
action visibility, resize segments and two synchronized-by-timeline view streams.
Report capture/encoding overhead and dropped frames under load. Choose composite
recording only after the per-view contract is stable.

### Phase 5 CI and distribution

- Add display-free fixture jobs and targeted checks to the Rust-shell task tooling
  and [BETA workflow](.github/workflows/flutter-rust-beta.yml). Keep protocol,
  scheduler and cache tests runnable without a GPU.
- Package headless crates, generated runner changes and optional Dart bridge with
  matching engine artifacts. Test from an extracted SDK, not only this checkout.
- Verify Linux debug first, then native semantics/input/capture in profile/release.
  Keep test-only bridge code opt-in and ordinary release builds free of control
  endpoints. Require exact engine/runner ABI compatibility.
- Add a Windows headless lane after Linux passes; validate software/hardware Vulkan,
  transport, process ownership and release packaging independently.
- Add an MCP adapter only after the protocol and scenario workflow are stable.

Gate: a clean CI environment builds and verifies a generated app from packaged
artifacts without a desktop session. Supported build modes have explicit capability
tests and the existing native Linux/Windows/Android regression lanes still pass.

## Verification matrix

| Area | Required cases |
| --- | --- |
| Renderer | Empty/first frame, channel/alpha/orientation pattern, odd-width readback, fractional DPR, external wgpu and pixel textures, resize while capturing, resource reuse, GPU failure. |
| Scheduler | Cross-thread wakeups, due tasks, idle sleep, bounded fairness, requested vsync, no animation, infinite animation, shutdown with pending work. |
| Windows | Implicit and explicit views, two visible views, parent/modal focus, popup anchors and dismissal, closing an owner, removal while input/capture is pending, resize generations. |
| Semantics | Incremental merge, pruning, transforms/clipping, duplicate labels, identifiers, disabled/hidden nodes, text selection, restart invalidation, unsupported action. |
| Input | Tap versus semantic activation, drag/scroll, focus transfer, held modifiers, Unicode and RTL editing, composing ranges, pointer cancel, disconnect with held input. |
| Protocol | Malformed/oversized input, version mismatch, concurrent reads, ordered mutations, timeout/cancel, app log noise, client EOF, engine exit, stale epoch. |
| Assertions | Positive and negative controls, zero/multiple matches, delayed UI updates, no-frame updates, errors during an otherwise passing scenario, fresh build provenance. |
| Evidence | Frame-after-observation ordering, image encoding, missing/disk-full artifacts, idle video timing, backpressure, encoder/app crash, view closure/resize. |
| Distribution | Extracted SDK, local engine, unavailable adapter/encoder, no display environment or sockets, debug and supported AOT modes, ordinary native launch regression. |

Use exact pixels for controlled renderer patterns. Use declared tolerances and
environment fingerprints for UI goldens; fonts, rasterizer and driver differences
can change antialiasing. Never update baselines automatically merely to turn a
failed verification green. Measure frame/capture latency and memory versus view
count before setting performance budgets; no unmeasured performance promises.

## Boundaries and implementation decisions

Headless verification covers application behavior, shared shell logic and the
offscreen Vulkan path. It does not certify compositor pacing, native IME, real
monitor DPI changes, OS accessibility adapters, native dialogs or GPU-specific
swapchain behavior. Keep the existing native smoke and device tests for those.

Chosen defaults: Linux first, real monotonic time, semantic selectors with physical
input available, fresh-process scenario execution, local transport, bounded capture,
per-view PNG/video, one engine and one shared GPU device. No public Embedder API
migration, framework-wide testing feature or general remote-control service is
required.

Resolve through Phase 0 evidence: the exact Vulkan/wgpu image-state boundary,
minimum ABI additions, software Vulkan compatibility and the smallest frame
correlation hook. Resolve before Phase 3 release: Dart bridge dependency packaging
and which assertions remain available in AOT. These are implementation gates,
not reasons to defer the offscreen proof.
