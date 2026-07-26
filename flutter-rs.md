# Flutter Rust Shell Plan

## Status

Proposed architecture for a Flutter engine fork in which Rust provides the
cross-platform application shell and platform integration layer.

This is intentionally **not** based on Flutter's public Embedder API. The Rust
shell is compiled in-tree with the engine and uses private engine interfaces in
lockstep with the Flutter revision.

## Goals

- Add an opt-in, shared Rust runtime as an alternative to the existing GTK,
  Win32, AppKit/UIKit-style Flutter application shells.
- Preserve the existing Flutter shells in the fork so applications can select
  either the upstream platform shell or the cross-platform Rust shell.
- Use `winit` for cross-platform windows, event loops, input, and lifecycle.
- Use `wgpu` as the application-facing GPU API and, eventually, as the owner of
  the native GPU device and presentation surface.
- Support platforms in this delivery order: Linux, Windows, Android, web, and
  then macOS/iOS.
- Be compatible with `flutter_rust_bridge` for application-level Dart/Rust
  calls.
- Give Rust plugins a well-known API for GPU textures, pixel buffers, video,
  GIS renderers, and 3D engines.
- Let plugins notify Flutter that a texture has a new frame without rebuilding
  the Dart widget tree.
- Generate Flutter projects without checked-in native platform directories.
- Keep the fork maintainable by isolating changes behind a small set of
  internal engine seams.

## Non-goals

- Do not remove, disable, or deprecate Flutter's existing platform shells. The
  Rust shell is an additional backend selected explicitly by an application.
- Do not build on or preserve compatibility with the public Flutter Embedder
  API.
- Do not rewrite the Dart VM, Flutter framework, rasterizer, or Impeller.
  Impeller remains Flutter's renderer permanently; wgpu complements it as the
  Rust plugin GPU API and native device/presentation owner.
- Do not promise a stable binary ABI for third-party Rust plugins. Rust plugins
  initially compile from source as Cargo dependencies.
- Do not pretend platform-specific behavior disappears. Code signing, IME,
  accessibility, menus, and other OS services remain platform-specific, but
  their implementations live centrally in the Rust runtime rather than in
  every application.

## Architectural conclusion

The Rust integration should be an opt-in, first-class internal Flutter platform
backend under `engine/src/flutter/shell/platform/rust/`. It lives alongside the
existing platform backends; selecting it for an application does not remove or
disable the upstream shells.

```text
Flutter framework / Dart application
          |
          +-- flutter_rust_bridge
          |
          v
Application Rust workspace: runner-rs/
  app logic, GPU plugins, generated FRB glue
          |
          v
Flutter Rust runtime, shipped by the Flutter fork
  plugin registry, GPU broker, platform services
  winit event loop, accessibility, text input
          |
          v
Small private C++/Rust bridge, versioned with the engine commit
          |
          +-- PlatformViewRust
          +-- RustRenderingSurface
          +-- VsyncWaiterRust
          +-- RustExternalTexture
          +-- RustPlatformMessageHandler
          |
          v
Flutter engine internals
  Shell, Engine, Rasterizer, Dart VM, Impeller
```

Flutter's internal
[`PlatformView`](engine/src/flutter/shell/common/platform_view.h) is the main
integration seam. It already covers rendering-surface creation, vsync, pointer
dispatch, viewport metrics, semantics, platform messages, texture registration,
lifecycle notifications, external view composition, and Impeller setup.

The new backend should implement that contract directly rather than routing
through the public Embedder API.

## Repository layout

Shared shell implementation in the Flutter fork:

```text
engine/src/flutter/shell/platform/rust/
  BUILD.gn

  bridge/
    platform_view_rust.h
    platform_view_rust.cc
    rust_rendering_surface.h
    rust_rendering_surface.cc
    rust_external_texture.h
    rust_external_texture.cc
    rust_platform_message_handler.h
    rust_platform_message_handler.cc

  crates/
    flutter-shell-core/
    flutter-shell-winit/
    flutter-shell-wgpu/
    flutter-shell-gpu-interop/
    flutter-shell-platform/
    flutter-shell-accessibility/
    flutter-shell-web/
    flutter-plugin-sdk/
```

Generated application:

```text
my_app/
  lib/
  test/
  assets/
  pubspec.yaml

  runner-rs/
    Cargo.toml
    Cargo.lock
    src/
      lib.rs
      api.rs
      plugins.rs
    crates/
      generated_frb/
```

