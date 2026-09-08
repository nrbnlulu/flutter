---
name: application
description: Create, convert, build, and run a Flutter application with the nrbnlulu Flutter Rust shell. Use when an application needs the generated runner-rs Cargo runner, Rust plugins, or the flutter.shell rust setting. Currently supports Linux x64.
---

# Create a Flutter Rust-shell application

Install the fork with FVM before creating the application. If it is not yet
configured, use the sibling `fvm` skill.

## Create a new application

```sh
fvm spawn nrbnlulu/BETA create --shell=rust --platforms=linux my_app
cd my_app
fvm use nrbnlulu/BETA
```

For an existing Flutter application, select the fork in its root and generate
the Rust runner:

```sh
fvm use nrbnlulu/BETA
fvm flutter create --shell=rust --platforms=linux .
```

The application `pubspec.yaml` must contain:

```yaml
flutter:
  shell: rust
```

The command creates `runner-rs/`, including its Cargo manifest, lockfile,
pinned Rust toolchain, launcher, and generated plugin registrant. Commit this
directory and its lockfile. Do not edit
`runner-rs/src/flutter_plugins.rs` or the marked generated dependency block in
`runner-rs/Cargo.toml`.

## Resolve dependencies

```sh
fvm flutter pub get
```

This installs the Rust-shell SDK into `.dart_tool/flutter_rs/sdk` and refreshes
the Rust plugin list. `.dart_tool` remains generated and should not be
committed.

Add a Rust-shell plugin as an ordinary Dart dependency:

```yaml
dependencies:
  my_plugin:
    path: ../my_plugin
```

Run `fvm flutter pub get` again after adding, removing, or changing plugin
dependencies. Use the sibling `plugin` skill to create a compatible package.

## Build and run

```sh
fvm flutter build bundle
cargo run --locked --manifest-path runner-rs/Cargo.toml -- build/flutter_assets
```

Run both commands from the application root. The first creates the Dart asset
bundle; the second builds and launches the Rust runner against that bundle.
`fvm flutter run` is not currently the Rust-shell launch command.

For normal development checks, use:

```sh
fvm flutter analyze
fvm flutter test
cargo check --locked --manifest-path runner-rs/Cargo.toml
```

The downloadable Rust-shell SDK currently supports Linux x64. Do not configure
another target unless a corresponding fork release is available.
