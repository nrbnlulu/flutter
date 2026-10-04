# Native multi-view lifecycle regression

Run `task test-rust-shell-windowing` from the Flutter checkout in an active
desktop session with `VK_LAYER_KHRONOS_validation` installed.

The fixture continuously animates the implicit view while creating three
regular windows at a time, resizing them, and removing them. It waits for
native destruction callbacks before beginning the next cycle. After 60
removals it exits with three additional views alive, testing whole-shell
teardown as well. The Python harness requires completion, ongoing primary
presentation, a clean exit, and no Vulkan or framework diagnostics.

## Sustained performance evidence

Run `task benchmark-rust-shell-windowing -- --label=baseline` in an active
desktop session. Use `--output-dir=/absolute/new-directory` to choose where to
retain evidence; otherwise a persistent temporary directory is printed.
Unlike the lifecycle test, this task builds `lib/benchmark.dart`. Both tasks
rebuild their own entry point, so running the Python scripts directly requires
the matching bundle. The task currently builds a debug Dart bundle and host
runner; its results must not be described as release performance.

The fixture animates 1, 4, 5, 8, and 12 child windows plus the implicit view.
Every stage warms up for two seconds, measures six seconds, and allows one
second for batched engine timing delivery. Timings are filtered by build-start
timestamps, so warm-up and drain frames are excluded. All 30 child windows must
be removed successfully. Missing timing data, stalled views, incomplete stages,
timeouts, crashes, or Vulkan/framework errors fail the run. Low FPS is reported
as evidence rather than rejected against a machine-independent threshold.

Pass `--workload=implicit-only` after Task's `--` separator to animate the
implicit view while keeping the child views static. The default `all` workload
animates every view. Comparing the workloads distinguishes unavoidable
per-view presentation cost from recompositing unchanged views.
Static children may correctly produce zero submissions during measurement;
the implicit view must continue presenting in both workloads.

Each run retains:

- `metadata.json`: commit/worktree state, runner path/mtime, experiment label,
  monitor configuration, GPU/driver identity, and relevant validation settings.
- `runner.log`: complete engine output, including the VM service URL while live.
- `status` and `status.timings-*.json`: stage boundaries and raw Flutter timings.
- `presentations*`: the implicit and per-child presentation counters.
  Columns are count, width, height, total acquire microseconds, swapchain acquire
  microseconds, total present handoff microseconds, and queue present microseconds.
  Acquire includes configuration, retirement and the wgpu acquire submission;
  handoff includes encoding/submission and presentation. These durations overlap
  (swapchain is part of acquire, present is part of handoff), so do not sum them.
- `samples.jsonl`: interval process/thread CPU accounting and optional NVIDIA
  device-wide utilization samples. Thread IDs are kept distinct even when names
  match. Exited worker threads may be absent from stage thread totals; process
  CPU remains the authoritative total.
- `summary.json`: per-view submission rate, per-thread CPU, build/raster/queue
  duration distributions, frame-interval p50/p95/p99/max, and budget exceedance
  fractions. Failure runs retain logs and `partial-summary.json`/`failure.txt`.

Interpret build time as Dart frame construction, raster time as the entire
engine raster phase (including CPU work and waits), and raster queue time as
the gap from build completion to raster start. These measurements locate a
phase; they do not identify individual Vulkan calls or measure GPU execution
time. Frame timings describe engine frames, not individual views. Presentation
FPS counts submitted surfaces, not compositor-displayed frames.

Keep monitor refresh rates, workspace, native sizes, visibility, other GPU
workloads, and build settings consistent between runs. Requested window sizes
are 320x240, but a tiling compositor can override them. The harness records only
this process's window snapshots at each boundary and flags changes; it does not
reconfigure the desktop. Check those snapshots before comparing runs. The
implicit view also animates, so total view count is child count plus one.
CPU percentages use one core as 100%; GPU utilization includes other apps.

Use `--frame-budget-ms=8.3333` for a 120Hz comparison (default: 16.67ms), and
`--validation` for correctness investigations. Omitting `--validation` preserves
inherited/default layer settings: it does **not** guarantee validation is off.
The current presentation counters write one line per frame; instrumentation
overhead remains part of the measurement and should be held constant between
experiments. No automatic performance fix is inferred from these reports.

The GPU instance honors wgpu's standard environment flags while retaining its
debug defaults. To compare with wgpu debug/validation flags disabled, run:

```sh
WGPU_VALIDATION=0 WGPU_DEBUG=0 task benchmark-rust-shell-windowing -- --label=wgpu-flags-disabled
```

These flags do not override externally forced Vulkan layers. Keep correctness
runs separate from such profiling experiments. In the current debug workload,
disabling these flags alone did not eliminate the slowdown with eight or twelve
animated children. The dirty-view and host pacing fixes did eliminate the
static-child slowdown; the remaining all-animated limit is in the raster phase.
Queue-present and swapchain-acquire timings were small compared with total
raster duration. Broker timings exclude Flutter's rendering and its Vulkan
submissions, so they cannot attribute the remaining duration to a specific call.

Check benchmark accounting with:

```sh
python3 -m unittest discover -s engine/src/flutter/shell/platform/rust/tests -p benchmark_windowing_test.py
```
