# Flutter Rust Shell Progress (WIP)

This file records implementation progress for the architecture in
[`flutter-rs.md`](flutter-rs.md). It is an implementation log, not a replacement
for the architectural plan.

Headless agent verification is now specified in
[`flutter-rs-agent-plan.md`](flutter-rs-agent-plan.md), following a source review
on 2026-11-09. It covers fully offscreen rendering, virtual multi-window behavior,
semantics and optional widget assertions, input, screenshots, video and CI
evidence. Phases 0 and 1 are complete on Linux x64. The display-free virtual
host is available; agent observation/control and verification remain pending.

### Headless Phase 0 (2026-11-09)

Implemented the offscreen presentation seam (private ABI 15), owned targets,
padded readback and a minimal display-free engine probe. Native presentation
retains mode 0; mode 1 restores COLOR_ATTACHMENT_OPTIMAL for wgpu tracking.
Current runners check the engine-compiled ABI before passing shell callbacks.
The probe serializes capture on raster and publishes a bounded set of completed
frames; production asynchronous capture is later work.

Twelve real Flutter frames passed on both RTX 5060 and llvmpipe under Vulkan
validation with no display variables or X11/Wayland socket connections. Pixel
assertions cover channels, orientation, alpha, external wgpu texture content,
animation, odd widths, reuse, resize and fractional DPR. Retained artifacts
include raw pixels, normalized PNGs, reports, logs and syscall traces. The
[feasibility audit](engine/src/flutter/shell/platform/rust/docs/offscreen-feasibility.md)
records the exact validated sequence and reproduction commands.

Validation: 17 C++ tests, 1 core test, 6 wgpu tests and 52 winit tests passed;
5 existing manual GPU unit tests stayed ignored. The rebuilt native animated
texture fixture passed with 17 presentations. Evidence is in the Rust
workspace's `target/offscreen-probe/flutter-{hardware,software}-final/`.

The shared host, virtual views, plugin services and CLI launch follow in the
Phase 1 entry below. Agent transport, semantics, input, assertions, screenshot
observation boundaries, recordings and distribution remain pending.

### Headless Phase 1 (2026-11-09)

Added a display-free adapter sharing the task dispatcher, SDK registration and
GPU texture backend with the native host. Condition-variable wakeups use the
queue mutex; event/task batches are bounded. A single monotonic 60 Hz clock
serves requested engine frames. Virtual regular/dialog/popup/tooltip/satellite
views preserve asynchronous removal, owner cascades, focus, constraints,
shrink wrapping, placement and satellite reparenting. Clipboard is session-local.
Ordinary headless runs disable continuous GPU readback.

`rlutter run -d linux --headless --size WIDTHxHEIGHT --pixel-ratio SCALE` builds
and launches the current runner, preserving debugger attach. An extracted
per-project SDK layout and generated runner passed the local-engine CLI path.

The existing regular, owned and tooltip fixtures each completed 60 removals;
explicit tooltip resize/anchor/partial-update geometry checks passed. The SDK
pixel-texture fixture registered once, rendered correct pixels and reclaimed
its resources across fresh-process shutdowns. llvmpipe passed the regular-view
lifecycle gate too. Validation logs and display-socket traces were clean. A
static owning UI thread used zero CPU ticks over the measured idle second.
See the [host notes](engine/src/flutter/shell/platform/rust/docs/headless-host.md)
for commands, limits and artifact locations. Validation also passed 55 winit
tests, 6 wgpu tests and 109 CLI tests; the native texture regression passed
with 22 presentations. Agent transport, semantics, input,
assertions and production capture remain Phase 2/3 work.

## Current focus

Android hardware-buffer textures have an experimental SDK capability and a
bounded mailbox in `flutter-shell-android`. ABI 13 extends the existing external
texture bridge with producer leases and crop rectangles. The C++ adapter reuses
Impeller's AHB importer and command tracking; device creation enables supported
YCbCr conversion through the HAL callback's pNext chain. Producer leases survive
unregistration through GPU reads and foreign ownership release. The realtime
player shares output/sample/clock handling between Playbin and WSC-RTP and keeps
CPU fallback. Its Java Flutter plugin supplies GStreamer's VM/class loader and
uses the matching SDK's Java helpers. Runner version 8 adds Android DF_1_GLOBAL
visibility for FRB process lookup and native runtime providers.

On 2026-10-29, an Armor 21 (API 36, Mali-G57 MC2) displayed advancing H.264 video
through AHardwareBuffer output, completed the twenty-second clip at normal
speed, and passed session replacement and background/resume checks. CPU output
was separately verified with GStreamer 1.28.3. A development overlay SDK
(pkg-config 1.29.2.1) supplies the forthcoming 1.30 AHardwareBuffer API; the original
SDK was preserved. The YCbCr feature validation error was resolved on device.
The direct path also exposed missing stencil attachment synchronization:
RenderPassBuilderVK's incoming dependency now covers early/late fragment tests
and depth/stencil writes. That engine fix rebuilt successfully, and the WSC-RTP
APK built, but the device disconnected before their final validation run.
Remaining checks include clean Vulkan validation, WSC-RTP, dynamic resizing,
performance measurement, and a second GPU vendor.

Public Embedder API migration and prototyping are deferred; see the
[2026-10-18 reassessment](flutter-rs.md#embedder-api-reassessment-2026-10-18).
Source inspection confirms single-engine multi-view APIs, but Vulkan Impeller
compositor backing stores are unimplemented and the public Vulkan GPU handoff
requires host synchronization. No backend migration or runtime validation has
been performed. Continue developing the private Rust-shell integration. Revisit
when the public API provides Vulkan Impeller compositor backing-store support
and suitable GPU synchronization. Android's adoption of Impeller alone does not
resolve these gaps, because Android uses its internal platform integration.

Multi-window performance now uses upstream #179874's dirty-view compositing,
including external-texture repaint notifications and view lifecycle handling.
The Rust host requests redraw from one pacing window per engine vsync request
and bounds delivery by that monitor's nominal interval. A timed fallback keeps
static or hidden surfaces from starving engine frames. Static sibling windows
no longer trigger redundant surface submissions or a busy native event loop.
The debug benchmark on this machine sustained about 60 implicit-view submissions
per second with 1 through 12 static children, with native main-thread CPU around
4–6% of one core (previously about 97–99% with dirty-view compositing alone).
With all views animated, five children sustained about 60 submissions per second,
but eight and twelve still fell to about 28 and 12 respectively. Raster-thread stack
sampling showed that most of this was the Khronos validation layer. Debug builds
enable it through wgpu's `InstanceFlags::default()`, and because Impeller shares
the instance, it also intercepts every Impeller call. An earlier note claimed that
disabling wgpu validation did not help, but that was wrong: wgpu was built without
its `std` feature, so `WGPU_VALIDATION=0`/`WGPU_DEBUG=0` were silently ignored.
With `std` enabled and validation disabled, eight and twelve children sustain about
67 and 45 submissions per second (12-child raster p95 drops from 125 ms to 30 ms).
`benchmark_windowing.py` now disables wgpu validation unless `--validation` is
passed. The remaining per-view cost is NVIDIA driver submission work. The phased
plan in [`flutter-rs-perf-plan.md`](flutter-rs-perf-plan.md) addresses it. These
are debug measurements with compositor-assigned window sizes, not release
guarantees.
Phase 3b of that plan was implemented and correctness-validated, but reverted:
signalling the wgpu render semaphore from the tracked Impeller barrier submit
removed the signal-only submit while preserving `FenceWaiterVK` lifetime
tracking. Its 3-run immediate-baseline comparison showed only a 1.7% raster-p50
improvement at both 8 and 12 children, within noise, plus guard regressions.
The extra submit therefore remains until a different synchronization design
measures better.
Phase 3c was also correctness-validated then reverted. It moved the wgpu
acquire wait onto Impeller's first onscreen submit, including the batching
flush path, but raster p50 remained within noise and 8-child present p95
regressed by 20.5% beyond its 15% threshold.
Phase 3d changed the wgpu acquire handoff from a transparent clear to
`LoadOp::Load`. The Vulkan lifecycle fixture passed, but the required texture
fixture reached texture initialization and pixel capture before failing its
window cleanup/runner-exit step. It was reverted without performance
measurement because the correctness gate did not pass.
Validation for this change passed 32 Rust unit tests, 19 relevant framework
tests, five benchmark-accounting tests, and the native 60-removal fixture with
Vulkan validation forced. The native animated-texture screenshot check remains
unverified: submissions advanced, but captures showed the desktop lock screen.
Rerun `task test-rust-shell-texture` in an unlocked session before considering
the native pixel-change regression covered.

Multi-view GPU lifecycle hardening now defers initial surface configuration to
raster acquisition and releases removed views' swapchains on the raster runner
before notifying winit. ABI v11 adds the surface-release callback and a private
raster-task posting entry point for Android surface replacement and shutdown.
External-texture unregister explicitly reclaims GPU resources on raster even
when SDK handles remain retained by workers. The application-scoped GPU context
checks that presentation, configuration through acquire, and explicit release
all use the same thread. `task test-rust-shell-windowing` runs a native fixture
that creates, resizes, and destroys 60 windows while the primary view animates,
then exits with additional views alive to exercise whole-shell teardown.

Phase 0, the Phase 1 host, and the Phase 2 shared-GPU/texture proof are complete
for Linux. The current cross-cutting focus is generated Cargo application and
plugin aggregation; the next platform phase is Windows. Pointer,
window metrics, display updates, lifecycle, raw keyboard events, and a rendered
Impeller/wgpu frame are working. The explicit Vulkan semaphore broker is now
implemented and passes rapid-resize and in-flight teardown stress. Typed text
input/IME plumbing and compositor-driven Wayland vsync are implemented and
validated. The current milestone is single-engine multi-view: one Flutter
engine and Dart isolate per application, with one native winit window and GPU
presentation surface per Flutter view. Regular, dialog, tooltip, popup, and
satellite windows now work through the typed Rust backend; main-thread dispatch
is now wired through the plugin SDK and winit host queue, and deterministic
startup/shutdown ownership is covered in both unit and repeated native tests.
The engine-side `RustExternalTexture` seam, private registration ABI, and a
triple-buffered wgpu texture ring are implemented. An opt-in internal producer
has rendered continuously changing zero-copy frames in a real Flutter
`Texture` widget. The ring now implements the safe SDK handle and frame backend
using a bounded Tokio channel for asynchronous slot availability. Its creation
factory is installed in `PluginRegistrar` after shell startup, and the animated
proof now registers as a normal source-linked plugin using only public SDK
operations. Both hardware and CPU pixel-buffer fixtures are checked in and pass
locally. Texture lifecycle hardening now shuts down frame production, wakes
asynchronous slot waiters, and reclaims each retained ring after raster-thread
unregister completes. The real-runner lifecycle fixture also injects the
texture registry's context-destroyed/context-created sequence midway through
live texture replacement and proves presentation resumes afterward. The
general application plugin-registration entry point is now wired: a generated
Cargo application can pass `register_application` to the host, which invokes
it exactly once after the engine, implicit view, dispatcher, and GPU capability
are live. The end-to-end FRB fixture now proves root-isolate synchronous calls,
background-isolate worker calls, deferred non-reentrant dispatch, main-thread
execution, response delivery, and clean native shutdown through that entry
point.
The winit host is no longer hidden inside a Linux-only module: shared event-loop,
input, lifecycle, window bookkeeping, texture, and plugin code compiles at the
crate root. Native operations use statically dispatched platform traits, with
the current Linux implementation isolated under `platform/linux.rs`.
The Rust-shell SDK (crates plus prebuilt engine libraries) is now distributed
through a rolling BETA GitHub release built by CI from every push, so an
FVM-installed Flutter fork can use `rust-shell: true` without a local engine
checkout; a checked-out engine tree is still preferred automatically when
present. `flutter run -d rust` now supports `--release` in addition to debug:
the tool AOT-compiles the Dart app, builds `runner-rs` with `cargo build
--release`, and links it against a second, precompiled-runtime engine build
that the CI workflow now produces alongside the existing debug one.

Android work (milestones 1–2) has separately proved a GameActivity cdylib that
dynamically links a GN-built engine and renders a real Dart isolate, but only
in debug/JIT mode. Milestone 3 brings the same native release/AOT support
Linux already has to that Android scaffold: fixed a `build_rust.py` bug that
silently built every Cargo GN action's debug profile regardless of
`FLUTTER_RUNTIME_MODE`, added `aot_library_path` derivation to the Android
runner, and validated a real release-mode `app.so` rendering interactively on
a physical device with no `kernel_blob.bin` present (proving the AOT path,
not a silent JIT fallback). A generalized `AndroidRustShellDevice` in
flutter_tools (matching `RustShellDevice` for Linux) — generated-app Android
templates, real Gradle-driven `flutter run -d <android>`, and `AAssetManager`
asset loading instead of the adb-push placeholder — remains future work, the
same way Linux's own flutter_tools integration followed its native runner.

## Status

| Area | Status | Evidence |
| --- | --- | --- |
| Existing shells remain available | Complete | The Rust target is opt-in and is not added to the existing platform-selection group. |
| In-tree Rust platform target | Complete | `//flutter/shell/platform/rust:flutter_rust_shell` builds. |
| Internal PlatformView adapter | Complete | `PlatformViewRust` builds and its focused tests pass. |
| Rust/C++ ABI | Surface-lifecycle hardening complete | ABI v11 retains the external-texture and multi-view contracts and adds explicit raster-thread surface release. Engine-generated external-texture IDs still support acquire, mark-frame-available, release, and unregister operations for borrowed Vulkan image/image-view handles and semaphore pairs. Raster unregister reports completion so Rust can reclaim callback owners safely; a private test-only entry point injects texture-registry context destruction/recreation on the raster runner. |
| Rust workspace and `flutter-plugin-sdk` | Complete for foundation | Workspace uses Rust edition 2024, concrete toolchain 1.93.1, and passes its tests. |
| Winit event loop | Complete for phase 0 | Linux host owns the window and event loop, dispatches Flutter task batons, and drives the Rust-owned Vulkan presentation loop end to end. |
| Cross-platform host boundary | Linux adapter extracted | Shared host code is unconditionally compiled. `PlatformBackend` statically selects native popup creation, transient parenting, window attributes, compositor detection, and legacy key encoding; `LinuxPlatform` contains the Wayland/X11 implementation. `EngineBridge` similarly centralizes the C++ task-runner boundary and its test fake. No shell-owned backend trait uses dynamic dispatch, and test configuration is confined to the engine implementation selector plus `mod tests`. |
| Merged UI/platform task runner | Complete for phase 0 | `RustTaskRunner` queues batons for the Rust host, winit returns due batons through opaque C++ handles, and it now also drives Dart's per-task microtask flush (see below). |
| Impeller/wgpu interop | Explicit synchronization implemented and stress-tested | wgpu owns the Vulkan device/surface; C++ creates `ContextVK` from borrowed handles plus the in-tree Impeller Vulkan shader bundle. Per-frame binary semaphores now order wgpu acquire → Impeller render → wgpu present, synchronization objects stay alive through the consuming submission, and resize waits for that submission before swapchain replacement. A repeatable 500-resize compositor stress run, including hide/restore, fullscreen, and immediate teardown, exits cleanly with no wgpu or Impeller synchronization diagnostics. |
| Linux runnable shell | Complete for rendered-frame proof | `flutter_rust_shell_runner` boots a real kernel-snapshot Flutter app; a live Hyprland capture shows the Flutter title, text field, button, and debug banner rendered in the Rust shell. |
| Pointer input | Complete for phase 1 plumbing | Winit mouse, wheel, and touch events cross the private ABI and are converted into Flutter `PointerDataPacket`s; Rust translation and C++ conversion tests pass. |
| Mouse cursors | Complete for system cursors | The Rust host handles Flutter's standard `flutter/mousecursor` method channel, maps every `SystemMouseCursor` kind to winit, supports hidden cursors, and updates all native windows owned by the engine. |
| Window and display metrics | Complete for phase 1 plumbing | Initial, resize, and scale-factor changes report physical viewport size, the real device-pixel ratio, and current-monitor size/refresh rate; zero-sized surfaces are not configured. |
| Lifecycle | Complete for phase 1 plumbing | Focus, minimize/restore, winit suspend/resume, and shutdown are deduplicated in Rust and forwarded through `flutter/lifecycle`; Rust transition and C++ ABI conversion tests pass. |
| Keyboard input | Complete for phase 1 raw events | Winit physical/logical keys, down/up/repeat, characters, modifier sides, and synthesized state cross the private ABI as Flutter `KeyData` packets. |
| Text input and IME | Complete for phase 1 plumbing | The Rust host handles the standard `flutter/textinput` protocol with typed commands and validated UTF-16 editing state, controls winit IME activation/cursor geometry, translates preedit/commit events, and sends `TextInputClient.updateEditingState` back to Flutter. Ordinary typing, Backspace, and Ctrl+A were verified interactively; a legacy `flutter/keyevent` terminator keeps Flutter's modern key-data queue moving. |
| Vsync | Complete for the Linux Wayland host | Flutter's waiter requests one pacing-window redraw through the private ABI. Delivery is bounded by the monitor's nominal refresh interval, with a timer fallback when a static surface has no new compositor callback. Frame callbacks remain registered before actual wgpu presentation. C++ timestamps pulses in the FML clock domain. Non-Wayland backends retain `VsyncWaiterFallback`. |
| Multi-window | All five controller kinds implemented | View `0` remains the implicit engine view. Positive-ID winit windows share one engine, root isolate, plugin registry, task runner, and wgpu device while owning independent surfaces, metrics, input, and presentation state. Flutter's experimental window controllers select the Rust owner automatically in the Rust runner. Dialogs and satellites use native transient relationships. On Wayland, tooltip and popup views use real compositor-positioned `xdg_popup` roles; popup grabs are serial-bound and compositor dismissal enters the normal asynchronous Flutter view-removal path. Satellites support creation, shrink-wrap, reparenting, parent-driven teardown, and parent maximize/fullscreen visibility. Standard Wayland does not permit clients to choose absolute toplevel positions, so the initial satellite positioner is honored on X11 but compositor-selected on Wayland. |
| Main-thread dispatch | SDK, host, and Dart/FRB path complete | `flutter-plugin-sdk` exposes a cloneable worker-safe dispatcher through `PluginRegistrar`. Work is always queued rather than invoked inline, executes through winit's owning thread, is limited to 64 callbacks per event-loop turn, and is rejected after shell shutdown. Unit coverage verifies worker posting, thread identity, nested non-reentrant dispatch, starvation bounds, and shutdown. A real application-level FRB fixture verifies root and background Dart isolates through the Cargo-owned runner while displaying SDK wgpu and pixel-buffer textures. |
| Startup and shutdown ownership | Complete for merged runner | The dispatcher rejects work during bootstrap, starts only after the C++ shell and implicit view are installed, stops before shell/window teardown, and suppresses already queued callbacks after shutdown. A native lifecycle task requires 20 consecutive mapped-window startup, compositor-close, and status-zero shutdown cycles. |
| Application plugin registration | Runtime and Flutter-tool generation complete | `flutter-shell-winit::run_application` accepts `register_application(&mut PluginRegistrar)` and invokes it once after startup capabilities are installed. For `rust-shell: true` projects, the Flutter tool discovers source-linked Rust plugins from the resolved Pub graph, updates the marked dependency block in `runner-rs/Cargo.toml`, generates `flutter_plugins.rs`, and validates a single resolved plugin SDK. |
| Flutter CLI launch | Linux x64 debug and release complete | `flutter run -d rust` discovers the Rust shell as a built-in local device for `rust-shell: true` applications. Debug mode builds `build/flutter_assets` and the generated Cargo runner and uses the normal resident-runner lifecycle for logs, hot reload, and shutdown. Release mode AOT-compiles the Dart app to `app.so`, builds `runner-rs` with `cargo build --release`, and launches the release runner directly (no VM-service/resident-runner support, matching other release desktop targets). Profile mode is not yet supported. |
| Rust-shell SDK distribution | BETA channel complete | CI (`flutter-rust-beta.yml`) configures and builds the engine in both `debug` and `release` runtime modes, packages the workspace crates plus each engine's `libflutter_rust_engine.so`/`icudtl.dat` under `lib/debug/` and `lib/release/`, and republishes them to a rolling `BETA` GitHub release tagged to the latest imported Flutter version. `flutter pub get` on a `rust-shell: true` project prefers a local engine checkout's `out/host_debug`/`out/host_release` when present and otherwise downloads and unpacks the BETA tarball into `.dart_tool/flutter_rs/sdk/`; `runner-rs/build.rs` links whichever profile directory matches Cargo's own build profile. |
| Android scaffold (native) | AOT/release mode complete, debug/JIT already worked | The GameActivity cdylib (`crates/flutter-shell-android-runner`) dynamically links a GN-built `libflutter_rust_engine.so` and renders a real Dart isolate/Impeller frame in both runtime modes. `aot_library_path` now derives from `cfg!(debug_assertions)` instead of being hardcoded empty, matching `runner-rs/src/main.rs.tmpl`'s approach on Linux. Asset loading is still an adb-pushed placeholder (`flutter_assets/`/`icudtl.dat`/`app.so` copied into the app's private files dir before launch); real `AAssetManager`-backed loading and a flutter_tools-driven `AndroidRustShellDevice` remain future work. |
| Rust external texture | Engine seam complete | `RustExternalTexture` uses Flutter's existing texture registry and dirty-frame scheduling path. It retains the last good image, honors freeze, retries failed acquisition, imports borrowed wgpu Vulkan image/view handles without taking ownership, and brackets Impeller sampling with producer/consumer semaphores. Context loss, unregister, and repeated teardown are covered by focused tests. |
| Engine-owned wgpu texture | SDK runtime path complete | `WgpuTextureRing` owns three RGBA8 textures, views, and reusable semaphore pairs on the application's shared device. A bounded Tokio channel carries available slot IDs: `try_next_frame` applies immediate backpressure, `next_frame().await` sleeps until Flutter releases a slot, an unpresented reservation returns its slot on drop, and shutdown wakes waiters with `Shutdown`. Ready frames remain queue-serialized through Flutter's acquire callback. The opt-in animated proof now runs through normal `FlutterRustPlugin` registration and public SDK operations. |
| `WgpuTexture` plugin API | Public contract and Linux runtime adapter complete | `flutter-plugin-sdk` exposes validated texture descriptors, `GpuTextures::create_texture`, stable Flutter texture IDs, nonblocking `try_next_frame`, asynchronous `next_frame().await`, single-record frame reservations, consuming `present(self)`, and its pinned API crate at `gpu::wgpu` so plugins do not duplicate the Git dependency. The hidden backend uses `async-trait`; no manual `Future` or `Poll` API leaks into the SDK. The winit factory registers the callback-owning ring, routes dirty notifications and handle-drop unregister through the main thread, and releases each retained ring after C++ confirms raster-thread registry removal. The recording closure receives a device, encoder, and view—but no queue—so plugins cannot violate shared-queue external synchronization. |
| CPU pixel-buffer texture | Linux runtime path complete | `PixelBufferTexture` reserves one of three reusable shell-owned RGBA8 buffers through the same bounded channel/backpressure model. `write_pixels` lends the plugin that allocation and its row stride directly, holding only a per-slot lock. Widths divisible by 64 stage on the producer thread into reusable mapped upload buffers; acquire submits the GPU copy. Other widths use `write_texture` for legacy writes; the opt-in `write_pixels_direct` API eliminates the CPU staging copy at any width using padded upload rows. Slots recycle after both upload remapping and Flutter release. Registration, notification, freeze, release, unregister, and teardown reuse the proven hardware-texture seam. The permanent Dart fixture passes under Vulkan validation with changing pixels. |

