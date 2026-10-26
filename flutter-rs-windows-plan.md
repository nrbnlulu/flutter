# Flutter Rust Shell: Windows Plan

Plan for Phase 3 of [`flutter-rs.md`](flutter-rs.md). Progress is recorded in
[`flutter-rs-proggress.md`](flutter-rs-proggress.md). Nothing in this document
is implemented yet.

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
| C++ bridge (`cpp/`) | Portable. `rust_bridge.h` already defines a `_WIN32` `dllexport` macro. Only the fontconfig warning is Linux-specific. | Snapshot mapping, exports |
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
- JIT snapshot lookup: `DartSnapshot` falls back to
  `NativeLibrary::CreateForCurrentProcess()`, which on Windows is
  `GetModuleHandle(nullptr)` (the `.exe`), not the engine DLL. Set
  `settings.vm_snapshot_data`/`isolate_snapshot_*` mapping callbacks in
  `rust_shell.cc` from the linked `//flutter/lib/snapshot` symbols. This works
  on every platform and lets the snapshot symbols leave
  `rust_engine_exports.lst` and `rust_runner_exports.sym`.
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
   Rust positioner, snapshot mapping callbacks. Exit: all Linux tests and
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
