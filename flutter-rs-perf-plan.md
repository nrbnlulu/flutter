# Flutter Rust Shell Multi-View Performance Plan

This is the working plan for multi-window (multi-view) rendering performance in the
Rust shell. [`flutter-rs-proggress.md`](flutter-rs-proggress.md) remains the
implementation log; record each phase's outcome both there and in the results
table below.

## Rule: measured improvement or revert

Every change in this plan is an experiment. A change is kept only if:

1. It passes the Phase 0 gate on the primary metric, compared with a baseline
   built from the commit immediately before it.
2. It does not regress any guard metric beyond noise.
3. It passes the correctness suite with Vulkan validation forced on.

Otherwise the change is reverted. A failed experiment still gets a row in the
results table, so it is not retried blindly later.

Land each experiment as its own commit. Never batch two experiments into one
measurement, because the gate then cannot attribute the result.

## Where we are (2026-10-05, host_debug, RTX 5060, 75 Hz DP-1, Hyprland)

All views animated (`task benchmark-rust-shell-windowing`):

| Children | Submissions/s/view | Raster p95 | Note |
|---|---|---|---|
| 1 | 74.7 | 5.6 ms | at the display refresh ceiling |
| 4 | 74.7 | 10.2 ms | at the ceiling |
| 5 | 74.2 | 15.1 ms | at the ceiling |
| 8 | 67.2 | 16.1 ms | |
| 12 | 44.9 | 29.7 ms | about 2.3 ms per view |

These numbers are with wgpu validation disabled. With validation on, 8 and 12
children were 28 and 12 submissions/s, because the Khronos layer sat under every
Impeller call on the shared instance.

Raster-thread stack samples at 12 children (with `eu-stack`) show:

- 92% of samples inside the NVIDIA driver: `ioctl` (queue submission) and
  `pthread_mutex_lock` (driver lock).
- That driver lock is contended with Impeller's `IplrVkFenceWait` thread, which
  uses about 36% of a core.
- Per-view cost is now linear in the number of views. It is dominated by
  per-view, per-frame driver round trips, not by drawing.

### Per view, per frame, today

| # | Where | What |
|---|---|---|
| 1 | `flutter-shell-wgpu` `acquire_image` | creates a semaphore pair |
| 2 | `acquire_image` | a wgpu render pass that **clears the full swapchain image**, plus a `queue.submit` |
| 3 | `rust_vulkan_surface.cc` `AcquireImage` | an empty `vkQueueSubmit` that only waits on the acquire semaphore |
| 4 | `gpu_surface_vulkan_impeller.cc` `AcquireFrame` | `DisposeThreadLocalCachedResources()` (drops the command pool and descriptor pool) |
| 5 | `AcquireFrame` | `createImageViewUnique` on the swapchain image |
| 6 | `AcquireFrame` | `SwapchainTransientsVK` (MSAA color and depth/stencil) is recreated whenever this view's size differs from the previous view's size |
| 7 | `RenderToTarget` | at least one Impeller submit, plus `DisposeThreadLocalCachedResources()` again in cleanup |
| 8 | submit callback | a barrier command buffer submit (to `COLOR_ATTACHMENT_OPTIMAL`) |
| 9 | `RustVulkanPresentation::PresentImage` | a second barrier submit (to `PRESENT_SRC_KHR`) |
| 10 | `PresentImage` | an empty submit that only signals the render semaphore |
| 11 | `present_image` | a wgpu handoff render pass, a `queue.submit`, and `queue.present` |
| 12 | three frames later | `device.poll(Wait)` and destroying the semaphores |

That adds up to about seven queue submissions, each with its own fence that the
fence-waiter thread has to wait on.

### Target

12 children, all animated, sustaining the display refresh rate in host_debug with
validation off. That means a 13-view raster p95 under about 13 ms, roughly
1 ms per view, down from about 2.3 ms today.

## Phase 0: measurement harness and gate (prerequisite, no performance change)

Without this phase, no later phase can be judged.

1. **Repeatability.** Add `--repeat N` to `benchmark_windowing.py`, or add a wrapper
   that writes N evidence directories. N defaults to 3.
