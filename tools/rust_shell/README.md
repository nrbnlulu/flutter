# Rust shell tasks

`Taskfile.yml` delegates to `tools/rust_shell/tasks.py`. The Python runner
owns environment setup, argument lists, builds, fixture bundles, process
lifetimes, and assertions. It does not require Bash, jq, sed, or pgrep.
`DEPOT_TOOLS` and `DEPOT_PY_BIN` remain optional environment overrides.

Native automation has a common API in
`engine/src/flutter/shell/platform/rust/tests/native_desktop.py`:

- Hyprland: window discovery, compositor commands, and grim captures.
- Windows: PID-scoped HWND discovery, native resize/minimize/restore/close,
  keyboard input, mouse-driven live resize, and deterministic GDI PNG captures.

Benchmark measurements use `benchmark_native.py` and `benchmark_linux.py`.
Windows collects native process/thread CPU times, thread descriptions,
monitor geometry, and unlocked-desktop evidence. The shared benchmark retains
its complete-stage, frame-liveness, geometry, and timing-accounting checks.
Unsupported hosts fail explicitly; macOS and mobile desktop automation are
not implemented. Android build/APK tasks remain available from supported hosts.

Examples (the same commands on Linux and Windows):

```
task build-rust-shell
task test-rust-shell
task test-rust-shell-gpu
task test-rust-shell-lifecycle LIFECYCLE_ITERATIONS=20
task stress-rust-shell RESIZE_ITERATIONS=500
task test-rust-shell-frb-dispatch
```

Python can also be invoked directly, using `python3` on Linux:

```
python tools/rust_shell/tasks.py --help
python tools/rust_shell/tasks.py build
python tools/rust_shell/tasks.py test
```

Override `PYTHON` when Task cannot find the desired interpreter. Override
`LOCAL_ENGINE`, sample/example paths, and iteration counts using Task variables.
Lifecycle and stress tasks default to the checked-in counter/text-field smoke
fixture instead of a machine-specific external project. `TARGET` still selects
the multiple-windows entry point; benchmark arguments follow Task's `--`.

On Windows the build uses Visual Studio 2022 Build Tools by default when found;
standard `GYP_MSVS_*` overrides take precedence. A generated build-local
`python3.cmd` points Dart SDK helper scripts at the running Python interpreter,
avoiding the Microsoft Store alias without changing system configuration.
The DLL, import library, Dart SDK and frontend server come from the matching GN
output. Cargo links/stages the smoke executable and DLL beside each other.
Windows Rust unit tests explicitly link the engine import library; C++ tests
use the GN target as before.

GPU tests require a working Vulkan driver and the Khronos validation layer.
The four hardware tests cover uploads, shutdown and an empty presentation pass,
and exclude the manual benchmark. One upload test catches
intentional producer panics while verifying that reserved slots are recovered.
The Intel HD Graphics 530 empty-pool driver workaround is documented in the
vendored HAL's `README.flutter` and the progress log.

Runtime tests select only windows owned by their runner PID, require an initial
presentation, and require status zero on native close. Stress additionally
requires new frames after Tab input and a changed-size frame after the resize
burst, exercises minimize/restore and maximization, checks presentations during
Win32 mouse-driven resizing, and scans logs for GPU/runtime diagnostics.
Logs and presentation statistics are retained in temporary evidence directories,
including on failure; successful adapter unit tests clean up their windows.

Interactive input needs an unlocked desktop and Windows foreground permission.
The modal-drag adapter unit test reports a skip when that permission is absent;
the actual stress task fails if it cannot perform its input/liveness checks.
A process staying alive or a test being skipped is never counted as a render
or input pass. The full benchmark requires a sufficiently large monitor for its
thirteen non-overlapping windows and rejects smaller displays explicitly.
