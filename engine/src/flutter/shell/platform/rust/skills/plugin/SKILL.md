---
name: plugin
description: Create or modify a Dart/Rust plugin for the Flutter Rust shell, including flutter_rust_bridge APIs, automatic registration, engine-owned wgpu textures, CPU pixel-buffer textures, and safe lifecycle handling. Use for plugins consumed by an application created with this fork's Rust shell.
---

# Create a Flutter Rust-shell plugin

A Rust-shell plugin is a Dart package with a Rust crate in `rust/`. The Dart
side exposes the Flutter API, `flutter_rust_bridge` carries control calls and
values, and the generated application compiles and registers the Rust crate.

## Create the package

Use the fork's Flutter command to create a Dart package:

```sh
fvm spawn nrbnlulu/BETA create --template=package my_plugin
cd my_plugin
mkdir -p rust/src
```

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

Add the matching FRB runtime to `pubspec.yaml`:

```yaml
dependencies:
  flutter:
    sdk: flutter
  flutter_rust_bridge: 2.12.0
```

Configure binding generation:

```yaml
# flutter_rust_bridge.yaml
rust_input: crate::api
rust_root: rust
rust_output: rust/src/frb_generated.rs
dart_output: lib/src/rust
dart_entrypoint_class_name: RustLib
web: false
dart_format: true
rust_format: true
```

Create `rust/Cargo.toml`:

```toml
[package]
name = "my_plugin_rs"
version = "0.1.0"
edition = "2024"
rust-version = "1.93"
publish = false

[package.metadata.flutter]
plugin = true
registrar = "register"

[dependencies]
flutter-plugin-sdk = "0.1.0"
flutter_rust_bridge = "=2.12.0"
```

Use the registry dependency exactly as shown. Do not point the plugin at a
relative `.dart_tool` path: Cargo resolves that path relative to the plugin,
but `.dart_tool` belongs to whichever application consumes it. The generated
application patches crates.io's `flutter-plugin-sdk` to
`../.dart_tool/flutter_rs/sdk/crates/flutter-plugin-sdk`, giving every plugin
the implementation bundled with the selected Flutter fork.

Do not add a direct `wgpu` dependency. Use
`flutter_plugin_sdk::gpu::wgpu`; this keeps the plugin's GPU types identical to
the shell's pinned wgpu types.

## Register the plugin

Export the function named by `package.metadata.flutter.registrar`:

```rust
// rust/src/lib.rs
mod frb_generated;
pub mod api;

use flutter_plugin_sdk::{FlutterRustPlugin, PluginRegistrar, Result};

struct MyPlugin;

impl FlutterRustPlugin for MyPlugin {
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<()> {
        let _dispatcher = registrar.main_thread_dispatcher().clone();
        // Call register_textures(registrar) here if this plugin uses textures.
        Ok(())
    }
}

pub fn register(registrar: &mut PluginRegistrar) -> Result<()> {
    MyPlugin.register(registrar)
}
```

Registration runs once when the application starts. Clone any capability that
must outlive registration; do not retain `&PluginRegistrar`.

Expose application-facing functions from `rust/src/api.rs`, generate the FRB
bindings, and export them from the Dart library:

```sh
flutter_rust_bridge_codegen generate --config-file flutter_rust_bridge.yaml
fvm dart format lib
cargo +1.93.1 fmt --manifest-path rust/Cargo.toml --all
```

Initialize `RustLib` in every Dart isolate that calls the plugin. Use FRB for
commands, small values, state, errors, and streams. Do not transfer pixel
frames or GPU handles through FRB.

## Add the plugin to an application

Add the Dart package to the Rust-shell application's `pubspec.yaml`:

```yaml
dependencies:
  my_plugin:
    path: ../my_plugin
```

Then run:

```sh
fvm flutter pub get
```

The Flutter tool finds the plugin's `rust/Cargo.toml`, adds its crate to the
application runner, and generates the registrar call. Do not edit
`runner-rs/src/flutter_plugins.rs` or the marked generated dependency block in
`runner-rs/Cargo.toml`.

## Create Flutter textures

The SDK offers two engine-owned texture types:

- `WgpuTexture` lets the plugin record GPU commands without owning or
  submitting to the shared queue.
