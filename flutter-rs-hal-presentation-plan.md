# Phase 3e: presentation below wgpu-core

Status: investigated, implementation not started. This design uses the pinned
wgpu revision `014d9e84813a2946febfa4888694c0b70565b2f5`. Performance decisions
remain subject to the gate in [the performance plan](flutter-rs-perf-plan.md).
Measurement prerequisites are complete: the texture harness passes, all three
resource/submission counters and CPU guards are recorded, and the controlled
five-run baseline passes the independent A/A calibration. Uncontrolled tiled
evidence was discarded after finding mixed refresh rates and tiny child sizes.
See the performance plan and implementation log for the current baseline and
its 7.2% primary noise at 12 children.

## Findings that change the approach

- `wgpu-core/src/present.rs` imports acquired images as `UNINITIALIZED`.
  `command/render.rs::add_pass_texture_init_actions` marks `LoadOp::Load` as
  requiring initialized memory. Switching Clear to Load therefore does not
  establish that GPU clearing disappeared: core can insert initialization.
  The earlier Phase 3d experiment did not prove removal of this work.
- HAL Vulkan `SurfaceTexture` hides its image index and semaphore metadata.
  `Queue::submit` consumes private acquire semaphores, allocates/signals private
  present semaphores, and updates the acquire fence value. `Queue::present`
  depends on that state. Raw Impeller submission cannot simply replace these
  HAL submissions using the public API.
- HAL submit also advances queue-wide relay semaphores shared with wgpu plugin
  submissions. External synchronization of the raw queue and GPU ordering are
  separate requirements; both must remain explicit.
- HAL native `discard_texture` is currently a no-op. Dropping an acquired image
  is not a complete cancellation protocol.
- The current `submits=N` instrumentation counts only the two Rust presentation
  submits, not C++ acquire/render/barrier/signal submits. It cannot verify the
  end-to-end submission reduction claimed by the original plan.
- The observed texture-test exit 7 included a Hyprland Lua dispatch syntax
  error. The previous conclusion that the window had disappeared was not
  supported. Suppressing that error then left the runner running. The test
  failure does not establish a Phase 3d rendering regression.

## Prerequisites

1. Fix the texture harness command compatibility in a separate change. Inspect
   local compositor command help; close only the fixture's exact window. Keep
   exit-status checking, validate pixel changes and clean process exit, and
   preserve logs/screenshots on failure. Do not use the temporary wrapper that
   suppresses exit 7. Verify the unchanged baseline passes first.
2. Complete measurement tooling before another acceptance decision: count all
   relevant Rust and C++ submissions separately, report their sum, and include
   raster/fence-waiter CPU guards. Reject incomplete evidence. Record monitor,
   geometry, refresh, build identity and lock state; refuse locked sessions.
3. Build and retain an immutable baseline runner from the immediate predecessor
   commit. Capture five baseline runs and an A/A comparison. The previous
   three-run datasets have very large noise at several stages and should not
   be reused as proof for this larger change. Keep fixture bundles separate.

## Experiment A: HAL surface with semaphore bridges

Remove wgpu-core's ownership of presentation images while retaining HAL's
swapchain and semaphore machinery. Keep the wgpu instance/device/queue and
public plugin texture API. Create an owned HAL surface through the existing
HAL instance with the retained native window handles; do not configure one
surface through both core and HAL. Check adapter support and preserve format,
present mode, alpha mode, extent and frame-latency policy.

Per-view frame sequence:

1. HAL acquire returns an owned HAL surface texture. Borrow its raw image for
   Impeller; do not import it into wgpu-core or create a wgpu texture view.
2. Stage the broker acquire signal and submit an empty HAL command-buffer list
   with this surface texture. HAL consumes the WSI acquire semaphore and updates
   its own synchronization state. This is still a queue submission, but has no
   render pass, clear, or core tracker work.
3. Preserve C++'s acquire wait and Impeller rendering for this experiment.
   Audit the first actual image access, including offscreen/readback rendering,
   and establish the correct initial layout in Impeller. Fresh acquisitions may
   discard old contents via UNDEFINED only when the render path fully defines
   the presented image. Do not assume the old acquire-pass layout remains valid.
4. Preserve the GENERAL-to-PRESENT barrier and C++ render signal.
5. Stage the render wait, submit an empty HAL command-buffer list with the same
   surface texture, then HAL-present it. HAL signals the presentation semaphore.
   The image is already in PRESENT_SRC_KHR; there is no handoff render pass.

Use a broker-owned HAL fence and monotonically increasing values consistently
for acquire and both bridge submits. Never invent wgpu SubmissionIndex values
for HAL work or depend on device.poll to retire it. Retire broker semaphores
after the HAL fence proves their consuming submissions completed. HAL continues
to own its WSI semaphore reuse. Bound outstanding frames; no steady-state idle
wait. Fence ownership/domain must not be mixed between views or with core's
private fence.

