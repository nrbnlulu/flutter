# Offscreen rendering feasibility

Phase 0 is complete on Linux x64. The wgpu baseline and private-engine probe
both pass on hardware Vulkan and llvmpipe, with clean validation and display
socket traces. This is a synchronous rendering feasibility probe, not the agent
runtime or a pipelined production capture implementation.

Run from `engine/src/flutter/shell/platform/rust`:

```sh
cargo test -p flutter-shell-wgpu --lib
cargo build -p flutter-shell-wgpu --example offscreen_probe
mkdir -p target/offscreen-probe/hardware
env -u DISPLAY -u WAYLAND_DISPLAY WGPU_VALIDATION=1 WGPU_DEBUG=1 \
  target/debug/examples/offscreen_probe target/offscreen-probe/hardware \
  >target/offscreen-probe/hardware/stdout.log \
  2>target/offscreen-probe/hardware/stderr.log
```

Use a separate directory and `VK_DRIVER_FILES=/absolute/path/to/software_icd.json`
for a software Vulkan candidate. Adapter selection must be confirmed in
`adapter.txt`; passing on a hardware adapter does not establish software support.
Install the Khronos validation layer before claiming validation coverage. Inspect
stderr even on exit code zero: Vulkan-layer diagnostics are independent of wgpu
error scopes. A passing marker describes this baseline only.

The example creates no event loop, surface or window. It rejects display
environment variables, uses `GpuContext::headless`, renders a shader pattern,
and compares every RGBA byte across twelve captures. Three target allocations
(257×129, 131×67, 257×129) each undergo four render/copy/map cycles; an alternating
right column detects stale captures. This exercises channel order, alpha,
orientation, odd-width row padding, staging reuse and target recreation.
It does not test live Flutter resize or resources still in flight on removal.

Artifacts include the adapter/driver fingerprint, tightly packed linear
`Rgba8Unorm` bytes and PPM previews. Raw bytes preserve alpha; PPM previews omit
alpha and contain the unconverted linear RGB values. These baseline artifacts are diagnostic. The engine harness additionally emits
straight-alpha linear PNGs (gAMA=1.0), unpremultiplying the engine's raw pixels. Copy completion
and mapping are required before CPU access. GPU waits have a timeout and occur
after releasing the shared queue lock. The helper owns one staging buffer per
target and never continuously reads frames.

## Source audit: native presentation assumptions

| Location | Current assumption | Offscreen implication |
| --- | --- | --- |
| `GpuContext::create` in `crates/flutter-shell-wgpu/src/lib.rs` | Vulkan instance/device creation accepts a source without a surface. Adapter selection requests high performance, not forced fallback. | This is device creation only. Record the actual selected adapter. |
| `GpuBroker` / `Presentable` | Holds a winit window and optional surface, uses surface capabilities/configuration, acquire retries and native redraws. | Use a separate owned-image broker; do not manufacture a hidden window. |
| `GpuBroker::acquire_image` | Acquires `SurfaceTexture`; clear submission initializes wgpu's swapchain state and signals an acquire semaphore. | Owned targets have no WSI acquire semaphore. Establish initialization and tracked state explicitly. |
| `RustVulkanPresentation::AcquireImage` in `cpp/rust_vulkan_surface.cc` | Requires two nonzero binary semaphore handles, consumes the acquire signal on the graphics queue. | A new broker must honor this synchronization contract or change it explicitly. |
| `WrapTextureSourceVK` / `TextureSourceVK` in Impeller | Borrowed image gets a fresh wrapper whose layout starts undefined. | Initial discard, target reuse and actual image state need validation; no preserved content may be assumed. |
| `GPUSurfaceVulkanImpeller::AcquireFrame` in `shell/gpu/gpu_surface_vulkan_impeller.cc` | Wraps borrowed image, caches MSAA/depth/stencil transients per view/size and uses `WrapSwapchainImage`. | Audit usage/format and image lifetime, and invalidate per-view caches on removal. The name alone does not prove WSI is required. |
| Same file, Windows Intel 1912 path | Renders into a cached owned target, copies to borrowed image, transitions through transfer layouts. | An offscreen mode must deliberately preserve/bypass this workaround; Linux proof says nothing about it. |
| Same file, submit callback | Transitions the wrapped image to `COLOR_ATTACHMENT_OPTIMAL` before the delegate callback. | This is the bridge's current input layout. |
| `RustVulkanPresentation::PresentImage` | Unconditionally transitions to `PRESENT_SRC_KHR`, then signals render semaphore. | Illegal for ordinary owned images. Introduce an opt-in presentation mode/final layout in the paired ABI before sharing this path. |
| `GpuBroker::present_image` | Waits on render semaphore in a wgpu load pass, calls native `pre_present_notify` and queue present; recycles semaphore pairs after submission completion. | Replace WSI with completed copy/fence tracking; callback success alone is not completion. |
| `release_surface`, retired frames and `CollectView` | Raster-side detach releases native resources and cached per-view transients. | Owned images/captures need equivalent retirement before reuse/destruction. |
| Impeller `CapabilitiesVK` | Capability selection includes surface/swapchain extension requirements even for imported devices. | Test actual private-context creation on each driver before claiming Impeller support. Extension availability does not require opening a display. |

