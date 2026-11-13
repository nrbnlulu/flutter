# Flutter Rust Shell: macOS Plan

Plan for macOS support in [`flutter-rs.md`](flutter-rs.md), following the
structure of [`flutter-rs-windows-plan.md`](flutter-rs-windows-plan.md).
Progress will be recorded in [`flutter-rs-proggress.md`](flutter-rs-proggress.md).
Status (2026-11-13), branch `macos/m0-engine-build`: M0 and M1 done. M2–M4
done to the extent verifiable unattended: the AppKit host renders regular
windows through wgpu/Impeller on MoltenVK, the 60-view windowing test passes
under validation, and a staged runner relocates. Input, live resize, Retina,
release AOT and bundling MoltenVK remain open. See the progress log.

Target: macOS 13+ on Apple Silicon (`aarch64-apple-darwin`). Intel (x64) is
out of scope for now.

Revised 2026-11-13 against `rust/flutter-rs-shell` at `7c8b771`. Windows
support has since landed on the branch, so most of the shared seam work this
plan originally depended on already exists. Paths below are relative to
`engine/src/flutter/shell/platform/rust/` unless stated otherwise.

## Decisions (proposed, 2026-11-05; revised 2026-11-13)

### GPU backend: Vulkan on MoltenVK for both wgpu and Impeller

The shell depends on wgpu and Impeller sharing one device and queue. macOS
keeps the Linux/Android/Windows arrangement: wgpu owns a Vulkan instance,
device, queue and `CAMetalLayer`-backed surface (`VK_EXT_metal_surface`), and
Impeller's `ContextVK` borrows them. Both run on MoltenVK.

Why this is the first step:

- `impeller_enable_vulkan` already includes `is_mac`
  (`engine/src/flutter/impeller/tools/args.gni`), and the engine documents
  MoltenVK for Impeller development
  (`docs/engine/impeller/Setting-up-MoltenVK-on-macOS-for-Impeller.md`).
  Both verified.
- The broker, semaphore chain, external-texture bridge and plugin SDK stay
  as they are. Most of the work is cfg widening and platform glue, not a new
  GPU design.
- wgpu supports Vulkan on Apple targets through its portability path
  (`VK_KHR_portability_enumeration`/`VK_KHR_portability_subset`), enabled by
  the `vulkan-portability` feature. Confirm the feature name at the pinned
  wgpu revision `014d9e84`.

Rejected or deferred alternatives:

- **Impeller Metal and wgpu Metal sharing one `MTLDevice` and command queue.**
  This is the natural long-term macOS/iOS backend: no translation layer and
  full Metal performance. It needs a new broker backend (`wgpu-hal` Metal
  device from a raw `MTLDevice`), a `ContextMTL` that borrows it, and a new
  synchronization design to replace the Vulkan semaphore chain. **Deferred**
  until MoltenVK is running, then reassessed against measured MoltenVK
  overhead. It shares most of its design with iOS.
- **Impeller Metal with wgpu on a separate device, textures through
  IOSurface.** Two devices, which breaks the single-device model in the same
  way as the rejected Windows DXGI option. **Rejected.**
- **Impeller GLES.** OpenGL is deprecated on macOS, and wgpu's GLES backend is
  its weakest. **Rejected.**

MoltenVK risks: Impeller on MoltenVK is a development configuration, not a
shipped Flutter one. Expect gaps from `VK_KHR_portability_subset` (for example
some sampler/format features) and possible validation-layer noise. Each gap
gets a workaround or is logged in the progress file. Nothing is papered over.

### Vsync: timer fallback, then display link

winit's `request_redraw` on macOS is not a vblank-aligned pulse. Plan:
configure surfaces with `Fifo`, drive frames with the existing timer
fallback, then add a `CVDisplayLink` (or `NSView.displayLink` on macOS 14+)
vsync source. Windows already has its own source in
`crates/flutter-shell-winit/src/platform/windows_vsync.rs` (DWM timing with a
monitor-rate timer fallback); model the macOS source on it and reuse its
per-window pacing rather than designing a new hook.

### Threads

AppKit and winit require the event loop on the main thread, so the platform
task runner stays on the main thread. Raster stays on its own thread.
MoltenVK presentation from the raster thread is fine. Any `CAMetalLayer`
property changes (size, contents scale) are made on the main thread.

## Current portability (verified 2026-11-13)

