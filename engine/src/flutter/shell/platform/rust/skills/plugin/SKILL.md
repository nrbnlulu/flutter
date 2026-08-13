---
name: plugin
description: Create or modify source-linked flutter_rust_bridge plugins for this Flutter Rust shell, including plugin registration, FRB control APIs, engine-owned wgpu textures, CPU pixel-buffer textures, Dart Texture widgets, frame production, lifecycle, and validation. Use when implementing a Dart/Rust package that must obtain shared GPU services from flutter-plugin-sdk or publish GPU/pixel frames to Flutter without Dart rebuilds or CPU readback.
---

# Build an FRB texture plugin

Keep the boundary honest:

```text
Dart --FRB control calls--> Rust plugin
                            |
                            +-- WgpuTexture: record GPU commands
                            +-- PixelBufferTexture: write shell-owned RGBA8 memory
                                         |
                                         v
                              Flutter Texture(textureId: id)
```

Use FRB for commands, IDs, state, and streams. Never send frames, GPU handles,
or per-frame notifications through FRB. The shell owns the wgpu device, queue,
texture rings, synchronization, uploads, and Flutter texture registration.

## Inspect the live contracts first

Before editing a plugin, read:

- `engine/src/flutter/shell/platform/rust/crates/flutter-plugin-sdk/src/lib.rs`
- `engine/src/flutter/shell/platform/rust/tests/frb_fixture/`
- The `DemoTexturePlugin` and frame-production code in
  `engine/src/flutter/shell/platform/rust/crates/flutter-shell-winit/src/lib.rs`

Treat those files as authoritative. Do not reproduce private shell traits or
call the C++ ABI from a plugin.

## Create the package

Use this layout:

```text
my_plugin/
  pubspec.yaml
  flutter_rust_bridge.yaml
  lib/
    my_plugin.dart
    src/rust/                 # generated Dart bindings
  rust/
    Cargo.toml
    src/
      api.rs
      frb_generated.rs       # generated Rust bindings
      lib.rs
```

Declare the registrar in `rust/Cargo.toml`:

```toml
[package]
name = "my_plugin_rs"
edition = "2024"

[package.metadata.flutter]
plugin = true
registrar = "register"

[dependencies]
flutter-plugin-sdk = "1"
flutter_rust_bridge = "=2.12.0"
```

During in-tree development, use a path dependency on the current SDK. Let the
generated application aggregate pin/patch the SDK in production. Require
`cargo metadata` to resolve exactly one `flutter-plugin-sdk` package. Keep the
fixture/application lockfile checked in and use `--locked` in validation.

The SDK exposes its pinned wgpu crate as `flutter_plugin_sdk::gpu::wgpu`.
Always use that re-export for descriptors, constants, and values passed through
SDK callbacks. Do not add a direct `wgpu` dependency: two sources or versions
produce incompatible Rust types and can silently duplicate the dependency.

Configure FRB to generate both halves from published Rust API modules. Do not
edit generated files manually. Use the checked-in FRB fixture configuration as
the version-matched starting point.

## Register shell capabilities

Implement the public plugin trait and expose the manifest-named function:

```rust
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}};

use flutter_plugin_sdk::{
    gpu::wgpu,
    FlutterRustPlugin, GpuTextures, MainThreadDispatcher, PixelBufferTexture,
    PluginError, PluginRegistrar, Result, TextureDescriptor, TextureFormat,
    WgpuTexture,
};

struct PluginState {
    gpu: GpuTextures,
    dispatcher: MainThreadDispatcher,
    gpu_texture_id: i64,
    pixel_texture_id: i64,
    gpu_textures: Mutex<HashMap<i64, Arc<WgpuTexture>>>,
    pixel_textures: Mutex<HashMap<i64, Arc<PixelBufferTexture>>>,
}

static STATE: OnceLock<Arc<PluginState>> = OnceLock::new();

pub struct MyPlugin;

impl FlutterRustPlugin for MyPlugin {
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<()> {
        let gpu = registrar.gpu()?.clone();
        let gpu_texture = Arc::new(gpu.create_texture(descriptor(256, 256))?);
        let pixel_texture = Arc::new(
            gpu.create_pixel_buffer_texture(descriptor(256, 256))?,
        );
        let gpu_texture_id = gpu_texture.texture_id();
        let pixel_texture_id = pixel_texture.texture_id();
        STATE
            .set(Arc::new(PluginState {
                gpu,
                dispatcher: registrar.main_thread_dispatcher().clone(),
                gpu_texture_id,
                pixel_texture_id,
                gpu_textures: Mutex::new(HashMap::from([(
                    gpu_texture_id,
                    gpu_texture,
                )])),
                pixel_textures: Mutex::new(HashMap::from([(
                    pixel_texture_id,
                    pixel_texture,
                )])),
            }))
            .map_err(|_| PluginError::Unsupported)
    }
}

pub fn register(registrar: &mut PluginRegistrar) -> Result<()> {
    MyPlugin.register(registrar)
}

fn descriptor(width: u32, height: u32) -> TextureDescriptor {
    TextureDescriptor {
        width,
        height,
        format: TextureFormat::Rgba8Unorm,
    }
}
```

