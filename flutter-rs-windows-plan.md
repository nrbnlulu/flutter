# Flutter Rust Shell: Windows Plan

Plan for Phase 3 of [`flutter-rs.md`](flutter-rs.md). Progress is recorded in
[`flutter-rs-proggress.md`](flutter-rs-proggress.md). W0 is verified on Windows
x64: the engine DLL builds and loads, and all 17 C++ shell tests pass. W1 hardware transfer tests and the W2 ordinary-window
host checks pass. A device-scoped owned-image/copy workaround repairs captured
text/colors on the Intel HD 530. The ordinary-window first-build milestone is
verified on this host: click, text entry, 500-resize stress including modal
live resize, minimize/restore and clean shutdown. Full Phase 3 parity remains
pending; see the progress log for hardware coverage and evidence.

## First Windows build: execution plan (2026-11-08)

Target: native Windows x64, `x86_64-pc-windows-msvc`, debug/JIT, one
ordinary window. Keep the private engine bridge and the shared wgpu/Impeller
Vulkan device. GN produces the engine DLL; Cargo produces the application.
The first-build milestone ends with a visible, interactive Flutter frame and
clean shutdown. Full Phase 3 parity follows separately.

### Starting point

- The progress log records installation of Windows SDK 10.0.22621.0,
  Debugging Tools, and the DEPS-pinned Clang package, followed by successful
  GN generation and the start of compilation.
- `engine/src/out/host_debug/args.gn` exists and targets Windows x64 with
  `flutter_runtime_mode = "debug"`. This is an optimized engine configuration
  (`is_debug = false`), not a release/AOT runtime.
- W0 completed on 2026-11-08: Ninja produced `flutter_rust_engine.dll`
  (36,725,760 bytes) and `flutter_rust_engine.dll.lib` (41,230 bytes) in
  `engine/src/out/host_debug`. Python ctypes loaded the DLL and resolved all
  39 private ABI exports, including all 15 window functions. A null
  `FlutterRustShellRunTask` call returned the expected zero result.
- PE import inspection shows Windows system DLL dependencies, with no
  separately staged MSVC runtime DLL required on this host. This load check
  does not initialize Dart, Vulkan, or a native window.
- All 17 `flutter_rust_shell_unittests.exe` tests pass. The test target needed
  `//flutter/testing:testing_fixtures` to resolve `GetFixturesPath`; its Rust
  core static library built and linked successfully without extra system libs.
- Existing uncommitted GN/build-helper changes are preserved. The winit
  platform selector still rejects Windows, and the GPU crates still gate
  the required code to Linux/Android. **W1 GPU checks are verified; continue with checkpoint 3 (W2).**

### Current first-frame investigation (2026-11-08)

- Windows GPU crates and all three hardware upload/readback/shutdown tests pass
  under driver validation after the documented Intel empty-pool HAL workaround.
- The minimal Windows host compiles; all 47 host unit tests and 17 C++ shell
  tests pass. Ordinary-window startup/presentation/close was exercised through
  the staged Cargo smoke runner. Additional window kinds remain unsupported.
- The matching Dart SDK/frontend server and counter/text-field bundle build.
  A normal validation-only run recorded two 800x600 presentations and exited
  cleanly on native close.
- A direct swapchain capture shows corrupted text/colors. **Investigate this
  rendering failure next; presentation counts alone do not complete checkpoint 5.**
  See the progress log for retained evidence and the separate screenshot-layer
  close timeout. No rendering-correctness claim is made.
- Desktop captures and real input/live-resize checks were blocked by the open
  Start menu/foreground restrictions. The shared Python harness detects these
  conditions instead of accepting unrelated pixels or silently skipping stress.
- Validation commands now live behind one Taskfile and per-platform Python APIs;
  see [`tools/rust_shell/README.md`](tools/rust_shell/README.md).

### Ordered checkpoints

