# Local headless agent protocol v1

Linux x64 supports `rlutter agent --headless --stdio --artifacts DIR` with the
same build-mode, target and Dart defines as `rlutter build`. The directory must
be fresh. Build and application output goes to stderr and logs. Only NDJSON
responses/events go to stdout. A private 0600 Unix socket in a session-owned
0700 directory connects the CLI to its owned runner. EOF closes the runner;
startup checks parent liveness. Ordinary launch creates no endpoint.

Send `{"id":1,"method":"session.handshake","params":{"protocolVersion":1}}`
first. IDs are nonnegative integers or nonempty strings of at most 128 bytes.
Messages are limited to 1 MiB, pending requests to 32. `timeoutMs` defaults to
5000 and must be 1–120000. Parameters are checked, including unknown fields.
Errors have `code`, `message`, and optional `data`; unknown capabilities fail.
Capabilities include the actual engine revision, engine-compiled ABI/build mode,
GPU/driver, epoch and virtual window defaults. ABI 16 is mandatory.

Methods:

| Method | Parameters |
| --- | --- |
| session.handshake | protocolVersion |
| session.capabilities, session.health, session.shutdown | none |
| session.subscribe, session.unsubscribe | events array |
| request.cancel | id of active request |
| windows.list | none |
| windows.inspect, windows.focus, windows.close | viewId |
| windows.resize | viewId, width, height (logical pixels) |
| semantics.snapshot | viewId |
| semantics.find | viewId, selector; optional all |
| semantics.perform | viewId, selector or handle, action, optional args |
| input.tap | viewId, selector or handle, or x/y |
| input.pointer | viewId, phase (move/down/up/cancel), x, y |
| input.scroll | viewId, x, y, deltaX, deltaY |
| input.key | viewId, physical, logical, down, optional character |
| input.insertText, input.replaceText | viewId, text |
| input.release, diagnostics.snapshot | none |
| verify.assert, verify.wait | typed kind/viewId/selector or where; count/exists/value/state; see verification.md |
| recording.start, recording.stop, recording.status | see recording.md |
| capture.screenshot | viewId; optional mode, afterFrame, sizeGeneration, name |

Selectors support identifier, label, role, enabled and focused. A unique match is
required unless `all` is true. Handles carry epoch/view/node/revision; updates
invalidate them. Snapshots merge incremental updates, prune unreachable nodes,
retain transforms/actions, and compute physical-pixel view bounds. Obscured
values are redacted. Text replacement uses the active editing client and UTF-16
selection; it is distinct from physical key input and native IME behavior.

Action acknowledgement proves dispatch only. Observe the resulting semantics or
window state separately. Input coordinates are logical pixels. Identifier taps
convert physical semantics geometry by the view's pixel ratio and use real
pointer hit testing. Semantic activation is a separate method. Modal descendants
control focus; held input is released on focus loss, removal and shutdown.

Capture defaults to `afterObservation`: the merged UI thread records its frame
boundary and requests a rebuild; GPU readback must have a strictly newer UI
build stamp. `afterFrame` also requires sizeGeneration; `latestCompleted` may
return an existing completed capture. Metadata records UI boundary/build stamp,
GPU-completed presentation/frame ID, size generation, semantics revision, action
sequence, dimensions, epoch and DPR separately. This does not claim atomic
semantics/pixel pairing. PNGs contain straight-alpha linear RGBA8 with gAMA=1.0.
Readback and encoding never wait on the UI thread.

Events are windows.changed, semantics.updated, action.dispatched,
capture.completed and app.error, enabled on the public stream by subscription. Lifecycle events are retained in
artifacts even when the client does not subscribe. The ordered
`events.jsonl` trace is bounded to 64 MiB and redacts editing payloads. Failure
artifacts include the window/semantics state, recent diagnostics and available
completed frames. Disk errors fail the operation/session explicitly. Capture
basenames cannot escape the artifact directory and existing files are preserved.

`tests/test_agent.py` runs the real-engine Unicode/two-view scenario and checks
ambiguous/stale selectors, resize generations, capture ordering, cancellation,
removal while capture waits, held input cleanup, log noise and clean shutdown.
The fixture intentionally prints malformed JSON to stdout. GPU-free cache,
protocol and PNG tests live in flutter-shell-agent. Vulkan validation remains
required for the integration lane. Native desktop/IME/OS accessibility behavior
requires the existing native regression tests.
