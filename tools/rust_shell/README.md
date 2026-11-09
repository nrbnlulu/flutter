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

Text/clipboard/current-scale viewport validation is available through
`task test-rust-shell-text-input` (or `python tools/rust_shell/tasks.py text-input`).
It drives real native clicks and keyboard shortcuts, checks Flutter's reported
text/UTF-16 selection, verifies Unicode clipboard copy/paste and `hasStrings`,
checks minimize/restore lifecycle and compares native client size/DPI with
Flutter metrics before and after resize. Widget-reported control bounds keep
pointer coordinates independent of display scale. Windows uses Win32 input and
`GetDpiForWindow`; Hyprland uses `sendshortcut`, `movecursor` and `ydotool`
(with its daemon) for clicks. Hyprland coverage is not yet validated here.

The fixture temporarily replaces clipboard text and restores the previous text
before closing; rich clipboard formats are not preserved. Logs, screenshots and
`coverage.json` remain in a `flutter-rust-input-*` temporary directory. The
coverage report explicitly separates current-scale checks from actual display
scale transitions and installed-layout reporting from interactive IME
composition. Neither composition nor cross-monitor DPI is inferred from a
successful typing/clipboard test.

The default texture tasks compile `examples/texture/lib/rust_shell_main.dart`
through the standalone smoke package; they do not resolve the entire example
workspace. A custom `RUST_TEXTURE_EXAMPLE` still uses that project's own package
configuration. Windows wgpu/pixel-buffer animation and 40-rotation texture
replacement/context-recreation checks have passed under Vulkan validation.

`task test-rust-shell-windowing` validates regular multi-view creation/resize/
teardown. `task test-rust-shell-owned-windows` exercises dialogs, satellites and
reparenting through Dart and the native adapter. Both require each retired view
to present and retain logs/status/per-view statistics on success and failure.
Windows ownership/tool-window/reparenting semantics are also checked by a native
winit test in the ordinary host suite. Tooltip and popup checks are described
below.

`task test-rust-shell-tooltips` renders owned no-activate tooltips alongside
regular/dialog/satellite views and verifies 60 owner-driven removals and each
view's presentations. Windows has initial monitor-work-area placement with
unit-tested flip/slide/resize constraints; native tests check owner/style and
foreground preservation. The owned-window fixture also uses parent-only destruction;
native tests verify owned children hide/return on owner minimize/restore.

The Windows tooltip task additionally drives owner movement through the Python
platform API and checks that the owned tooltip follows. A Dart/host handshake
changes size constraints and verifies the same anchor remains fixed. Geometry
snapshots are retained with the lifecycle evidence. The host reacts to native
move/resize/DPI notifications; actual display-DPI transitions remain unverified.

The Windows tooltip motion check also exercises explicit anchor/positioner and
positioner-only updates on the same native window. This uses private shell ABI
14; rebuild the matching engine and Rust runner together after the ABI change.

`task test-rust-shell-popups` exercises Windows grabbing popups with native mouse
and keyboard input. An inside click must retain the popup and its grab; an
outside click, Escape and owner activation loss must dismiss it. Twenty cycles
require all 60 views to present and report destruction, including owner-driven
popup removal, with Vulkan validation and clean exit. Evidence remains in the
reported temporary directory. Nested popups and switching to an external
application are not covered by this fixture.

Windows host vsync now samples `DwmGetCompositionTimingInfo` and QPC to schedule
the next composition tick through the event loop. Each request resamples the
native phase so delayed callbacks do not accumulate drift. Invalid or unavailable
DWM timing uses the monitor-rate timer fallback, and subsequent requests retry
DWM. Startup logs identify DWM selection and its interval or fallback selection.
The existing one-shot HWND wake timer also services these deadlines during modal
move/resize; idle windows do not gain a periodic timer or helper thread.

The DWM API requires a null HWND on supported Windows versions, so this clock
is composition-wide. Mixed-refresh monitors, VRR and physical presentation
latency require separate measurements; a passing resize/lifecycle test verifies
liveness and teardown rather than precise display scanout timing.
