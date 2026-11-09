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
  `app.so` (Windows AOT already uses ELF `app.so`).
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