ABI 15 adds a per-view presentation mode to the paired callback tables and an
engine-compiled version getter. Current native and probe hosts query it before
passing versioned shell callbacks. Mode 0 preserves native behavior; mode 1
leaves the output in COLOR_ATTACHMENT_OPTIMAL. Unknown modes are rejected.

## Validated ownership and layout contract

1. Wgpu owns an ordinary Rgba8Unorm image with RENDER_ATTACHMENT | COPY_SRC
   usage and a single padded MAP_READ staging buffer per probe target.
2. A wgpu clear submission initializes the image in COLOR_TARGET state
   (Vulkan COLOR_ATTACHMENT_OPTIMAL) and signals the broker's acquire semaphore.
3. C++ consumes that signal before Impeller accesses the image. Impeller wraps
   it with an initially undefined layout, discarding prior contents, renders
   through the existing MSAA/depth/stencil path and restores
   COLOR_ATTACHMENT_OPTIMAL in GPUSurfaceVulkanImpeller.
4. Offscreen presentation skips the WSI-only PRESENT_SRC_KHR transition and
   signals the render semaphore. Safe wgpu operations are prohibited throughout
   this borrow. The final actual layout now matches wgpu's tracked COLOR_TARGET.
5. A shared-queue submission waits on render at ALL_COMMANDS. The subsequent
   wgpu copy transitions the tracked image to COPY_SRC. CPU access waits for
   submission and mapping completion, outside the engine queue lock.
6. The next clear transitions from COPY_SRC back to COLOR_TARGET. The probe
   serializes these operations on raster, reuses its two binary semaphores only
   after copy completion, and retires targets on raster before destroying them.
   Per-target device-idle retirement is acceptable for this spike; a production
   bounded target pool must use completion-based retirement instead.
7. Completed owned bytes cross a two-frame bounded channel. Full queues discard
   probe frames without blocking raster; sequence numbers retain those gaps.
   No UI-thread encoding, mapping or GPU wait occurs in the render path.

Transport version 1 will use local, session-owned NDJSON requests with
`id`, `method`, `params` and typed errors, on a private channel separate from
application output. The frame-correlation approach is a UI build sequence
propagated with the layer tree to raster, separately from completed GPU-frame
and size-generation counters. A screenshot after an assertion must constrain
that UI build sequence; the current probe's GPU counter alone cannot satisfy
such a request. That hook is part of Phase 2, before exposing agent capture.

## Reproduce the engine probe

From the Rust workspace (with matching host_debug artifacts):

```sh
FLUTTER_RUST_ENGINE_DIR=/absolute/path/to/engine/src/out/host_debug \
CARGO_TARGET_DIR="$PWD/target" \
  cargo build --manifest-path tests/headless_probe/Cargo.toml
```

Build `tests/headless_fixture` using `flutter --local-engine-src-path=...`
`--local-engine=host_debug --local-engine-host=host_debug build bundle`.
Then, from the Rust workspace:

```sh
python3 tests/test_headless_probe.py \
  --runner target/debug/flutter-rust-headless-probe \
  --assets tests/headless_fixture/build/flutter_assets \
  --icu /absolute/path/to/icudtl.dat \
  --strace /absolute/path/to/strace \
  --artifacts target/offscreen-probe/new-hardware-run
```

The artifacts directory must be fresh. For software add `--icd` and, if its
shared library is outside the system search path, `--driver-library-dir`.
No Python packages are required. The harness runs with a process deadline,
checks exit and validation logs, rejects X11/Wayland connect attempts, and
writes PNGs, a report and the complete logs/trace.

The fixture renders six channel/orientation stripes, a half-alpha patch, an
animated corner marker and a real external wgpu texture. The probe checks four
frames per target at 801×601/DPR 1, 1001×751/DPR 1.25, then 801×601/DPR 1.
Tight physical layout constraints match the native host; the ordinary viewport
size alone is insufficient on this fork. Texture unregistration releases GPU
resources on raster before shell, task runner and callback-owner teardown.

## Evidence, 2026-11-09

- NVIDIA RTX 5060 / driver 615.78.08: twelve Flutter frames passed, including
  exact background, six stripes, texture pixels, premultiplied alpha, changing
  animation marker, target reuse and resize/generation assertions.
- llvmpipe / Mesa 26.2.5 / LLVM 23.1.3: the same twelve-frame gate passed.
- Both runs used WGPU_VALIDATION=1, WGPU_DEBUG=1 and the installed Khronos layer;
  retained logs have no validation diagnostics. `strace -f -e trace=connect`
  recorded zero X11/Wayland socket connections. Both display variables were unset.
- Engine C++ tests: 17 passed. Rust core: 1 passed. Wgpu: 6 passed, 5 existing
  manual GPU tests ignored. Native winit: 52 passed.
- Rebuilt native runner and animated external-texture fixture passed on the
  native WSI path (17 presentations).
- Local evidence is under `target/offscreen-probe/flutter-hardware-final/`
  and `target/offscreen-probe/flutter-software-final/`. Wgpu-only baseline
  evidence is under `hardware/` and `software/` in the same build directory.
  Build outputs and locally extracted validation tools are intentionally untracked.

Phase 1 must replace the probe-only startup with the shared runtime and virtual
window host. Phase 2 must add on-demand bounded capture, asynchronous completion
and the UI/raster correlation hook before offering observation-constrained
screenshots. Windows and Android behavior are not certified by this Linux gate.
