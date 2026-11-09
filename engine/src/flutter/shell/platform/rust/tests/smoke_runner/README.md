# Cargo smoke runner

This small executable links the private engine boundary and accepts the same
`ASSETS_DIRECTORY ICU_DATA_FILE` arguments as the Linux GN runner. It is used
by the shared Python lifecycle, texture and stress tests on Windows.

From the repository root, build the matching debug engine/Dart tools and stage
the runner:

```
task build-rust-shell
```

If the engine and Dart artifacts already exist:

```
task build-rust-shell-smoke
```

Windows staging is `engine/src/out/host_debug/rust-smoke/`, containing
`flutter-rust-shell-smoke.exe`, `flutter_rust_engine.dll`, and `icudtl.dat`.
Cargo links `flutter_rust_engine.dll.lib` from the GN output directory. The
checked-in lockfile keeps this fixture aligned with workspace dependencies;
all wgpu crates use the pinned revision, including the documented HAL patch.

The ordinary-window counter/text-field app is in `../smoke_fixture`. Run the
shared checks with `task test-rust-shell-lifecycle` or `task stress-rust-shell`.
See `tools/rust_shell/README.md` in the repository root for platform adapters,
evidence retention, and required interactive-desktop access.

Full window-kind parity and rlutter Windows packaging are later checkpoints.
This runner deliberately uses the implicit view and does not depend on the
Dart Rust-window symbol-discovery path.