`runner-rs/` contains application Rust code, plugin dependencies, and generated
FRB bindings. It must not contain a generated copy of the shared shell runtime.
The runtime belongs to the SDK fork so fixes and engine changes remain
centralized.

All Rust crates in the shell, plugin SDK, and generated `runner-rs/` workspace
use Rust edition 2024. The Flutter tool pins a concrete Rust toolchain that
supports that edition for reproducible application and engine builds.

## C++/Rust boundary

Rust should not directly inherit Flutter C++ types or consume arbitrary C++
headers through bindgen. Flutter's C++ types rely on templates, virtual
inheritance, `std::shared_ptr`, and thread-affinity rules that make such a
boundary unnecessarily fragile.

Instead:

- A small C++ `PlatformViewRust` subclasses `flutter::PlatformView`.
- C++ owns `Shell`, `Engine`, `Rasterizer`, Dart VM state, and Impeller objects.
- Rust owns the winit event loop, native windows, platform-service adapters,
  application plugin registry, and GPU broker.
- Cross-language values are plain C-compatible structures and opaque handles.
- The ABI uses explicit retain/release or unique ownership operations; borrowed
  Rust references never escape a call.
- Callbacks are posted to the correct task runner rather than invoked on an
  arbitrary caller thread.
- Headers may be produced with `cbindgen`; Rust declarations may be generated
  with `bindgen` or maintained by hand when small enough.
- The ABI is private and versioned with an exact Flutter engine revision. It is
  not a new public stability commitment.

This arrangement confines most upstream merge work to a narrow adapter.

### Who owns the final link

Two build topologies satisfy the boundary above, and they differ in which
build system produces the final executable:

- **GN owns the link (current phase 0 approach).** GN builds the C++ engine
  adapter as a `source_set` and a small `executable` with a `main()` that
  immediately calls into Rust; the Rust half is a `staticlib` that GN's build
  action compiles with Cargo and links in. Rust never has to know how to link
  Dart, Impeller, or Skia.
- **Cargo owns the link.** GN instead builds the same C++ adapter as a
  `shared_library` (the engine already does this for the embedder API and the
  GTK shell), and a real Cargo `bin` crate links against it dynamically via a
  `build.rs` that locates the GN output directory and emits the matching
  `rustc-link-lib`/`rustc-link-search`/rpath flags. `cargo run`/`cargo build`
  become the actual entry point instead of a ninja-built executable.

Phase 0 uses the first approach because it is the smallest change that proves
the seam: ninja already shells out to Cargo for the Rust archives, so one
extra `executable` target with a two-line `main.cc` is enough. The generated
application's `runner-rs/` (see Repository layout) implies the second model
eventually, since it is described as an ordinary Rust workspace, not a GN
target. Revisit this choice once tooling and project generation (see Tooling
and project generation) needs `runner-rs/` to feel like a normal Cargo
project; the phase 0 shim should not be read as the intended long-term shape.

## Threading and event loop

Winit's event loop owns the native main thread. The Rust shell assigns both
Flutter's platform task runner and UI task runner to that thread, so the root
Dart UI isolate, platform services, and winit event processing share one event
loop.

This follows Flutter's current threading direction: iOS and Android merge UI
and platform threads by default starting with Flutter 3.29, and macOS and
Windows do so starting with Flutter 3.35. The Rust shell should use the merged
model consistently across its native targets, including Linux, rather than
preserving the older split-thread topology.

| Thread | Owner | Responsibilities |
| --- | --- | --- |
| Main/UI/platform | Rust/winit and Flutter engine | Events, windows, lifecycle, platform tasks, root Dart UI isolate, and frame construction |
| Raster | Flutter engine | Impeller rasterization and Flutter texture painting |
| IO | Flutter engine | Assets and renderer upload support |
| Rust workers | Rust runtime | Plugins, media, GIS, networking, computation |

The merged main task-runner adapter must:

- Supply the same underlying task runner for Flutter UI and platform work.
- Wake winit through `EventLoopProxy` when either runner posts work.
- Track the next Flutter task deadline and select the matching winit wait mode.
- Drain due Flutter tasks before returning to sleep.
- Preserve Flutter's UI-thread and platform-thread checks on their shared
  thread.
- Marshal Rust plugin events to the merged main thread or raster thread
  explicitly.
- Support clean shutdown without callbacks reaching destroyed Rust state.