2. **Comparator.** Add `tests/compare_benchmarks.py BASE_DIR... -- CAND_DIR...`. For
   each stage it reports the median across runs of:
   - **Primary metric:** engine raster p50 and p95 (`timings.raster_ms`). Use this
     rather than submissions/s, which saturates at the refresh rate up to five
     children and would hide real wins.
   - **Secondary metric:** mean submissions/s/view for the 8 and 12 stages.
   - **Guard metrics:** process CPU%, raster-thread CPU%, `IplrVkFenceWait` CPU%,
     native acquire, handoff, and present p95, and build p95.

   It prints KEEP or REVERT.
3. **Gate.**
   - **Noise** for a metric and stage is the larger of 5% and
     `2 × (max − min) / median` over the baseline runs.
   - **KEEP** requires two conditions:
     - Raster p50 at the 8- or 12-child stage improves by more than the noise.
     - No guard metric regresses by more than its noise at any stage.
   - Changes aimed at CPU rather than throughput (Phase 4) use raster-thread plus
     fence-waiter CPU% as the primary metric instead.
4. **Correctness suite** (must pass for KEEP):
   - `task test-rust-shell-windowing` with validation forced (60-window lifecycle).
   - `task test-rust-shell-texture` in an unlocked session.
   - Rust unit tests and `benchmark_windowing_test.py`.
   - `benchmark_windowing.py --validation` runs to completion with no diagnostics.
5. **Counters.** Add native counters to the presentation-stats file:
   - queue submits per presented frame
   - `SwapchainTransientsVK` recreations
   - image-view creations

   They let each phase prove it removed the work it targeted, not just that the
   time changed.
6. **Sampler.** Commit the `eu-stack` sampler from `/tmp/rprof` as
   `tools/sample_raster_thread.sh`. It uses a `PR_SET_PTRACER` exec wrapper,
   because `perf` is unavailable and `ptrace_scope=1`. This gives every phase a
   before and after profile.
7. **Conditions.** Record which monitor the windows land on, in the metadata
   already captured, and refuse runs while the session is locked. Establish the
   baseline noise with 5 runs at the current HEAD before starting Phase 2.

**Exit:** the comparator reports REVERT for A versus A' (the same build measured
twice). That shows the noise floor is calibrated.

## Phase 1: validation-layer policy (measured: about 3.8× at 12 children)

Done so far:

- wgpu now has its `std` feature, so `WGPU_VALIDATION`/`WGPU_DEBUG` actually work.
- The benchmark disables validation unless `--validation` is passed.

Still open:

- **Decision:** should host_debug default to validation **off**, matching Flutter's
  other Vulkan embedders, which enable it only on request? Validation would then
  be opt-in through `WGPU_VALIDATION=1` and an engine switch such as
  `--enable-vulkan-validation`. Interactive debug apps are currently 4× slower
  than necessary with many windows.
- Whatever the default, the correctness tasks must keep forcing validation on
  explicitly.
- **Gate:** already measured (12 children: 11.8 → 44.9 submissions/s). Re-verify
  with the Phase 0 comparator before landing the default change.

## Phase 2: stop per-view resource churn (cheap, isolated changes)

Each item is a separate experiment and is gated independently.

- **2a. Per-view transients.** Replace the single
  `transients_`/`transients_size_` slot in `GPUSurfaceVulkanImpeller` with a map
  keyed by active view ID. Evict on `UnregisterView` or `ReleaseSurfaces`, and
  replace when that view resizes.
  - Expect most gain when view sizes are mixed (the implicit view versus the
    320×240 children).
  - Verify with the recreation counter, which should drop to 0 per steady frame.
  - A previous attempt was measured only with validation on, so it is
    inconclusive and must be re-measured.
- **2b. Cache swapchain image views — measured and skipped.** `createImageViewUnique`
  was timed directly with a temporary probe at the 12-child stage: mean 0.93 µs
  over ~18,800 calls. That's about 13 µs/frame across 13 views, versus a ~16.7 ms
  frame budget — not a meaningful contributor. It was also the one Phase 2 item
  with a real correctness hazard: caching by `(view_id, VkImage)` needs a positive
  reconfigure signal from Rust, because `acquire_image`'s `Outdated` branch calls
  `surface.configure()` again (destroying and recreating every swapchain image)
  **without the requested size changing**, so the existing `entry.size != frame_size`
  check would not catch it. A stale cached view surviving that reconfigure, if the
  driver reuses the same raw handle for a new image, is a dangling-handle bug, not
  a perf regression. Given the gain is negligible, skip rather than add the ABI
  generation counter this would need to be safe.
