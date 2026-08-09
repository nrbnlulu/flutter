# Flutter Rust Shell Progress

This file records implementation progress for the architecture in
[`flutter-rs.md`](flutter-rs.md). It is an implementation log, not a replacement
for the architectural plan.

## Current focus

Phase 0 is complete. Phase 1 — winit platform host — is in progress. Pointer,
window metrics, display updates, lifecycle, raw keyboard events, and a rendered
Impeller/wgpu frame are working. The next load-bearing item is completing GPU
handoff synchronization; text input/IME and real vsync follow.

## Status

| Area | Status | Evidence |
| --- | --- | --- |
| Existing shells remain available | Complete | The Rust target is opt-in and is not added to the existing platform-selection group. |
| In-tree Rust platform target | Complete | `//flutter/shell/platform/rust:flutter_rust_shell` builds. |
| Internal PlatformView adapter | Complete | `PlatformViewRust` builds and its focused tests pass. |
| Rust/C++ ABI | Complete for phase 1 plumbing | ABI v1 covers task-runner callbacks, Vulkan context/presentation callbacks, shell create/run/destroy, viewport/display metrics, lifecycle, pointer, and raw keyboard events. |
| Rust workspace and `flutter-plugin-sdk` | Complete for foundation | Workspace uses Rust edition 2024, concrete toolchain 1.93.1, and passes its tests. |
| Winit event loop | Complete for phase 0 | Linux host owns the window and event loop, dispatches Flutter task batons, and drives the Rust-owned Vulkan presentation loop end to end. |
| Merged UI/platform task runner | Complete for phase 0 | `RustTaskRunner` queues batons for the Rust host, winit returns due batons through opaque C++ handles, and it now also drives Dart's per-task microtask flush (see below). |
| Impeller/wgpu interop | Working, synchronization incomplete | wgpu owns the Vulkan device/surface; C++ creates `ContextVK` from borrowed handles plus the in-tree Impeller Vulkan shader bundle. An acquire barrier now prevents black frames, and resize reconfiguration is deferred until the outstanding frame is presented. Rapid resize stress still reports Impeller fence/invalid-image errors; explicit completion/semaphore ownership remains the next GPU milestone. |
| Linux runnable shell | Complete for rendered-frame proof | `flutter_rust_shell_runner` boots a real kernel-snapshot Flutter app; a live Hyprland capture shows the Flutter title, text field, button, and debug banner rendered in the Rust shell. |
| Pointer input | Complete for phase 1 plumbing | Winit mouse, wheel, and touch events cross the private ABI and are converted into Flutter `PointerDataPacket`s; Rust translation and C++ conversion tests pass. |
| Window and display metrics | Complete for phase 1 plumbing | Initial, resize, and scale-factor changes report physical viewport size, the real device-pixel ratio, and current-monitor size/refresh rate; zero-sized surfaces are not configured. |
| Lifecycle | Complete for phase 1 plumbing | Focus, minimize/restore, winit suspend/resume, and shutdown are deduplicated in Rust and forwarded through `flutter/lifecycle`; Rust transition and C++ ABI conversion tests pass. |
| Keyboard input | Complete for phase 1 raw events | Winit physical/logical keys, down/up/repeat, characters, modifier sides, and synthesized state cross the private ABI as Flutter `KeyData` packets; IME/text editing remains separate. |

## Implementation log

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
  `rust_shell.cc`.
- Wired `flutter-shell-winit`'s `resumed()`/`window_event()` to actually create
  and run the `RustShell`: it extracts Vulkan handles via
  `GpuBroker::with_vulkan_context`, builds the presentation callback table from
  `GpuBroker::presentation_callbacks`, and calls the new shell lifecycle FFI.
- Added `main.cc` and the `flutter_rust_shell_runner` GN executable: a
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

## Validation

- `git diff --check` passes.
- C++ sources were formatted with `clang-format`.
- Built `//flutter/shell/platform/rust:flutter_rust_shell`,
  `//flutter/shell/platform/rust:flutter_rust_shell_unittests`,
  `//flutter/shell/platform/rust:flutter_shell_winit_rust`, and
  `//flutter/shell/platform/rust:flutter_rust_shell_runner` with the
  host-debug GN configuration (`et build`-managed `out/host_debug`).
- Ran `flutter_rust_shell_unittests`: 14 tests passed.
- Ran `cargo +1.93.1 test --workspace --locked`: all crate and documentation
  tests pass.
- Ran `task run-flutter` through the app's `runner-rs` Cargo target against a
  real JIT kernel snapshot: the process stays alive, the Rust-shell window is
  mapped and visible, and a live capture shows rendered Flutter content. No
  public Embedder API is involved; the existing GTK Linux shell target remains
  untouched. Aggressive resize still produces the known GPU handoff errors.

## Next implementation steps (phase 1)

1. Complete Vulkan acquire/render/present synchronization between wgpu and
   Impeller, including fence/semaphore ownership across resize and teardown.
2. Add text input and IME integration on top of the raw keyboard event path.
3. Replace the vsync fallback timer with a real winit/compositor-driven vsync
   source.
4. Add main-thread dispatch for background isolate and Rust-worker callbacks,
   and test synchronous FFI reentrancy and main-thread starvation behavior.
5. Add deterministic startup and shutdown ownership tests for the merged
   runner.

## Constraints carried into implementation

- The Rust shell is optional; existing Flutter shells remain buildable.
- Impeller remains Flutter's renderer permanently.
- The public Flutter Embedder API is not used.
- Plugin-facing GPU types come from the semantically versioned
  `flutter-plugin-sdk` crate.
- `flutter_rust_shell_runner`'s `main.cc` is a phase 0 expedient (GN owns the
  final link so it can pull in the Dart VM/Impeller/Skia archives); it is not
  the tooling model generated applications will use. See "Who owns the final
  link" in `flutter-rs.md`.
