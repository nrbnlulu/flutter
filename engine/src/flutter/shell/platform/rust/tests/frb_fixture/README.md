# Flutter Rust shell FRB texture fixture

This application-level fixture verifies the real Dart/FRB/Rust path without
adding `flutter_rust_bridge` to the shell runtime. Its Cargo binary links the
private `libflutter_rust_engine.so`, registers the fixture through
`flutter_shell_winit::run_application`, and exports the generated FRB symbols
from the process for `ExternalLibrary.process()`.

The Dart entry point makes a synchronous FRB call on the root isolate, then a
normal FRB call from a named background isolate. The Rust API proves that the
normal call executes away from the winit thread, posts through
`MainThreadDispatcher`, runs its callback on the winit thread, and observes
that work queued by the synchronous call did not run reentrantly.

The registered fixture plugin also creates one engine-owned wgpu texture and
one shell-owned RGBA pixel-buffer texture. Texture IDs cross FRB from both
isolates, Dart displays the IDs in real `Texture` widgets, then an FRB control
call starts a Rust producer after the first Dart frame. The native harness
verifies both producer counters advance and that each half of the window
changes independently.

Regenerate the checked-in bindings with FRB 2.12.0:

```sh
PATH="/path/to/flutter/bin/cache/dart-sdk/bin:$PATH" \
  flutter_rust_bridge_codegen generate \
    --config-file flutter_rust_bridge.yaml
/path/to/flutter/bin/dart format lib
cargo +1.93.1 fmt --manifest-path rust/Cargo.toml --all
```

Run the full native fixture from the Flutter repository root:

```sh
task test-rust-shell-frb-dispatch
```