## Implementation log

### Pixel-buffer staging and producer concurrency (2026-10-27)

- Pixel writers hold only their slot's storage lock, allowing raster to acquire
  and release other slots while plugins generate pixels. An atomic reservation
  prevents concurrent present/write calls, and unwinding restores reservations.
  Shutdown can detach GPU resources while a CPU writer finishes safely.
- RGBA widths divisible by 64 use three reusable mapped upload buffers. The
  producer stages its pixels before publishing; raster encodes the GPU copy
  with the existing producer/consumer semaphore handoff. Other widths retain
  `write_texture`, preserving the SDK's tightly packed rows. The CPU staging
  copy remains because wgpu's write-only mappings cannot safely back the SDK's
  read/write slice. Additional retained upload memory is three image buffers.
- A shared GPU-context worker polls mapping completion independently of Flutter
  rendering. An atomic handshake recycles each slot only after mapping and
  Flutter release, without taking the ring lock in reentrant wgpu callbacks.
  Context teardown closes and joins the worker before Vulkan destruction.
- Headless Vulkan tests cover aligned/unaligned slot reuse, shutdown during a
  slow producer, and pending remaps. Unit coverage checks both recycle orders,
  a concurrent completion/release race, and reservation recovery on panic.
  All five unit tests, both explicit Vulkan tests, Clippy, and formatting pass.
  Native Vulkan-validation fixtures pass for CPU pixels (19 presentations),
  wgpu (18 presentations), and 40 replacement/resize/unregister rotations
  alternating both producer kinds (475 presentations).
- The opt-in `benchmark_pixel_upload` compares the old `write_texture` path
  against reusable staging on the same ring. On this machine's RTX 5060, with
  wgpu validation disabled, raster acquire p50 fell from 798 to 21 microseconds
  at 1080p and from 3127 to 43 microseconds at 4K. Producer p50 rose from 595 to
  1356 microseconds and 1417 to 4654 microseconds respectively. Total time for
  90 frames was approximately unchanged at 1080p (127/126 ms) and slightly
  higher at 4K (413/427 ms). These debug microbenchmarks demonstrate reduced
  raster latency, not a throughput or end-to-end FPS improvement.

### Direct pixel-buffer writes (2026-10-27)

- Added `PixelBufferTextureFrame::write_pixels_direct`, an opt-in callback
  taking `gpu::wgpu::WriteOnly<'_, [u8]>` and a byte stride. Generators can fill
  the mapped GPU upload buffer directly, eliminating the CPU staging copy.
  The existing `write_pixels` API remains available for read/write slices.
  No unsafe conversion from write-only mappings to mutable slices is used.
- Direct writes support arbitrary widths through padded row strides. Slots
  without an upload buffer allocate one on first direct use. Switching back to
  `write_pixels` preserves its tight layout and stages each row into the padded
  upload. The GPU transfer and CPU shadow buffers remain; copying a preexisting
  image into the direct destination still incurs that producer's copy.
- Updated the native animated pixel fixture to generate into direct uploads.
  GPU readback tests verify every visible pixel at widths 1, 64, 65, and 257,
  including alternating direct and staged frames. SDK tests cover exclusive
  writes and fallback when a backend returns `Unsupported`; Vulkan lifecycle
  tests exercise panic recovery and shutdown during either writer type.
- Validation passes: 12 SDK/broker unit tests, three explicit Vulkan tests,
  Clippy for SDK/broker/winit, formatting, the direct pixel fixture (16
  presentations), and 40 native lifecycle rotations (474 presentations).
- The debug fill microbenchmark on the RTX 5060, with wgpu validation disabled,
  measured staged/direct producer p50 of 1054/762 microseconds at 1080p and
  4046/3047 microseconds at 4K. Total time for 90 frames fell from 103 to 70 ms
  and from 368 to 277 ms respectively. Raster acquire p50 remained small
  (15/9 microseconds). These measurements compare generated fills using the
  two APIs, not application FPS or decoding into an existing CPU buffer.

### Phase 0 exit — controlled baseline and A/A calibration

Five baseline runs and three independent same-build runs completed on DP-1
(74.973 Hz), using the frozen runner at `/tmp/flutter-hal-baseline` and the
controlled benchmark bundle. Evidence is at
`/tmp/flutter-hal-controlled-baseline/run-{1..5}` and
`/tmp/flutter-hal-controlled-aa/run-{1..3}`; the full comparison is
`/tmp/flutter-hal-controlled-comparison.txt`.

| Children | Baseline raster p50 | Raster p95 | Submissions/s/view | Raster p50 noise |
|---|---|---|---|---|
| 8 | 11.91 ms | 19.09 ms | 71.3 | 26.0% |
| 12 | 16.78 ms | 20.31 ms | 58.1 | 7.2% |

A/A reports **REVERT: no primary improvement beyond noise**, with no guard
regressions. At 12 children the second set measures 16.64 ms p50 (-0.8%) and
57.6 submissions/s/view. Baseline raster-thread CPU is 62.71% and fence-waiter
CPU 36.96% at that stage. Counts remain six submits/frame (2 Rust + 4 C++),
one image-view creation/frame and zero steady transient recreations.

A separate profiling run (`/tmp/flutter-hal-controlled-profile`) captured 30
raster stacks: 97% include NVIDIA driver code; 50% include `ioctl` and 37%
`pthread_mutex_lock`. This confirms submission/driver contention still dominates.
The sampler's raster-thread discovery was repaired after the live check exposed
a `pipefail` loop issue. Benchmark evidence now includes an explicit runner PID.
The profiling run is excluded from the acceptance datasets.

Runner SHA-256:
`21de056eede477940623a227289d70b9d0c6e61e6729cfb94b1fedeb6ed48767`.
Runtime source is the ABI-v12 instrumentation build; the later geometry commit
changes only the harness/bundle. Concurrent uncommitted `rust_shell.cc`
diagnostics are outside these commits and are absent from the frozen runner.
The next rendering experiment is HAL semaphore bridges (experiment A in the
design note); no HAL rendering change has been implemented or accepted yet.

### Phase 3e prerequisite — controlled benchmark geometry

Fresh repetitions exposed a measurement condition the old harness only
observed: Hyprland placed some children on 60 Hz HDMI-A-1 and others on 75 Hz
DP-1, and tiling squeezed children down to 1×-6 compositor-reported sizes.
Evidence at `/tmp/flutter-hal-baseline-runs` is unsuitable as a HAL baseline;
the fourth run was interrupted and these datasets must not gate an experiment.

The harness now arranges only its own windows on a selected monitor (`--monitor`,
defaulting to the initially focused output), with a non-overlapping grid of
320×240 children and a 640×480 implicit view. Dart waits for placement to
complete, then warms up for two seconds. Start/end checks reject mixed outputs,
wrong sizes, hidden/tiled windows or changing monitor/geometry, and the
comparator refuses differing scenes across repetitions or builds. No desktop
configuration is changed. Historical tiled-run numbers are not directly
comparable with this controlled workload.

Fourteen accounting tests pass; the controlled five-stage benchmark completes
under forced Vulkan validation without diagnostics
(`/tmp/flutter-hal-fixed-validation`). Fresh baseline calibration follows.

### Phase 3e prerequisite — complete benchmark accounting

The private bridge is ABI v12: C++ reports successful graphics-queue submits,
swapchain image-view creations and transient recreations to Rust's presentation
stats. Total submissions now include the two Rust bridge submits and all C++
submits during that view's acquire-to-present interval. Initial validation
evidence confirms six submits per presented frame (2 Rust + 4 C++), one image
view per frame, and zero transient recreations once view sizes stabilize.

The benchmark supports `--repeat` (default 3), refuses locked or unverifiable
graphical sessions, and records runner/kernel hashes alongside monitor/window
snapshots. The comparator rejects partial runs or missing/nonfinite metrics,
adds raster/fence-waiter CPU guards, reports 8-/12-child submissions/s/view,
and checks native timing guards across all views. The stack sampler now has a
checked repository-owned ptracer exec wrapper and explicit status/PID options.
Profiling remains separate from acceptance measurements.

Validation: build and workspace Rust tests pass; 13 accounting tests pass;
forced-validation lifecycle passes 60 removals/419 presentations; wgpu texture
and CPU pixel-buffer fixtures pass (20/23 presentations). The full five-stage
validation benchmark also completes without diagnostics
(`/tmp/flutter-hal-validation`). Baseline repetition is being established
before experiment A.

### Phase 3e prerequisite — texture harness compatibility

Updated the texture fixture to select Hyprland's Lua window dispatchers when
available, with exact window-address selectors and checked command results.
Legacy dispatch remains available for older compositors. Failed runs preserve
their logs and screenshots. The unchanged renderer, rebuilt from current source,
passes `task test-rust-shell-texture` (18 presentations, changing pixels and
clean exit). This corrects the harness failure behind the inconclusive 3d run.

### Phase 3e — HAL presentation investigation (no runtime change)

Added [the HAL presentation design](flutter-rs-hal-presentation-plan.md), based
on the pinned wgpu source. Public HAL submission owns private WSI semaphore and
fence bookkeeping; bypassing core removes render passes but does not by itself
eliminate semaphore bridge submissions. The design separates those experiments
and specifies retirement, cancellation, queue serialization and Android scope.
The Phase 3d texture failure was previously misinterpreted: captured Hyprland
output showed dispatch syntax rejection, not proof of an already-closed window.
Its rendering result remains inconclusive. The next implementation steps are
harness compatibility and complete submission/CPU instrumentation before a new
baseline and HAL experiment.

### Phase 0 — PlatformView seam

- Added `engine/src/flutter/shell/platform/rust/`.
- Added the standalone GN target
  `//flutter/shell/platform/rust:flutter_rust_shell` and the convenience group
  `//flutter/shell/platform:rust`.
- Added `PlatformViewRust`, an internal C++ `PlatformView` subclass. It keeps
  Flutter C++ inheritance on the engine side and accepts callbacks that the
  future Rust bridge will own.
- Implemented safe default platform-message behavior: messages with no Rust
  handler complete an empty response, matching Flutter's existing platform-view
  behavior.
- Added `flutter_rust_shell_unittests` covering callback forwarding, the
  empty platform-message response fallback.
- Added an edition-2024 Cargo workspace with the private `flutter-shell-core`
  runtime crate and the public `flutter-plugin-sdk` crate.
- Added ABI v1 in Rust and C++ header form, including a GN-linked ABI discovery
  test. Callback ownership remains the next ABI increment.
- Added a GN Cargo action that builds the Rust static library for the private
  ABI test.
- Added the `flutter-shell-winit` Linux runtime crate with a winit-owned window
  and event loop. It is intentionally not connected to Flutter task runners
  until the callback ABI has ownership semantics.
- Added `RustTaskRunner`, a private C++ task runner that gives the host loop
  opaque batons and runs them when winit returns the baton on its main thread.
  It is the basis for sharing the UI and platform task runner without using the
  Embedder task-runner API.
- Extended ABI v1 with a Rust-owned task-runner callback table and opaque C++
  runner handle. The C++ side converts `fml::TimePoint` to a relative delay;
  the Rust host queues the baton against `Instant`, so no clock epoch crosses
  the boundary.
- Added Rust task-runner host state with thread-affinity and destruction
  tracking. Its stable boxed address is the callback `user_data` owned by the
  winit host.
- Bound the Rust host queue to winit's user-event wake and monotonic
  `WaitUntil` timer. Production builds return due batons through the private
  opaque C++ runner handle; standalone Cargo tests use test-only FFI stubs.
- Added a GN action that produces the separate `flutter-shell-winit` static
  archive, keeping its platform system-library link requirements out of the
  core ABI archive.
- Added `CreateRustVulkanContext`, the internal C++ factory that validates
  borrowed Vulkan instance/device/queue handles and creates Impeller
  `ContextVK` with its in-tree `EmbedderData` mechanism. It is not exposed as
  Flutter's public Embedder API.
- Added a pinned `flutter-shell-wgpu` broker crate that owns wgpu's Vulkan
  instance, adapter, device, queue, and winit surface, and exposes raw Vulkan
  values only through a scoped wgpu-hal handoff.
- Left existing platform shells unmodified and unselected. The Rust runner
  selects its own target explicitly.

### Phase 0 — completing the seam

- Implemented real swapchain acquire/present in `GpuBroker`: `acquire_image`
  calls `Surface::get_current_texture`, extracts the raw `VkImage` through
  wgpu-hal, and holds the acquired `SurfaceTexture` until `present_image` hands
  it back to `Queue::present`. `configure` picks a swapchain format from a
  fixed preference list (`Bgra8Unorm`, then `Rgba8Unorm`) rather than the
  surface's first-reported format, because Impeller's Vulkan backend
  (`VkFormatToImpellerFormat`) only recognizes those two formats and silently
  rejects sRGB/other variants at frame-acquire time.
- Added the FFI-safe mirror of `RustVulkanContextData`/`FlutterVulkanImage`/
  presentation callbacks/settings in `flutter-shell-core`, plus C ABI functions
  `FlutterRustShellCreateShell`, `FlutterRustShellRunShell`,
  `FlutterRustShellSetViewportMetrics`, and `FlutterRustShellDestroyShell` in
  `cpp/rust_shell.cc`.
- Wired `flutter-shell-winit`'s `resumed()`/`window_event()` to actually create
  and run the `RustShell`: it extracts Vulkan handles via
  `GpuBroker::with_vulkan_context`, builds the presentation callback table from
  `GpuBroker::presentation_callbacks`, and calls the new shell lifecycle FFI.
- Added `cpp/main.cc` and the `flutter_rust_shell_runner` GN executable: a
  two-line C++ process entry point that calls straight into Rust's
  `FlutterRustShellRun` (in `flutter-shell-winit`), which owns the winit event
  loop for the rest of the process lifetime. See "Who owns the final link" in
  `flutter-rs.md` for why this is a GN-owned executable rather than a Cargo
  `bin` crate, and why that is a phase 0 expedient rather than the intended
  shape of generated apps' `runner-rs/`.
- Linked the Dart VM, Impeller's Vulkan shader bundle, and
  `//flutter/shell/gpu:gpu_surface_vulkan` into the runner; added
  `export_dynamic_symbols` so the Dart VM's "look inside the currently loaded
  process" JIT snapshot resolution can find the statically linked snapshot
  symbols despite the engine's default hidden-visibility build config.
- Set `Settings::application_kernel_asset = "kernel_blob.bin"` for the JIT-only
  phase 0 path, and `Settings::enable_impeller = true` explicitly — this only
  defaults to true on Android/iOS; every other platform (including Linux)
  defaults to false, and leaving it unset makes the Dart-level paragraph/text
  layer build Skia-flavored `DlText` objects that crash the moment anything
  draws text against an Impeller-only surface.
- Added `RustShell::SetViewportMetrics`, called once after the shell starts
  running and again on every winit resize. Without it the engine has no valid
  implicit view to schedule frames for, so `PlatformView::NotifyCreated` alone
  produces a working surface but no frame is ever requested.