1. **W0: engine DLL (verified 2026-11-08).** From the repository root in PowerShell:

   ```powershell
   $env:DEPOT_TOOLS_WIN_TOOLCHAIN = '0'
   $env:GYP_MSVS_VERSION = '2022'
   $env:GYP_MSVS_OVERRIDE_PATH = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools'
   python engine/src/flutter/tools/gn --runtime-mode debug --no-goma --no-prebuilt-dart-sdk
   ninja -C engine/src/out/host_debug flutter_rust_engine.dll
   ```

   Stop on a failed command and resolve the actual compiler/linker error.
   Check the DLL and generated import library, inspect private C ABI exports
   (including window functions), and verify DLL loading/dependencies. Record
   the actual import-library path for Cargo. Windows JIT already links its
   snapshot symbols statically; no snapshot callback refactor is needed.
   This target builds the C++ engine boundary without requiring a working
   Windows winit adapter.

2. **Windows Rust GPU path (W1; GPU crates verified 2026-11-08).** Add Windows dependencies in
   `flutter-shell-wgpu`, `flutter-shell-winit`, and `flutter-gpu-queue` as
   needed. Widen shared Vulkan and desktop-surface cfgs while retaining
   Linux-only Wayland/X11 code. Use the pinned workspace dependencies and
   toolchain. Select Vulkan explicitly, require a present-capable adapter,
   and use FIFO presentation with the existing timer pacing. Exit: relevant
   crates pass `cargo check --target x86_64-pc-windows-msvc`; unavailable
   Vulkan produces a useful diagnostic.

   GPU crate checks and unit tests pass on Windows. All three non-benchmark
   hardware upload/readback/shutdown tests pass under driver validation on
   Intel HD Graphics 530 after the documented empty-descriptor-pool HAL fix.
   The host crate is covered by W2; swapchain/runtime validation remains W2/W3.

3. **Add the minimal Windows host (W2 subset).** Implement
   `platform/windows.rs`, select it in `platform.rs`, and gate Linux-only
   imports in the shared host. Support ordinary-window creation, metrics,
   pointer input, Windows keyboard messages, focus and close. Use explicit
   unsupported results for window kinds not yet implemented. Preserve the
   merged platform/UI runner and existing GPU synchronization. Exit: the
   host compiles and can create and destroy its native window.

4. **Link a small Cargo smoke runner.** Adapt the existing
   `tests/frb_fixture/rust/build.rs` linking pattern or add a dedicated small
   fixture. Link the W0 import library and stage the DLL beside the executable;
   stage `icudtl.dat` and debug Flutter assets from this matching checkout.
   Keep this reproducible with checked-in fixture instructions. Investigate
   `_window_rust.dart` symbol discovery if the fixture uses Rust window APIs;
   only add allocation exports if Windows lookup actually requires them.
   Full rlutter Windows support is not a prerequisite for this checkpoint.

5. **Prove the first frame.** Launch a minimal counter/text-field app from
   the staged bundle. Verify readable text, redraw after a click, keyboard
   input, resize, minimize/restore, focus changes and clean close. Exercise
   live resizing to detect Win32 modal-loop starvation. Run repeated
   resize/close with Vulkan validation enabled and investigate synchronization
   or lifetime errors. Record the GPU/driver, commands and results in the
   progress log; distinguish compile success from runtime success.

Run focused Python build-helper tests and relevant Rust tests for changed
code. Build/run the C++ shell tests where supported, resolving any separate
Rust static-library link requirements in that test target. Check affected
Linux/Android configurations before calling shared-code changes regression
validated; record unavailable host/device coverage explicitly.

After the first frame: complete IME/clipboard/DPI validation, texture and FRB
smokes, additional window kinds, DWM pacing, then rlutter Windows debug/run
and hot reload. Release/AOT, managed SDK packaging and Windows CI follow those
checks. Menus, drag-and-drop and accessibility remain Phase 3 parity work.

## Decisions (2026-10-26)

### GPU backend: Vulkan for both wgpu and Impeller

The shell depends on wgpu and Impeller sharing one device and queue. Windows
keeps the Linux/Android arrangement: wgpu owns a Vulkan instance, device,
queue and Win32 surface, and Impeller's `ContextVK` borrows them.
`impeller_enable_vulkan` already includes `is_win`.