Registration runs once on winit's owning thread after the engine, implicit
view, dispatcher, and GPU capability exist. Retain only public SDK handles.
Never retain `&PluginRegistrar` or any borrowed capability.

Texture creation is currently main-thread-only. Prefer creating initial
textures during `register`. For dynamic creation requested by a normal FRB
worker call, post creation through the retained `MainThreadDispatcher` and
return the result asynchronously. Never synchronously wait for dispatched work
from a root-isolate synchronous FRB call: Dart and winit share that thread and
would deadlock.

Call creation through the capability, not through the registrar:

```rust
let gpu = registrar.gpu()?;
let texture = gpu.create_pixel_buffer_texture(descriptor)?;
```

`PluginRegistrar` is a temporary registration context. `GpuTextures` groups an
optional shell capability, reports `Unsupported` when unavailable, and can be
cloned for later main-thread operations. Do not add convenience creation
methods directly to `PluginRegistrar` or retain a registrar reference.

## Create and expose texture IDs

Create each handle through `GpuTextures`, store it for its full Dart-visible
lifetime, and return only its integer ID through FRB. For the registered state
above, the FRB getters can remain synchronous because they only read IDs:

```rust
#[flutter_rust_bridge::frb(sync)]
pub fn gpu_texture_id() -> i64 {
    STATE.get().expect("plugin not registered").gpu_texture_id
}

#[flutter_rust_bridge::frb(sync)]
pub fn pixel_texture_id() -> i64 {
    STATE.get().expect("plugin not registered").pixel_texture_id
}
```

Map `PluginError` to the plugin's FRB-facing error type rather than exposing
private runtime errors directly. Keep synchronous FRB getters small; returning
a stored texture ID is appropriate, creating a texture or waiting for a frame
is not.

In Dart, initialize FRB and use the returned ID:

```dart
await RustLib.init(/* application loader */);
final int gpuId = api.gpuTextureId();
final int pixelId = api.pixelTextureId();

Row(children: [
  Expanded(child: Texture(textureId: gpuId)),
  Expanded(child: Texture(textureId: pixelId)),
])
```

Flutter repaints an existing `Texture` layer when Rust calls `present`; Dart
does not rebuild per frame. Rebuild only when switching to a different ID.
Initialize FRB separately inside every Dart isolate that calls the plugin.

FRB maps Rust `i64` texture IDs to its platform-integer Dart type and Rust
`u64` counters to `BigInt`. Convert a `BigInt` with `.toInt()` before passing it
to `jsonEncode`; do not assume every generated numeric field is a Dart `int`.

## Mount textures before producing frames

Do not begin a producer during `register`. If all three ring slots are
published before Flutter installs a `TextureLayer`, nothing consumes them and
`try_next_frame` remains `Busy`. Use this startup order:

1. Create and retain texture handles during plugin registration.
2. Return their IDs through a small synchronous FRB getter.
3. Build the Dart `Texture` widgets.
4. After the first Dart frame, call an idempotent FRB `start` API.
5. Start the Rust worker and treat `Busy` as skipped-frame backpressure.

```dart
class TextureViewState extends State<TextureView> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      api.startTextureProducer();
    });
  }
}
```

For replacement, use a stronger handshake: publish the new ID to Dart, wait
until Dart acknowledges after `endOfFrame`, then produce into the new ring.
Keep `start` idempotent because widgets and isolates may retry control calls.

## Produce wgpu frames

Use `try_next_frame` for a timer/render loop that can skip frames. Use
`next_frame().await` when the producer should sleep until a slot is released.
Treat `PluginError::Busy` as backpressure, not failure.

```rust
fn render_clear(texture: &WgpuTexture, color: wgpu::Color) -> Result<bool> {
    let mut frame = match texture.try_next_frame() {
        Ok(frame) => frame,
        Err(PluginError::Busy) => return Ok(false),
        Err(error) => return Err(error),
    };

    frame.render(move |_, encoder, view| {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("my plugin texture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    })?;
    frame.present()?;
    Ok(true)
}
```