Queue submit/present and staged semaphore add/remove must remain serialized on
the raster thread, including plugin submissions. Hold HAL guards only for their
documented lifetime and avoid core calls while holding guards that could lock
the same internals. On failure remove unconsumed staged waits/signals and retire
the acquired generation rather than allowing a later unrelated submit to
consume them.

Expected result: two render passes removed, zero wgpu-core presentation imports,
but **no reduction in the two Rust bridge submissions**. Measure this independently;
do not count it as achieving the original three-submit target. Revert if the
primary metric fails the gate.

## Experiment B: eliminate the semaphore bridges, only if still necessary

Public HAL APIs cannot expose the WSI acquire/present synchronization needed for
this step. Recommended design to evaluate: own a native Vulkan swapchain in the
shell using a HAL-created native surface and the existing borrowed device/queue.
Keep the HAL surface unconfigured. This avoids mutating HAL's private swapchain
bookkeeping. It is a larger Vulkan-specific implementation, not a small HAL call
replacement. Alternative: a pinned wgpu fork exposing a complete external-submit
transaction (semaphores, fence registration and failure rollback), not raw
semaphore getters. Choose explicitly before implementing B.

For the shell-owned option, vkAcquireNextImageKHR signals a shell acquire
semaphore; C++ can initially retain its wait-only submission. The final tracked
presentation barrier should eventually signal a per-image present semaphore,
and vkQueuePresentKHR waits directly on it. Keep each synchronization optimization
separately measured where feasible; do not silently reintroduce the rejected
3b/3c implementations as prerequisites.

Use per-frame acquire reuse only after its consuming submission completes.
Use per-image present semaphores; a render fence alone does not prove the
presentation engine consumed a present wait. Establish reuse through image
reacquisition or supported presentation completion facilities. Keep swapchain
generation, image index and frame token in the private ABI if C++ needs them;
bump both Rust/C++ ABI definitions together. No cache may identify an image
solely by its raw handle across generations.

## Lifecycle and failure contract for either experiment

- One outstanding acquired frame per view, with explicit acquired/submitted/
  presented-or-abandoned states. Include an abort path for failures after acquire
  but before C++ installs or presents the frame; do not leave active_frame or a
  pending semaphore attached to the next view.
- Defer resize to raster and retire the old generation before reconfigure.
  Outdated at the same size also creates a new generation. Bound acquire retries;
  surface loss and device loss must terminate or recover explicitly.
- Zero-sized and suspended views do not acquire. Android replacement preserves
  broker address and shared device; cancel/retire old frames before releasing
  the old surface/window. Never present against a destroyed native window.
- Unregister and whole-shell teardown release GPU surfaces on raster before
  acknowledging native window destruction. Idle waits are acceptable during
  exceptional retirement, resize or teardown, not as a per-frame shortcut.
- An abandoned HAL acquisition requires a deliberate generation-retirement path
  because discard is a no-op. Prove WSI and GPU completion before destroying
  old resources; preserve HAL's own retirement protections in experiment A.

## Implementation map and acceptance

`flutter-shell-wgpu/src/lib.rs`: Presentable, SurfaceState, PendingFrame,
RetiredFrame, configure/acquire/present, suspension and retirement. Factor an
internal Vulkan presentation module if needed; preserve the plugin-facing API.
`rust_vulkan_surface.{h,cc}` and `rust_bridge.h`/Rust ABI mirror: frame token,
abort and completion seams as required. `gpu_surface_vulkan_impeller.cc`: verify
initial layout/full-image definition and propagate abandoned-frame cleanup.

Before KEEP: all Rust and benchmark-accounting tests; native 60-window lifecycle
with validation forced; texture screenshot and CPU pixel-buffer tests in an
unlocked session; full benchmark with validation and no diagnostics. Add focused
failure-path tests for acquire failure, render/submit failure, same-size Outdated,
suspend with an acquired frame, and unregister with in-flight frames. Exercise
Android suspend/replacement on a device before claiming Android coverage; if
unavailable, retain the existing Android backend and document Linux-only scope.

Run candidate performance repetitions with validation off against the immutable
baseline, with no concurrent builds. Compare all specified guards and primary
metrics; verify removed work with counters and a before/after raster profile.
Run a 60-second bounded-resource check. Record results in both existing logs and
commit each accepted experiment separately. The immediate next implementation
work is experiment A, using the frozen controlled baseline and bundle. Preserve
the concurrent diagnostic edits outside this experiment's commit and make build
provenance explicit before comparing another runner.