Rejected alternatives:

- **Impeller GLES on ANGLE, wgpu GLES on the same ANGLE.** A shared EGL context
  is possible in principle (wgpu-hal can load `libEGL.dll` behind its
  `windows_angle` cfg), but wgpu's GLES backend is its weakest (lower limits,
  weaker compute, less mature synchronization), which would degrade the
  plugin API. It would also need a new interop path and synchronization
  design.
- **Impeller GLES on ANGLE (D3D11), wgpu on DX12.** Two APIs and two devices:
  textures would cross via DXGI shared handles with cross-API fences or keyed
  mutexes. This breaks the single-device model and adds the most unsafe code.
- **An Impeller D3D backend.** Out of scope; Impeller has none, and writing one
  contradicts the "Impeller is not rewritten" non-goal.

Stock Flutter on Windows renders Skia through ANGLE (D3D11); that path remains
available through the upstream Win32 shell. Missing Vulkan (VMs, remote
desktop, some older Intel GPUs) is a hard error with a clear message for now.
Revisit a fallback only if that becomes a real deployment problem.

### Vsync: timer fallback, then DWM timing

wgpu has no cross-platform vsync or presentation-timing API at the pinned
revision (`014d9e8`):

- `PresentMode` only throttles acquire/present on the raster thread, after the
  frame is built. Flutter needs a timestamped pulse on the UI thread before
  building it.
- `Features::VULKAN_GOOGLE_DISPLAY_TIMING` has no wgpu-level API (only
  `Surface::as_hal()`), reports past presentation times rather than a wakeup,
  and is mostly available on Android and Mesa. Desktop Windows driver support
  is unverified and expected to be absent.
- There is no `VK_KHR_present_wait`/`present_id` support.

winit's `request_redraw` is not vblank-aligned on Windows (it maps to
`WM_PAINT`); Linux is only aligned because Wayland frame callbacks sit
underneath.

