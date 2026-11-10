# Display-free Rust host

Phase 1 is implemented on Linux x64. Build a matching ABI 15 engine and current
runner sources, then use:

```sh
rlutter run -d linux --headless --size 1280x800 --pixel-ratio 1
```

`--size` is the initial physical viewport; application window APIs and input use
logical pixels. The CLI removes display variables from the child environment.
The host itself can run with those variables present: it selects this adapter
before creating any winit event loop, native window, clipboard or display handle.
Normal launches retain the native host. No agent endpoint is opened by this mode.

The implementation is an adapter in `flutter-shell-winit/src/headless.rs`, using
the same private engine dispatch, task queue, SDK dispatcher, texture backend,
registration entry point, text-editing state and application-exit handshake as
the native host. The host-event sender now supports a condition variable, with
the queue check and sleep under the producer mutex to prevent lost wakeups.
Event and task dispatch use bounded batches. Engine-wide frame pulses are armed
only on demand and use a monotonic 60 Hz clock. Real Dart timers remain real.

One shared Vulkan device and engine/root isolate serve all virtual views. The
registry tracks view identity, kind, parent, title, bounds, scale, constraints,
focus and asynchronous lifecycle. The implicit view retains its special lifetime.
Add/remove completion keeps callback owners alive through raster detach; closing
an owner removes descendants, and satellites can be reparented without cycles.
Tooltip/popup anchors reuse the pure native placement code, including negative
origins and partial anchor updates. Shrink-wrapped views report their actual
rendered dimensions without reading GPU pixels. Maximize/fullscreen use a
synthetic 1920×1080 physical desktop, with restoration of previous bounds.

Virtual clipboard contents stay in the session. Unsupported native services
retain explicit unhandled/error results; the host does not open a native dialog
or report a successful OS action. Text editing consumes the shared client/session
commands. Agent input injection and semantics arrive in Phase 2.

Rendering uses the Phase 0 owned-image contract. Ordinary headless runs disable
readback; no per-frame mapping or image encoding occurs. The current synchronous
probe readback remains available for explicitly requested test artifacts. Later
agent capture must add asynchronous completion and observation constraints.

## Verification

`tests/test_headless_host.py` runs the existing native fixture bundles with
both display variables absent, Vulkan validation enabled and a retained connect
trace. Build `tests/smoke_runner` against the local engine, and build
`tests/windowing_fixture` with `FLUTTER_WINDOWING=true` and matching local-engine
Flutter flags. Use separate asset directories for `main.dart`, `owned.dart` and
`tooltips.dart` (`flutter build bundle -t ... --asset-dir ...`). For example,
from the Rust workspace:

```sh
python3 tests/test_headless_host.py \
  --runner target/debug/flutter-rust-shell-smoke \
  --assets tests/windowing_fixture/build/tooltip_assets \
  --icu /absolute/path/to/icudtl.dat \
  --strace /absolute/path/to/strace \
  --tooltip-motion --artifacts target/offscreen-probe/new-tooltip-run
```

The output directory must be fresh. `--plugin` uses `tests/headless_fixture` and
a smoke-runner registration entry point that asserts exactly one SDK registration,
creates/presents a pixel texture, and checks rendered pixels across captures.
`--idle` uses a static fixture and measures the owning UI thread after startup;
its intentional termination is separate from the graceful-shutdown fixtures.
`--icd`/`--driver-library-dir` select a software Vulkan candidate.

Test-only environment options support fixture setup and diagnosis:
`FLUTTER_RUST_HEADLESS=1`, logical `FLUTTER_RUST_HEADLESS_WIDTH/HEIGHT`,
`FLUTTER_RUST_HEADLESS_PIXEL_RATIO`, `FLUTTER_RUST_HEADLESS_ARTIFACTS` (explicit
continuous raw capture), and `FLUTTER_RUST_HEADLESS_STATE` (window state JSON).
They do not create a control transport or replace the later agent protocol.

Evidence on 2026-11-09:

- Existing lifecycle fixture: 60 regular views created, resized and removed;
  clean whole-engine shutdown with additional views alive.
- Owned fixture: 60 removals across parent/dialog/satellite cascades and reparenting.
- Tooltip fixture: 60 removals; exact resize/anchor/partial-update bounds checked,
  including negative origins. Observed coordinates: (-60,-20), (-33,4), (-43,-3).
- SDK plugin fixture passed in fresh processes, registered once and rendered
  exact texture pixels; teardown reclaimed its GPU resources.
- llvmpipe also passed the 60-window lifecycle fixture.
- Each retained fixture trace had zero X11/Wayland socket connections and clean
  Vulkan validation logs.
- Static owning UI thread consumed 0 CPU ticks over the measured idle second.
  This establishes sleeping behavior on this machine, not a performance budget.
- A freshly generated project passed `rlutter run --headless -d linux --size
  801x601 --pixel-ratio 1.25`, debugger attach and application-initiated exit.
  Its virtual bounds were 640.8×480.8 logical pixels at DPR 1.25.

Local artifacts are under `target/offscreen-probe/phase1-*`; the generated CLI
project and log are at `/tmp/flutter-rs-headless-cli/`. These build outputs are
not committed. Native desktop coverage remains necessary for OS behavior.

The current agent boundary is ABI 16 (Phase 2). Phase 1's ABI 15 is historical;
current source and packaged runners enforce the engine-compiled ABI exactly.