The closure receives `Device`, `CommandEncoder`, and `TextureView`, but no
`Queue`. Record commands only. Never submit, present, transition raw Vulkan
images, or create a second device/queue for the shared texture. The shell
serializes wgpu and Impeller submissions.

A frame reservation records at most once and `present(self)` consumes it.
Dropping an unpublished reservation returns its slot. Do not retain the view,
encoder, or other borrowed callback values.

## Produce pixel-buffer frames

Write directly into the shell allocation and honor the supplied row stride:

```rust
fn render_pixels(
    texture: &PixelBufferTexture,
    width: usize,
    height: usize,
    rgba: [u8; 4],
) -> Result<bool> {
    let mut frame = match texture.try_next_frame() {
        Ok(frame) => frame,
        Err(PluginError::Busy) => return Ok(false),
        Err(error) => return Err(error),
    };

    frame.write_pixels(move |pixels, row_bytes| {
        for row in pixels.chunks_mut(row_bytes).take(height) {
            for pixel in row[..width * 4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&rgba);
            }
        }
    })?;
    frame.present()?;
    Ok(true)
}
```

The format is tightly interpreted RGBA8, but `row_bytes` remains authoritative.
Do not retain the slice. Do not allocate a second full frame merely to copy it
into the callback; generate or decode directly into shell-owned memory when
possible. The shell performs the unavoidable CPU-to-GPU upload.

## Handle concurrency and lifecycle

- Keep application/plugin state `Send + Sync`; FRB normal calls run on workers.
- Initialize the generated FRB library inside each isolate that calls Rust.
  Let only the root isolate own and build Flutter `Texture` widgets; background
  isolates may issue controls and read the same texture IDs.
- Keep mutex guards out of `dispatch`, `render`, `write_pixels`, `present`, and
  all Rust-to-Dart response paths.
- Use `MainThreadDispatcher` only for short main-thread-only operations.
- Never block the winit thread waiting for a worker, FRB response, or dispatch.
- Stop producer tasks before removing handles.
- Drop the last texture handle to request asynchronous unregister and resource
  reclamation. There is currently no completion notification for unregister.
  Treat `PluginError::Shutdown` as terminal.
- Expect context destruction/recreation; publish fresh frames after recreation
  rather than caching private GPU objects outside SDK handles.

Treat the `OnceLock` registration shown above as process-lifetime state. It
does not support unloading and registering the same plugin again in one
process. The SDK does not currently provide a plugin-unregister lifecycle
callback, so expose an explicit FRB shutdown API (and call it from the
application's shutdown path) when the plugin owns producer threads or tasks.
That API must stop and join producers before dropping the texture handles.

If resize or replacement requires a new texture, asynchronously dispatch its
creation to the main thread, publish the new ID only after creation completes,
rebuild the Dart `Texture` with that ID, then drop the old handle. Choose a
oneshot/promise mechanism appropriate to the application's async runtime; the
SDK deliberately does not prescribe one.

## Aggregate and validate

Generate explicit application registration; do not use linker discovery:

```rust
pub fn register_application(registrar: &mut PluginRegistrar) -> Result<()> {
    app_runner::register(registrar)?;
    my_plugin_rs::register(registrar)?;
    Ok(())
}
```

Validate in this order:

1. Run FRB generation, Dart format, and Rust format.
2. Run `cargo metadata` and `cargo tree -i wgpu`; confirm exactly one
   `flutter-plugin-sdk` and one SDK-pinned wgpu package resolve. The plugin
   manifest must not contain a direct wgpu dependency.
3. Run Cargo tests with the lockfile and build the Flutter bundle.
4. Launch through the Cargo-owned Rust-shell runner, not GTK or the public
   Embedder API.
5. Display both returned IDs in real Dart `Texture` widgets, start production
   after mounting them, and prove pixels change without Dart rebuilds. Capture
   the GPU and pixel-buffer regions separately; counters alone prove calls to
   `present`, not successful Flutter sampling.
6. Exercise `Busy`, dropped reservations, handle removal, context recreation,
   resize, and shutdown under Vulkan validation.
7. For background-isolate APIs, verify FRB worker execution dispatches required
   operations to winit and exits without pending callbacks or deadlock.

Use `task test-rust-shell-frb-dispatch`, `task test-rust-shell-texture`,
`task test-rust-shell-pixel-buffer-texture`, and
`task test-rust-shell-texture-lifecycle` as reference validation workflows.