The merged topology makes root-isolate Dart FFI calls useful for main-thread-only
native APIs: a synchronous call originating on that isolate normally enters
Rust on the winit thread. It does not make arbitrary FFI calls main-thread-safe;
background Dart isolates and Rust workers still require an explicit main-thread
dispatcher.

Because winit events, platform work, and Dart UI work now share a thread, all
synchronous Dart-to-Rust calls on that thread must be short and non-blocking.
Media decoding, GIS processing, networking, shader preparation, and other
expensive work must move immediately to Rust workers and post only their result
back to the main thread. Rust must also avoid calling back into Dart while
holding locks, because platform messages and FFI can otherwise create reentrant
deadlocks.

Flutter's raster and IO runners remain separate. Replacing FML scheduling with
a Rust executor is a separate project and provides little value to the first
shell.

## GPU architecture

### Destination: wgpu-owned device and presentation

The desired final ownership model is:

```text
Rust GPU broker
  owns wgpu Instance, Adapter, Device, Queue, Surface
          |
          +-- safe wgpu access for Rust plugins
          |
          +-- private native-handle access
                     |
                     v
                 Impeller context
```

Backend selection for the first native implementation:

| Platform | Shared native backend |
| --- | --- |
| Linux | Vulkan |
| Windows | Vulkan |
| Android | Vulkan |
| Web | WebGPU, with a separate web integration path |
| macOS/iOS | Metal |

Impeller already has useful internal support for externally supplied devices:

- [`ContextVK::EmbedderData`](engine/src/flutter/impeller/renderer/backend/vulkan/context_vk.h)
  accepts a Vulkan instance, physical device, device, queue, and queue family.
- [`ContextMTL::Create`](engine/src/flutter/impeller/renderer/backend/metal/context_mtl.h)
  can accept an existing `MTLDevice` and `MTLCommandQueue`.

The Vulkan type is internal despite its current `EmbedderData` name. The fork
should refactor it into a general `ExternalDeviceData` abstraction, make
ownership explicit, and remove assumptions tied to the public Embedder API.

The Rust shell acquires a wgpu surface image, exposes its native image/texture
to the internal Flutter rendering surface, lets Impeller rasterize into it, and
then presents through the Rust GPU broker.

### Bootstrap option

If wgpu-owned presentation blocks the first prototype, Impeller may temporarily
own the Metal/Vulkan device and Rust may wrap it using `wgpu-hal`. This is a
bootstrap path for GPU device ownership only. Impeller remains the Flutter
renderer in both ownership models.

### Impeller is the permanent Flutter renderer

The Rust shell will not implement an Impeller backend on top of wgpu and will
not replace Impeller with a Rust renderer. Impeller continues to rasterize
Flutter content. Wgpu provides the shared native GPU infrastructure and the API
used by Rust texture producers, video renderers, GIS renderers, and 3D engines.
The interop layer composes those two permanent responsibilities.

### GPU interop broker

Safe `wgpu` does not expose all native handles and synchronization operations
needed to share resources with Impeller. A small, version-pinned module built on
`wgpu-hal` is therefore expected:

```text
flutter-shell-gpu-interop/
  device ownership
  queue serialization
  surface acquisition and presentation
  image and texture state transitions
  fences or timeline semaphores
  Impeller native-handle conversion
  context-loss handling
```

This should be the only unsafe GPU component. Plugins receive safe wgpu APIs
and cannot manipulate native handles or Impeller objects.

The interop contract must explicitly define:

- Who owns and destroys each device, queue, surface, and image.
- Which queue last wrote each texture.
- Which layout/state Impeller receives and must return.
- How completion is signaled between wgpu and Impeller work.
- What happens during resize, device loss, application suspension, and engine
  teardown.

GPU synchronization is the highest-risk part of the project and should be
validated before broad platform work.

## Rust plugin and texture model

Use Flutter's existing internal
[`Texture`](engine/src/flutter/common/graphics/texture.h) and texture registry
rather than creating a parallel Dart compositor protocol.

The C++ bridge implements `RustExternalTexture`, which obtains the latest
plugin-produced image and paints it on the raster thread. Flutter's existing
texture-frame notification path already marks a texture dirty and schedules a
new frame without rebuilding the Dart widget tree; see
[`Shell::OnPlatformViewMarkTextureFrameAvailable`](engine/src/flutter/shell/common/shell.cc).

