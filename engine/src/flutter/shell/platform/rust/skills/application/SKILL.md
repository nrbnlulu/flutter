---
name: application
description: Create, convert, build, and run a Flutter application with the nrbnlulu Flutter Rust shell. Use when an application needs the generated runner-rs Cargo runner, Rust plugins, or the rust-shell pubspec setting. Includes headless agent control, reproducible verification, and bundled Dart CLI and editor LSP shortcuts through rlutter.
---

# Create a Flutter Rust-shell application

For standalone `rlutter`, use the workflows below; FVM is not required. For
the Flutter-tool workflow, install the fork with FVM first. If FVM is not yet
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
rust-shell: true
```

The command creates `runner-rs/`, including its Cargo manifest, pinned Rust
toolchain, launcher, and generated plugin registrant. The first `flutter run -d
rust` generates `runner-rs/Cargo.lock`; commit this directory and its lockfile.
Do not edit
`runner-rs/src/flutter_plugins.rs` or the marked generated dependency block in
`runner-rs/Cargo.toml`.

## Bundled Dart CLI and editor LSP (standalone rlutter)

The standalone `rlutter` CLI manages its own Flutter SDK; these shortcuts do
not require FVM or a global Dart installation:

```sh
rlutter dart --version
rlutter dart analyze
rlutter dart format lib
rlutter lsp
```

`rlutter dart <args...>` runs the bundled Dart CLI with arguments unchanged.
`rlutter lsp <args...>` runs `dart language-server --protocol=lsp`, forwarding
additional arguments and inheriting stdin/stdout. Configure an editor's LSP
command as `rlutter` with arguments `["lsp"]`. No project build or engine
artifact download is needed. The first invocation downloads the managed
Flutter SDK if it is absent; download diagnostics go to stderr.

Both commands use `$XDG_CACHE_HOME/rlutter/flutter-BETA` (default
`~/.cache/rlutter/flutter-BETA`). For an explicit SDK, set `FLUTTER_ROOT` or put
`--flutter-root DIR` before `dart` or `lsp`. `--help` after either shortcut is
forwarded to Dart/the language server. The FVM commands elsewhere in this
skill describe the older Flutter-tool integration.

## VS Code: F5 debugging with standalone rlutter

From the application root, generate and open the VS Code configuration:

```sh
rlutter vscode
code rlutter.code-workspace
```

`rlutter.code-workspace` is a JSON configuration file created in the application
root. It opens the same application folder and supplies its SDK setting,
background run task, and debugger profile. Open this file through the command
above or VS Code's **File → Open Workspace from File…**; opening only the folder
with `code .` does not load the generated workspace configuration.

Install the recommended Flutter extension, select **rlutter (Linux)** in Run
and Debug, and press **F5** (or click its green start button). This builds,
launches, and attaches automatically; no separate `rlutter run` command or
VM service URL copying is needed.

The workspace records the current rlutter binary, Flutter SDK, and explicit
local-engine override. If these paths move, update the workspace or rename the
existing file before regenerating it with `rlutter vscode`. The command refuses
to overwrite an existing `rlutter.code-workspace` and leaves `.vscode/` settings
untouched.

The background task builds and runs the Linux app and writes
`.dart_tool/rlutter-vm-service.json`. VS Code reads that file to attach its Dart
debugger, providing breakpoints, stepping, and variable inspection. Use `r`/`R`
in the task terminal for hot reload/restart. Stopping the VS Code debugger
detaches; press `q` in the task terminal to stop the app. F5 can reconnect to an
app whose task is still running. This generated configuration currently targets
Linux; it does not configure Android port forwarding or native Rust debugging.

For a custom editor task, the underlying option is:

```sh
rlutter run -d linux --vm-service-info-file .dart_tool/rlutter-vm-service.json
```

The task is ready when it prints `rlutter: debugger ready`. The generated JSON
service-info file contains Flutter attach's DDS `uri`, which carries the source
mappings needed by Dart's debug adapter. Do not substitute the raw engine URI.

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
fvm flutter run -d rust
```