- Gave `RustTaskRunner` its own task-observer registry
  (`AddTaskObserver`/`RemoveTaskObserver`), invoked after every `RunTask` call.
  The winit-owned UI thread never installs a real `fml::MessageLoop`, so
  `Settings::task_observer_add/remove` (which `UIDartState` uses to flush the
  root isolate's microtask queue after each task) had nowhere to go; this
  mirrors `fml::MessageLoopImpl::FlushTasks`'s run-task-then-notify-observers
  order.
- Fixed a real identity-confusion bug in `FlutterRustShellRunTask`: it cast its
  `task_runner` argument to `RustTaskRunnerHandle*` (the wrapper returned by
  `FlutterRustShellCreateTaskRunner`), but the value that actually flows
  through `schedule_task` and back is the raw `RustTaskRunner*` (`this`, from
  `PostTaskForTime`) — a different pointer to a different object. The old code
  only appeared to work because the one test exercising it happened to pass
  the handle both ways; rewrote that test (`OwnsOpaqueRunnerHandleForRustHost`)
  to post through the real `fml::TaskRunner` interface so it exercises the
  actual identity contract.
- Fixed a real dangling-pointer bug in `flutter-shell-winit`: the presentation
  callback table's `user_data` was captured as `&gpu_broker` while `gpu_broker`
  was still a local stack variable in `resumed()`, then that local was moved
  into `self.gpu_broker` afterward — invalidating the address C++ holds for the
  shell's entire lifetime. Reordered so the broker moves into its final,
  stable location (a field of `ShellApplication`, which winit never moves once
  `run_app` starts using it by `&mut`) before anything takes its address.
- Verified end to end against a real Flutter counter app (a fork checkout's
  `build/flutter_assets`, JIT `kernel_blob.bin`): the runner boots the Dart VM,
  runs the root isolate, acquires and presents real Vulkan swapchain images
  every frame with no crashes or errors, and Hyprland (the Wayland compositor
  in the dev environment) reports the "Flutter Rust Shell" window `mapped: 1`,
  `visible: 1`, at the requested size. A screenshot could not be captured in
  this session (the desktop session was locked), so this is confirmed by
  process stability plus compositor window state rather than a rendered image.

### Phase 1 — pointer input

- Added a private, value-only pointer event to the Rust/C++ ABI with explicit
  phase, device-kind, and signal-kind translation on the engine side. Invalid
  enum values are dropped instead of being cast into Flutter's internal enums.
- Added winit mouse state tracking for add/remove, hover/drag motion, primary,
  secondary, middle, back, and forward button masks.
- Forwarded wheel input as Flutter scroll signals. Winit line deltas use the
  Linux shell's 53-physical-pixel line unit, and vertical deltas are normalized
  from winit's positive-up convention to Flutter's positive-down convention.
- Forwarded winit touch started/moved/ended/cancelled phases with stable touch
  device identifiers and contact button state.
- Added Rust tests for synthesized mouse entry, hover/drag transitions, button
  state, and scroll normalization, plus C++ tests for private-ABI conversion and
  rejection of unknown enum values.
- Declared directly linked Rust archives as GN inputs so Rust-only changes
  reliably relink the native runner and ABI test executable.

### Phase 1 — window metrics and lifecycle

- Replaced the hardcoded `1.0` viewport device-pixel ratio with winit's real
  window scale factor, including `ScaleFactorChanged` handling.
- Added current-monitor physical size and refresh rate to the private metrics
  call. C++ publishes those through `Shell::OnDisplayUpdates` before updating
  the implicit view's viewport metrics.
- Kept Vulkan surface configuration gated on non-zero physical dimensions and
  used zero-sized resize events to represent a hidden/minimized window.
- Added a Rust lifecycle state machine that combines application activity,
  window visibility, and focus into deduplicated resumed, inactive, hidden,
  paused, and detached transitions.
- Added an explicitly translated private lifecycle enum and forwarded valid
  states through the engine's `flutter/lifecycle` channel. Unknown values are
  ignored rather than cast across the ABI.
- Added Rust lifecycle transition tests and C++ lifecycle enum conversion
  coverage.

### Phase 1 — keyboard input

- Added a value-only private key-event ABI and dispatched validated Flutter
  `KeyDataPacket`s over the engine's `flutter/keydata` channel.
- Translated the common winit `KeyCode` set to Flutter USB HID physical key
  IDs, including left/right modifier identity, and used Flutter logical key
  constants for named and numpad keys.
- Forwarded down, up, repeat, character, and synthesized-event state. Unknown
  XKB keys use the same private GTK key plane convention as Flutter's Linux
  keyboard implementation; unrepresentable native keys are dropped.
- Added Rust translation tests and C++ packet-layout/invalid-input tests. Text
  editing and IME composition remain the next, separate input layer.

### Phase 1 — rendered frame and resize safety

- Added the missing Rust-engine export script and assigned a stable engine ID
  during `RustShell::Run`, allowing Flutter's Linux windowing initialization to
  complete in the standalone Rust shell.
- Added a wgpu acquire barrier/initialization submission before handing a raw
  swapchain image to Impeller. This fixes the previously observed black frame.
- Serialized surface state, deferred resize configuration while a frame is in
  flight, rejected overlapping acquisitions, and recovered once from an
  `Outdated` surface result.
- Verified a rendered Flutter window and a 100-event resize smoke test. The
  remaining fence/invalid-image messages under aggressive resize are tracked as
  incomplete GPU interop synchronization rather than hidden as success.

### Phase 1 — Vulkan synchronization broker

- Bumped the lockstep private shell ABI to v2 and carried broker-owned acquire
  and render-complete Vulkan semaphore handles alongside each borrowed image.
- Made wgpu's acquire/initialization submission signal the acquire semaphore;
  `RustVulkanPresentation` consumes that semaphore on Impeller's graphics queue
  before returning the image to the rasterizer.
- Made Impeller signal the render-complete semaphore after its final image
  layout transition. The broker's final load/store submission waits on that
  semaphore and touches the `SurfaceTexture`, ensuring wgpu's actual present
  semaphore is not signalled before Impeller completes.
- Retained semaphore pairs until the consuming wgpu submission completes,
  bounded normal-operation retirement to three frames, and waited/drained all
  retired pairs before deferred resize reconfiguration. Teardown waits for the
  borrowed Vulkan device to become idle before destroying any remaining pairs.

### Phase 1 — text input and IME

- Bumped the lockstep private shell ABI to v3 and added generic, borrowed-byte
  platform-message callbacks in both directions. C++ owns Flutter's
  `PlatformMessage` objects and response completion; Rust copies and decodes
  messages before the callback returns.
- Installed a queued `flutter/textinput` handler so framework calls cannot
  re-enter mutable winit application state while the merged UI/platform runner
  is executing a Flutter task.
- Decoded the JSON method codec immediately into typed Rust commands, client
  IDs, editing states, affinities, and cursor rectangles. Malformed commands,
  non-finite rectangles, out-of-range offsets, and offsets that split UTF-16
  surrogate pairs are rejected at the boundary.
- Implemented `setClient`, `setEditingState`, `show`, `hide`, `clearClient`,
  caret/marked-text rectangles, and safe no-op handling for current geometry,
  style, selection-rectangle, configuration, and autofill calls.
- Connected show/hide to `Window::set_ime_allowed`, geometry updates to
  `Window::set_ime_cursor_area`, and winit preedit/commit events to a
  UTF-16-aware editing model. Framework updates use the standard
  `TextInputClient.updateEditingState` method call.
- Kept raw key-data delivery separate from committed text, preventing the host
  from inserting the same character through both keyboard and IME paths.
- Added the legacy Linux `flutter/keyevent` compatibility message after every
  modern key-data packet so Flutter dispatches queued `HardwareKeyboard`
  events. Interactive checks confirmed ordinary editing, Backspace, and
  Ctrl+A selection.
- Validated non-Latin preedit and commit through Fcitx 5's Wayland frontend and
  Mozc. `nihongo` produced the composing string `にほんご`; Backspace changed it
  to `にほん`; conversion and commit produced `日本語`; and left-arrow followed
  by Backspace edited the committed value to `日語` without corrupting the
  UTF-16 selection state.

### Phase 1 — compositor-driven vsync

- Bumped the lockstep private shell ABI to v4 and added a typed vsync request
  callback. Rust returns only the frame interval; C++ records frame start in
  the FML monotonic clock domain, so unrelated clock epochs never cross FFI.
- Routed Wayland requests through `Window::request_redraw`. Requests are
  coalesced until winit observes them, while `Window::pre_present_notify` is
  issued at the broker's actual presentation boundary immediately before
  `SurfaceTexture` presentation.
- Fixed a frame-liveness bug in the first implementation: registering the
  Wayland frame callback at every vsync pulse could arm one for Flutter's
  secondary, non-rendering vsync requests. With no following surface commit,
  winit correctly throttled every later redraw, making resize and input appear
  frozen even though their events reached the engine.
- Kept Flutter's timer waiter when the active winit backend is not Wayland or
  cannot provide compositor-aligned redraws.
- Active Vulkan validation exposed two previously hidden hazards. Borrowed
  swapchain images now return to wgpu in `PRESENT_SRC_KHR`, Impeller's incoming
  render-pass dependency includes early depth/stencil writes, and resize
  configurations are coalesced and applied only at a safe acquire boundary.
- Frame-liveness validation then exposed a resize-generation race: a newer
  deferred swapchain size could be paired with depth/stencil attachments from
  an older Flutter layer tree. The acquire callback now treats Flutter's
  requested dimensions as the current frame generation, configures exactly
  that size, and preserves a newer winit resize for the following frame.
- Pinned the complete wgpu workspace to upstream revision
  `014d9e84813a2946febfa4888694c0b70565b2f5` until the fix after 30.0.0 is
  released. That revision stops Linux from passing wgpu's Windows-only reusable
  fence to `vkAcquireNextImageKHR`; pinning the whole workspace keeps its
  internal Rust types coherent.

### Phase 1 — single-engine multi-view seam

- Defined multi-window as Flutter multi-view: view `0` is the implicit main
  window and every additional native window is a positive view ID inside the
  same `Shell`, engine, root isolate, plugin registry, and task runner.
- Bumped the private ABI to v5 and added layout-compatible Rust/C types for
  view IDs, physical metrics, focus state/direction, and asynchronous
  add/remove completion callbacks.
- Routed viewport metrics and pointer packets by view ID and forwarded native
  focus changes through Flutter's `ViewFocusEvent` API.
- Added engine-private `PlatformView::AddView` and `RemoveView` entry points;
  duplicate or invalid presentation registration fails before a Flutter view
  can be left without a render target, and failed Flutter additions roll their
  presentation registration back.
- Added a default-no-op active-view hook to `Surface`, forwarded it through
  `GPUSurfaceVulkanImpeller`, and made `RustVulkanPresentation` select separate
  callback and semaphore state for each view. This preserves all existing
  single-view surfaces while allowing one rasterizer to acquire and present
  the swapchain belonging to the layer tree's view ID.
- Replaced the single winit window with bidirectional view/window maps. Every
  positive view owns a stable `GpuBroker`, native window, pointer state, and
  keyboard state; all brokers share one application-wide wgpu instance,
  adapter, device, and queue.
- Added a typed Dart FFI regular-window contract for creation, destruction,
  state queries, sizing, constraints, titles, activation, maximize/minimize,
  and fullscreen. The standalone runner explicitly exports only that window
  surface plus the VM snapshot symbols needed by `DynamicLibrary.process()`.
- Added `WindowingOwnerRust` and `WindowControllerRust`. The ordinary
  `WindowController` factory selects them only when the Rust-shell symbol is
  present, preserving the GTK Linux owner in existing Linux embedders.
- Routed state, close-requested, and destroyed events through one typed
  `NativeCallable.listener` per engine. Close requests honor the framework
  delegate and retain the native window and presentation surface until
  Flutter's asynchronous `RemoveView` completion succeeds.
- Removed registry borrows from every call into C++ and from native window
  destruction. This permits Dart delegate callbacks, engine view removal, and
  synchronous winit destruction events to re-enter the host without RefCell
  panics.
- Extended the typed platform-message boundary to handle
  `System.exitApplication` and `SystemNavigator.pop`. Required exits are queued
  through winit before the event loop terminates; cancelable exits remain
  conservatively canceled until the shell implements the framework's
  `System.requestAppExit` response round trip.
- Added a typed dialog request alongside the regular-window request. The Dart
  controller accepts only Rust-owned parents, while the host independently
  verifies that the parent is a live view in the same engine.
- Applied native transient relationships with `xdg_toplevel.set_parent` on
  Wayland and `XSetTransientForHint` plus the dialog window type on X11. The
  host retains the parent's native window handle for the child's lifetime and
  removes descendants before their parent, so asynchronous Flutter view
  teardown cannot leave a dangling compositor relationship.
- Migrated the host from winit 0.30 to exact `0.31.0-beta.2`, including the
  beta's object-safe windows, surface lifecycle callback, unified pointer
  events, and untyped wake proxy. Kept a narrowly vendored copy of only
  `winit-wayland` so the Rust shell can add the missing role-aware constructor
  without forking the rest of winit.
- Bumped the private ABI to v6 and added a validated popup request carrying
  kind, parent view, layout constraints, anchor rectangle, parent/child
  anchors, offset, and the six xdg constraint-adjustment flags. Dart enum
  values are checked before becoming Rust enums, and parents must be live
  views in the same engine.
- Implemented real Wayland `xdg_positioner`/`xdg_popup` roles for tooltip and
  popup controllers. Render and input use the popup's actual `wl_surface`;
  interactive popups use the pointer's latest nonzero button serial for
  `xdg_popup.grab`, and compositor `popup_done` requests normal controller
  destruction.
- Carried loose min/max viewport constraints into Flutter instead of making
  child views permanently tight at their bootstrap surface size. This lets
  tooltip and popup widget trees choose their content dimensions, which flow
  through the existing layer-tree-sized Vulkan acquire path.
- Replaced the test-only create/destroy callback implementations with a
  `WindowingCallbackHost` seam. The production extern callbacks and Cargo tests
  now share the same request decoding and dispatch functions; injected-host
  tests use named creation records instead of positional tuple vectors.
- Bumped the private ABI to v7 and added a typed satellite request plus native
  reparent operation. Satellite controllers accept only regular or dialog
  parents, use utility/transient native relationships, participate in the
  existing descendant teardown ordering, and retain absolute position across
  reparenting where the platform exposes it.
- Added shrink-wrap constraints for regular-style views, initial satellite
  positioning on X11, and automatic satellite hiding while its parent is
  maximized or fullscreen. Standard Wayland deliberately leaves persistent
  toplevel placement to the compositor; unlike tooltip/popup surfaces, a
  movable, resizable, keyboard-focusable satellite cannot use an `xdg_popup`
  role.
- Implemented the cancelable desktop shutdown handshake without another ABI
  version increment. The host recognizes `System.initializationComplete`,
  retains asynchronous framework-to-host responses with an opaque one-shot
  handle, invokes `System.requestAppExit`, and completes the original
  `System.exitApplication` call with the framework's `cancel` or `exit`
  response. Native close requests use the same coordinator, duplicate requests
  are coalesced, and required exits still terminate immediately.
- Added an injectable regular-window binding seam to the Rust framework owner
  and automated its production lifecycle dispatcher. Framework tests now cover
  typed creation parameters, synchronous native state, every regular-window
  mutation, delegated close cancellation and acceptance, duplicate close
  coalescing, and the asynchronous transition from a requested removal to a
  host-confirmed destroyed window.

### Phase 1 — main-thread dispatch

- Added `MainThreadDispatcher` to the semantically versioned plugin SDK and
  exposed it through `PluginRegistrar`. Clones are `Send`/`Sync`, retain no
  private engine objects, and accept ordinary `FnOnce + Send + 'static` work
  from Rust workers or background-isolate FFI entry points.
- Routed dispatcher work through a typed winit host event. Dispatch is always
  asynchronous, even from the main thread, so plugin callbacks cannot
  unexpectedly re-enter Dart or mutably borrowed platform state in the middle
  of a synchronous FFI call.
- Bounded host-event draining to 64 callbacks per event-loop turn and re-wake
  winit when work remains. This preserves FIFO order while allowing window
  input, Flutter tasks, and lifecycle events to make progress during a worker
  callback flood.
- Retained the dispatcher and registrar for the shell lifetime and disable all
  dispatcher clones before native window and engine teardown begins.
- Replaced the dispatcher's Boolean acceptance flag with explicit starting,
  running, and shutdown states. Calls made before the implicit view exists fail
  with `NotReady`; startup is one-shot; shutdown is terminal; and every queued
  callback rechecks the shared state before touching plugin or shell data.
- Added `task test-rust-shell-lifecycle`, which launches the real GN runner,
  identifies its mapped Wayland window by PID, requests compositor close,
  records the actual process status through a wrapper, and repeats the complete
  ownership cycle 20 times. Failure cleanup is bounded and force-stops only the
  exact runner PID.
- Added an application-level FRB 2.12 fixture outside the shell crates. Its
  Cargo binary links `libflutter_rust_engine.so`, registers through
  `run_application`, and exports generated FRB symbols from the process. Dart
  makes a synchronous call on the root isolate and a normal call from a named
  background isolate; the Rust worker posts through `MainThreadDispatcher` and
  reports whether its callback ran on winit's owning thread.
- Added a synchronous-reentrancy probe that marks the FFI frame active, queues
  main-thread work, and proves the callback observes the frame already
  unwound. The worker probe also blocks only its FRB worker—not the winit
  thread—while awaiting main-thread completion.
- Extended the same application fixture with one engine-owned wgpu texture and
  one CPU-written pixel-buffer texture. Both IDs cross FRB from the root and
  background isolates and are displayed side by side as real Dart `Texture`
  widgets. Dart starts the Rust producer after its first frame so publication
  cannot fill the three-slot rings before their `TextureLayer`s exist.

### Phase 2 — Rust external texture seam

- Bumped the lockstep private ABI to v8 and added a Vulkan external-texture
  frame descriptor containing borrowed image and image-view handles, dimensions,
  format, and an acquire/render binary-semaphore pair.
- Added engine-generated texture registration, frame notification, and
  idempotent unregister calls. They route through `PlatformView` and Flutter's
  existing `TextureRegistry`, so notifications schedule frames without a Dart
  widget rebuild or a parallel compositor protocol.
- Added `RustExternalTexture`. On the raster thread it waits for the producer,
  wraps the borrowed image/view in an Impeller `TextureVK`, preserves the last
  good frame across failed acquisition and frozen paints, then signals and
  releases replaced frames. Rust remains owner of all Vulkan objects.
- Added focused native tests for notified/frozen rotation, failed-acquire retry,
  context loss, repeated unregister, and post-unregister suppression.
- Added an engine-owned triple-buffered wgpu texture ring. Each slot retains
  its wgpu texture/view and binary semaphore pair, explicitly transitions from
  color-target to shader-resource state, and is not reused until Flutter has
  returned it through the ABI release callback.
- Added an opt-in `FLUTTER_RUST_TEXTURE_DEMO` producer to validate the private
  path before making it public SDK surface. Rendering is deferred to Flutter's
  acquire callback so wgpu and Impeller submissions to their shared `VkQueue`
  remain externally serialized.
- Fixed a Vulkan submit-info lifetime bug found by validation: the wait
  semaphore/stage arrays must remain alive through `vkQueueSubmit`. Also retain
  the texture ring through C++ shell destruction so asynchronous unregister on
  the raster runner completes before its semaphores are destroyed.
- Added the semver-facing `WgpuTexture` SDK contract and hidden runtime backend
  traits. Plugins record commands against an engine-owned target and call
  `present`; only the shell may submit the command buffer or notify Flutter.
  The SDK deliberately does not expose `wgpu::Queue`, preventing plugins from
  racing Impeller on the shared Vulkan queue.
- Refined the contract around explicit frame reservations. Producers may use
  `try_next_frame()` for immediate backpressure or `next_frame().await` without
  blocking a thread. A reserved `WgpuTextureFrame` records at most once and is
  consumed by `present(self)`; presenting before render and recording twice are
  rejected before reaching the runtime backend.
- Implemented those reservation traits on the real `WgpuTextureRing`. Its
  three available slot IDs live in a bounded Tokio `mpsc` channel; releases
  wake asynchronous waiters, nonblocking reservation reports `Busy`, and
  dropping an unpublished frame returns its slot. `async-trait` supplies the
  object-safe hidden async backend method, so the public SDK contains no manual
  polling implementation. `present` queues the recorded closure, dispatches
  Flutter's dirty notification through the main thread, and leaves actual wgpu
  submission in the raster acquire callback to preserve shared-queue ordering.
- Added the Linux `WgpuTextureBackend` factory and install it in the registrar
  once the engine and implicit view exist. Each public handle delegates to a
  stable callback-owning ring; drop shuts down producers and schedules
  idempotent unregister on the main thread. ABI v9 posts a completion behind
  raster registry removal, allowing the host retention list to release each
  callback owner promptly and safely. The animated demo is now a
  source-linked `FlutterRustPlugin` that creates, records, and presents frames
  entirely through the public SDK.
- Added `PixelBufferTexture` as the CPU producer path. Each ring slot owns a
  reusable, fallibly allocated RGBA8 buffer only when the pixel-buffer
  constructor is selected. `write_pixels` invokes the plugin directly against
  that storage with a validated tight row stride; `present` queues it and the
  raster acquire callback performs the single necessary CPU-to-GPU upload on
  the shared wgpu queue. Dropped reservations recycle normally, and no
  plugin-owned `Vec` is copied into a second shell allocation.
- Kept `WgpuTexture` unconditional in the public plugin SDK. Instead of hiding
  core API behind a Cargo feature, removed `flutter-shell-core`'s SDK dependency
  and let the lockstep ABI core own its expected numeric SDK version. The
  existing C++/Rust ABI assertion checks that value while the ABI archive stays
  independent of the full wgpu graph.
- Added deterministic lifecycle coverage for bounded-slot backpressure, slot
  reuse, and shutdown while an asynchronous producer is waiting. Shutdown
  closes the only channel sender, wakes the waiter with `PluginError::Shutdown`,
  prevents new reservations/presents, and leaves already acquired Flutter
  frames alive until their normal release callback.

### Phase 3 — Flutter-tool distribution, CLI device, and release mode

- Added `refreshRustPlugins`/`rust_plugins.dart`: for `rust-shell: true`
  projects, discovers source-linked Rust plugins from the resolved Pub graph,
  symlinks each into `.dart_tool/flutter_rs/plugins/`, rewrites the marked
  dependency block in `runner-rs/Cargo.toml`, regenerates
  `runner-rs/src/flutter_plugins.rs` calling each plugin's declared registrar,
  and validates via `cargo metadata` that exactly one `flutter-plugin-sdk`
  resolves across the whole workspace.
- Added `_ensureRustShellSdk`: materializes `.dart_tool/flutter_rs/sdk/` either
  by symlinking a local engine checkout's crates/engine libraries (fast path
  for engine development) or by downloading and unpacking the BETA SDK
  tarball, and `runner-rs/build.rs.tmpl` links the engine library out of that
  directory rather than requiring a hand-authored path.