The Rust-facing API should be source-stable and capability-oriented:

```rust
pub trait FlutterRustPlugin: Send + Sync + 'static {
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<()>;
}

pub trait GpuTextureProducer: Send + 'static {
    fn descriptor(&self) -> TextureDescriptor;

    fn render(
        &mut self,
        frame: &mut TextureFrame<'_>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<()>;
}
```

Typical plugin usage:

```rust
let texture = registrar.gpu().create_texture(descriptor)?;

texture.render(|encoder, view| {
    renderer.draw(encoder, view);
})?;

texture.present()?;
```

`present()` performs the hard lifting:

1. Complete or submit the plugin's GPU work.
2. Record the required cross-renderer synchronization.
3. Rotate an engine-owned texture ring.
4. Mark the Flutter texture as having a new frame.
5. Schedule a Flutter frame without asking Dart to rebuild.

Prefer engine-owned double- or triple-buffered textures. This provides a clear
lifetime model and makes resize, context loss, and synchronization safer.

Provide three texture capability levels:

1. `PixelBufferTexture`: CPU pixel data for simple and portable producers.
2. `WgpuTexture`: an engine-owned GPU texture rendered through safe wgpu APIs.
3. `NativeImportedTexture`: advanced zero-copy import for video decoders and
   external graphics APIs.

Rust plugins initially compile together as Cargo dependencies. A binary plugin
system would require a separate, stable C ABI and can be considered later.

## flutter-plugin-sdk

`flutter-plugin-sdk` is the public, semantically versioned Rust compatibility
boundary between plugins and the private shell runtime. Cargo, rather than a
second shell-specific version field, resolves plugin API compatibility.

```text
Plugin crate
    |
    v
flutter-plugin-sdk       Public and semantically versioned
    |
    v
flutter-shell-core       Private and tied to an engine commit
    |
    v
C++/Rust engine bridge   Private and tied to an engine commit
```

The SDK exposes source-level Rust APIs such as:

- `PluginRegistrar` and `FlutterRustPlugin`.
- Main-thread dispatch.
- Lifecycle and platform-service handles.
- Pixel-buffer and GPU texture creation.
- Frame notification and texture presentation.
- Logging and common error types.
- FRB registration helpers.
- The shell-compatible wgpu API.

It must not expose Flutter C++ objects, Impeller objects, raw engine FFI, or
private shell-runtime types.

A plugin declares an ordinary Cargo dependency:

```toml
[dependencies]
flutter-plugin-sdk = "1.4"
flutter-rust-bridge = "..."
```

The generated application aggregation crate pins the exact SDK version bundled
with the selected Flutter fork:

```toml
[dependencies]
flutter-plugin-sdk = "=1.7.2"

[patch.crates-io]
flutter-plugin-sdk = { path = "/flutter-sdk/engine/src/flutter/shell/platform/rust/crates/flutter-plugin-sdk" }
```

Plugins may use compatible Cargo ranges such as `"1"` or `">=1.6, <2"`.
The path patch guarantees that the resolved implementation matches the engine
artifact. Since Cargo can otherwise place incompatible major versions of a
crate in the same graph, the Flutter tool must inspect `cargo metadata` and
require exactly one resolved `flutter-plugin-sdk` package. This is validation
of Cargo's result, not a second versioning system.

The SDK re-exports the wgpu version used by the shell:

```rust
use flutter_plugin_sdk::gpu::wgpu;
```

All types crossing the plugin boundary must use that re-export. A plugin may
use another wgpu version internally, but it cannot pass those types to the
shell. This prevents duplicate-wgpu type mismatches in `Device`, `Queue`,
`Texture`, and `TextureView` APIs.

## Adding Dart/FRB packages with Rust plugins

For a package containing both Dart and Rust, the Pub dependency is the source
of truth. An application adds it once:

```sh
flutter pub add rust_video_player
```

No corresponding manual `cargo add` is required in `runner-rs/`.

A Rust-shell-aware package follows a directory convention:

```text
rust_video_player/
  pubspec.yaml
  lib/
    rust_video_player.dart
    src/frb_generated.dart
  rust/
    Cargo.toml
    src/
      lib.rs
      frb_generated.rs
```

Its Rust manifest identifies the standard registrar using Cargo metadata:

```toml
[package]
name = "rust_video_player_rs"

[package.metadata.flutter]
plugin = true
registrar = "register"

[dependencies]
flutter-plugin-sdk = "1"
```

