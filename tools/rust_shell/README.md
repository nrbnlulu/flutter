# Rust shell tasks

`Taskfile.yml` delegates to `tools/rust_shell/tasks.py`. The Python runner
owns environment setup, argument lists, builds, fixture bundles, process
lifetimes, and assertions. It does not require Bash, jq, sed, or pgrep.
`DEPOT_TOOLS` and `DEPOT_PY_BIN` remain optional environment overrides.
On Unix, subprocesses use the checkout's DEPS-pinned Python when available so
depot_tools and engine helpers remain compatible with newer system Pythons.

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
task test-rust-shell-framework
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

`test-rust-shell-framework` (or `framework-tests`) builds `flutter_tester`, the
Dart SDK and frontend server from the selected local engine before running the
foundation suite and the view-metrics, rendering-binding and texture tests. A
Rust-shell runner build alone does not update `flutter_tester`; mixing an older
tester with the current compiler can fail with `Invalid SDK hash`.

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
`GetDpiForWindow`; Hyprland uses its shortcut and cursor dispatchers (including
their Lua forms on newer releases) for clicks, without a separate input daemon.
Mouse injection preserves the cursor's hit-test point instead of supplying a
window selector, which would reset the pointer to the window's top-left corner.

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
view's presentations. On Wayland the fixture waits for the compositor to map
each parent before creating its tooltip; frame submission alone is insufficient.
Windows has initial monitor-work-area placement with
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

Windows local-engine rlutter workflows:

```powershell
cargo +1.93.1 build --locked --manifest-path engine/src/flutter/shell/platform/rust/rlutter/Cargo.toml
# Create outside the Flutter SDK; Flutter rejects nested projects.
rlutter create C:/work/my_app --rust-platforms windows --flutter-root C:/path/to/flutter
# From the generated application:
rlutter build windows --debug --flutter-root C:/path/to/flutter
rlutter build windows --release --flutter-root C:/path/to/flutter
rlutter run -d windows --flutter-root C:/path/to/flutter
```

Use the executable built by Cargo above, or add its directory to PATH.
`--local-engine-src` can override the engine/src directory. The Windows bundle is
`runner-rs/target/<debug|release>/<name>.exe`, engine/plugin DLLs beside it, and
`data/{icudtl.dat,flutter_assets}`. Release assets contain the AOT `app.so` instead
of a debug kernel. The executable contains a PerMonitorV2/UTF-8/common-controls
manifest. Icon/version resources remain separate work.

Once the Windows BETA archives have been published, omitting `--flutter-root`
uses the managed Windows x64 SDK. Its Flutter tools are cached under
`%LOCALAPPDATA%/rlutter/flutter-BETA` (or `$XDG_CACHE_HOME/rlutter/flutter-BETA`).
Each project downloads `flutter-rust-windows-x64.tar.gz` for its physical shell
sources and matching debug/release engines. Profile remains a local-engine
workflow; the Windows distribution currently ships debug and release.
`package_windows_sdk.py` stages complete compiler tools and both patched Rust
source dependencies. Flutter packaging requires `--version-tag`, fetches its
history and validates the SDK's numeric version before archiving. GitHub Actions
builds generated apps using the extracted archives before publishing them alongside `rlutter.exe` and checksums.

Runner version 10 updates the desktop dependency table, MSVC link arguments and
the required vendored wgpu-hal patch. Migrating an existing runner with a lockfile
updates only wgpu-hal offline. Windows uses symlinks where possible and copy
fallback otherwise; the small flutter_patched_sdk directory is always physical
because the AOT frontend rejects its directory symlink in the tested setup.

`task test-rlutter-runtime` creates a disposable desktop project, runs it through
rlutter/Flutter attach, clicks its widget, verifies changed rendered output and
preserved state on hot reload, then verifies hot restart resets state. It also
copies the bundle, launches it from an unrelated working directory, compares the
centered widget pixels and requires status-zero native close. Logs, service URI,
state observations, screenshots and copied bundle remain in a reported
`flutter-rust-rlutter-*` directory. The script accepts `--project` to reuse a
disposable generated project, with its fixture in `lib/rlutter_probe.dart`.

The runtime task also connects to DDS over WebSocket using Flutter tools'
`vm_service` package configuration. It checks expression evaluation, stops at a
source breakpoint after the native click, evaluates a paused-frame local variable
and resumes before testing reload/restart. Results are retained in
`debugger-status.json`. DDS HTTP requests bypass its expression compiler service;
the earlier HTTP-only probe was a false negative, not an engine debugger defect.

Viewport stabilization is shared with Linux. Windows `resize_client` accounts
for native frame borders; the existing `resize` action retains outer-window
semantics. The new debugger probe and client-sizing check are Windows-validated.
Linux debugger/DAP conformance, Android runtime,
native IME composition and display-scale transitions remain separate coverage.

`task test-rlutter-aot` builds a release engine and a disposable generated app,
requires `app.so` with no debug kernel, relocates its executable/DLL/data bundle,
checks a native click changes the rendered frame, and requires clean native
close under Vulkan validation. Logs, captures and `result.json` remain in the
reported `flutter-rust-release-*` evidence directory. The underlying script also
accepts `--mode=profile` when a matching local profile engine is available.