Run the command from the application root. The Flutter tool builds the Dart
asset bundle and generated Cargo runner, then launches it through the Rust
shell. Debug-mode VM-service attachment and hot reload use Flutter's normal
resident-runner path.

For normal development checks, use:

```sh
fvm flutter analyze
fvm flutter test
cargo check --locked --manifest-path runner-rs/Cargo.toml
```

The downloadable Rust-shell SDK currently supports Linux x64. Do not configure
another target unless a corresponding fork release is available.

## Headless agent verification (standalone rlutter)

Run from a generated Rust-shell application root (`rust-shell: true` and
`runner-rs/`). Use `rlutter create .` to add the shell to an existing app, or
`rlutter create my_app` for a new app. Headless execution currently requires
Linux x64 and a working Vulkan driver, including qualified Mesa llvmpipe on
CPU-only hosts. No display server or Xvfb is needed.

Choose the command that matches the task:

```sh
# Development launch with debug hot reload.
rlutter run -d linux --headless --size 1280x800 --pixel-ratio 1
# Interactive agent control; use a new artifact directory per session.
rlutter agent --headless --stdio --artifacts build/agent/session-001
# Reproducible assertions; use a new report/artifact destination per run.
rlutter verify test/agent/save_dialog.json --headless --report build/agent/run-001/result.json
```

For an MCP client, use the official Rust SDK based stdio adapter:

```sh
rlutter mcp --headless --stdio --artifacts build/agent/mcp-001
```

Launch from the application directory. Discovery is immediate; the first tool
call builds and starts the app. Tools include `rlutter_windows_list`,
`rlutter_input_tap`, `rlutter_verify_wait`, and `rlutter_capture_screenshot`
(which returns an MCP image plus metadata). Finish with
`rlutter_session_shutdown`; closing stdin also cleans up the owned app.
Cancellation does not undo dispatched input; never retry an uncertain mutation.
See [MCP configuration and lifecycle](../../docs/mcp.md).

`run` does not expose the agent protocol. For `agent`, keep stdin open while
reading responses, send one JSON request per line, and handshake first:

```json
{"id":1,"method":"session.handshake","params":{"protocolVersion":1}}
{"id":2,"method":"windows.list","params":{}}
```

Discover actual view IDs before querying semantics or sending input. Prefer
stable `Semantics(identifier: ...)` selectors. Observe the result with
`verify.wait` after an action; an input acknowledgement proves dispatch only.
Do not retry mutations automatically after a timeout. Capture PNG evidence with
`capture.screenshot` and finish with `session.shutdown`. Stdout is NDJSON only;
build/app logs go to stderr and artifacts. Closing stdin ends the owned app.

For default verification, add the `flutter_rust_agent` Dart package as a path
dependency at `.dart_tool/flutter_rs/sdk/dart/flutter_rust_agent` (or
`dart/flutter_rust_agent` under the source SDK). Call
`FlutterRustAgent.install()` after app error handlers and before `runApp`.
Agent/verify builds enable the bridge automatically. A deliberately native-only
scenario can set `require_framework_errors: false`; report that reduced scope.

Read [the protocol](../../docs/agent-protocol.md) before writing a controller,
and [verification](../../docs/verification.md) before writing a version-1 JSON
scenario. Scenarios and selectors must match the app; `save_dialog.json` is a
fixture example, not a file generated in every project. A failed assertion or
infrastructure failure exits nonzero and retains a report plus artifacts.
Use a negative control to show that the changed behavior can fail verification.
See [recording](../../docs/recording.md) only when video evidence is needed.

For a local checkout, pass `--flutter-root /path/to/flutter` and
`--local-engine-src /path/to/flutter/engine/src` before the subcommand; the latter
is the engine `src` directory, not an `out/host_debug` directory. CLI discovery:
`rlutter --help`, `rlutter agent --help`, and `rlutter verify --help`.