Package authors generate and publish both halves of the FRB glue. Application
builds do not modify packages in the Pub cache.

After Pub resolves the Dart graph, the Flutter tool scans the resolved package
roots for `rust/Cargo.toml`, reads their Cargo metadata, and generates one
aggregation crate:

```text
.dart_tool/flutter_rs/runner/
  Cargo.toml
  src/lib.rs
```

Its generated dependency graph conceptually contains:

```toml
[lib]
crate-type = ["staticlib", "cdylib"]

[dependencies]
app_runner = { path = "../../../runner-rs" }
rust_video_player_rs = {
  path = "/resolved/pub-cache/rust_video_player-2.1.0/rust"
}
```

Registration is generated explicitly:

```rust
pub fn register_application(registrar: &mut PluginRegistrar) -> Result<()> {
    app_runner::register(registrar)?;
    rust_video_player_rs::register(registrar)?;
    Ok(())
}
```

Explicit calls behave predictably with iOS static linking, WebAssembly,
tree-shaking, and dead-code elimination. They are preferred over linker-based
discovery.

The aggregation crate compiles the application and all Rust-shell plugins into
one artifact. This ensures they share the shell's wgpu device, queue, plugin SDK
types, and lifecycle. A conventional FRB package that builds its own Native
Asset remains usable as an independent library, but it does not receive the
shell registrar or shared GPU services unless it adopts this source-plugin
convention.

## flutter_rust_bridge integration

FRB remains the application control-plane bridge:

```text
Dart --FRB--> application/plugin Rust code --registrar--> Rust shell runtime
```

FRB is appropriate for commands, state, streams, and application data. It
should not carry video frames, GPU handles, or per-frame texture notifications.
Those stay within the native Rust/engine runtime.

Synchronous FRB/FFI calls from the root Dart UI isolate execute on the merged
winit/UI/platform thread and must finish quickly. Long-running Rust operations
use workers and return through asynchronous FRB responses or streams. Calls
originating from background Dart isolates must use the shell's main-thread
dispatcher before touching windows or other main-thread-only platform APIs.
Rust callbacks must not re-enter Dart while holding application or plugin
locks.

The application Rust artifact exposes:

- FRB-generated symbols.
- A well-known `flutter_rust_app_init` entry point.
- Plugin registration metadata.
- Shutdown, suspension, resume, and context-loss hooks as needed.

On native targets the crate can be statically linked into the final application
artifact. On web it is compiled into the generated Wasm module.

## Platform services

Within the optional Rust backend, winit provides most window and event-loop
shell code, but it does not abstract every operating-system service. Define
shared Rust traits such as:

```rust
pub trait PlatformServices {
    fn clipboard(&self) -> &dyn Clipboard;
    fn text_input(&self) -> &dyn TextInput;
    fn accessibility(&self) -> &dyn Accessibility;
    fn menus(&self) -> &dyn Menus;
    fn lifecycle(&self) -> &dyn Lifecycle;
}
```

Likely implementation crates:

- `windows-rs` on Windows.
- Wayland/X11 ecosystem crates on Linux.
- `android-activity` and `jni` on Android.
- `objc2` on macOS and iOS.
- `web-sys` on web.
- AccessKit adapters where they meet Flutter's semantics requirements.

Platform-specific Rust modules will still be required for:

- IME and editable-text integration.
- Accessibility and semantics translation.
- Clipboard and drag-and-drop.
- Menus, cursors, and window decoration behavior.
- Application lifecycle and deep links.
- Display-link/vsync integration where winit is insufficient.
- Code signing, entitlements, permissions, and application metadata.
- Native platform views, if supported.

The benefit is not zero platform code. It is one coherent Rust backend with a
shared interface and centrally maintained OS adapters.

## Web architecture

Flutter Web does not run the native C++ `Shell`/`PlatformView` architecture, so
`PlatformViewRust` cannot simply be compiled to Wasm.

The product-level API should be common while the engine integration is split:

```text
Native:
  Rust runtime -> PlatformViewRust -> C++ engine and Impeller

Web:
  Rust Wasm runtime -> private web_ui adapter -> Flutter web renderer
```

The Flutter web engine currently lacks a completed native-style texture path;
[`TextureLayer.addTexture`](engine/src/flutter/lib/web_ui/lib/src/engine/layer/layer_scene_builder.dart)
is the relevant starting point in this fork.

