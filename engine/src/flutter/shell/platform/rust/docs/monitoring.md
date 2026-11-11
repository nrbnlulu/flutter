# Attach to a running Linux application

Linux applications can opt into MCP monitoring in debug, profile or release
(AOT), with no VM service dependency. The normal window and process belong to
the application. Disconnecting MCP, detaching, or closing the IDE leaves the app
running. This is separate from the existing CLI-owned headless sessions.

## Opt in

In the application's `runner-rs/Cargo.toml`, enable the feature on its existing
shell dependency (outside the generated plugin block):

```toml
flutter-shell-winit = { path = "../.dart_tool/flutter_rs/sdk/crates/flutter-shell-winit", features = ["agent-monitor"] }
```

For Flutter framework errors, add the `flutter_rust_agent` package as described
in [verification](verification.md), call `FlutterRustAgent.install()` after the
app's error handlers and before `runApp`, and build with:

```sh
rlutter build linux --release --dart-define=FLUTTER_RUST_AGENT=true
```

The native Cargo feature and launch-time socket path are independent opt-ins.
The Dart define enables additional error observations; it does not create an
endpoint. A build without the Cargo feature ignores the monitor environment
variable. A build with the feature creates no endpoint unless explicitly
launched with an absolute socket path in an existing user-owned private (0700)
directory. The socket itself is 0600. An existing socket is never overwritten.

For example, from an application root (replace `my_app` with its binary name):

```sh
mkdir -m 700 build/monitor-session
FLUTTER_RUST_MONITOR_SOCKET="$PWD/build/monitor-session/control.sock" \
  runner-rs/target/release/my_app
```

Register `rlutter mcp --stdio` in the client as usual. Call
`rlutter_session_attach(socket_path)` with that absolute path. Pass the returned
`session_id` to `rlutter_session_capabilities`, `rlutter_session_health`,
`rlutter_windows_list`, `rlutter_semantics_snapshot(viewId)`,
`rlutter_diagnostics_snapshot`, and `rlutter_monitor_events(after)`.
Finish with `rlutter_session_detach`. For attached sessions, `session_shutdown`
also only detaches. Input, shutdown of the application, screenshots, recording,
and verification predicates are not capabilities of this observation endpoint.
The headless tools retain their existing behavior.

## Long-running clients

One client attaches at a time. Reattach after disconnect using the same socket.
Window state, semantics and errors continue to be collected while disconnected.
Store the returned application `epoch` and `nextSequence` outside the MCP process.
Pass the cursor as `after` on the next events call. The event history is bounded
by 256 events and 1 MiB. `gap: true` explicitly reports overwritten history; read
a fresh snapshot and record the gap. A changed epoch means the app restarted.
This history is in memory, not a durable audit log; an external supervisor owns
polling, persistence, alerts and crash detection. A crash or transport failure
invalidates the attached session ID; reconnect explicitly, without replaying calls.

Health reports cumulative error count, whether the Dart bridge has announced
itself, and UI heartbeat age. An error or a heartbeat older than five seconds
makes `healthy` false. Errors remain observable without terminating the app.
A once-per-second event-loop deadline keeps idle windows distinguishable from
stalled UI work. The worker can answer health while the UI thread is blocked.
`frameworkErrors` means the bridge announced coverage; it does not certify that
application code has preserved its installed error handlers. Other isolates and
errors handled entirely inside application code require explicit instrumentation.

Semantics inspection enables accessibility semantics for this process and has a
runtime cost. Window snapshots and bounded diagnostic/event history are collected
only while monitoring is opted in. Obscured semantics values use the existing
redaction behavior. This endpoint is local to the current Linux user; remote
network transport is not exposed.

## Verification

`tests/test_monitor.py` accepts a release runner built from
`tests/monitor_fixture/main.dart` in a generated application with the feature and
Dart bridge enabled. It exercises normal launch without an endpoint, MCP attach,
semantics, detach, EOF/reconnect, retained errors and continued app liveness.
The fixture reports one deliberate Flutter error at twelve seconds and keeps
updating its visible uptime. Unit coverage checks bounded history, cursor gaps,
UI stalls, fragmented messages, malformed clients and shutdown rejection.
