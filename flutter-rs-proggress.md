# Flutter Rust Shell Progress

This file records implementation progress for the architecture in
[`flutter-rs.md`](flutter-rs.md). It is an implementation log, not a replacement
for the architectural plan.

## Current focus

Phase 0 is complete. Phase 1 — winit platform host — is in progress. Pointer,
window metrics, display updates, lifecycle, raw keyboard events, and a rendered
Impeller/wgpu frame are working. The explicit Vulkan semaphore broker is now
implemented and passes rapid-resize and in-flight teardown stress. Typed text
input/IME plumbing and compositor-driven Wayland vsync are implemented and
validated. The current milestone is single-engine multi-view: one Flutter
engine and Dart isolate per application, with one native winit window and GPU
presentation surface per Flutter view. Regular windows and required application
shutdown now work end to end; additional window kinds, main-thread dispatch,
and deterministic startup/shutdown coverage follow that milestone.

## Status

| Area | Status | Evidence |
| --- | --- | --- |
| Existing shells remain available | Complete | The Rust target is opt-in and is not added to the existing platform-selection group. |
| In-tree Rust platform target | Complete | `//flutter/shell/platform/rust:flutter_rust_shell` builds. |
| Internal PlatformView adapter | Complete | `PlatformViewRust` builds and its focused tests pass. |
| Rust/C++ ABI | Multi-view regular-window extension complete | ABI v5 adds typed view IDs, per-view metrics, pointer and focus routing, asynchronous add/remove-view operations, per-view Vulkan presentation registration, and a typed synchronous regular-window control surface with asynchronous lifecycle events. |
| Rust workspace and `flutter-plugin-sdk` | Complete for foundation | Workspace uses Rust edition 2024, concrete toolchain 1.93.1, and passes its tests. |
| Winit event loop | Complete for phase 0 | Linux host owns the window and event loop, dispatches Flutter task batons, and drives the Rust-owned Vulkan presentation loop end to end. |
| Merged UI/platform task runner | Complete for phase 0 | `RustTaskRunner` queues batons for the Rust host, winit returns due batons through opaque C++ handles, and it now also drives Dart's per-task microtask flush (see below). |
| Impeller/wgpu interop | Explicit synchronization implemented and stress-tested | wgpu owns the Vulkan device/surface; C++ creates `ContextVK` from borrowed handles plus the in-tree Impeller Vulkan shader bundle. Per-frame binary semaphores now order wgpu acquire → Impeller render → wgpu present, synchronization objects stay alive through the consuming submission, and resize waits for that submission before swapchain replacement. A repeatable 500-resize compositor stress run, including hide/restore, fullscreen, and immediate teardown, exits cleanly with no wgpu or Impeller synchronization diagnostics. |
| Linux runnable shell | Complete for rendered-frame proof | `flutter_rust_shell_runner` boots a real kernel-snapshot Flutter app; a live Hyprland capture shows the Flutter title, text field, button, and debug banner rendered in the Rust shell. |
| Pointer input | Complete for phase 1 plumbing | Winit mouse, wheel, and touch events cross the private ABI and are converted into Flutter `PointerDataPacket`s; Rust translation and C++ conversion tests pass. |
| Window and display metrics | Complete for phase 1 plumbing | Initial, resize, and scale-factor changes report physical viewport size, the real device-pixel ratio, and current-monitor size/refresh rate; zero-sized surfaces are not configured. |
| Lifecycle | Complete for phase 1 plumbing | Focus, minimize/restore, winit suspend/resume, and shutdown are deduplicated in Rust and forwarded through `flutter/lifecycle`; Rust transition and C++ ABI conversion tests pass. |
| Keyboard input | Complete for phase 1 raw events | Winit physical/logical keys, down/up/repeat, characters, modifier sides, and synthesized state cross the private ABI as Flutter `KeyData` packets. |
| Text input and IME | Complete for phase 1 plumbing | The Rust host handles the standard `flutter/textinput` protocol with typed commands and validated UTF-16 editing state, controls winit IME activation/cursor geometry, translates preedit/commit events, and sends `TextInputClient.updateEditingState` back to Flutter. Ordinary typing, Backspace, and Ctrl+A were verified interactively; a legacy `flutter/keyevent` terminator keeps Flutter's modern key-data queue moving. |
| Vsync | Complete for the Linux Wayland host | Flutter's waiter requests a winit redraw through the private ABI. Wayland `RedrawRequested` pulses are throttled by compositor frame callbacks registered immediately before actual wgpu presentation; C++ timestamps each pulse in the FML clock domain and uses the active monitor's nominal interval as its target. Non-Wayland backends retain `VsyncWaiterFallback`. |
| Multi-window | Regular and dialog windows working end to end | View `0` remains the implicit engine view. Positive-ID winit windows share one engine, root isolate, plugin registry, task runner, and wgpu device while owning independent surfaces, metrics, input, and presentation state. Flutter's experimental `WindowController`, `DialogWindowController`, and `WindowManager` APIs select the Rust owner automatically in the Rust runner. Parented dialogs use native Wayland/X11 transient relationships and are removed with their parent. Popup, tooltip, and satellite window kinds remain unsupported. |

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
  tests pass, including typed text-input decoding, invalid UTF-16 range
  rejection, Unicode selection replacement, hidden-cursor composition, and
  framework update serialization. The winit crate now has 20 passing tests,
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

## Next implementation steps (phase 1)

1. Add automated framework/host coverage for regular-window create, state,
   delegated close, and asynchronous destruction rather than relying only on
   the end-to-end compositor smoke test.
2. Implement the remaining native window kinds required by the reference app,
   starting with popup/tooltip positioning; satellite windows follow.
3. Implement the cancelable `System.requestAppExit` response round trip.
4. Complete interactive non-Latin composition checks with a configured system
   IME.
5. Add main-thread dispatch for background isolate and Rust-worker callbacks,
   and test synchronous FFI reentrancy and main-thread starvation behavior.
6. Add deterministic startup and shutdown ownership tests for the merged
   runner.
7. After windowing feature coverage is complete, replace the broad
   `cfg(not(test))` callback guards with an injected shell/view-operation
   interface so Cargo tests can exercise the real create/destroy callback
   paths using a fake C++ bridge.

## Constraints carried into implementation

- The Rust shell is optional; existing Flutter shells remain buildable.
- Impeller remains Flutter's renderer permanently.
- The public Flutter Embedder API is not used.
- Multi-window means Flutter multi-view: one engine/root isolate per
  application, never one engine per native window.
- Plugin-facing GPU types come from the semantically versioned
  `flutter-plugin-sdk` crate.
- `flutter_rust_shell_runner`'s `main.cc` is a phase 0 expedient (GN owns the
  final link so it can pull in the Dart VM/Impeller/Skia archives); it is not
  the tooling model generated applications will use. See "Who owns the final
  link" in `flutter-rs.md`.