Implement web in two stages:

1. Render Rust output into a managed canvas or `OffscreenCanvas` and compose it
   as a Flutter web scene layer.
2. Coordinate a shared WebGPU device with the Flutter web renderer for direct
   GPU texture composition.

WebGPU textures cannot be shared between unrelated WebGPU devices, so device
ownership and scene composition must be designed together. The Dart-facing
`RustTexture` or `RustSurface` API should hide the native/web difference.

## Tooling and project generation

Rust is a shell selection, not a deployment platform. Prefer:

```sh
flutter create --shell=rust \
  --platforms=linux,windows,android,web,macos,ios my_app
```

over `--platform=rust`.

The generated project contains `runner-rs/` but no checked-in `linux/`,
`windows/`, `android/`, `web/`, `macos/`, or `ios/` directories. Platform
packaging projects are generated into build output when needed:

```text
.dart_tool/flutter_build/rust_shell/linux/
.dart_tool/flutter_build/rust_shell/windows/
.dart_tool/flutter_build/rust_shell/android/
.dart_tool/flutter_build/rust_shell/web/
.dart_tool/flutter_build/rust_shell/macos/
.dart_tool/flutter_build/rust_shell/ios/
```

Android still requires Gradle packaging, a manifest, signing configuration, and
Android resources. Apple platforms still require Xcode packaging, signing,
plist data, and entitlements. These become SDK-managed templates configured
from `pubspec.yaml` or a small `rust_shell.yaml`, not source directories users
must maintain.

The Flutter tool needs commands or build targets for:

- Cargo dependency resolution.
- Discovering Rust-shell plugins in the resolved Pub package graph.
- Generating the application/plugin Cargo aggregation crate.
- Pinning the bundled `flutter-plugin-sdk` through a Cargo path patch.
- Validating with `cargo metadata` that one compatible plugin SDK version was
  resolved.
- FRB code generation.
- Native and Wasm Rust compilation.
- Linking the application crate into the Rust shell artifact.
- Generating transient platform packaging projects.
- Hot restart and debug-symbol handling.
- Packaging Cargo assets and native plugin resources.
- Producing deterministic diagnostics when Rust toolchains are missing.

## Fork maintenance strategy

- Keep the implementation additive under `shell/platform/rust` wherever
  possible.
- Refactor generic internal GPU interfaces only when the Rust backend needs
  them; avoid broad unrelated engine changes.
- Pin Rust, winit, wgpu, wgpu-hal, and FRB revisions in the engine dependency
  configuration and lockfiles.
- Publish `flutter-plugin-sdk` with semantic versioning while keeping its
  implementation matched to the copy bundled by the Flutter fork.
- Treat the C++/Rust bridge as lockstep with the engine commit.
- Maintain conformance tests comparing official and Rust shells for input,
  lifecycle, viewport metrics, semantics, platform messages, and frame output.
- Run upstream rebase/merge CI frequently so internal interface changes are
  discovered early.
- Keep native/web differences behind the same Dart and application-Rust APIs.
- Avoid new Flutter framework APIs where the existing `Texture` widget and
  platform-channel conventions are sufficient.

## Cross-cutting application tooling and plugin SDK

This work begins after the Linux architecture is proven and continues as each
platform is added:

- Generate `runner-rs/` projects.
- Integrate FRB generation and build hooks.
- Publish the semantically versioned `flutter-plugin-sdk` crate.
- Discover package-local Rust crates from the Pub dependency graph.
- Generate the Cargo aggregation crate and explicit plugin registration.
- Add SDK path pinning and single-version `cargo metadata` validation.
- Add pixel-buffer, wgpu-texture, video, GIS, and 3D example plugins.
- Add `flutter create --shell=rust` and corresponding run/build/test support.

## Delivery phases

Platform enablement is intentionally ordered as Linux, Windows, Android, web,
and finally macOS/iOS.

### Phase 0: prove the internal seam

- Add the Rust shell GN target.
- Implement the minimum `PlatformViewRust` C++ adapter.
- Boot a basic Flutter application on Linux.
- Render Flutter content with Impeller from the first working shell.

Exit condition: a Flutter counter application runs through the Rust backend,
without the public Embedder API or routing through the GTK shell. The regular
GTK shell remains available as a separate build target.