- Added `.github/workflows/flutter-rust-beta.yml`: on every push, imports the
  latest upstream Flutter version tag, builds the Rust shell against the
  engine, packages the Cargo workspace and prebuilt engine artifacts into a
  tarball alongside the built `flutter_tools.snapshot`, and republishes them to
  a rolling `BETA` GitHub release so an FVM-installed fork can consume prebuilt
  artifacts without a local engine checkout.
- Added `RustShellDevice`/`RustShellDevices` (`rust_device.dart`): registers
  `rust` as a built-in `DesktopDevice` for `rust-shell: true` projects,
  building the generated Cargo runner and launching it with Flutter's normal
  desktop VM-service environment, hot reload, and shutdown lifecycle in debug
  mode.
- Extended the private engine ABI with an optional `aot_library_path` in
  `FlutterRustShellSettings`/`FlutterRustShellRun`: when the linked engine was
  itself built to run precompiled code (`DartVM::IsRunningPrecompiledCode()`,
  decided at GN `--runtime-mode` build time, not by the Dart app or
  `runner-rs`'s own Cargo profile), `rust_shell.cc` wires that path into
  `Settings.application_library_paths` instead of the JIT
  `kernel_blob.bin` asset. Threaded the same optional path through
  `flutter-shell-core`, `flutter-shell-winit`, `main.cc`, and `rust_runner.h`.
- Made `RustShellDevice` support `BuildMode.release`: `buildForDevice` runs a
  small custom `Target` (`AotElfRelease` plus a copy step) to AOT-compile the
  Dart app to `app.so`, since the tool's generic bundle-builder target does not
  do AOT compilation on its own, and builds `runner-rs` with
  `cargo build --release`. `executablePathForDevice` and
  `launchArgumentsForDevice` select `target/release` and pass the compiled
  `app.so` path to the runner. Fixed a latent bug found while doing this: the
  device previously never passed `icu_data_path` at all and instead forwarded
  `debuggingOptions.dartEntrypointArgs` (which the private ABI has no field
  for and which broke `main.cc`'s positional `argc` check whenever any
  dart-entrypoint args were present); launch arguments are now always
  `<assets_path> <icu_data_path> [app.so]`.
- Made `runner-rs/build.rs.tmpl` link `sdk/lib/<profile>/libflutter_rust_engine.so`,
  keyed off Cargo's own `PROFILE` env var, and extended `_ensureRustShellSdk`
  to maintain separate `sdk/lib/debug/` and `sdk/lib/release/` engine
  directories from either a local engine checkout (`out/host_debug` and
  `out/host_release`) or the downloaded BETA tarball.
- Extended `flutter-rust-beta.yml` to configure and build the engine a second
  time with `--runtime-mode release` into `out/host_release`, and to package
  both engine variants into the BETA tarball under `lib/debug/` and
  `lib/release/`.
- Split `flutter-rust-beta.yml` into three jobs so the debug and release engine
  builds run concurrently instead of sequentially in one job: `build-tool`
  resolves/validates the Flutter version tag and builds `flutter_tools.snapshot`;
  `build-engine` is a `[debug, release]` matrix, each leg doing its own
  checkout/`gclient sync`/`gn`/`ninja` and uploading its engine artifacts;
  `publish` downloads both plus the tool snapshot, packages the tarball, and
  publishes the rolling BETA release as before.
- Fixed two bugs found while actually running a generated application in
  release mode end to end (not just compiling it):
  - The real generated-app entry point is `runner-rs/src/main.rs.tmpl`, which
    is entirely separate from `cpp/main.cc`/`flutter_rust_shell_runner` (the
    phase-0 GN test executable). It hardcodes its own ICU path at compile time
    and never referenced an AOT library path at all, so passing those as
    process arguments from `rust_device.dart` was dead code that the real
    runner never read. Fixed by having `main.rs.tmpl` self-derive both paths:
    `cfg!(debug_assertions)` (true for Cargo's `dev` profile, false for
    `release`) selects `sdk/lib/<debug|release>/icudtl.dat` and, in release,
    the AOT library at `<assets-path>/app.so`. Simplified
    `RustShellDevice.launchArgumentsForDevice` back down to just the assets
    directory, matching what the runner actually reads from `argv`.
  - `_RustAotBundle`'s dependencies were only `AotElfRelease` (compiles
    `app.so`), skipping the normal release asset-copy target
    (`ReleaseCopyFlutterBundle`/`InstallCodeAssets`) that does icon-font
    tree-shaking in sync with the compiled kernel. This left `build/flutter_assets`
    with whatever a prior build (e.g. debug) had left behind, so an icon like
    the counter FAB's `Icons.add` rendered as a mismatched glyph (`å`) instead
    of a plus sign under a real run. Fixed by adding
    `const ReleaseCopyFlutterBundle()` alongside `AotElfRelease` in
    `_RustAotBundle.dependencies`, so both share the same `KernelSnapshot` and
    assets stay consistent with the AOT-compiled code.

### Milestone 3 — Android AOT/release mode

- Fixed `build_rust.py`: it derived `profile_dir` from
  `FLUTTER_RUNTIME_MODE` to decide where to look for the built artifact, but
  never actually passed `--release` to the `cargo build` invocation itself —
  so cargo always produced a debug binary regardless of the requested mode,
  and the script would then fail to find it under `target/release`. This
  affected every platform's Cargo GN action (`flutter_shell_core_rust`,
  `flutter_shell_winit_rust`, `flutter_shell_android_runner_rust`, ...), not
  only Android.
- Replaced the Android runner's hardcoded `aot_library_path: String::new()`
  with a `cfg!(debug_assertions)` derivation mirroring
  `runner-rs/src/main.rs.tmpl`: JIT (empty path) in debug, `{files_dir}/app.so`
  in release, keeping the existing adb-pushed `flutter_assets/`/`icudtl.dat`
  placeholder convention.
- Added a `release` Gradle build type to `android_shell_app/app/build.gradle`
  (debug-signed, since this milestone-scaffold app has no release keystore)
  and disabled `lintVital`, which otherwise blocks `assembleRelease` under
  this environment's JDK independent of anything in this source tree. Left
  `debuggable true` on the release build type deliberately: it only controls
  the manifest attribute that gates `adb run-as` access to the app's private
  files directory (needed to push the placeholder assets/AOT library into a
  non-rooted device), not the Rust cdylib itself, which is always a genuine
  `cargo build --release` artifact regardless of this flag.
- Gated the Android runner crate's body with `#![cfg(target_os = "android")]`
  to match its already-`cfg(target_os = "android")`-gated dependencies in
  `Cargo.toml`. Without it, `cargo test --workspace` on a host Linux checkout
  failed to compile the crate (unresolved `android_logger`/`log`/
  `flutter-shell-winit` imports) — a pre-existing gap since the crate was
  first added, not something introduced by this milestone's other changes,
  but it blocked running the workspace test suite as a regression check.
- Configured `out/android_release_arm64` (`--android --android-cpu arm64
  --runtime-mode release`) and built `flutter_shell_android_runner_rust`,
  `flutter_rust_engine`, and the host-side `clang_x64/gen_snapshot` cross
  tool, plus `dart_sdk`/`frontend_server_aot` in `out/host_release` (needed
  to produce a real AOT snapshot; they were not already built there).

### Validation — Milestone 3

- Ran `cargo +1.93.1 test --workspace --locked` after the `build_rust.py` and
  `cfg` fixes: all crate and documentation tests pass (3 + 28 unit tests
  across `flutter-shell-core`/`flutter-shell-winit`, 0 doc tests, no
  regressions).
- Rebuilt `flutter_shell_android_runner_rust` for `out/android_release_arm64`
  and confirmed cargo actually invokes `--release` now (`Finished \`release\`
  profile [optimized] target(s)` in the build log, where it previously always
  built debug).
- Produced a real release AOT snapshot (`app.so`) and tree-shaken
  `flutter_assets/` for an existing multi-window sample app by invoking
  `gen_snapshot --deterministic --snapshot_kind=app-aot-elf` directly against
  the kernel dill `flutter build bundle --release --target-platform
  android-arm64` produced, using the locally built `out/android_release_arm64`
  engine and `out/host_release`'s frontend server via `--local-engine`.
- Built a release-variant APK signed with the debug key through the release
  build type's signing configuration (`android_shell_app`'s Gradle project,
  JDK from Android Studio's bundled JBR since the system default was too new
  for this Gradle/AGP version) with both
  `libflutter_shell_android_runner.so` and `libflutter_rust_engine.so` staged
  into `jniLibs/arm64-v8a/`, installed it
  on a physical Android device over adb, and adb-pushed `flutter_assets/`,
  `app.so`, and `icudtl.dat` into its private files directory — deliberately
  without a `kernel_blob.bin`, so a successful boot could only mean the AOT
  path engaged, not a silent JIT fallback.