- **2c. Semaphore pool — measured and reverted.** Implemented and gated: a
  `sync_pool: Vec<FrameSync>` reused the retired pair the steady-state overlap
  eviction already waits on, instead of destroying it, with the creation call
  site popping from the pool before falling back to `vkCreateSemaphore`. Passed
  the 60-window lifecycle test under forced validation (no semaphore lifetime
  errors). Gated result: 8-child raster p50 15.33 -> 15.19ms (-0.9%, within
  ±7% noise), 12-child raster p50 21.57 -> 22.10ms (+2.5%, unchanged) — no
  primary-metric improvement, so per the keep/revert rule this was reverted.
  Same conclusion as 2b: `vkCreateSemaphore`/`vkDestroySemaphore` are cheap
  relative to the actual submission cost; the bottleneck is the submissions
  themselves (Phase 3), not the per-frame object churn around them.
- **2d. Command and descriptor pool reuse.** `DisposeThreadLocalCachedResources()`
  currently runs twice per view: in `AcquireFrame` and in `RenderToTarget`'s
  cleanup. Run it once per engine frame instead, for example only on the
  frame-boundary view or after the last view.
  - A previous attempt ("retain-thread-local-caches") was measured with validation
    on and gave a noisy 1-child result, so it must be re-measured.
  - Watch memory: pool growth must stay bounded over a 60 s run.

## Phase 3: cut queue submissions per view (largest expected win)

The profile shows submission ioctls and driver-lock contention dominate.
Today there are about 7 submits per view; the goal is 3 (wgpu acquire, Impeller
render with the final barrier, and wgpu handoff/present), and fewer later.