The native runner built here is a GN executable with a two-line `main.cc` that
calls straight into Rust (see "Who owns the final link" under C++/Rust
boundary). That is a phase 0 expedient to avoid teaching Cargo how to link the
engine; it is not the tooling model application authors will see.

### Phase 1: winit platform host

- Make winit own the main thread and native window.
- Supply the same winit-backed task runner for Flutter UI and platform work.
- Implement merged-runner wake/deadline integration.
- Add resize, viewport metrics, pointer, keyboard, lifecycle, and basic vsync.
- Add main-thread dispatch for background isolate and Rust-worker callbacks.
- Test synchronous FFI reentrancy and main-thread starvation behavior.
- Add deterministic startup and shutdown ownership tests.

Exit condition: normal Flutter interaction and resize work through winit, the
root Dart isolate and platform callbacks run on the winit thread, and expensive
Rust work cannot block that thread.

### Phase 2: shared GPU and texture proof

- Create a wgpu-owned Vulkan device and presentation surface on Linux.
- Construct Impeller from the externally supplied device and queue.
- Implement queue and resource synchronization in the GPU interop broker.
- Implement `RustExternalTexture` and an animated wgpu texture plugin.
- Verify context loss, resize, texture unregister, and teardown.

Exit condition: a Flutter `Texture` widget displays continuously animated wgpu
content without Dart rebuilds and without CPU readback.

### Phase 3: Windows

- Add Windows using the Vulkan path.
- Add Windows clipboard, cursors, drag-and-drop, window state, and menus.
- Establish accessibility and IME conformance suites.
- Generate transient Windows packaging and application metadata.

### Phase 4: Android

- Integrate winit with Android activity, window, and surface lifecycles.
- Add the Vulkan shared-device and Impeller presentation path.
- Complete touch, keyboard, IME, semantics, suspension, surface recreation,
  and memory-pressure behavior.
- Generate transient Gradle packaging, Android manifest, resources, and signing
  configuration.

### Phase 5: web

- Add the Rust Wasm runtime.
- Implement the `web_ui` Rust surface/texture layer.
- Start with canvas composition.
- Investigate and then implement shared-WebGPU-device composition.

### Phase 6: macOS and iOS

- Add the shared Metal device and Impeller presentation path.
- Integrate macOS winit windows, lifecycle, input, menus, accessibility, and
  IME.
- Integrate iOS winit lifecycle with UIKit requirements.
- Complete iOS touch, keyboard, IME, semantics, display link, suspension, and
  memory-pressure behavior.
- Generate transient Xcode packaging, plist, entitlements, and signing
  configuration for both platforms.

## Primary risks

1. **Wgpu/Impeller synchronization**: queue ownership, image state, fences, and
   surface presentation must be correct across two renderer abstractions.
2. **Windows GPU backend choice**: using Vulkan avoids an Impeller D3D12
   backend but narrows the driver strategy and may need a fallback.
3. **IME and accessibility**: winit is not a complete cross-platform solution,
   especially on Android, iOS, and web.
4. **Android lifecycle and packaging**: activity recreation, surface loss,
   backgrounding, Gradle packaging, and application resources need dedicated
   Rust-shell integration despite the shared winit architecture.
5. **Web divergence**: web needs a sibling `web_ui` integration rather than the
   native shell implementation.
6. **Apple packaging**: native project folders can be hidden from source control
   but Xcode and signing cannot be removed from the build pipeline.
7. **Unsafe wgpu-hal coupling**: native GPU interop will track pinned wgpu
   internals and must have extensive validation tests.
8. **Merged-thread starvation and reentrancy**: a blocking FFI/plugin call now
   stalls Dart UI work, native event processing, and platform services at once;
   callbacks can also deadlock if Rust re-enters Dart while holding locks.
9. **Fork maintenance**: private engine interfaces change, so upstream merge CI
   and a narrow patch surface are mandatory.

## First decisive milestone

The first milestone that validates the architecture is:

> A Linux Flutter application whose window and event loop are owned by winit,
> whose Vulkan device and surface are owned by the Rust GPU broker, and whose
> Flutter `Texture` widget displays animated wgpu-rendered content without CPU
> readback, Dart rebuilds, routing through GTK, or the public Embedder API. The
> existing GTK shell remains intact as an alternative target.

If this succeeds, the core architecture is sound. If it fails, the likely cause
will be the wgpu/Impeller device and synchronization boundary, which is why that
boundary should be tested before investing heavily in tooling or additional
platforms.