- Launched the app: it rendered real interactive UI (a modal dialog, live
  text field, and soft keyboard from the sample's windowing/IME test screen),
  stayed alive and responsive, and produced no `FATAL`/panic in `logcat` for
  the whole session. A screenshot confirmed the rendered content matched the
  sample app, not a blank or crashed surface.

## Validation

- Created a fresh generated Rust-shell application and ran it through `flutter
  run -d rust`. The tool discovered the built-in device, generated the initial
  Cargo lockfile, built and launched the runner, attached to its VM service,
  completed hot reload and hot restart, and terminated the native process with
  the standard `q` command.
- `git diff --check` passes.
- Added focused Flutter-tool coverage for `rust-shell: true`, strict generated
  Cargo section replacement, and `flutter create --shell=rust`. A real generated
  application with a source-linked Pub/Cargo plugin resolved one shared SDK and
  completed `cargo +1.93.1 build`; the template-manifest check also passes.
- After adding the application registration entry point, ran
  `cargo +1.93.1 test --workspace --locked`: all 34 Rust unit tests and all
  documentation tests pass. Rebuilt the GN runner, shared engine, and native
  test binary; all 17 C++ tests pass. The real Wayland wgpu texture fixture
  also passed through the new registration path with changing frames and a
  clean shutdown.
- Ran `task test-rust-shell-frb-dispatch`. It built the generated Dart and Rust
  FRB 2.12 bindings, linked the Cargo-owned fixture runner to the host-debug
  engine, called Rust from both root and named background isolates, verified
  the FRB worker was not the winit thread, verified both queued callbacks ran
  on the winit thread after synchronous FFI unwound, verified both texture IDs
  from both isolates, waited for both frame counters, compared changing
  compositor captures from each half of the window under Vulkan validation,
  then closed through the compositor and exited with status zero.
- C++ sources were formatted with `clang-format`.
- Built `//flutter/shell/platform/rust:flutter_rust_shell`,
  `//flutter/shell/platform/rust:flutter_rust_shell_unittests`,
  `//flutter/shell/platform/rust:flutter_shell_winit_rust`, and
  `//flutter/shell/platform/rust:flutter_rust_shell_runner` with the
  host-debug GN configuration (`et build`-managed `out/host_debug`).
- Ran `flutter_rust_shell_unittests`: 17 tests passed.
- Rebuilt `flutter_rust_shell_runner`, `flutter_rust_shell_unittests`, and
  `libflutter_rust_engine.so` after installing the public SDK texture factory;
  the final Rust/C++ static link succeeds using the repository's bundled
  depot_tools environment.
- Added and ran `task test-rust-shell-texture`. It builds the permanent
  `examples/texture/lib/rust_shell_main.dart` entry point, launches the real
  Rust runner and source-linked plugin under Vulkan validation when available,
  verifies engine texture ID `1`, waits for continued presentation, compares
  two compositor captures after ten additional frames, scans synchronization
  diagnostics, and closes cleanly. The recorded run passed with 32 presented
  frames and visibly changing texture pixels.
- Added and ran `task test-rust-shell-pixel-buffer-texture`, using the same
  real Dart `Texture` widget, compositor capture comparison, Vulkan diagnostic
  scan, and clean-shutdown checks. The CPU direct-write path passed with 30
  presented frames.
- Ran `cargo +1.93.1 test --workspace --locked`: all crate and documentation
  tests pass, including typed text-input decoding, invalid UTF-16 range
  rejection, Unicode selection replacement, hidden-cursor composition, and
  framework update serialization. The winit crate now has 24 passing tests,
  including monitor-refresh-to-frame-interval conversion, target-view
  preservation for pointer events, and application-exit decoding.
- Ran the focused Vulkan surface test proving that the raster surface forwards
  the selected Flutter view ID to its presentation delegate.
- Compiled the changed Rust Vulkan presentation C++ translation unit and its
  ABI consumers with the host-debug compile commands, then completed a full
  `flutter_rust_shell_runner` host-debug build using the engine's bundled
  depot_tools and the locally managed Python bypass.
- Added `task build-rust-shell` and `task stress-rust-shell`. With the opt-in
  `FLUTTER_RUST_PRESENTATION_STATS` stream, the stress task now proves an
  initial presentation, a new presentation after a compositor-delivered Tab
  key, and a newly sized presentation after 500 Hyprland resizes with periodic
  hide/restore and fullscreen transitions. It then closes immediately to
  overlap teardown with queued work and rejects known Vulkan, wgpu, and
  Impeller synchronization diagnostics. This liveness assertion caught both
  the compositor frame-callback deadlock and the layer-tree/swapchain resize
  race that process-only stress had missed. After installing
  `vulkan-validation-layers` 1.4.350.1-1, the current 500-resize run passed
  with `VK_LAYER_KHRONOS_validation` explicitly enabled and no Vulkan, wgpu,
  or Impeller synchronization diagnostics, including no acquire-fence reuse
  VUIDs. Three consecutive synchronization-and-liveness runs passed.
- Rebuilt both the standalone runner and `libflutter_rust_engine.so`, then
  rebuilt and launched the sample's `runner-rs` target against the then-current
  ABI v4 (multi-view work subsequently advances the lockstep ABI to v5).
  Ordinary typing, Backspace, and Ctrl+A selection work in the visible text
  field.
- Repeated the interactive check with compositor vsync enabled after moving
  `pre_present_notify` to the actual wgpu presentation boundary. Input-driven
  frames and window-size changes remain live; temporarily selecting
  `VsyncWaiterFallback` was used only to isolate the original freeze.
- Ran `task run-flutter` through the app's `runner-rs` Cargo target against a
  real JIT kernel snapshot: the process stays alive, the Rust-shell window is
  mapped and visible, and a live capture shows rendered Flutter content. No
  public Embedder API is involved; the existing GTK Linux shell target remains
  untouched. The synchronization broker and validation-layer stress run above
  supersede the GPU handoff errors observed before explicit semaphores were
  added.
- Built and ran the repository's unmodified `examples/multiple_windows` app
  with windowing enabled. Its initial regular window rendered through the Rust
  shell as Flutter view `1`, including the reference app's controls and window
  registry UI.
- Built a temporary two-controller smoke entry point from the same example.
  Two mapped native windows (views `1` and `2`) rendered simultaneously in one
  process; closing view `1` left view `2` alive, and closing view `2` completed
  both asynchronous removals without a panic. The temporary source was removed
  after validation.
- Closed the unmodified reference app's delegated main window and verified
  that its subsequent required `System.exitApplication` request terminates the
  Rust-shell process without an external signal.
- Built a temporary dialog smoke target with one regular parent, one modal
  dialog, and one modeless dialog. All three rendered concurrently as views in
  one engine. Hyprland treated the Wayland transient as a native floating
  dialog; closing the parent removed the modal child while the modeless dialog
  remained alive. The temporary source was removed after validation.
- Rebuilt and ran the unmodified `examples/multiple_windows` app after the
  winit migration and ABI v6 change. The regular view remained live, and its
  Show Popup control created a compositor-positioned, independently rendered
  child surface whose content size exceeded the one-pixel bootstrap size.
- Re-ran `task stress-rust-shell` after the winit 0.31 migration: keyboard
  frame liveness, 500 compositor resizes, hide/restore, fullscreen changes,
  immediate teardown, and Vulkan validation all passed.
- Built a temporary cancelable-exit smoke target whose first
  `didRequestAppExit` response was `cancel` and whose second was `exit`. The
  first `ServicesBinding.exitApplication(cancelable)` future completed with
  `cancel`; the second terminated the Rust-shell process cleanly with status
  zero. The temporary source was removed after validation.
- Re-ran `task stress-rust-shell` with the native-close path routed through
  `System.requestAppExit`; its default `exit` response still completed the
  500-resize validation run and in-flight teardown cleanly.
- Ran the focused Rust-window framework tests together with the existing
  windowing suite: 98 tests passed. Static analysis of the Rust framework
  backend and its new test reports no issues.
- Ran an interactive Japanese IME check against the real Rust-shell window
  using Fcitx 5/Mozc over Wayland. Preedit, candidate conversion, Backspace
  during composition, commit, cursor movement, and editing after commit all
  remained live and produced the expected character counts and candidates.
  The sample's bundled font rendered Japanese as missing-glyph boxes, so the
  Fcitx candidate UI and controlled edit transitions were used to verify the
  values independently of glyph rendering.
- Ran `cargo +1.93.1 test --workspace --locked`: all 29 crate and documentation
  tests pass. New tests post from a real worker thread, assert execution on the
  recorded main thread, prove nested dispatch is deferred to a later queue
  turn, reject dispatch after shutdown, and cap a 65-callback flood at 64
  callbacks in its first event-loop turn. One hundred repeated lifecycle-unit
  cycles also verify bootstrap rejection, one-shot activation, terminal
  shutdown, and suppression of a callback queued before teardown.
- Rebuilt `flutter_rust_shell_runner` and `libflutter_rust_engine.so` through
  the host-debug GN build after wiring the SDK dependency into the real winit
  host; the final native link passes. A live post-build launch mapped the
  Rust-shell window normally, and immediate process teardown completed without
  a dispatcher or registrar diagnostic.
- Ran `task test-rust-shell-lifecycle`: 20 consecutive real runner processes
  mapped their Wayland windows, handled compositor close through the framework
  exit path, and terminated with status zero. No iteration hung or required
  failure cleanup.
- Rebuilt `flutter_rust_shell_runner`, `libflutter_rust_engine.so`, and the
  native test binary against ABI v8. All 17 native tests and all 29 Rust tests
  pass; strict rustdoc, Cargo formatting, and `git diff --check` pass.
- Built a temporary Dart bundle containing `Texture(textureId: 1)`, then
  restored the test application's source. With `FLUTTER_RUST_TEXTURE_DEMO=1`,
  the real Wayland runner displayed the 256×256 wgpu texture; successive screen
  captures differed as the producer changed color. The final validation run
  emitted no Vulkan or queue-threading diagnostics, and compositor-driven
  unregister/shutdown exited cleanly. No CPU readback was used.
- Added SDK fake-backend coverage; the workspace now has 30 passing Rust tests.
  `flutter-shell-core` now has an empty dependency tree and remains free of
  both the SDK implementation and wgpu.
- Built the runner and shared engine against ABI v9 after adding raster-thread
  unregister completion. The focused wgpu/winit suites pass: 3 wgpu tests and
  24 host tests, including the new blocked-waiter shutdown case.
- Re-ran the real Wayland hardware-texture fixture against ABI v9 under Vulkan
  validation. It produced 30 presentations with changing captures, then
  completed raster unregister, reclaimed the ring, and exited without a Vulkan
  diagnostic.
- Added `task test-rust-shell-texture-lifecycle`. A private fixture handshake
  prevents dirty notification from racing ahead of Dart's replacement
  `TextureLayer`: Dart reports readiness, Rust registers and announces the new
  ID, Dart acknowledges after `endOfFrame`, and only then does Rust publish.
  The real Wayland runner completes 40 live generations with varied dimensions,
  alternating wgpu and CPU pixel-buffer producers, while the harness resizes
  the window every five generations. Each generation is acquired and released
  by Flutter before its predecessor unregisters. The validation-layer run
  produced 468 presentations, reclaimed all retired rings, and exited cleanly.
- Advanced the private bridge to ABI v10 and added a raster-runner lifecycle
  hook that delivers `OnGrContextDestroyed` followed by `OnGrContextCreated` to
  the external-texture registry. The 40-generation real Wayland fixture invokes
  it once at generation 20, blocks new producers until its completion callback,
  then completes the remaining wgpu/pixel-buffer replacements. The validation
  run produced 472 presentations after the injection and exited without a
  Vulkan or teardown diagnostic.
- Removed the crate-wide Linux guard and the transitional `host` wrapper from
  `flutter-shell-winit`. Platform operations now cross a statically dispatched
  `PlatformBackend`, selected as `CurrentPlatform = LinuxPlatform`; the native
  implementation owns all direct Wayland/X11 imports. Legacy Linux/GTK key
  metadata and fallback key-plane policy moved behind the same boundary. The
  private task-runner FFI is similarly selected through `EngineBridge`, with
  its only test `cfg` declarations centralized in `engine.rs`. Shared host code
  contains no production `cfg(test)` branches, and windowing test helpers are
  generic rather than trait objects.
- Added `flutter-shell-winit::run_application` as the application-facing
  registration entry point. It accepts the generated Cargo application's
  registration function after the engine, implicit view, main-thread
  dispatcher, and GPU capability exist, invokes it exactly once, and reports
  `PluginError` through `RunError`. The opt-in texture fixture now supplies its
  `DemoTexturePlugin` through this path rather than registering directly from
  the host's surface-creation callback.
- Rebuilt `flutter_rust_shell_runner` and `libflutter_rust_engine.so` in both
  `out/host_debug` and a freshly configured `out/host_release`
  (`--runtime-mode release`) after the `aot_library_path`/ABI change; the full
  engine closure, including the changed `rust_shell.cc` and `main.cc`
  translation units, compiles and links successfully in both modes.
  `cargo check -p flutter-shell-core -p flutter-shell-winit` passes. `dart
  analyze` and `dart test test/general.shard/rust_device_test.dart` pass for
  the Flutter-tool changes.
- Ran a fresh generated application through `flutter run --release -d rust`
  end to end against the real `out/host_release` engine: the tool AOT-compiled
  the app, built `runner-rs` with `cargo build --release`, and the release
  runner launched, stayed live and interactive for the full test window
  (including responding to input), and exited cleanly with status zero — no
  ICU or AOT-loading crash. This surfaced and led to fixing the two
  `main.rs.tmpl`/`_RustAotBundle` bugs described above (paths the real runner
  never read, and a stale/un-tree-shaken assets directory that rendered the
  counter FAB's `Icons.add` as a mismatched glyph). After both fixes, a clean
  rebuild renders the plus icon correctly.

## Next implementation steps

1. Add profile mode, which needs the same AOT plumbing as release plus an
   `out/host_profile` engine build.
2. Begin Phase 3 with a Windows platform adapter and the existing Vulkan GPU
   path once the generated application workflow is usable without hand-written
   Cargo glue. See [`flutter-rs-windows-plan.md`](flutter-rs-windows-plan.md)
   (Vulkan for both wgpu and Impeller, decided 2026-10-26); its first
   milestone is a no-behavior-change seam refactor on Linux.
3. Generalize Android from a fixed milestone-scaffold app to generated
   applications: a flutter_tools `AndroidRustShellDevice` (matching
   `RustShellDevice`'s role for Linux), per-project `runner-rs` Android
   templates, real Gradle-driven `flutter run -d <android-device>`, and real
   `AAssetManager`-backed asset loading in place of the adb-push placeholder.

## Constraints carried into implementation

- The Rust shell is optional; existing Flutter shells remain buildable.
- Impeller remains Flutter's renderer permanently.
- Keep the private engine integration. Public Flutter Embedder API migration
  and prototyping are deferred until the upstream rendering and synchronization
  prerequisites in the architectural reassessment are met.
- Multi-window means Flutter multi-view: one engine/root isolate per
  application, never one engine per native window.
- Plugin-facing GPU types come from the semantically versioned
  `flutter-plugin-sdk` crate.
- `flutter_rust_shell_runner`'s `main.cc` is a phase 0 expedient (GN owns the
  final link so it can pull in the Dart VM/Impeller/Skia archives); it is not
  the tooling model generated applications will use. See "Who owns the final
  link" in `flutter-rs.md`.

## Tooling moved to `rlutter`

The Rust-shell code that lived in `packages/flutter_tools` (`flutter create --shell=rust`, `flutter build rust`,
`flutter run -d rust`, plugin discovery, the Linux pseudo-device) was replaced by the standalone `rlutter` CLI in
`engine/src/flutter/shell/platform/rust/rlutter`, which delegates the Dart side to the unmodified `flutter` tool. Earlier
entries in this log that mention those tool commands describe the previous implementation. Plan, decisions and what is
and is not verified (notably Android release and anything on a device): `flutter-rs-cli-plan.md`.

## Linux font fallback (emoji) requires fontconfig

Emoji (and any script missing from the app's fonts) rendered as missing-glyph boxes on Linux because the engine was
configured without `--enable-fontconfig`. txt then uses Skia's directory font manager, whose
`onMatchFamilyStyleCharacter` always returns `nullptr`, so there is no per-character fallback at all. Only the local
`host_debug` output had been switched to fontconfig (by the Hebrew task); `host_release` and the BETA CI artifacts that
`rlutter` downloads were not. The BETA workflow now configures with `--enable-fontconfig` (the bullseye sysroot already
ships fontconfig), `task build-rust-shell`/`build-rust-shell-release` flip fontconfig on for their output directories,
and a Linux engine built without it prints a warning at startup. Verified with a probe app rasterizing 😀 through
`Picture.toImage`: release without fontconfig produced 0 colored pixels plus the warning; after enabling it, both debug
and release produced 3084 colored pixels (Noto Color Emoji).

## Standalone rlutter installation

`install.sh` installs the CLI; its first command downloads the framework, Flutter
tool, and full tool Dart SDK from the BETA GitHub release into
`$XDG_CACHE_HOME/rlutter/flutter-BETA` (default `~/.cache/rlutter/flutter-BETA`).
Default SDK discovery no longer uses PATH, FVM, or the CLI's build-time checkout.
`--flutter-root` / `FLUTTER_ROOT` remain explicit development overrides. The
matching engine compilers, full engine Dart SDK, shader tools, fonts tool,
`dart:ui` package and Rust shell sources come from the engine release archives.
The BETA publish job verifies packaged debug and release application builds
with a failing global `flutter` on PATH before uploading the assets. Rust/Cargo
and native platform dependencies are still needed; Android also needs the
Android SDK/NDK and JDK.

Verified locally with a copied release layout and a failing `flutter` on PATH:
project preparation, debug and AOT release builds, `rlutter run`, debugger attach,
hot reload, and clean quit. All 90 CLI unit tests and workflow actionlint pass.
Strict Clippy reports two pre-existing argument-count warnings in unchanged
functions; all other lints pass. CLI publication now follows a successful BETA
build and verifies that the SDK release SHA matches the CLI revision.

## Optimized Rust code for rlutter debug builds

Generated `runner-rs` projects are independent Cargo workspaces. They do not
inherit the engine workspace's optimized dev profile, so `rlutter run` was
compiling the Rust shell and wgpu at optimization level zero. The CLI now
passes the dev-profile optimization settings directly to Cargo for Linux
runners, Android runners, and separately built native plugin libraries. The
application crate uses optimization level 1 and dependencies use level 3. This
also applies to existing projects without rewriting their Cargo manifests.
Debug assertions, debug information, debug artifact paths, the JIT engine,
and Dart hot reload remain enabled. Release builds keep their release profile.

Verification: all 91 CLI tests pass; Clippy passes with only the existing
`too_many_arguments` lint suppressed. A generated Linux debug runner compiles
its dependencies with `-C opt-level=3 -C debug-assertions=on` and stays on the
JIT engine. A local 512-rectangle animation comparison did not show a clear
frame-time improvement (roughly 7–9 ms median raster time before and 8–9 ms
after). The configuration fix alone does not establish the cause of a user's
reported severe debug/release performance gap; that still needs an app
reproduction and the user's platform/workload.

The optimized debug runner also passed debugger attach and a real one-library
hot reload (115 ms), then quit cleanly.

## Repair incomplete rlutter release caches

Standalone SDK preparation now checks for both host engine modes and their
compiler artifacts, rather than accepting Rust sources alone. This repairs
older per-project caches that otherwise fail Android builds with “no
host_debug engine build found”. Source links left by a local checkout also
trigger a fresh release bundle when switching to the managed Flutter SDK,
so sources and engine libraries stay paired. Explicit local checkout
preparation retains its existing behavior. Downloaded archives receive the
same completeness check before use.

Verification: all 93 CLI tests pass, including incomplete-cache repair, host
layout linking after repair, cache reuse, and stale-source-link rejection.
Clippy passes with the existing argument-count lint suppressed. This verifies
the reported preparation failure; an Android device build was not run.

## FlutterActivity Android embedding and plugin packaging

`rlutter build android` now uses `FlutterActivity` and the standard Flutter
Android engine for window, input, lifecycle, Dart VM, and Java/Kotlin plugins.
A small optional `FlutterJNI` attachment hook passes the Rust-owned Vulkan
context to Impeller. The Rust runner still supplies plugin registration and
wgpu/pixel-buffer textures. Existing GameActivity projects are migrated with
backups of the original Gradle and manifest files.

The generated `android/` link exposes `android-rs/` to `flutter pub get` so
Flutter writes `.flutter-plugins-dependencies` and its own Java registrant.
Flutter's Gradle plugin builds the Dart bundle, AOT code, plugin projects and
JNI libraries. `rlutter` stages Rust libraries and selects matching local
embedding/native Maven artifacts. The BETA workflow now builds and packages
those artifacts in debug and release archives and compiles an Android plugin
smoke APK from the standalone release bundle before publishing.

Local verification: the debug and release Android engines and embedding JARs
build; CLI tests and Clippy pass. Generated debug and release APKs include the
JNI plugin libraries (`libdartjni.so` and `libdatastore_shared_counter.so`),
standard `libflutter.so`, the Rust runner, and mode-appropriate Dart kernel or
AOT code. A second app built both modes from copied host/Android engine bundles
with a nonexistent engine source path, exercising the release asset layout.
The original GameActivity Gradle templates also migrated in place and built a
debug APK with the Java plugins.
This exposed and fixed missing debug snapshot files in the Android archive.

On an Armor 21 running Android 16, debug and release APKs both launched with
`FlutterActivity`, read and wrote `SharedPreferencesAsync` through Dart JNI,
resolved `path_provider`, and displayed a Rust pixel-buffer texture. Returning
from the launcher and recreating the Activity also passed. Device testing
found and fixed the old GameActivity `android_main` link reference, Android's
duplicate `JNI_OnLoad` lookup through `libflutter.so`, and an invalid cast of
Impeller's Android surface context while painting external textures. Release
testing found that R8 removed JNI-referenced plugin methods, so generated and
migrated Android projects now disable release code/resource shrinking until
JNI-aware keep rules can be generated.

The Android runner no longer links winit's GameActivity glue. Its winit
dependency is Linux-only, including for existing runners migrated during
`rlutter build android`; wgpu shares its Vulkan device setup through a
statically dispatched surface-source trait. FlutterActivity's Android path
supplies no winit surface, and the obsolete `android_main` shim is gone. A
debug and release smoke APK build passed after this change; the Android Cargo
dependency tree contains neither winit nor android-activity. The obsolete GN
GameActivity runner target has also been removed.

On the connected Armor 21, the `ui-flutter` debug APK built from the local
engine and reached its login screen. Device testing exposed two later crashes:
the location foreground service was launched before runtime permission, and
the plugin's persisted boot/watchdog restart could create a Vulkan Flutter
engine before the Rust activity. The app now starts tracking only with `always`
location permission, disables the plugin's automatic boot/watchdog receivers,
and explicitly starts tracking after permission is granted. A subsequent
launch stayed alive beyond the foreground-service timeout without a fatal
exception or invalid Vulkan surface. The no-permission path was verified;
location tracking after permission grant and media playback remain untested.
Standard secondary engines created after a Rust runtime select Impeller
OpenGLES to avoid replacing Vulkan-Hpp's process-wide default dispatcher.
Vulkan validation also reported a depth-buffer write synchronization warning
on an earlier run; its impact has not been established.

## Keyboard state sync (stuck modifiers, numpad scrambling)

Two Linux symptoms shared one cause: Flutter's `HardwareKeyboard` only knows
what the embedder tells it, and the Rust host forwarded raw winit events
without reconciling state the way the GTK, Windows, macOS, Android, and web
embedders do.

- **Backspace, Delete, Ctrl+A silently dead.** A modifier released while the
  window lacked focus (typically Super/Alt consumed by a Hyprland binding)
  never produced an Up, so Flutter kept it "pressed". Text shortcuts are
  exact-match `SingleActivator`s, so a phantom Meta/Alt disabled them, while
  plain typing kept working because the host commits text itself.
- **Numpad digits scrambling after a few presses.** Flutter learns NumLock
  only from NumLock key-downs, so if it was already on at launch Flutter
  assumed it off. Linux's default shortcuts then bind unshifted
  `numpad1`-`numpad9` to caret/page movement, and each key both inserted its
  digit (host-committed text) and moved the caret.

`KeyboardState` (`shell.rs`) now keeps a physical→logical map of keys Flutter
saw go down and sends synthesized (`synthesized = 1`) packets ahead of the
real one: Ups for modifiers the winit mask no longer holds (on
`ModifiersChanged`), Downs for modifiers held but never seen, and Ups for
everything still down on `Focused(false)`. A late real Up for a key already
released this way is dropped, a repeated Down closes the first, and an Up
reuses its Down's logical key. The modifier owning the current event is
skipped because winit may deliver the mask and the key in either order.
Right Alt is not reconciled: as AltGr (Hebrew, German, ...) xkb reports it as
level 3, not `ALT`, so the mask cannot say whether it is held.

**NumLock is inferred, not read.** winit's `ModifiersState` has no lock bits.
xkb has already applied NumLock when a numpad key arrives, so an unshifted
numpad key that produced a character (digit or `.`) means NumLock is on and
one that produced a navigation key means it is off. Shifted numpad keys are
ignored: current xkeyboard-config's `KEYPAD` type ignores Shift (verified
against this machine's compiled keymap), whereas older keymaps use Shift to
flip the result, so Shift carries no reliable information. When the inferred
state disagrees with the state Flutter believes, the host sends a synthesized
NumLock down+up before the key, and it flips its belief on real NumLock
presses exactly as `HardwareKeyboard` does. Gated by
`PlatformBackend::INFERS_NUM_LOCK` (on for Linux, off for Android).

Why inference over reading xkb's `mods_locked`: it needs no platform code, so
it works unchanged on Wayland and stock `winit-x11` (not vendored here) and on
future Windows/macOS adapters, and it learns the state at the only moment it
matters, a numpad press, before shortcut matching. Known limits: Flutter's
`lockModesEnabled` can be stale until the first numpad press; a layout using
`numpad:mac` (always digits) is inferred as NumLock-on;
CapsLock/ScrollLock are not synced. The exact route, if these ever matter, is
keeping `num_lock`/`caps_lock` from `winit-common`'s xkb `ModifiersState`
(dropped at the conversion to the public type) in the vendored
`winit-wayland` and a `XkbGetNamedIndicator` query for X11, behind a new
`PlatformBackend` method, with inference as the fallback.

State is per view while Flutter's keyboard is global; this is consistent
because focus changes are sequential (the old view's `Focused(false)` releases
its keys before the new view reports its held modifiers). Verified with unit
tests only (`cargo test -p flutter-shell-winit`); not yet confirmed
interactively on a Hyprland session.

## Android VM service port forwarding

`rlutter run -d android` now passes the device VM service URI directly to
`flutter attach --debug-url`. Flutter's Android attach path creates the adb
forward itself. The extra forward previously created by rlutter caused attach
to interpret an allocated host port as the device port and wait indefinitely.
On the connected Armor 21 the original process had a live VM service despite
the Vulkan depth/stencil synchronization diagnostics; a direct `getVM` request
through the first forward succeeded, while attach had created a second forward
to the wrong device port. Removed rlutter's redundant forwarding and URI rewrite
helpers. Linux continues to pass its locally reachable service URI to attach.

Verification: all 99 remaining CLI unit tests, formatting, diff checks, and
Clippy (with the existing `too_many_arguments` lint suppressed) pass. Installed
the corrected CLI locally. A fresh generated Android Rust-shell probe app built
and ran on the Armor 21 using the local debug engine. Attach received the raw
device URI, created one forward to that device port, and exposed DevTools.
Changing the Dart title hot-reloaded one library in 751 ms; hot restart
completed in 3,413 ms; quitting exited with status zero and stopped the app.
The temporary APK was then uninstalled. A rebuild of the user's `ui-flutter`
app was blocked by its package configuration switching to the FVM beta
framework, incompatible with the local engine's `SemanticsRole.listBox`;
the completed full run/reload/restart verification used the isolated probe.
The Vulkan synchronization diagnostic is not fixed by this change.

## Incremental runner bundle staging

Linux and Android rlutter builds now preserve unchanged staged files instead
of deleting and copying the bundle on every invocation. Content comparisons
use bounded buffers for large engine libraries. Linux mirrors the Dart assets
and prunes removed native plugins; Android prunes removed libraries and ABIs.
The intermediate native-plugin staging directory also retains unchanged files.
Cargo still checks native inputs on every build, so native changes remain
tracked independently of Dart changes.

Verification: all 100 CLI unit tests pass, including preserved timestamps,
same-size content updates, and removed assets/plugins/ABIs. Formatting and
Clippy pass (existing too_many_arguments lint suppressed). End-to-end build
time improvements have not yet been measured.

## Bundled Dart and LSP shortcuts

`rlutter dart <args...>` now runs the selected Flutter SDK's Dart CLI directly.
`rlutter lsp <args...>` starts its Dart analysis server with
`language-server --protocol=lsp` for editor integration. Both replace the CLI
process, preserving stdio, exit status and signals; SDK preparation diagnostics
stay on stderr. Explicit SDK overrides work as before. Usage is documented in
the Rust-shell application skill.

## VS Code F5 launch and automatic debugger attachment

`rlutter vscode` generates a separate `rlutter.code-workspace` containing a
Linux background run task, SDK selection, and a Dart debugger attach profile.
F5 builds and launches the application, then attaches using the VM service
info file written by `rlutter run --vm-service-info-file FILE`. Existing editor
configuration is not overwritten. Hot reload/restart stay in the task terminal;
debugger Stop detaches, while terminal `q` stops the application. Android and
native Rust debugging are outside this generated configuration.

Verification: all 104 CLI tests, formatting, Clippy (existing argument-count
lint suppressed), skill validation and diff checks pass. Workspace generation
and refusal to overwrite existing files were exercised through the built CLI.
A real Linux Rust-shell app was launched with the new service-info option;
a Dart debug-adapter client attached through the file, hit a breakpoint after
hot restart, inspected the stack, resumed, and disconnected. Publishing
Flutter attach's DDS URI, rather than the raw engine URI, is required for
source-path resolution. The VS Code GUI itself was not exercised.

## Linux profile mode

`rlutter build linux --profile` and `rlutter run -d linux --profile` now
compile Dart with `aot_snapshot_profile`, link `host_profile`, and build an
optimized Rust runner with debug information. Profile bundles live under
`runner-rs/target/flutter-profile/`, separate from release. Run attaches
Flutter with `--profile` for DevTools; hot reload/restart remain debug-only.
Existing generated runners are refreshed automatically. Android profile mode
is explicitly rejected.

For local engines, first configure from `engine/src` with
`flutter/tools/gn --runtime-mode profile --no-goma --enable-fontconfig`, then
run `task build-rust-shell-profile` from the repository root. The BETA workflow
now builds, packages, and smoke-builds Linux profile artifacts; managed SDKs
require that updated bundle to be published.

Verification: all 106 rlutter unit tests, formatting, and Clippy pass (with the
existing too_many_arguments lint suppressed). No host_profile engine exists
in the local checkout, so a real profile build, launch and DevTools session
have not been verified.

The `build-rust-shell-profile` task now configures `host_profile` automatically
when `args.gn` is absent, with fontconfig enabled. Existing configurations are
preserved. Verified from a missing output directory: GN generated the build
successfully and Ninja started compiling the profile engine.


## Windows engine build preparation (2026-11-08)

W0 is started, but the engine DLL has not been built. GN now selects MSVC
Rust target triples and `.lib` archives on Windows, and omits the GN-owned
standalone runner there so Cargo can own the executable link. Unix `dl` and
`pthread` libraries are Linux-only. The build helper stages `.lib`/`.dll`
outputs using the target platform, including cross-builds. Existing C ABI
functions, including window exports, already have `dllexport` declarations.

The Windows GN entry point now passes its Python interpreter explicitly:
`.gn` hardcodes the Unix hermetic `bin/python3` layout, which cannot execute
on this host. The pinned GN binary was installed locally from DEPS.

Source inspection corrected the Windows plan: Windows JIT snapshots already
use static symbol references in `runtime/dart_snapshot.cc`; new snapshot
callbacks and changes to Linux snapshot exports are unnecessary.

Verification: the Python build-helper tests pass (target naming and staging
in debug/profile/release), GN formatting and diff checks pass. A real minimal
Cargo crate built with Rust 1.93.1 for `x86_64-pc-windows-msvc`; the helper
staged its static library and DLL, and loading the DLL and calling its export
returned the expected value. This verifies build-helper plumbing only.

Engine generation stops in Windows toolchain setup: the engine requires
Windows SDK `10.0.22621.0`, but this host has `10.0.26100.0`. Visual Studio
Build Tools 2022 is installed and requires an explicit path for this version
of the engine's detection script. The pinned engine Clang package is also
absent. Install/sync these prerequisites before resuming from PowerShell:

```powershell
$env:DEPOT_TOOLS_WIN_TOOLCHAIN = '0'
$env:GYP_MSVS_VERSION = '2022'
$env:GYP_MSVS_OVERRIDE_PATH = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools'
python engine/src/flutter/tools/gn --runtime-mode debug --no-goma --no-prebuilt-dart-sdk
ninja -C engine/src/out/host_debug flutter_rust_engine.dll
```

Full GN generation, C++ compilation/linking, engine exports, and C++ tests
remain unverified. The Windows winit adapter, GPU cfg changes, first frame,
rlutter support, packaging, and runtime validation remain pending.


### Windows build prerequisites installed

Installed Windows SDK 10.0.22621.0 and its Debugging Tools for Windows, plus
the DEPS-pinned Windows Clang package. With the Build Tools override and
`--no-prebuilt-dart-sdk` command above, GN generation succeeded and Ninja
started compiling `flutter_rust_engine.dll`. The engine build must still
complete before the DLL or runtime can be considered verified.


### Windows W0 engine DLL verified (2026-11-08)

Resumed the incremental Windows x64 debug/JIT build with the documented
Visual Studio environment. All 2,739 remaining Ninja steps completed, including
the private C++ bridge and DLL link. No engine compiler or linker fix was
needed beyond the existing W0 preparation. Clang emitted ignored
`-fno-omit-frame-pointer` warnings during Dart compilation.

Artifacts in `engine/src/out/host_debug`:

- `flutter_rust_engine.dll`: 36,725,760 bytes.
- `flutter_rust_engine.dll.lib`: 41,230 bytes; use this import library for Cargo.
- `flutter_rust_engine.dll.pdb`: debug symbols.

`llvm-readobj.exe --coff-exports --coff-imports` confirmed the x64 PE format,
Windows system DLL imports, and 39 private `FlutterRustShell*` exports, including
15 window functions. Python ctypes successfully loaded the DLL and resolved
every private ABI export. Calling `FlutterRustShellRunTask(nullptr, 0)` returned
zero as expected. No separate runtime DLL had to be staged for this host load.
This verifies DLL loading and symbol lookup, not Dart/Vulkan initialization.
Windows JIT retains the existing statically linked snapshot path.

The C++ test executable initially failed to link because Flutter's testing
support references `flutter::testing::GetFixturesPath`. Added the standard
`//flutter/testing:testing_fixtures` dependency to the Rust shell test target.
Rebuilding and running `flutter_rust_shell_unittests.exe` then passed all 17
tests across platform-view ABI forwarding, external textures, and task runners.
The Rust core static archive built with the pinned 1.93.1 toolchain and linked
without additional Rust system-library flags. The two focused Python
build-helper tests also pass.

Commands (after setting the environment documented above):

```powershell
ninja -C engine/src/out/host_debug flutter_rust_engine.dll
ninja -C engine/src/out/host_debug flutter_rust_shell_unittests
engine/src/out/host_debug/flutter_rust_shell_unittests.exe
python engine/src/flutter/shell/platform/rust/build_rust_test.py
engine/src/flutter/buildtools/windows-x64/clang/bin/llvm-readobj.exe --coff-exports --coff-imports engine/src/out/host_debug/flutter_rust_engine.dll
```

Local build logs are `engine/src/out/host_debug/rust-windows-build.log` and
`rust-windows-tests-build.log`; the PE inspection is `rust-windows-pe.txt`.
These generated outputs are not checked in. W1 (Windows Rust Vulkan cfgs and
dependencies) is next. The Windows host, Cargo smoke runner, first frame,
resize/input/validation checks, and Linux/Android regression builds remain
unverified.


### Windows W1 Vulkan broker and Intel descriptor-pool fix (2026-11-08)

Enabled Windows dependencies in the GPU broker, shared queue, and winit host
manifests. Desktop surface and presentation cfgs now include Linux and Windows;
shared Vulkan texture/queue code includes Android as before. Vulkan-only
backend selection, surface-compatible adapter selection, FIFO presentation,
and the existing queue mutex/semaphore ownership are preserved. Failure to
find a compatible adapter now explains the Vulkan driver requirement.

Pinned Rust 1.93.1 Windows checks pass for `flutter-shell-wgpu` and
`flutter-gpu-queue`. Five GPU unit tests and the raw-queue-access integration
test pass (four hardware/benchmark tests are ignored in the ordinary run).
The three non-benchmark hardware tests were also run explicitly under
`VK_LAYER_KHRONOS_validation`, `WGPU_VALIDATION=1`, and `WGPU_DEBUG=1`.

Hardware validation exposed an access violation during device creation on
Intel HD Graphics 530 (PCI 8086:1912, Windows driver 31.0.101.2111, Vulkan
1.3.215). CDB located it inside `igvk64.dll` during `vkCreateDescriptorPool`
for wgpu's internal empty bind-group layout. An independent Vulkan-only
ctypes reproduction crashed with `poolSizeCount=0` and succeeded with one
unused sampler descriptor. This occurs before shell upload/presentation.

Vendored `third_party/wgpu-hal` from the exact workspace revision
`014d9e84813a2946febfa4888694c0b70565b2f5`, retaining its licenses and
normalizing Cargo workspace inheritance. The only source change adds one
unused sampler descriptor to otherwise empty pools. Workspace and fixture
Git-source patches retain all other wgpu crates at the same revision.
See `third_party/wgpu-hal/README.flutter` for provenance and the local delta.
With this fix, direct padded-row upload/readback, aligned/unaligned slot reuse,
and slow-writer/shutdown tests pass on this GPU under driver validation.
The slot-reuse test intentionally catches producer panics as part of its
reservation-recovery coverage; those expected panic messages are not failures.

Commands from `engine/src/flutter/shell/platform/rust`:

```powershell
cargo +1.93.1 check --locked --target x86_64-pc-windows-msvc -p flutter-shell-wgpu -p flutter-gpu-queue
cargo +1.93.1 test --locked --target x86_64-pc-windows-msvc -p flutter-shell-wgpu -p flutter-gpu-queue
$env:VK_INSTANCE_LAYERS = 'VK_LAYER_KHRONOS_validation'
$env:WGPU_VALIDATION = '1'
$env:WGPU_DEBUG = '1'
cargo +1.93.1 test --locked --target x86_64-pc-windows-msvc -p flutter-shell-wgpu direct_pixel_upload_preserves_padded_rows_and_can_mix_with_staged_writes -- --ignored --nocapture
cargo +1.93.1 test --locked --target x86_64-pc-windows-msvc -p flutter-shell-wgpu pixel_upload_reuses_aligned_and_unaligned_slots -- --ignored --nocapture
cargo +1.93.1 test --locked --target x86_64-pc-windows-msvc -p flutter-shell-wgpu slow_pixel_writer_allows_other_slots_and_shutdown -- --ignored --nocapture
```

This validates device creation and texture transfers, not a Win32 swapchain or
Flutter frame. Linux/Android builds and device regressions have not been run
on this Windows host. The minimal Windows host is the next checkpoint.


### Windows W2 subset and portable validation harness (2026-11-08)

The Windows platform adapter now compiles with the pinned Rust 1.93.1 toolchain.
Ordinary windows use the existing merged winit platform/UI runner, timer
pacing, pointer/metrics/focus/close handling, and shared Vulkan synchronization.
Other window kinds return explicit unsupported errors for this first-build
subset. Desktop clipboard handling is shared with Linux. Windows legacy key
JSON uses native virtual keys/scans, Windows modifier bits and character code
points; AltGr text is distinguished from Ctrl+LeftAlt shortcuts. Native lock
state is read with GetKeyState and reconciled before key packets, including
avoiding a double toggle on the lock key's own down event.

All 47 Windows host unit tests pass, including Windows key-message encoding,
lock reconciliation and AltGr. MSVC retains C ABI references in the Rust test
executable, so the harness explicitly links the matching engine import library.
All 17 C++ shell tests and both Python build-helper tests pass through the
portable task runner. The benchmark accounting suite passes all 14 tests.
Basic native desktop tests create a temporary Tk window and verify PID-scoped
lookup, resizing, minimize/restore, deterministic capture and clean close.
Modal drag is a separate test and reports a skip when foreground input is
unavailable; the real stress task treats unavailable input as a failure.

`Taskfile.yml` now delegates its shell/build/test orchestration to
`tools/rust_shell/tasks.py`. Lifecycle and GPU/input/resize stress loops are
Python, with process exit checks and retained evidence instead of Bash, jq,
pgrep and status-file wrappers. Native desktop and benchmark operations have
Hyprland and Win32 adapters. Texture and FRB pixel checks use the same adapters;
Win32 captures reject occlusion so overlapping windows cannot produce a false
pixel-change pass. Host-independent benchmark accounting remains shared.
See `tools/rust_shell/README.md` for commands, overrides and coverage limits.
No Linux/Android runtime regression coverage was available on this host.

The portable Windows build also completed the matching Dart SDK and frontend
server targets. Dart's SDK helper initially found the Windows Store python3
alias; the runner now generates a build-local python3.cmd pointing at its own
Python interpreter, without changing machine configuration. The root Flutter
CLI bootstrapped successfully. Task itself was tested using a local v3.40.1
binary in the ignored GN output tree.

### Cargo smoke runner and first-frame investigation (2026-11-08)

Added a small Cargo fixture in `tests/smoke_runner` and a counter/text-field
Flutter app in `tests/smoke_fixture`. The runner uses the same C entry point as
the Linux GN executable, including presentation-statistics and texture-fixture
environment switches. The Windows bundle in `out/host_debug/rust-smoke` holds
the executable, engine DLL and ICU data. Its import-library link and checked-in
lockfile use the matching engine and pinned workspace dependency revisions.
The FRB fixture's build helper also selects the Windows import library instead
of Unix rpath/export-dynamic arguments.

`python tools/rust_shell/tasks.py lifecycle --lifecycle-iterations 1` built the
Dart bundle, started a VM service, recorded two 800x600 presentations and closed
with status zero under Khronos validation. This proves startup/presentation/
close, not correct rendered output. The first desktop capture was obscured by
the Windows Start menu, which also prevented foreground input from the test
process; running outside the sandbox did not remove that foreground restriction.
The API now detects occluded captures, and the real stress task refuses to
claim input or modal-resize success when it cannot obtain foreground access.

An independent LunarG screenshot-layer run captured the application's own
swapchain (`out/host_debug/rust-swapchain-capture/0.ppm` and `1.ppm`, with a PNG
conversion for inspection). It shows genuine corrupted text/colors, independent
of desktop occlusion. This rendering failure remains unresolved. That diagnostic
run also exceeded its close timeout and was terminated; it is not a shutdown
pass. The normal validation-only lifecycle run did close cleanly. No Vulkan
validation VUID accompanied the corrupted frame in the retained runner logs.

**The readable, interactive first-frame milestone is not complete.** Investigate
the Impeller/shared-device rendering corruption before claiming W3/first-frame
success. Keyboard/live-resize automation also needs an unobstructed interactive
desktop. Full window-kind parity, DWM pacing, rlutter Windows support, release
packaging and CI remain pending.

Git author identity was configured from the authenticated GitHub account on
2026-11-09, allowing the verified checkpoints to be committed separately.

### Windows frame diagnostics and window enumeration (2026-11-09)

The Win32 desktop adapter now excludes visible zero-sized event-loop helper
windows from application enumeration. A focused regression test covers the
filter. Closing only the main window exits with status zero under both ordinary
Vulkan validation and the screenshot layer; the earlier diagnostic timeout
resulted from also closing the helper HWND, not demonstrated renderer teardown
failure. Native desktop tests: seven passed, one foreground/modal-input skip.

A six-color rectangle/text probe reproduces the rendering corruption. Win32
`PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT)` independently reproduces the
same pixels, including with the Vulkan screenshot layer disabled. CPU uniform
values are correct at descriptor binding; uniform alignment, shader precision,
single-sample rendering and cached host memory experiments did not repair the
frame. These experimental renderer changes were not retained. The readable,
interactive first-frame milestone remains incomplete.

Retained `smoke_fixture/lib/render_probe.dart` with documented coordinates and
expected colors. The hardware GPU task now also reads back six uniform-driven
BGRA color bands after the broker-style empty load/store render pass. All four
hardware upload/readback/shutdown tests pass under Vulkan validation on the
Intel adapter. This isolated test covers the wgpu pass and buffer offsets; it
does not prove correctness of the external Impeller rendering or semaphore
handoff. A dedicated Impeller buffer-allocation experiment also left the
corruption unchanged and was reverted. The engine and runner were rebuilt with
all exploratory renderer/HAL edits removed.

The restored counter bundle starts and closes with status zero under Vulkan
validation, with the same corrupted pixels. The full lifecycle command correctly
fails its occlusion guard on this desktop; evidence is retained in
`%TEMP%/flutter-rust-lifecycle-1qyegb7a`. This is not a lifecycle-suite pass.
Foreground input and live-resize checks still require an unobstructed desktop.

### Intel HD 530 swapchain rendering workaround (2026-11-09)

Isolated the corruption before the wgpu handoff: an Impeller-owned image reads
back the expected background and all six primary/secondary color bars, whereas
rendering the same scene into a borrowed Win32 swapchain image already contains
incorrect pixels before presentation. Changing the render-pass final layout or
adding transfer usage alone did not repair direct rendering. Rendering into an
owned image and copying it into the swapchain does produce correct colors and
readable text. The underlying Intel driver defect has not been identified.

Added a conservative workaround only for Windows Intel vendor `8086`, device
`1912` (the tested HD Graphics 530). The shell caches one owned resolve target
per view/size and copies its pixels on the existing shared Vulkan queue before
the established presentation handoff. Both textures remain tracked until GPU
completion; transfer/read/write barriers order reuse. There is no CPU readback
or queue-idle wait in this path. The broker requests `COPY_DST` only for this
device and reports a clear error if the surface cannot provide it. The cost is
one additional single-sample image per view and one GPU copy per frame on this
device. Other adapters retain direct rendering.

The six-color/text probe now displays correct pixels with the production
workaround (no experimental readback code). Forty programmatic resizes followed
by minimize/restore and native close retained the correct colors and exited
zero with synchronization validation enabled; no VUID or synchronization hazard
was logged. Evidence: `out/host_debug/render-probe-scoped-resize/`.
This does not validate mouse-driven modal resizing, focus, or keyboard input.
Linux/Android and other Windows GPU/driver coverage remain unavailable.

`python tools/rust_shell/tasks.py test` passed: 47 host tests, 17 C++ tests,
core/queue checks, five ordinary GPU tests, two build-helper tests, 14 benchmark
accounting tests and all eight desktop tests. Foreground access became available
in this run. The real 40-resize stress command passed foreground keyboard redraw
and programmatic resizing, then failed its live-resize check with `Win32 modal
live resize starved Flutter presentations`. Evidence:
`%TEMP%/flutter-rust-stress-4bdwrcvj`. This is a newly exercised failure, not a
stress-suite pass; fixing modal-loop scheduling is the next first-frame task.

### Ordinary Windows first-frame milestone verified (2026-11-09)

Fixed Win32 modal-loop starvation without adding a UI thread. Windows now arms
a one-shot HWND timer for the next Flutter task/vsync deadline; its native
callback kills the timer and posts `WM_PAINT`, retaining no Rust pointer. The
redraw/proxy handlers service Flutter work and rearm the next deadline even
while Win32 owns the move/resize loop. Native window destruction removes its
timers. Idle windows do not acquire a periodic polling timer.

The Python-backed stress task passes both 40 and 500 iterations, including
foreground keyboard-triggered redraw, resize/maximize/minimize/restore,
mouse-driven modal live resize with presentations advancing during dragging,
and exit status zero under Khronos Vulkan validation. Evidence:
`%TEMP%/flutter-rust-stress-vxwvl0oa` and
`%TEMP%/flutter-rust-stress-co81_g0m`. The full ordinary check suite also passes:
47 host tests, 17 C++ tests, core/queue and ordinary GPU checks, two Python
build-helper tests, 14 accounting tests and all eight native desktop tests.

A separate real mouse/keyboard run clicked Increment and entered `hello123`
in the text field. Its client capture visibly shows `Count: 1` and `hello123`
with readable text, followed by status-zero close and no Vulkan VUIDs. Evidence:
`out/host_debug/counter-input-validated/window.png` and `runner.log`.
The six-color probe also has readable text and correct colors with the
production Intel workaround, including after programmatic resize under
synchronization validation.

The first-build milestone (ordinary window, correct Flutter frame, pointer and
basic keyboard input, resizing, minimize/restore, focus and clean shutdown) is
now verified on Windows x64 / Intel HD Graphics 530 / driver 31.0.101.2111.
This is not full Phase 3 parity. IME conformance, clipboard/DPI, textures/FRB,
additional window kinds, DWM pacing, rlutter Windows tooling and release/CI
remain pending. Other Windows adapters and Linux/Android regression runtime
coverage are unavailable on this host.

### Native text, Unicode clipboard and current-scale DPI checkpoint (2026-11-09)

Added `smoke_fixture/lib/input_probe.dart` and the portable Python-backed
`test-rust-shell-text-input` task. The fixture reports widget control geometry,
editing/composition/UTF-16 selection, focus/lifecycle and viewport metrics; Python
still drives real native mouse and keyboard events. Added click/chord/metrics
operations to the desktop API with Win32 and Hyprland implementations. Windows
compares Flutter DPR against `GetDpiForWindow`; Hyprland distinguishes compositor
logical coordinates from physical viewport dimensions. Native key/mouse focus
must succeed before typing can pass. The fixture restores prior clipboard text
on exit (rich formats are not preserved).

A native Windows run passed typing `hello123`, Ctrl+A/C, Clipboard.getData and
hasStrings, copying `A` + emoji + Hebrew through Clipboard.setData, Ctrl+V back
into the field, exact UTF-16 selection `[7,7]` with no composing range, readable
Unicode capture, minimize/restore lifecycle, resize/native size agreement, DPR
1.0 agreement at 96 DPI, and clean close under Vulkan validation. Evidence:
`%TEMP%/flutter-rust-input-fs6hodjs`. An earlier run failed the pointer/focus
step; the harness now explicitly checks field focus and allows the native
pointer move/button events to settle before typing.

Added a protocol-level regression for preedit cancellation/recommit, UTF-16
replacement and suppressed duplicate keyboard text for both full-state and
delta-model clients. The ordinary suite passes: 48 host tests, 17 C++ tests,
core/queue/ordinary GPU checks, two build-helper tests, 14 accounting tests and
all eight native desktop tests. The Dart input fixture analyzes without issues.

**Coverage remains partial:** loaded-layout inspection reports US English
`04090409`; an IMM32-reported layout is not evidence of a working composition
session. Real candidate/preedit/commit/cancel behavior has not been exercised
with a composition IME. Current-scale metrics do not validate per-monitor DPI
transitions. No Linux/Android or additional Windows display/GPU coverage was
available. These remain parity checks, not completed conformance milestones.

The final formatted fixture also passes through
`python tools/rust_shell/tasks.py text-input`; repeat-run evidence is
`%TEMP%/flutter-rust-input-seiwvefs`. Task v3.40.1 lists the new task correctly.

### Windows texture checkpoint (2026-11-09)

The shared Python texture tasks pass on Windows with Khronos Vulkan validation:
wgpu animation (19 presentations), CPU pixel-buffer animation (20 presentations)
and 40 live texture lifecycle rotations (279 presentations). Animation checks
require changing captured pixels in the Flutter widget, not only frame counters.
The lifecycle check verifies replacement between producer types, resize,
unregistration/reclamation of all retired rings and exactly one context
recreation, followed by clean status-zero close. No GPU/runtime diagnostic was
reported. Successful texture runs clean their temporary captures by design.

The default texture task now compiles the same example Dart entry point using
the standalone smoke fixture's package configuration, avoiding dependency
resolution for the entire Flutter example workspace. Custom texture-example
projects retain their own package/bundle configuration. Linux/Android runtime
coverage remains unavailable here. FRB dispatch is the next checkpoint.

### Windows FRB dispatch checkpoint (2026-11-09)

The first Cargo-linked FRB run built successfully but Dart process lookup failed
on `frb_get_rust_content_hash`. Rust `no_mangle` symbols in the rlib were not PE
exports of the final executable. The fixture build helper now uses explicit
MSVC `/EXPORT:` arguments for the 15 entry points in pinned FRB 2.12.0's wire
and MultiPackageCBinding. `llvm-readobj --coff-exports` confirms all 15 names.
This is fixture linking support; general rlutter Windows packaging remains
pending. The import-library link and staged engine DLL are unchanged.

The direct fixture and a full `python tools/rust_shell/tasks.py frb` repeat both
pass under Khronos Vulkan validation with wgpu validation/debugging enabled.
The named background isolate executes its FRB call off the host thread; its
callback runs on the main thread, and a callback enqueued by a synchronous call
is deferred. Both isolates see the same distinct texture IDs (1 and 2).
Both wgpu and pixel-buffer producer counters advance, each half-window capture
changes independently, and native close returns zero without GPU diagnostics.
Evidence is retained at `%TEMP%/flutter-rust-frb-w4uav6uo` and
`%TEMP%/flutter-rust-frb-q509rxmi`; the original symbol failure is retained at
`%TEMP%/flutter-rust-frb-l0_5anx0`.

The FRB Python harness now preserves evidence on success and failure instead of
deleting it while unwinding, and shares the texture diagnostic matcher (including
panics and threading errors). Python compilation and formatting checks pass.
Linux/Android runtime regression validation remains unavailable on this host.
Windows texture/FRB smoke parity is now verified on the Intel HD 530 host;
additional window kinds, DWM pacing and rlutter tooling remain next, with native
IME composition and real DPI transitions still awaiting suitable coverage.

### Windows Rust window-owner routing (2026-11-09)

Enabled the existing symbol-probed Rust window owner on Windows. Both the
availability getter and `_window_io.dart` selection order needed changes:
Windows previously constructed the stock Win32 owner before considering the
Rust host, causing an unresolved `InternalFlutterWindows_WindowManager_Initialize`
at binding initialization. Standard processes without Rust ABI symbols still
select their platform owner. Native malloc/free lookup worked in the actual
Windows multi-view fixture; no allocation exports were needed.

`python tools/rust_shell/tasks.py windowing` now passes 60 view removals with
284 primary presentations, child resize and additional rendered views alive
through whole-shell teardown under Vulkan validation, with status-zero exit.
Evidence: `%TEMP%/flutter-rust-windowing-2kztf4p7`. The windowing harness now
retains logs/status/statistics, including timeout failures, and accepts a bounded
timeout override for diagnosis. The original owner-selection error is retained
at `%TEMP%/flutter-rust-windowing-z2j8_nl9`. This checkpoint verifies regular
multi-view routing/lifetime, not additional window-kind parity.

### Windows dialogs and satellites checkpoint (2026-11-09)

The Windows adapter now accepts regular, dialog and satellite window requests.
Dialog/satellite parenting uses `SetWindowLongPtrW(GWLP_HWNDPARENT)` for owned
top-levels, including satellite reparenting. It clears/checks last-error to
correctly distinguish successful zero-valued previous ownership from failure.
Satellites use winit skip-taskbar behavior plus `WS_EX_TOOLWINDOW`, clear
`WS_EX_APPWINDOW`, and apply a frame-change notification. Shared creation now
calls a platform configuration hook; Linux/Android keep a default no-op.

A real Win32/winit native test verifies dialog ownership, absence of WS_CHILD,
satellite tool-window styling and reparenting without moving the window.
The full ordinary check suite passes (49 host tests, 17 C++ tests, core/queue
and ordinary GPU checks, two build-helper tests, 14 accounting tests and all
eight native desktop tests).

Added `task test-rust-shell-owned-windows` backed by the shared Python runner
and `windowing_fixture/lib/owned.dart`. Fifteen cycles create two regular
parents, an owned dialog and a satellite, render/resize them, reparent the
satellite to the other regular parent and then to the dialog, unmount and
release all four, and verify 60 destruction callbacks. It passes under Vulkan
validation with 252 primary presentations and status-zero close. Evidence:
`%TEMP%/flutter-rust-windowing-adefi6up`. The harness additionally requires a
presentation record for every retired view, rather than inferring child frames
from primary activity. Popups/tooltips are still explicitly unsupported;
parent-close cascade/minimization behavior is not claimed validated by this
fixture. Other Windows GPUs and Linux/Android runtime coverage remain unavailable.

The strengthened harness repeat passes with all 60 per-view presentation files
nonempty and 238 primary presentations; evidence:
`%TEMP%/flutter-rust-windowing-9r7ln04s`. The owned-window Dart fixture analyzes
without issues. Python compilation/format checks pass.

### Owned-window parent lifecycle coverage (2026-11-09)

The owned-window fixture now destroys only the regular owners, leaving dialog
and satellite descendant removal to the host. All 60 per-view presentation
records and destruction callbacks pass with 262 primary presentations, clean
exit and no Vulkan/framework diagnostics. Evidence:
`%TEMP%/flutter-rust-windowing-m7ib3dp6`. The native Win32 test now shows each
owned dialog/satellite and verifies owner minimize hides it and owner restore
shows it again. The full ordinary suite passes (49 host tests, 17 C++ tests,
core/queue/ordinary GPU checks and Python suites). This closes the previously
unverified parent-cascade and native minimize/restore checks on this host;
no production teardown change was required.

### Initial Windows tooltip support and desktop positioner (2026-11-09)

Added a pure desktop positioner for the shared ABI anchor/gravity/offset and
per-axis flip, slide and resize flags, including negative monitor origins.
Its tests cover unconstrained offsets, independent axes, flip-before-slide and
oversized resizing. Windows converts parent client origin and monitor work area
through the parent's scale before resolving placement; Wayland still delegates
to its compositor. No Linux/Android runtime coverage is available here.

Windows now accepts tooltip requests and creates owned, undecorated, nonresizable
`WS_POPUP`/tool windows with permanent `WS_EX_NOACTIVATE`. A native test verifies
owner, popup/no-activate styles and unchanged foreground window. This caught
winit clearing the extended style while showing a window: the implementation
now shows with activation disabled, then installs the permanent native styles.
Grabbing popup requests remain explicitly unsupported.

Added `task test-rust-shell-tooltips` using the shared Python lifecycle harness.
Twelve cycles render two regular windows, a dialog, satellite and tooltip,
resize/reparent the owned views, then destroy only the owners and require all
60 descendant callbacks and per-view presentation records. The initial run
passed with 193 primary presentations and clean Vulkan-validation shutdown:
`%TEMP%/flutter-rust-windowing-ph67thyv`. The final native style fix passes the
ordinary suite: 52 host tests, 17 C++ tests, core/queue/ordinary GPU checks,
two build-helper tests, 14 accounting tests and all eight desktop tests.
The tooltip fixture analyzes without issues.

This verifies initial work-area placement and owner-driven lifetime on this
host. Reactive repositioning after parent movement, DPI or content-size changes
and grabbing-popup outside-click/activation-loss dismissal still require work;
this is not full popup/tooltip parity. DWM pacing and rlutter tooling remain
pending, as do real IME composition and display-scaling transition coverage.

The final rebuilt tooltip runner also passes all 60 per-view checks with 191
primary presentations and zero-status close under Vulkan validation. Evidence:
`%TEMP%/flutter-rust-windowing-60vi5kmj`.

### Reactive Windows tooltip geometry checkpoint (2026-11-09)

The host retains each tooltip's logical placement request and re-evaluates it
for native parent movement, surface resize and scale-factor events, traversing
owned descendants while skipping views being removed. Windows reuses the
initial-placement work-area/DPI calculation and applies only changed physical
geometry with no activation or z-order change. This guards against native
move/resize event feedback. Explicit constraint updates also refresh desired
size and apply geometry immediately: changing min/max constraints alone did not
resize the HWND in the first diagnostic run.

The Python platform API now exposes native movement and owner-scoped discovery.
On Windows, `test-rust-shell-tooltips` coordinates with the Dart fixture to move
the owner by 60x40 physical pixels and require the tooltip to follow by exactly
the same delta. It then changes logical constraints from 160x100 to 200x120,
checks physical size using native DPI, and verifies the centered anchor is
preserved to pixel-rounding tolerance. Snapshot files retain before/after
geometry. All 60 per-view presentation and descendant-destruction checks then
pass under Vulkan validation with 201 primary presentations and clean close.
Evidence: `%TEMP%/flutter-rust-windowing-9fjs2ct5`.

The first failed size-change run is retained at
`%TEMP%/flutter-rust-windowing-5_280rm2`. An intervening retry started before
runner staging finished, leaving the old executable locked; it was discarded
and staging completed before the successful run. The ordinary suite passes:
52 host tests, 17 C++ tests, core/queue/ordinary GPU checks and all Python suites.
The Dart fixture analyzes without issues.

Actual display-scale transitions and multi-monitor movement remain unavailable
on this host. Explicit framework `updatePosition` anchor changes still need a
private-ABI operation; this checkpoint reacts to retained anchor requests and
native geometry/constraint changes. Grabbing-popup outside-click/Escape/
activation-loss dismissal remains unsupported and is the next implementation
checkpoint before DWM pacing. Linux/Android runtime coverage is unavailable.

The final shared task repeat also passes (184 primary presentations) with
DPI-scaled geometry expectations and retained movement/resize snapshots:
`%TEMP%/flutter-rust-windowing-qaribdlc`.

### Explicit popup/tooltip anchor updates (2026-11-09)

Added the private `FlutterRustShellWindowUpdatePopupPosition` operation and an
appended host callback, bumping the lockstep shell ABI from 13 to 14 (plugin SDK
API remains 1). The request reuses the existing decoder's placement validation;
the host rejects missing/removing views or a changed kind/parent and preserves
the desired size while replacing anchor, gravity, offset and adjustment flags.
The Windows path repositions the existing native view. Linux continues to allow
position updates to be ignored. Engine DLL, Cargo runner and C++ tests were
rebuilt together; PE export inspection confirms the new window function.

Both Dart tooltip and popup controllers now route `updatePosition` through the
operation, using a shared request allocator with creation and ignoring calls on
controllers already being destroyed. The native fixture changes the anchor and
positioner, verifies an exact 27x24 logical-pixel movement of the same HWND,
then changes only the positioner and verifies a -10x-7 shift while retaining the
previous anchor. Parent movement, anchored resizing, all 60 presented views,
owner cascade and clean shutdown also pass under Vulkan validation (203 primary
presentations). Evidence: `%TEMP%/flutter-rust-windowing-v9zodnid`.

The ordinary suite passes: 52 host tests, 17 C++ tests including paired ABI
checks, core/queue/ordinary GPU tests and Python suites. The tooltip fixture
analyzes cleanly. Grabbing-popup creation/dismissal remains the next checkpoint;
real display-DPI transitions and Linux/Android runtime coverage are unavailable.

### Windows grabbing-popup checkpoint (2026-11-09)

Windows now accepts grabbing popups through the owned, no-activate WS_POPUP
path with native mouse capture. Outside presses, Escape and owner focus loss
request CloseRequested through the existing asynchronous Dart/raster/native
teardown. Inside button release restores capture after winit releases its
button grab. A dismissed marker prevents duplicate close callbacks; surviving
popups reacquire capture after view removal. Capture acquisition fails explicitly
without an active host input thread.

The first activation-loss check failed: WM_NCACTIVATE delivers focus loss before
GetForegroundWindow necessarily changes. Trusting winit's focus transition fixes
that path. Diagnostic evidence: `%TEMP%/flutter-rust-windowing-25uwqxkl`.
Temporary focus logging was removed from the final runner.

Added `task test-rust-shell-popups` and the Dart popup fixture. Native automation
verifies inside-click retention followed by outside-click dismissal, Escape,
and activation of another regular host window. Twenty cycles require all 60
views to present and report destruction; remaining cycles leave popup removal
to owner teardown. The final run passes with 304 primary presentations, clean
status-zero close and no Vulkan/framework diagnostics under Khronos validation.
Evidence: `%TEMP%/flutter-rust-windowing-gzubypi8`.

The ordinary suite passes: 52 host tests, 17 C++ tests, core/queue/ordinary GPU
checks, two build-helper tests, 14 accounting tests and eight desktop tests.
The popup fixture analyzes without issues. Rust formatting, Python compilation
and diff whitespace checks pass.

This verifies initial single-popup behavior on this Intel HD 530 Windows host.
Nested-popup interaction and external-application activation are not covered by
this fixture. Real IME composition, display-DPI transitions, other Windows GPUs
and Linux/Android runtime coverage remain unavailable. DWM pacing is the next
planned implementation checkpoint; rlutter tooling remains pending.

### Windows DWM-derived vsync scheduling (2026-11-09)

Enabled RustVsyncWaiter callbacks on Windows through PlatformBackend, retaining
Wayland's existing frame-callback behavior. The Windows timing hook samples
DwmGetCompositionTimingInfo with its required null HWND and pairs QPC with Rust
Instant. It computes the strictly next tick from qpcVBlank/qpcRefreshPeriod using
bounded integer arithmetic. Every request resamples phase instead of carrying
callback lateness into subsequent deadlines. Invalid/failing samples select the
nominal monitor timer fallback, and future requests retry DWM. The generated
windows-sys 0.61.2 binding supplies the SDK's packed native layout; fixture locks
were updated without changing dependency versions.

The existing event-loop deadline and one-shot HWND wake timer deliver requested
frames during Win32 modal resizing. Windows does not request an immediate redraw
as a substitute for vsync. No helper thread, periodic idle timer or private ABI
change was introduced. The existing C++ callback still timestamps frame start
at delivery; this is DWM-derived host scheduling, not measured scanout timing.

All 55 host tests pass, including phase preservation after late callbacks,
fractional/large QPC conversion and invalid-sample fallback. The ordinary suite
also passes 17 C++ tests, core/queue/ordinary GPU checks, two build-helper tests,
14 accounting tests and eight native desktop tests. The smoke runner rebuilt
and passed 500-resize stress under Khronos Vulkan validation, including keyboard
redraw, minimize/restore, maximization, modal live-resize presentations and clean
close. Evidence: `%TEMP%/flutter-rust-stress-2cc1rkyf`.

Runtime logs confirm DWM selection at 31,250,000 ns. An independent native API
probe reports refresh and composition ratios 10000000/312500, QPC frequency
10000000 and period 312500: this desktop reports 32 Hz. The retained
`dwm-timing.json` records that sample. Tooltip parent movement, anchored resizing,
explicit/partial updates, all 60 view presentations/destruction callbacks and
clean validation shutdown also pass (196 primary presentations). Evidence:
`%TEMP%/flutter-rust-windowing-7pmuhmg0`.

Mixed-refresh displays, VRR, physical frame latency and an unavailable-compositor
runtime remain unverified. The native clock is composition-wide, not a per-HWND
monitor timing API. Linux/Android runtime coverage remains unavailable. The next
planned workstream is rlutter Windows tooling, then release packaging and CI.

The popup fixture also passes with DWM pacing active: inside-click retention,
outside-click/Escape/owner-focus-loss dismissal, 60 per-view presentation and
destruction checks, 305 primary presentations and clean validation shutdown.
Evidence: `%TEMP%/flutter-rust-windowing-pqi5pq5t`. Rust formatting and diff
whitespace checks pass. Native API failure/fallback recovery was not forced in
this runtime; the unit coverage exercises invalid timing samples.

### rlutter Windows local-engine debug checkpoint (2026-11-09)

Ported rlutter's native desktop path to Windows: Windows device discovery and
selection, create/build/run commands, .bat/.exe Flutter/Dart paths, SDK DLL/import
library artifacts, Windows native-library names and shared desktop process
lifetime. Unix-only permissions/process/symlink code is gated. Windows links
have a copy fallback; directory symlinks are removed with the correct native
operation. Host build commands reject cross-host desktop builds explicitly.
Windows managed SDK downloads report that a local checkout is required.

Runner version 10 adds the Windows engine variant, MSVC import-library linkage
and embedded PerMonitorV2/UTF-8/common-controls manifest while retaining Linux
rpath and Android linking behavior. DLL staging tracks only tool-owned DLLs so
obsolete plugins are removed without deleting executables, data or unrelated
files. Windows drive/UNC package URIs resolve correctly. Android cross-library
naming remains .so. Applicable Android tooling unit tests also pass on Windows;
this is not Android runtime validation.

The first real build exposed two portability failures that unit fixtures had
not exercised: the AOT frontend rejects a symlinked flutter_patched_sdk root,
and CRLF prevented dependency-line replacement, duplicating flutter-shell-winit.
The SDK layout now copies the small patched-SDK directory on Windows and the
migration normalizes line endings. Generated apps also need the workspace's
vendored wgpu-hal patch to preserve the Intel workaround; templates/migration
now install it. Existing lockfiles update only that package offline. SDK cache
completeness now requires its HAL sources. Tests cover the migration, CRLF,
physical patched-SDK directory, path handling and selective DLL pruning.

A real `rlutter create --rust-platforms windows` project outside the SDK and
`rlutter build windows --debug` succeeded. Added `task test-rlutter-runtime` with
retained evidence: Flutter attach publishes its DDS URI; a native click increments
the counter; hot reload changes the rendered label while preserving state=1;
hot restart applies the changed source with state=0; quit detaches and removes
the owned native process. The relocated executable/DLL/data bundle renders the
same centered widget from an unrelated working directory and closes with status
zero under Khronos Vulkan validation. Evidence:
`%TEMP%/flutter-rust-rlutter-em7ezyx4`; generated project:
`%TEMP%/rlutter-windows-0833f1b8`. PE inspection confirms the embedded manifest.

All 108 applicable rlutter tests pass; Clippy with warnings denied, Rust
formatting and Python compilation pass. The Linux-only managed-download test
remains Linux-gated. The native runtime harness compares the centered widget
because desktop corner/off-screen pixels vary between native window positions.
An early restart check was tightened to wait for the first post-restart build.

A VM expression-evaluation probe reports `_compileExpression: No compilation
service available`; successful attach, field inspection and hot reload do not
close that debugger gap. Initial probe evidence remains at
`%TEMP%/flutter-rust-rlutter-ykutnpz6`. Source expressions and breakpoint
conformance, managed SDK downloads, release/profile/AOT builds, general plugin/
FRB packaging, icon/version resources and Windows CI remain follow-ups.
Symlink-denied fallback, Linux/Android runtime regressions and broader Windows
GPU/display coverage remain unverified; real IME/DPI-transition gaps are unchanged.


## Linux test repairs and matching framework tester

The debug metrics dispatcher now forwards the texture-frame and all-views-render
callbacks required by the local engine. The texture rendering test supplies its
fake view's dispatcher. Framework test configuration starts with experimental
windowing disabled; individual windowing tests opt in explicitly, independent of
the developer's global Flutter settings.

Added `task test-rust-shell-framework`, which builds the matching Flutter tester,
Dart SDK and frontend server before testing. The previous tester was stale and
failed with an invalid SDK hash. Hyprland automation now uses the correct Lua
focus, cursor, shortcut and workspace dispatchers, and mouse injection preserves
the requested position instead of resetting it through a window selector.
Wayland tooltip tests wait for compositor-confirmed parent mapping. Generated-app
pixel comparisons use matching viewports and a stable reference frame after
native resizing.

Verified on Linux: 448 foundation/windowing/rendering/texture framework tests;
Rust workspace and 17 C++ tests; Python suites; 108 rlutter tests and Clippy;
four hardware GPU tests; animated wgpu/pixel textures, 40 texture replacements,
regular/owned/tooltip multi-view lifecycle, 20 startup/shutdown cycles, 500 resizes,
native Unicode typing/clipboard/selection/metrics, and FRB. Generated-app launch,
rendered hot reload with preserved state, hot restart, relocation and clean close
also pass. Formatting, analysis against current engine sources, Python compilation
and whitespace checks pass. Windows-only automation is skipped on Linux; native
IME composition, actual monitor-scale transitions and Windows popup behavior are
not established by these runs.

### Headless agent Phase 2 — observation and control

Implemented protocol v1 over a private local socket with CLI stdio isolation,
owned-process shutdown, bounded parsing/dispatch, diagnostics and event traces.
ABI 16 adds per-view semantics enable/update/actions and UI build frame stamps.
Incremental caches expose identifiers, actions, transformed bounds and revisioned
handles; obscured values and editing traces are redacted. Virtual windows support
real pointer/key input, Unicode editing, focus and correlated PNG capture.

The real-engine fixture passes Unicode/RTL input, two windows, pointer activation,
persisted values, fractional DPR captures, stale/ambiguous targets, cancellation
and view removal while capture is pending. GPU-free protocol/cache/PNG checks
and native shell tests remain separate. See
[agent protocol](engine/src/flutter/shell/platform/rust/docs/agent-protocol.md).

### Headless agent Phase 3 — reproducible scenarios

Added `rlutter verify`, bounded predicate waits/assertions, separate action and
observation results, fresh-process builds with SHA-256 provenance, and reports
with per-view evidence. The opt-in Dart bridge supplies ValueKey/public property
queries, named predicates and chained structured framework error hooks. Ordinary
builds leave it disabled. Fixed comma splitting of multiple assemble Dart defines.

The generated multi-window fixture passes the Unicode save workflow. The same
scenario fails with the intentional persisted-value regression, and startup and
interaction framework errors cannot pass. See
[verification](engine/src/flutter/shell/platform/rust/docs/verification.md).

### Headless agent Phase 4 — video evidence

Added bounded per-view FFmpeg/FFV1 recording with idle duration preservation,
resize segments, session timestamps, source frame/UI stamps and drop/overhead
counters. Screenshot and recording readback requests coexist. Stream/view/session
shutdown finalizes encoding off the UI thread, with supervised timeout and partial
manifests after encoder or runner failures. Retention limits fail explicitly.

Decoded real-engine recordings validate dimensions, timestamps, idle duplication,
visible pointer actions, resize, and two views on a common timeline. Missing,
crashing and blocked encoders and a killed owned runner all retain explicit
failure evidence. See [recording](engine/src/flutter/shell/platform/rust/docs/recording.md).

### Headless agent Phase 5 — Linux CI and extracted SDK delivery

Added GPU-free Linux/Windows protocol jobs, a Linux software-Vulkan
render/host/agent/video lane, and an extracted-SDK publication gate. Source
packaging includes both patched dependencies, the headless/agent code, Dart
bridge and fixture. Compiled-engine ABI/build-mode manifests reject mixed
artifacts, while native startup enforces ABI 16.

The actual archived SDK creates and verifies a fresh generated application in
Linux debug, profile and release. Ordinary release disables the bridge and owns
no agent endpoint. Six controls verify the positive workflow and reject a stale
behavior regression, startup and interaction errors, replaced error hooks, and
an unavailable Vulkan driver. Final lifecycle/plugin/idle, software-Vulkan
protocol, decoded-video and native desktop texture checks pass.

Windows headless runtime transport, Vulkan and packaging validation remain a
separate pending platform gate; a Windows Vulkan host was not available in this
Linux workspace. The Windows CPU schema job is not a runtime certification.
An MCP adapter remains deferred until those distribution/runtime gates stabilize.
See [distribution](engine/src/flutter/shell/platform/rust/docs/distribution.md).

### Main merge and Windows DDS debugger validation (2026-11-09)

Merged upstream main (acade0f6b53) and pushed merge 559da144d84. Two Vulkan
synchronization conflicts were resolved using upstream's attachment-read/write
and fragment-stage handling. Both conflicted source files compile. Dependencies
were synced to Dart 3b174b7217 and Skia b11315d0a0, then GN regenerated. The first
native build stopped because Dart's generated package configuration still used
language 3.13 for the new frontend, which requires 3.15. Running build hooks with
the existing Visual Studio Build Tools override refreshed it. The matching DLL,
Dart SDK, frontend server and Cargo smoke runner now build successfully.
Build logs: `engine/src/out/rebuild-windows-main*.log`.

Before committing, fetched and fast-forwarded to the user's Linux fixes in
6791498d4ef. The local Windows changes reapplied cleanly with a three-way patch;
no conflict markers remain. Linux viewport stabilization, Hyprland fixes and
Wayland parent-mapping changes are retained. The refreshed Dart formatter's
fixture-only changes are committed separately.

The expression-evaluation gap was an HTTP probe error: DDS proxies HTTP to the
raw VM, bypassing the external expression compiler registered with DDS. A new
Dart probe uses vm_service over the DDS WebSocket. It evaluates `probeState + 41`
and `probeLabel`, adds a source breakpoint, waits for the native click to pause
in incrementProbe, evaluates local `next == 1`, removes the breakpoint and
resumes. No native-engine debugger change was required.

The combined runtime task passes debugger checks, state-preserving rendered hot
reload, state-resetting hot restart, attach quit/owned-app cleanup, and relocated
bundle pixel comparison/status-zero native close under Vulkan validation.
Evidence: `%TEMP%/flutter-rust-rlutter-cf6orzam`, including
`debugger-status.json`, VM state observations, captures and logs. The first run
stopped before debugger setup because Windows Search held foreground; that run
was not counted as a pass. Search was dismissed before retrying real input.

Linux's new stable 800x600 viewport check exposed an adapter difference: Win32
resize takes outer-window dimensions. Added resize_client using measured native
frame margins, preserving resize's original contract and Linux's implementation.
The native adapter test checks an exact 420x300 client viewport and passes all
eight tests without skips. The ordinary suite also passes 55 host tests, 17 C++
tests, core/queue/ordinary GPU checks, two build-helper tests and 14 accounting
tests. All 108 rlutter tests and Clippy pass. Dart probe analysis reports no issues.

This verifies one breakpoint and paused local-expression scope on this Windows
host. The new debugger probe has not run on Linux; the user's existing Linux
coverage above remains distinct. Full DAP/editor behavior, Windows managed SDK,
release/profile/AOT and general plugin packaging, CI, real IME composition and
monitor-scale transitions remain follow-ups.


## Windows release/AOT checkpoint (2026-11-09)

The matching Windows host_release engine DLL, Dart SDK, frontend server, AOT
compiler, shader compiler, font subsetter and sky_engine package build from the
post-main checkout. The normal Windows build task now requests these compiler
artifacts; its release Cargo smoke runner also links successfully.

`test-rlutter-aot` creates and builds a disposable release app, requires app.so
and no debug kernel, relocates executable/DLL/data, launches from an unrelated
working directory, checks the exact initial and clicked frame colors, and
requires native close with status zero under Vulkan validation. The Windows
runtime check passes; retained evidence is `%TEMP%/flutter-rust-release-0f6axvzj`
(build/runtime logs, full captures, relocated bundle and result.json).

Current upstream Windows x64 AOT emits a PE/COFF DLL named app.so. The original
plan's ELF note and an initial probe assertion were corrected; the initial
format-rejected run was not counted as a runtime pass. The native engine already
loads this library correctly, so no AOT engine change was needed. Profile is not
covered by this release run. SDK distribution and input follow as separate phases.


## Windows SDK distribution checkpoint (2026-11-09)

Windows x64 managed SDK discovery now uses LOCALAPPDATA (or XDG_CACHE_HOME),
Windows BETA archive names and dart.exe validation. The physical Rust SDK ships
debug/release engines with their import libraries, ICU, compiler tools, matching
Dart SDKs, patched SDKs and sky_engine. Both patched Rust source dependencies
(winit-wayland and wgpu-hal) are packaged; the Linux archive also now includes
its required wgpu-hal patch. Profile remains a local-engine workflow on Windows.

The standalone Flutter packager imports the resolved numeric Flutter tag with
its history, checks the common ancestor, warms the Windows wrapper and rejects
unknown version metadata before archiving. A fresh-create test caught the first
local clone's missing numeric tag (0.0.0-unknown); the corrected archive reports
3.50.0-1.0.pre-666 and passes creation/dependency resolution without overrides.

Local validation extracts both exact archives into an isolated managed cache
and a new project, with the packaging-stage directory hidden. No checkout engine
or external source links are used. Both debug (kernel) and release (PE app.so,
no kernel) generated-app builds pass. Evidence: engine/src/out/windows-distribution-
result.json and windows-distribution-builds.log; project:
%TEMP%/rlutter-windows-distribution-rv0v88gs. All 111 rlutter tests and Clippy pass,
including managed-cache reuse, required import libraries and forced symlink-
privilege failure with real copy/refresh/stale-file cleanup.

The BETA workflow now has parallel Windows debug/release engine jobs and a
Windows packaging job that tests extracted SDKs before publishing Windows SDK
archives, rlutter.exe and checksums. actionlint and the PowerShell blocks pass
local syntax checks. Cloud execution and actual BETA download/publication are
pending the branch merge; local archive validation is distinct from publication.


## Windows packaged release input checkpoint (2026-11-09)

The native text-input suite passes against both the current debug smoke runner
and an AOT release app compiled by the extracted Windows SDK, with the packaging
stage still hidden. It verifies real keyboard typing, copy/paste, the emoji/RTL
Unicode clipboard sample, UTF-16 selection, minimize/restore lifecycle, resize,
Flutter/native physical dimensions and current monitor scale, clipboard text
restoration and status-zero shutdown under Vulkan validation. Evidence:
%TEMP%/flutter-rust-input-mp6jhfg0 (debug) and
%TEMP%/flutter-rust-input-at9z3ktr (packaged release).

The adapter now enumerates native monitor work areas. When multiple displays
are available, the input probe moves between them and compares Flutter/native
metrics; it reports a scale transition as passed only if different native scales
were actually observed. This RDP host exposes one 1024x720 work area at scale 1,
so real mixed-DPI transitions remain unverified. IMM32 layout enumeration is not
proof of composition; real IME composition remains unverified too.

The fixture places its 800x600 viewport within a small Windows desktop. Initial
capture/focus failures were traced by native hit-testing to Start/Search system
menus; only those verified menus were dismissed before retrying. Occlusion and
foreground checks remain strict, and blocked runs were not counted as passes.
All eight native adapter tests pass without skips, including real monitor
geometry enumeration and exact client resizing. Python compilation/formatting
and whitespace checks pass. No native input implementation change was needed.


### Windows CI bootstrap follow-up (2026-11-09)

The first cloud Windows jobs stopped in gclient's offline-pub audit: the Flutter
smoke app was missing from the exclusions already used for FRB/windowing app
fixtures. Added its narrow exclusion and a regression test proving that unknown
packages under the same Rust test tree remain audited. The actual engine tree
has no unlisted packages, the regression passes, and the full offline-pub hook
exits zero locally. Windows CI runs the regression after sync. Artifact publication
still depends on the restarted cloud builds; no failed run is counted as a pass.


The next Windows bootstrap run passed the package audit and exposed the hook
installer's legacy git.bat assumption. It now resolves installed Git from PATH
and honors its explicit argv. A Windows regression installs and uninstalls the
hooks in a real temporary repository whose PATH contains only native git.exe.
The regression passes; hooks remain enabled in CI. Both bootstrap regression
checks run after sync before the engine build.


The cloud sync itself then completed; the regression exposed the runner's 8.3
TEMP alias. Hook setup now canonicalizes and case-normalizes Git/common paths
and hook/repository paths, avoiding false linked-worktree detection and invalid
relative hook paths. Real native-Git tests cover an available short-path alias
and a linked worktree, including preservation of worktree-scoped configuration.
Both tests pass locally; the CI step continues to run them before compilation.


Windows CI subsequently passed sync and both bootstrap regressions, then GN
found the same legacy git.bat assumption in revision discovery. GN and the build
revision helper now resolve installed Git from PATH. A real temporary-repository
regression covers both helpers, and full Windows GN generation succeeds with
all git.bat-containing directories removed from PATH (1901 targets). The CI
bootstrap step runs this regression too; native runtime code is unchanged.


## Windows CLI installer (2026-11-09)

Added a Windows x64 PowerShell installer with GitHub CLI download support,
HTTPS fallback, SHA-256 verification, configurable release/repository/install
location, and user PATH setup. The rolling CLI workflow now builds and packages
Windows alongside Linux, runs offline installer integration checks with Windows
PowerShell 5.1, and includes both install commands in its release notes.

Local offline checks pass for authenticated-download argument construction,
HTTPS fallback, installation into paths with spaces, upgrades, checksum rejection
preserving the installed binary, and temporary-download cleanup. Live Windows
rolling-release downloads remain dependent on successful workflow publication.