Plan: configure Windows surfaces with `Fifo` from the start so presentation
cannot tear or outrun the display, and drive Flutter frames with the existing
timer fallback. Then add DWM timing (`DwmGetCompositionTimingInfo`, as
Flutter's Windows embedder does, or `DwmFlush` on a helper thread) behind a
`PlatformBackend` vsync-source method, reusing the per-window pacing. All
windowed presentation on Windows goes through DWM, so it is the
authoritative clock.

## Current portability

| Layer | State | Windows work |
| --- | --- | --- |
| C++ bridge (`cpp/`) | Windows x64 debug/JIT DLL builds and loads; 39 ABI exports resolve; 17 C++ tests pass. | Verify Dart/Vulkan initialization with the Cargo smoke runner |
| `flutter-shell-core`, `flutter-plugin-sdk`, `flutter-gpu-queue` | No OS dependencies; `flutter-gpu-queue` is only gated to `linux \|\| android`. | Widen cfgs |
| `flutter-shell-wgpu` | Every `target_os = "linux"` gate really means "desktop with a winit surface". The broker and semaphore chain are plain Vulkan/wgpu-hal. | Retarget cfgs |
| `flutter-shell-winit` | Shared host behind `PlatformBackend`; `compile_error!` for other targets. | `platform/windows.rs`, protocol fixes |
| `rlutter` | Linux-specific: `.so` names, rpath, `os::unix::fs::symlink`, `linux-x64` artifacts, only `LinuxHost`. | Most new code |

## Reuse strategy

1. **Desktop cfg alias** for `linux` and `windows` (small `build.rs` with
   `cfg_aliases`, or explicit `any(...)`) in `flutter-shell-wgpu`,
   `flutter-gpu-queue` and the shared parts of `flutter-shell-winit`.
   `target_os = "linux"` remains only for Wayland, X11 and xkb code.
2. **Platform differences move into `PlatformBackend`** instead of constants
   and `cfg` blocks in `shell.rs`:
   - `fn legacy_key_message(...)` replaces `LEGACY_KEYMAP`/`LEGACY_TOOLKIT`.
     The GTK-shaped JSON is wrong for the `"windows"` keymap (`keyCode`,
     `scanCode`, `characterCodePoint`, Windows modifier bits); Android has the
     same problem.
   - `fn lock_state() -> Option<LockState>`: Windows reads `GetKeyState`
     exactly; Linux keeps NumLock inference as the fallback.
   - `fn vsync_source(...)` replaces `is_wayland()`.
3. **Popup positioning in Rust.** Wayland's compositor solves
   `xdg_positioner`; Win32 and later macOS need the host to compute placement
   from the ABI v6 anchor rect, gravity, offset and constraint-adjustment
   flags. Implement it as a pure, unit-tested shared function.
4. **Refactor on Linux first** with no behavior change, gated by the existing
   tests and fixtures.

## Workstreams

### W0: engine build on a Windows host (GN)

- `BUILD.gn`: platform-aware artifact names (`flutter_shell_winit.lib`, not
  `lib*.a`); exports through `FLUTTER_RUST_SHELL_EXPORT` (`.def` files cannot
  express the `FlutterRustShell*` wildcard), including the
  `FlutterRustShellWindow*` FFI functions; `libs = ["dl", "pthread"]` only on
  Linux.
- Build only `flutter_rust_engine.dll` on Windows and skip the GN
  `flutter_rust_shell_runner`. Cargo owns the final link (runner-rs and the
  FRB fixture already work this way), avoiding Rust staticlib system-library
  and CRT mismatches under clang-cl/lld-link.
- JIT snapshot lookup needs no Windows change: `DART_SNAPSHOT_STATIC_LINK`
  in `runtime/dart_snapshot.cc` already selects linked snapshot symbols on
  Windows and Android JIT builds. The original concern about process lookup
  applied to the dynamic fallback, which Windows JIT does not use. Keep the
  existing Linux export lists and the `//flutter/lib/snapshot` dependency.
- `build_rust.py`: `x86_64-pc-windows-msvc` and `.lib`/`.dll` names.

### W1: wgpu broker

- Windows target-dependency block: `ash`, `wgpu` (`vulkan`), `winit` (Win32
  backend), no Wayland/X11 features.
- Retarget cfgs to the desktop alias.
- Validate: wgpu-hal throttles Windows acquire with a fence
  (`wgpu-hal/src/vulkan/swapchain/native.rs`), alongside our
  acquire→Impeller→present semaphore chain. Run the resize stress and
  in-flight teardown checks under Vulkan validation.

### W2: winit host (`platform/windows.rs`)

- Window kinds: dialog as an owned window (`with_owner_window`); satellite as
  an owned tool window with skip-taskbar; reparenting via
  `SetWindowLongPtr(GWLP_HWNDPARENT)`; popup/tooltip as a no-activate
  `WS_POPUP` owned window placed by the shared positioner. `grab` popups
  dismiss on outside click or activation loss and reuse the asynchronous view
  removal path of Wayland's `popup_done`. Absolute positioning means the
  initial satellite positioner is honored, as on X11.
- Keyboard: `FALLBACK_KEY_PLANE = 0x01600000000` (`windowsPlane`), Windows
  legacy key encoder, lock state from `GetKeyState`, and AltGr reported as
  Ctrl+RightAlt in modifier reconciliation.
- Vsync as decided above.
- Verify only: clipboard (`arboard` without its Wayland feature), IME (winit
  IMM32), cursors, lifecycle, per-monitor DPI.
- Risk: Win32's modal move/resize loop may stall `WaitUntil` deadlines.
  Confirm Flutter tasks and frames continue during window drag/resize.

### W3: framework (Dart)

- `packages/flutter/lib/src/widgets/_window_rust.dart` gates on
  `Platform.isLinux`; allow Windows too (still with the symbol probe) so the
  Rust owner wins over the Win32 window controller.
- `@Native(symbol: 'malloc'/'free')` needs process-wide lookup; on Windows
  `malloc` lives in `ucrtbase.dll`. If Dart does not resolve it, export
  `FlutterRustShellAlloc`/`Free` from the engine.

### W4: rlutter and packaging

- `build.rs.tmpl`: `("windows", "x86_64") => "windows-x64"`, link
  `flutter_rust_engine.dll.lib`, copy DLLs beside the executable (no rpath),
  embed a manifest (per-monitor-v2 DPI, UTF-8 code page, common controls),
  icon and version info.
- `Cargo.toml.tmpl`: host crate under `cfg(not(target_os = "android"))`.
  `android_runner.rs` rewrites the exact Linux target header string, so change
  both together.
- `sdk.rs`: platform-aware artifact names and a
  `flutter-rust-windows-x64` tarball.
- `util.rs`: junction or copy fallback instead of `os::unix::fs::symlink`.
- `device/windows.rs` (modelled on `linux.rs`) and `build windows`. Bundle:
  executable, engine DLL, `icudtl.dat`, `data/flutter_assets`, and release
  `app.so` (current Windows x64 AOT uses a PE/COFF DLL with this filename).
- Add `windows` to `PLATFORMS` and `--rust-platforms`.

### W5: CI and tests

- The engine cannot realistically be cross-built for Windows from Linux. Add a
  `windows-latest` job (depot_tools and Visual Studio) to
  `flutter-rust-beta.yml` that builds debug and release DLLs and publishes them
  to the BETA release.
- Cargo unit tests run unchanged on Windows; add positioner and legacy key
  encoder tests.
- The Python fixtures depend on Hyprland. Start Windows with an FRB-fixture
  smoke test (renders, texture, clean exit); port window control to Win32
  later. Tests that assume `flutter_rust_shell_runner` need a Cargo-runner
  equivalent.

## Milestones

1. **Seam refactor on Linux**: desktop cfg alias, `PlatformBackend` methods,
   Rust positioner. Exit: all Linux tests and
   fixtures pass with no behavior change.
2. **Engine DLL on Windows**: GN builds `flutter_rust_engine.dll` with Impeller
   Vulkan; C++ unit tests pass on Windows.
3. **First frame**: a Cargo-linked counter app renders through wgpu Vulkan
   with input, resize and IME. Exit: resize stress and clean shutdown under
   Vulkan validation.
4. **Parity**: wgpu and pixel-buffer textures, FRB dispatch, all five window
   kinds, DWM vsync, clipboard, cursors.
5. **Tooling**: `rlutter create/run/build windows` in debug and release, hot
   reload, CI tarball.
6. **Phase 3 extras**: drag-and-drop, menus, accessibility and IME
   conformance. Accessibility is unimplemented on Linux too, so treat it as a
   shared AccessKit workstream.

## Risks

- **Vulkan driver coverage** (see the GPU decision).
- **Swapchain behavior**: fence-throttled acquire and Win32 live-resize
  flicker need measurement before committing to the semaphore design on
  Windows.
- **Dropping the GN runner on Windows** requires Cargo-runner equivalents for
  tests that use it.

## Text/clipboard/DPI checkpoint (2026-11-09)

`task test-rust-shell-text-input` now exercises native typing, Unicode clipboard
copy/paste, UTF-16 selection, minimize/restore and native-versus-Flutter viewport
metrics on resize. It passes on the Windows host at 96 DPI. Native IME candidate
composition and real display-scaling transitions remain unverified; the task's
coverage report records those limits explicitly. See the progress log for
retained captures and the separate full/delta-model IME protocol tests.

## Texture/FRB checkpoints (2026-11-09)

Windows wgpu and pixel-buffer animation, 40 texture lifecycle rotations with
resize/context recreation/reclamation, and real FRB background-isolate dispatch
with both texture producers now pass under Vulkan validation. The FRB Cargo
fixture explicitly exports its pinned process ABI on Windows. See the progress
log for the original lookup failure and retained successful-run evidence.
Additional window kinds and DWM pacing are the next implementation checkpoints;
IME composition and DPI-transition conformance remain coverage gaps.

## Owned-window checkpoint (2026-11-09)

Windows now selects the symbol-probed Rust framework owner. Regular multi-view
lifecycle and the new dialog/satellite/reparenting fixture pass with 60 retired
views each under Vulkan validation. Win32 ownership/tool-window styles and
position-preserving reparenting are checked natively. Popups/tooltips, parent
cascade/minimization coverage and DWM pacing remain pending.

## Parent lifecycle and initial tooltip checkpoint (2026-11-09)

Parent-only cascade removal and native owner minimize/restore now pass. Windows
supports initially positioned, owned no-activate tooltips through the shared
positioner; the tooltip lifecycle fixture and native ownership/style tests pass.
Reactive repositioning and grabbing-popup dismissal remain the next window-kind
work before DWM pacing. These checks do not close IME/DPI-transition coverage.

## Reactive geometry checkpoint (2026-11-09)

Windows tooltips now react to retained parent movement/resize/scale requests
and explicit size-constraint changes. Native movement and anchored resizing pass
inside the Python-backed 60-view lifecycle test. Actual DPI transitions,
framework `updatePosition` anchor changes, and grabbing-popup dismissal remain
pending before considering full popup/tooltip parity complete.

## Explicit anchor and grabbing-popup checkpoint (2026-11-09)

Framework `updatePosition` now updates the existing Windows tooltip/popup HWND
through private shell ABI 14. Anchor/positioner and positioner-only movement
passed the tooltip fixture; see the progress log for retained geometry evidence.

Windows now accepts grabbing popups. Native capture keeps inside clicks in the
popup and dismisses outside clicks; Escape and owner focus loss request removal
through the existing asynchronous view teardown. `task test-rust-shell-popups`
passes inside-click retention, the three dismissal paths, all 60 per-view
presentation/destruction checks and clean shutdown under Vulkan validation.
Evidence: `%TEMP%/flutter-rust-windowing-gzubypi8`.

This completes the initial single-popup checkpoint on this host. Nested-popup
interaction and external-application activation are not covered by this fixture.
DWM pacing is the next planned implementation checkpoint. Native IME composition,
real DPI transitions, broader GPU/platform coverage and rlutter tooling remain.

## DWM pacing checkpoint (2026-11-09)

Windows now opts into the host vsync callback. A platform timing hook uses DWM's
QPC vblank and refresh period to schedule the next composition tick; each request
resamples the native phase. Invalid/unavailable native timing retains the
monitor-rate timer fallback. The existing one-shot native wake timer covers
modal move/resize without a helper thread or idle polling. FIFO presentation
and the private engine ABI are unchanged.

The ordinary suite passes with 55 host tests and 17 C++ tests. Windows stress
passes 500 resizes, keyboard redraw, minimize/restore, live modal resize and
clean Vulkan-validation shutdown. Tooltip geometry and the 60-view lifecycle
also pass. Logs confirm DWM selection at 31.25 ms on this desktop; a native probe
confirms both DWM's refresh ratio and QPC period report 32 Hz. Evidence:
`%TEMP%/flutter-rust-stress-2cc1rkyf` and
`%TEMP%/flutter-rust-windowing-7pmuhmg0`.

This verifies DWM-derived scheduling and runtime liveness on this host, not
physical scanout timing, latency, VRR or mixed-refresh monitor behavior. DWM's
null-HWND clock is composition-wide. Invalid timing samples are covered by unit tests; native API failure and an
unavailable-compositor runtime were not exercised here.
Linux/Android runtime coverage remains unavailable. Next: rlutter Windows
create/run/build support and hot reload, followed by release/packaging/CI.

## rlutter local-engine debug tooling checkpoint (2026-11-09)

Windows x64 device discovery, `create --rust-platforms windows`, `build windows
--debug` and `run -d windows` now work with an explicit local Flutter checkout.
Shared desktop staging places Windows DLLs beside the executable and assets/ICU
under data/. The runner's embedded manifest enables PerMonitorV2 DPI, UTF-8 and
common controls. Runner version 10 migrates existing desktop dependency tables,
normalizes CRLF and retains the fork's required vendored wgpu-hal patch, updating
only that lockfile package when needed. Native plugins use Windows DLL names;
Android cross-build names retain .so.

The 108 applicable rlutter tests and Clippy pass on Windows. A generated project
built successfully. The runtime harness passes native-click state changes,
rendered hot reload with state preservation, hot restart with reset state,
Flutter-attach quit/owned-process cleanup, and a relocated standalone bundle
rendering the same centered widget before status-zero native close under Vulkan
validation. Evidence: `%TEMP%/flutter-rust-rlutter-em7ezyx4`; project:
`%TEMP%/rlutter-windows-0833f1b8`. PE resource inspection confirms the manifest.

This is debug/JIT local-engine tooling coverage. Managed Windows SDK downloads,
release/profile/AOT builds, icon/version resources, general plugin/FRB packaging
and CI remain pending. VM source expression evaluation reports no compilation
service despite successful attach/reload/VM field inspection; debugger expression
and breakpoint conformance remain follow-ups. Symlink-denied copy fallback and
Linux/Android runtime regressions are not host-validated here. Real IME and
DPI-transition coverage remain open.

## Post-main debugger checkpoint (2026-11-09)

Upstream main was merged and pushed, with the two Vulkan synchronization
conflicts resolved using upstream's more complete attachment-access barriers.
Dependencies were synced to Dart 3b174b7217 and Skia b11315d0a0. Build hooks had to
refresh Dart's generated package language versions from 3.13 to 3.15 before the
matching Windows engine DLL, Dart SDK and frontend server could be rebuilt.

Linux fixes from 6791498d4ef were pulled before the Windows checkpoint commit;
its viewport stabilization and Wayland parent-mapping handshake are preserved.
Windows now uses an explicit client-size resize helper to exclude native frame
borders while retaining the adapter's existing outer-size resize operation.

The earlier expression-evaluation failure was an HTTP transport false negative.
DDS WebSocket expression evaluation succeeds with Flutter's compiler service.
The runtime probe verifies a source breakpoint in incrementProbe, local `next`
evaluation while paused, resume, native-click state, rendered hot reload with
state preservation, hot restart, detach cleanup and relocated native close under
Vulkan validation. Evidence: `%TEMP%/flutter-rust-rlutter-cf6orzam`.
The ordinary suite passes 55 host tests, 17 C++ tests and Python checks; all 108
rlutter tests and Clippy pass. All eight native adapter tests pass after clearing
a blocking Windows Search overlay; the first foreground-blocked run was not
counted as a debugger pass.

This covers one source breakpoint and local-expression scope on the Windows host,
not full DAP/editor conformance or Linux execution of the new debugger probe.
Managed SDK/release/AOT/plugin packaging and CI remain planned work, as do real
IME composition and display-scale transition coverage.


## Windows release/AOT checkpoint (2026-11-09)

The matching Windows host_release engine DLL, Dart SDK, frontend server, AOT
compiler, shader compiler, font subsetter and sky_engine package build from the
post-main checkout. The normal Windows build task now requests these compiler
artifacts; its release Cargo smoke runner also links successfully.

`test-rlutter-aot` creates and builds a disposable release app, requires app.so
and no debug kernel, relocates executable/DLL/data, launches from an unrelated
working directory, checks the exact initial and clicked frame colors, and
requires native close with status zero under Vulkan validation. The Windows
runtime check passes; retained evidence is `%TEMP%/flutter-rust-release-0f6axvzj`
(build/runtime logs, full captures, relocated bundle and result.json).

Current upstream Windows x64 AOT emits a PE/COFF DLL named app.so. The original
plan's ELF note and an initial probe assertion were corrected; the initial
format-rejected run was not counted as a runtime pass. The native engine already
loads this library correctly, so no AOT engine change was needed. Profile is not
covered by this release run. SDK distribution and input follow as separate phases.


## Windows SDK distribution checkpoint (2026-11-09)

Windows x64 managed SDK discovery now uses LOCALAPPDATA (or XDG_CACHE_HOME),
Windows BETA archive names and dart.exe validation. The physical Rust SDK ships
debug/release engines with their import libraries, ICU, compiler tools, matching
Dart SDKs, patched SDKs and sky_engine. Both patched Rust source dependencies
(winit-wayland and wgpu-hal) are packaged; the Linux archive also now includes
its required wgpu-hal patch. Profile remains a local-engine workflow on Windows.

The standalone Flutter packager imports the resolved numeric Flutter tag with
its history, checks the common ancestor, warms the Windows wrapper and rejects
unknown version metadata before archiving. A fresh-create test caught the first
local clone's missing numeric tag (0.0.0-unknown); the corrected archive reports
3.50.0-1.0.pre-666 and passes creation/dependency resolution without overrides.

Local validation extracts both exact archives into an isolated managed cache
and a new project, with the packaging-stage directory hidden. No checkout engine
or external source links are used. Both debug (kernel) and release (PE app.so,
no kernel) generated-app builds pass. Evidence: engine/src/out/windows-distribution-
result.json and windows-distribution-builds.log; project:
%TEMP%/rlutter-windows-distribution-rv0v88gs. All 111 rlutter tests and Clippy pass,
including managed-cache reuse, required import libraries and forced symlink-
privilege failure with real copy/refresh/stale-file cleanup.

The BETA workflow now has parallel Windows debug/release engine jobs and a
Windows packaging job that tests extracted SDKs before publishing Windows SDK
archives, rlutter.exe and checksums. actionlint and the PowerShell blocks pass
local syntax checks. Cloud execution and actual BETA download/publication are
pending the branch merge; local archive validation is distinct from publication.


## Windows packaged release input checkpoint (2026-11-09)

The native text-input suite passes against both the current debug smoke runner
and an AOT release app compiled by the extracted Windows SDK, with the packaging
stage still hidden. It verifies real keyboard typing, copy/paste, the emoji/RTL
Unicode clipboard sample, UTF-16 selection, minimize/restore lifecycle, resize,
Flutter/native physical dimensions and current monitor scale, clipboard text
restoration and status-zero shutdown under Vulkan validation. Evidence:
%TEMP%/flutter-rust-input-mp6jhfg0 (debug) and
%TEMP%/flutter-rust-input-at9z3ktr (packaged release).

The adapter now enumerates native monitor work areas. When multiple displays
are available, the input probe moves between them and compares Flutter/native
metrics; it reports a scale transition as passed only if different native scales
were actually observed. This RDP host exposes one 1024x720 work area at scale 1,
so real mixed-DPI transitions remain unverified. IMM32 layout enumeration is not
proof of composition; real IME composition remains unverified too.

The fixture places its 800x600 viewport within a small Windows desktop. Initial
capture/focus failures were traced by native hit-testing to Start/Search system
menus; only those verified menus were dismissed before retrying. Occlusion and
foreground checks remain strict, and blocked runs were not counted as passes.
All eight native adapter tests pass without skips, including real monitor
geometry enumeration and exact client resizing. Python compilation/formatting
and whitespace checks pass. No native input implementation change was needed.


### Windows CI bootstrap follow-up (2026-11-09)

The first cloud Windows jobs stopped in gclient's offline-pub audit: the Flutter
smoke app was missing from the exclusions already used for FRB/windowing app
fixtures. Added its narrow exclusion and a regression test proving that unknown
packages under the same Rust test tree remain audited. The actual engine tree
has no unlisted packages, the regression passes, and the full offline-pub hook
exits zero locally. Windows CI runs the regression after sync. Artifact publication
still depends on the restarted cloud builds; no failed run is counted as a pass.


The next Windows bootstrap run passed the package audit and exposed the hook
installer's legacy git.bat assumption. It now resolves installed Git from PATH
and honors its explicit argv. A Windows regression installs and uninstalls the
hooks in a real temporary repository whose PATH contains only native git.exe.
The regression passes; hooks remain enabled in CI. Both bootstrap regression
checks run after sync before the engine build.


The cloud sync itself then completed; the regression exposed the runner's 8.3
TEMP alias. Hook setup now canonicalizes and case-normalizes Git/common paths
and hook/repository paths, avoiding false linked-worktree detection and invalid
relative hook paths. Real native-Git tests cover an available short-path alias
and a linked worktree, including preservation of worktree-scoped configuration.
Both tests pass locally; the CI step continues to run them before compilation.