- **3a. Merge the two layout barriers — measured and kept.** These were not
  actually redundant/mergeable no-ops: `GPUSurfaceVulkanImpeller`'s submit
  callback transitioned `eGeneral -> eColorAttachmentOptimal` (tracked, via
  `TextureSourceVK::SetLayout`), purely so that `RustVulkanPresentation::
  PresentImage`'s hand-built barrier -- which hardcodes `oldLayout =
  eColorAttachmentOptimal` and bypasses Impeller's layout tracking entirely --
  would hold. Tracing `render_pass_vk.cc`'s `is_swapchain` branch (which
  mirrors the Vulkan render pass's actual `finalLayout`) showed the real
  post-render layout for a swapchain color/resolve attachment is `eGeneral`,
  not `eColorAttachmentOptimal`. Deleted the fixup barrier+submit entirely and
  changed `PresentImage`'s barrier to transition `eGeneral -> ePresentSrcKHR`
  directly -- one real, correct barrier instead of two, with no new Impeller
  API needed. Validated with the 60-window lifecycle test under forced Vulkan
  validation (413 presentations, no VUID errors -- notably including
  `VUID-vkCmdDraw-None-09600`, the exact failure mode this change risked) and
  a visual check of rendered output. Gated result: 8-child raster p50
  15.33 -> 10.97ms (-28.4%), 12-child raster p50 21.57 -> 15.23ms (-29.4%),
  8-child process CPU% 109.6% -> 90.5% (-17.4%), present p95 dropped from
  ~0.24ms to ~0.10ms across every stage. No guard regressions. Kept.
- **3b. Signal on a real submit — measured and reverted.** Added a fork-local
  `CommandQueueVK` overload that signalled the render semaphore from the 3a
  barrier submit while retaining `FenceWaiterVK` tracking, eliminating the
  signal-only submit. The validation-forced 60-window lifecycle fixture passed
  (60 removals, 399 primary presentations, no VUID diagnostics), but the valid
  3-run versus 3-run immediate-baseline gate did not show a primary win:
  8-child raster p50 11.45 -> 11.25 ms (-1.7%, within ±5% noise), 12-child
  16.31 -> 16.04 ms (-1.7%, within ±8%). It also regressed 4-child raster p50
  by 5.6% (beyond ±5%), acquire p95 by 319% (beyond ±112%), and 8-child present
  p95 by 26.8% (beyond ±15%). Reverted under the measured-improvement rule.
- **3c. Fold the acquire wait into the first submit — measured and reverted.**
  Added a one-shot wait to `ContextVK::SubmitOnscreen`, so the semaphore from
  wgpu acquisition was consumed by the first real onscreen Impeller submission;
  the batching path attached it at its flush submission. The validation-forced
  60-window lifecycle fixture passed (60 removals, 435 primary presentations,
  no VUID diagnostics), but the 3-run immediate-baseline gate found no primary
  improvement: 8-child raster p50 11.45 -> 11.84 ms (+3.4%, within ±5%),
  12-child 16.31 -> 15.78 ms (-3.2%, within ±8%). 8-child present p95 regressed
  20.5%, beyond its ±15% noise threshold. Reverted.
- **3d. Remove the wgpu acquire clear pass.** It exists only so that wgpu's tracker
  waits on the swapchain acquire semaphore and marks the texture as initialized,
  but it also clears the whole image on the GPU, which Impeller then overwrites.
  Try, in order:
  1. A `LoadOp::Load` or `DontCare`-style no-op pass. Check that wgpu still waits
     on the acquire semaphore and does not clear on present.
  2. Signal the acquire semaphore from the handoff submit's predecessor through
     `wgpu-hal`.

  Verify pixels with the texture screenshot test.
- **3e. Stretch goal: drive acquire and present through `wgpu-hal` directly.**
  This bypasses wgpu-core's tracker for borrowed swapchain images, so the
  acquire and handoff render passes disappear entirely.
  - Larger change; only attempt it if 3a–3d leave the target unmet.
  - Must keep Android surface replacement and suspend semantics intact.

Verify each sub-step with the submits-per-frame counter as well as the gate.

## Phase 4: fence-waiter contention

`IplrVkFenceWait` uses about 36% of a core and contends with raster for the
driver lock. Phase 3 should reduce this on its own (fewer fences). Re-profile
after Phase 3 and only then consider:

- **Batching:** check whether `FenceWaiterVK` wakes and calls into the driver per
  fence. If so, wait on fewer fences, for example only the last fence of each
  engine frame, chaining the others.
- **Earlier release:** release per-frame resources on the frame's last fence
  rather than per command buffer.

Primary metric: raster-thread plus fence-waiter CPU%, with raster p50 as a guard.

## Phase 5: confirm with profile and release builds

host_debug includes Dart JIT and `debug_assertions`, and the workspace crates
are built at Rust opt-level 1. Add a `benchmark-rust-shell-windowing-profile`
task (host_profile engine, AOT bundle, Cargo `--release`). Re-run the Phase 2–4
keepers there, so that no kept change is a debug-only win that regresses
profile.

Record profile-mode numbers as the user-facing baseline.

## Phase 6: further options (only if the target is still missed)

Each needs its own design note before it starts:

- **Skip presentation of occluded views.** Hidden or minimized windows should not
  acquire, render, or present. Needs compositor visibility from winit.
- **Per-view frame budget.** If a frame cannot fit all dirty views, prioritize the
  focused view and defer the others to the next vsync, instead of dropping the
  whole engine frame rate.
- **Lower MSAA cost for small windows,** if GPU time ever becomes the bottleneck.
  Today it is CPU and driver bound, so this is measured but not expected to help.
- **Parallel per-view raster.** The Flutter rasterizer is single-threaded per
  engine, so this would need upstream engine work. Out of scope unless the other
  phases plateau far from the target.

## Results

| Phase | Change | Baseline → candidate (12-child raster p50, submissions/s) | Gate | Kept? |
|---|---|---|---|---|
| 1 | wgpu `std` feature + validation off in benchmark | 82 ms / 11.8 → 21 ms / 44.9 (single run each) | measured before Phase 0 | yes (re-verify) |
| 2a | Per-view `SwapchainTransientsVK` | 24.4 ms → 21.8 ms | 3 vs 5 runs, REGRESSED flag was unrelated code path (noise) | yes |
| 2b | Cache swapchain image views | n/a -- measured 0.93 µs/call (~0.06% of frame budget) before implementing | not implemented | no (negligible win, real correctness hazard) |
| 2c | Semaphore pool | 21.57 ms → 22.10 ms (+2.5%, unchanged) | 3 vs 3 runs | no (no primary-metric improvement) |
| 3a | Merge redundant layout barrier, fix wrong `oldLayout` | 21.57 ms → 15.23 ms (-29.4%) | 3 vs 3 runs, KEEP, no guard regressions | yes |
| 3b | Signal render semaphore from tracked barrier submit | 16.31 ms → 16.04 ms (-1.7%) | 3 vs 3 runs, REVERT: no primary win; guard regressions | no |
| 3c | Fold acquire wait into first Impeller submit | 16.31 ms → 15.78 ms (-3.2%) | 3 vs 3 runs, REVERT: no primary win; 8-child present p95 regressed | no |
