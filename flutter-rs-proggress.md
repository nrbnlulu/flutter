# Flutter Rust Shell Progress

This file records implementation progress for the architecture in
[`flutter-rs.md`](flutter-rs.md). It is an implementation log, not a replacement
for the architectural plan.

## Current focus

Phase 0 — establish the in-tree engine seam on Linux without using the public
Embedder API.

## Status

| Area | Status | Evidence |
| --- | --- | --- |
| Existing shells remain available | Complete | The Rust target is opt-in and is not added to the existing platform-selection group. |
| In-tree Rust platform target | Complete | `//flutter/shell/platform/rust:flutter_rust_shell` builds. |
| Internal PlatformView adapter | Complete | `PlatformViewRust` builds and its four focused tests pass. |
| Rust/C++ ABI | Complete for discovery | ABI v1 is defined in `rust_bridge.h`, linked through GN, and exercised by C++ tests. |
| Rust workspace and `flutter-plugin-sdk` | Complete for foundation | Workspace uses Rust edition 2024, concrete toolchain 1.93.1, and passes its tests. |
| Winit event loop | Implemented; Flutter integration pending | Linux runtime crate owns a minimal winit window/event loop and passes its unit test. |
| Merged UI/platform task runner | C++ contract complete | `RustTaskRunner` queues Flutter task batons for the Rust host loop; its three focused tests pass. |
| Impeller/wgpu interop | Not started | Must follow a working rendering surface. |
| Linux runnable shell | Not started | Requires a Rust runner, a surface implementation, and task runners. |

## Implementation log

### Phase 0 — PlatformView seam

- Added `engine/src/flutter/shell/platform/rust/`.
- Added the standalone GN target
  `//flutter/shell/platform/rust:flutter_rust_shell` and the convenience group
  `//flutter/shell/platform:rust`.
- Added `PlatformViewRust`, an internal C++ `PlatformView` subclass. It keeps
  Flutter C++ inheritance on the engine side and accepts callbacks that the
  future Rust bridge will own.
- Implemented safe default platform-message behavior: messages with no Rust
  handler complete an empty response, matching Flutter's existing platform-view
  behavior.
- Added `flutter_rust_shell_unittests` covering callback forwarding, the
  empty platform-message response fallback.
- Added an edition-2024 Cargo workspace with the private `flutter-shell-core`
  runtime crate and the public `flutter-plugin-sdk` crate.
- Added ABI v1 in Rust and C++ header form, including a GN-linked ABI discovery
  test. Callback ownership remains the next ABI increment.
- Added a GN Cargo action that builds the Rust static library for the private
  ABI test.
- Added the `flutter-shell-winit` Linux runtime crate with a winit-owned window
  and event loop. It is intentionally not connected to Flutter task runners
  until the callback ABI has ownership semantics.
- Added `RustTaskRunner`, a private C++ task runner that gives the host loop
  opaque batons and runs them when winit returns the baton on its main thread.
  It is the basis for sharing the UI and platform task runner without using the
  Embedder task-runner API.
- Left existing platform shells unmodified and unselected. The Rust runner will
  select its own target explicitly.

## Validation

- `git diff --check` passes.
- C++ sources were formatted with `clang-format`.
- Built `//flutter/shell/platform/rust:flutter_rust_shell` and
  `//flutter/shell/platform/rust:flutter_rust_shell_unittests` with the
  host-debug GN configuration.
- Ran `flutter_rust_shell_unittests`: 7 tests passed.
- Ran `cargo +1.93.1 test --workspace --locked`: all crate and documentation
  tests pass, including the winit host task queue.

## Next implementation steps

1. Extend the private ABI with Rust-owned callback handles and lifetime
   operations, then bind `RustTaskRunner` to the winit wake/timer loop.
2. Construct `Shell` with the merged Rust task runner and `PlatformViewRust`.
3. Add an Impeller-backed Linux rendering surface and boot a Flutter app.

## Constraints carried into implementation

- The Rust shell is optional; existing Flutter shells remain buildable.
- Impeller remains Flutter's renderer permanently.
- The public Flutter Embedder API is not used.
- Plugin-facing GPU types come from the semantically versioned
  `flutter-plugin-sdk` crate.
