# MCP application control

Register the globally installed CLI once in your IDE:

```json
{"mcpServers":{"rlutter":{"command":"rlutter","args":["mcp","--stdio"]}}}
```

The server can start from any directory. Linux x64 and Vulkan are required
for app control. Discovery requires no SDK download or app build.

Call `rlutter_session_start` with `project_dir`, the absolute path to an
application in the desired worktree. It builds and launches the app and returns
`session_id` and its artifact directory. Pass `session_id` to every subsequent
app tool. Each session gets a fresh directory under that project's
`build/agent/`; no artifact option or launcher script is needed.

Multiple sessions can remain open across different worktrees. All operations
are serialized through one worker. Paths are resolved explicitly; the server
never changes its working directory. CLI build options apply to each session.
Explicit `--flutter-root` and `--local-engine-src` options select the SDK and
engine for all sessions; otherwise the managed SDK is used.

The official Rust MCP SDK (`rmcp`) handles stdio transport, initialization,
version negotiation, request dispatch and MCP cancellation. Discovery does not
resolve/download an SDK or build an app. The session.start call builds a fresh app
and handshakes with the existing native agent session. Build and app logs go to
stderr and the artifact directory; stdout carries only MCP messages. Ordinary
app launches still expose no control endpoint.

Tools use the prefix `rlutter_` and replace native method dots with underscores:
`windows.list` becomes `rlutter_windows_list`, `input.tap` becomes
`rlutter_input_tap`, and so on. The server advertises each tool's parameters.
Native handshake, subscriptions and request cancellation are internal rather
than separate tools. Use MCP cancellation notifications for MCP requests.

After starting a session, a typical sequence (passing its session_id each time) is:

1. Call `rlutter_windows_list` and discover actual view IDs.
2. Read `rlutter_semantics_snapshot` or `rlutter_semantics_find`.
3. Call `rlutter_input_tap` with a stable identifier selector.
4. Call `rlutter_verify_wait` to observe the resulting state.
5. Call `rlutter_capture_screenshot` for a PNG image plus frame metadata.
6. Check `rlutter_session_health`, then call `rlutter_session_shutdown`.

Input acknowledgement proves dispatch only. Screenshots preserve the native
fresh-frame contract and also return MCP image content (up to 16 MiB PNG).
Native errors retain their codes in text content and set MCP `isError`.
Install the [Dart bridge](verification.md) for framework error and widget/app
predicate checks. Native window, semantics and input tools work without it.

Only one app operation runs at a time. An overlapping tool call fails with a
busy result. Cancellation stops waiting for the MCP result; an already running
native operation may still complete and holds the operation slot until it does.
Never retry a mutation after cancellation or timeout. Native transport failure
closes the session and prevents later calls from restarting or replaying it.
Explicit shutdown is final for that session ID; call session.start for a new
app session. The MCP server stays available. EOF closes all owned apps after any in-flight native operation reaches its bounded
deadline. No HTTP listener, arbitrary Dart evaluation or general file tool is
exposed.

`tests/test_mcp.py` checks real-engine discovery, input and observation, a negative
assertion, screenshot image bytes, final shutdown and EOF cleanup. It accepts the
same generated agent-fixture project as `tests/test_verify.py`. Unit tests use
an in-memory SDK transport to verify lazy discovery, argument forwarding, errors
and cleanup without a GPU.