| Layer | State | macOS work |
| --- | --- | --- |
| C++ bridge (`cpp/`) | Portable apart from snapshot lookup and exports. Windows and Android link JIT snapshots statically via `runtime/dart_snapshot.cc:27` (`(FML_OS_WIN \|\| FML_OS_ANDROID) && FLUTTER_JIT_RUNTIME`). Linux uses an ELF version script (`cpp/rust_engine_exports.lst`). | Done in M0: Mach-O exported-symbols file. No snapshot change needed (macOS keeps the `RTLD_DEFAULT` lookup, which finds the dylib's exported snapshot symbols; verify at runtime in M4) |
| `flutter-shell-core`, `flutter-plugin-sdk` | No OS dependencies. | None |
| `flutter-gpu-queue` | Gated to `linux`, `android` and `windows`. | Add `macos` to its cfgs |
| `flutter-shell-wgpu` | One `Cargo.toml` dependency block per OS (`linux`, `android`, `windows`), each with wgpu `["std", "vulkan", "wgsl"]`; no macOS block. Source cfgs list `linux`/`windows`/`android` explicitly. | `macos` dependency block with `vulkan-portability`, portability instance/device flags, Metal-layer surface, `macos` in shared cfgs |
| `flutter-shell-winit` | `PlatformBackend` (`src/platform.rs`) already has `text_modifiers`, `lock_state`, `supports_window_kind` and `legacy_key_message`, with `linux.rs`, `windows.rs`, `android.rs`; `compile_error!` for other targets at `platform.rs:43`. Shared `positioner.rs` exists for Linux and Windows. | `platform/macos.rs` implementing the existing trait, macOS key mapping, `macos` in the platform cfgs |
| GN (`BUILD.gn`) | `shell/platform/BUILD.gn` already exposes `group("rust")` on all platforms, so `ninja flutter_rust_engine` is reachable on a macOS host. Linux-only `ldflags` and `libs`; Windows lets Cargo own the runner link. | `.dylib` install name, Mach-O exports, follow the Windows pattern of library-only GN output |
| `build_rust.py` | Already emits `lib<name>.dylib` for Apple targets. | Confirm the `aarch64-apple-darwin` staticlib path |
| Framework (`_window_rust.dart`) | Gates on `Platform.isLinux \|\| Platform.isWindows`. | Add `Platform.isMacOS` |
| `rlutter` | Linux and Windows hosts. | Minimal: enough to build and run the counter app |

## Reuse strategy

Windows already did the seam work this plan originally planned to share, so
macOS follows the Windows pattern instead of driving a refactor:

1. **Per-OS cfgs, not an alias.** The crates list target OSes explicitly and
   `Cargo.toml` repeats a dependency block per OS. Add `target_os = "macos"`
   alongside `windows` where the code is desktop-generic, and a `macos`
   dependency block. `target_os = "linux"` stays only for Wayland, X11 and
   xkb code.
2. **Implement the existing `PlatformBackend`.** `macos.rs` mirrors
   `windows.rs`. Extend the trait only where macOS needs something Windows
   did not.
3. **Keep Linux and Windows green.** Run the existing Linux tests and fixtures
   (and Windows where available) on every cfg change.

## Workstreams

### M0: engine build on a macOS host (GN)

- Configure: `./flutter/tools/gn --mac-cpu arm64 --no-goma ...`
  (Vulkan is already on for `is_mac`), then `ninja -C out/<config>
  flutter_rust_engine`. The target is already reachable, so the first step is
  to attempt the build and fix what breaks.
- Snapshots: no change needed (M0 finding). The dynamic lookup uses
  `RTLD_DEFAULT`, and the dylib exports `kDartSnapshotData`/`Text`. If the
  M4 runner cannot find them, add `FML_OS_MACOSX` to the static-link
  condition in `runtime/dart_snapshot.cc:27`, as Windows did.
- `BUILD.gn`: `libflutter_rust_engine.dylib` with an `@rpath` install name and
  a Mach-O `-exported_symbols_list` (leading-underscore names) for the
  `FlutterRustShell*` symbols, including the `FlutterRustShellWindow*` FFI
  functions. No Linux `libs` or version script.
- As on Windows, build only the engine library and let Cargo own the final
  link of the runner (the GN `flutter_rust_shell_runner` is currently `!is_win`).
- `build_rust.py`: verify the `aarch64-apple-darwin` staticlib for
  `flutter-shell-winit`. The `.dylib` naming is already there.

### M1: wgpu broker on MoltenVK

- `macos` dependency block: `ash`, `pollster`, `wgpu` (`vulkan`,
  `vulkan-portability`), `winit` (AppKit backend; state the needed features,
  following Windows' `default-features = false`), no Wayland/X11.
- Instance creation with `VK_KHR_portability_enumeration` and the
  enumerate-portability flag; device with `VK_KHR_portability_subset`.
- Surface from the winit window's `CAMetalLayer`.
- Load `libMoltenVK.dylib` (or the Vulkan loader) from the app bundle or the
  Vulkan SDK in development.
- The vendored `third_party/wgpu-hal` patch (an extra sampler descriptor for
  empty pools, an Intel Windows driver workaround) applies workspace-wide, so
  it also runs on MoltenVK. Confirm it is harmless there.
- Impeller `ContextVK` borrows the broker's device and queue as on Linux;
  confirm Impeller's required features and extensions under the portability
  subset.
- Validate: acquire→Impeller→present semaphore chain under validation layers,
  resize stress, clean teardown.

### M2: winit host (`platform/macos.rs`)

- Implement the existing `PlatformBackend` trait, using `windows.rs` as the
  template, and select it in `platform.rs`.
- Main window only for this milestone. Other window kinds are listed below.
- Keyboard: `FALLBACK_KEY_PLANE = 0x01400000000` (`macosPlane`), macOS legacy
  key encoder through `legacy_key_message` (`keymap: "macos"`, `keyCode`,
  `characters`, `charactersIgnoringModifiers`, macOS modifier bits),
  Cmd/Option mapping in `text_modifiers` and modifier reconciliation.
- Pointer and scroll, including trackpad pixel deltas and momentum phases.
- Resize and `scale_factor` changes (Retina, moving between displays),
  updating the `CAMetalLayer` drawable size.
- Lifecycle: app activation, window close, Cmd+Q.
- Vsync as decided above.

### M3: framework (Dart)

- `packages/flutter/lib/src/widgets/_window_rust.dart`: add
  `Platform.isMacOS` to the existing `isLinux || isWindows` check, keeping the
  symbol probe, so the Rust owner wins over the AppKit window controller.

### M4: run path

- Cargo runner linking `libflutter_rust_engine.dylib`, `@rpath` to the
  executable directory, assets and `icudtl.dat` beside it.
- Debug (JIT) first. Release AOT: verify Dart's ELF `app.so` loader works on
  macOS as it does on Windows, or fall back to a Mach-O `App` dylib.
- Ad-hoc code signing for local runs, including a bundled `libMoltenVK.dylib`.

## Milestones

1. **First frame**: the engine dylib builds on macOS arm64 with Impeller
   Vulkan, and a Cargo-linked counter app opens a window and renders through
   wgpu/MoltenVK. Exit: clean startup and shutdown, no validation errors on
   the happy path.
2. **Interactive**: pointer, scroll, keyboard, resize and HiDPI on the main
   window. The counter app and a text field (hardware keyboard) are usable.
   Exit: resize stress and clean shutdown under validation; Linux (and,
   where available, Windows) tests and fixtures still pass; build steps
   documented here and in the progress file; PR against
   `rust/flutter-rs-shell`.

## Out of scope for this delivery

- IME and full text-input conformance, plugins, wgpu/pixel-buffer textures and
  FRB dispatch parity.
- Dialog, satellite, popup and tooltip window kinds. The shared
  `positioner.rs` now exists for Linux and Windows, so these are cheaper than
  first estimated; reconsider after Milestone 2.
- Display-link vsync if the timer fallback is acceptable for the milestones.
- Native Metal backend (see decisions).
- `rlutter create/run/build macos`, CI jobs, release tarballs, notarization,
  Intel x64, iOS.
- Clipboard, cursors, menus, drag-and-drop, accessibility.

## Development host prerequisites

- Engine checkout synced with `gclient sync`; `xcode-select` pointing at
  `/Applications/Xcode.app` and the Xcode license accepted (otherwise the
  `darwin_sdk.py` gclient hook fails).
- Rust 1.93.1 (pinned in `rust-toolchain.toml`; `rustup` installs it on first
  use).
- Vulkan on macOS: LunarG Vulkan SDK, or Homebrew `molten-vk`,
  `vulkan-loader`, `vulkan-validationlayers` and `vulkan-tools`. Needed from
  M1.

## Risks

- **MoltenVK feature gaps** in Impeller (portability subset) may need
  engine-side workarounds.
- **Vendored wgpu-hal patch** runs on MoltenVK too and is untested there.
- **winit 0.31 beta AppKit backend** maturity and the feature set it needs.
- **Engine build time and disk** on the macOS host slow down iteration.
- **AppKit main-thread rules** may conflict with existing assumptions in
  `crates/flutter-shell-winit/src/shell.rs` (about 6.4k lines) about where
  windows and surfaces are touched.
- **Mach-O symbol export and snapshot lookup** differ from both ELF and PE;
  the static-link route is unverified on macOS.
- **Release AOT format** on macOS is unverified (see M4).
- **MoltenVK loading and signing** for local runs and, later, distribution.
- **Live resize**: AppKit's modal resize loop may stall `WaitUntil` deadlines,
  as with Win32. Confirm frames continue during window drag/resize.