- `PixelBufferTexture` lets the plugin write RGBA8 pixels directly into
  reusable shell-owned memory.

Create textures during registration and retain their handles for as long as
Dart uses their IDs:

```rust
use std::sync::{Arc, OnceLock};
use flutter_plugin_sdk::{PixelBufferTexture, PluginError, PluginRegistrar,
    Result, TextureDescriptor, TextureFormat, WgpuTexture};

struct Textures {
    gpu: Arc<WgpuTexture>,
    pixels: Arc<PixelBufferTexture>,
}

static TEXTURES: OnceLock<Textures> = OnceLock::new();

fn register_textures(registrar: &mut PluginRegistrar) -> Result<()> {
    let descriptor = TextureDescriptor {
        width: 256,
        height: 256,
        format: TextureFormat::Rgba8Unorm,
    };
    let textures = registrar.gpu()?;
    TEXTURES.set(Textures {
        gpu: Arc::new(textures.create_texture(descriptor)?),
        pixels: Arc::new(textures.create_pixel_buffer_texture(descriptor)?),
    }).map_err(|_| PluginError::Unsupported)
}
```

Return `texture_id()` through a small synchronous FRB getter and display it in
Dart with `Texture(textureId: id)`. Start producing only after Dart mounts the
widget, usually from an idempotent FRB method called in a post-frame callback:

```dart
WidgetsBinding.instance.addPostFrameCallback((_) {
  api.startTextureProducer();
});
```

Calling `present()` repaints the existing texture without rebuilding the Dart
widget tree.

### Record a wgpu frame

```rust
use flutter_plugin_sdk::{gpu::wgpu, PluginError, Result, WgpuTexture};

fn draw(texture: &WgpuTexture, color: wgpu::Color) -> Result<()> {
    let mut frame = match texture.try_next_frame() {
        Ok(frame) => frame,
        Err(PluginError::Busy) => return Ok(()),
        Err(error) => return Err(error),
    };
    frame.render(move |_, encoder, view| {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("plugin texture"),
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
    frame.present()
}
```

Record commands only. Do not retain the borrowed device, encoder, or view, and
do not submit commands or create another device for this texture. Use
`try_next_frame()` when frames may be skipped, or `next_frame().await` when the
producer should wait asynchronously for a free slot. `Busy` is normal
backpressure.

### Write a pixel-buffer frame

```rust
use flutter_plugin_sdk::{PixelBufferTexture, PluginError, Result};

fn fill(texture: &PixelBufferTexture, width: usize, height: usize) -> Result<()> {
    let mut frame = match texture.try_next_frame() {
        Ok(frame) => frame,
        Err(PluginError::Busy) => return Ok(()),
        Err(error) => return Err(error),
    };
    frame.write_pixels(move |pixels, row_bytes| {
        for row in pixels.chunks_mut(row_bytes).take(height) {
            for pixel in row[..width * 4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[0x20, 0x80, 0xff, 0xff]);
            }
        }
    })?;
    frame.present()
}
```

Honor `row_bytes` and do not retain the pixel slice. Decode or generate into
the supplied memory when possible to avoid a full-frame copy.

## Threading and shutdown

Normal FRB calls run on workers. Keep shared plugin state `Send + Sync`, and
use the cloned `MainThreadDispatcher` for short operations that create or
replace textures. Never wait synchronously for dispatched work from a
synchronous root-isolate FRB call.

Stop producer threads or tasks before dropping texture handles. Dropping the
last handle unregisters the texture asynchronously. Treat
`PluginError::Shutdown` as terminal. For resize or replacement, create the new
texture on the main thread, let Dart mount its new ID, start its producer, and
then release the old texture.

## Check the plugin

Run checks through a consuming Rust-shell application so its SDK patch is in
effect:

```sh
fvm flutter pub get
cargo check --locked --manifest-path runner-rs/Cargo.toml
fvm flutter build bundle
cargo run --locked --manifest-path runner-rs/Cargo.toml -- build/flutter_assets
```

Confirm the generated registrar includes the plugin, the Dart API reaches
Rust, textures visibly update after mounting, and the application exits after
stopping its producers.
