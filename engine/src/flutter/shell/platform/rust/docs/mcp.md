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
2. Read `rlutter_ui_snapshot` for bounded text with directly actionable refs.
3. Call `rlutter_semantics_perform` with a ref and supported action, or
   `rlutter_input_tap` for a pointer tap. Stable identifier selectors also work.
4. Call `rlutter_verify_wait` to observe the resulting state.
5. Only when visual evidence is needed, request `rlutter_ui_screenshot`.
6. Check `rlutter_session_health`, then call `rlutter_session_shutdown`.

Input acknowledgement proves dispatch only. Screenshots preserve the native
fresh-frame contract and also return MCP image content (up to 16 MiB PNG).
Native errors retain their codes in text content and set MCP `isError`.
Install the [Dart bridge](verification.md) for framework error and widget/app
predicate checks. Native window, semantics and input tools work without it.

## Compact observations and references

`rlutter_ui_snapshot(session_id, viewId)` is the normal starting point for app
inspection. It projects the existing native semantics snapshot without changing
the runtime or NDJSON protocol. The existing Linux runtime/distribution gate and
app MCP adapter remain prerequisites; the engine build MCP server is unrelated.

The response contains JSON lines: complete node records followed by one `meta`
record. For example (metadata abbreviated):

```json
{"depth":0,"label":"Name","ref":"s7-12","role":"textField","states":["focused"],"value":"Ada"}
{"actions":["tap"],"depth":0,"label":"Save","ref":"s7-14","role":"button"}
{"meta":{"viewId":1,"revision":42,"generation":7,"returned":2,"omitted":0,"nextOffset":null}}
```

Labels, values, identifiers and other text have escaped previews of up to 96
characters; actions, state, text and scroll containers remain discoverable.
Useful groups retain `depth` and numeric `parent` node IDs. Empty wrappers are
collapsed. Hidden semantics subtrees are omitted and obscured values stay
redacted. Disabled controls remain visible as disabled. Semantics hidden status
is not proof of pixel visibility or absence of occlusion.

`max_chars` defaults to 4,000 and accepts 0 through 64,000. It bounds the entire
text, including metadata (itself bounded to 512 characters), in Unicode scalar
values, not tokens or UTF-8 bytes. No partial records are emitted. Very small
budgets can return empty text or metadata only; increase the budget if a page
cannot fit one record. Text previews may shrink to fit. `omitted` counts records
outside the page; `hidden` and `collapsed` count excluded native nodes separately.

Continue truncated output by passing `offset=meta.nextOffset` and
`generation=meta.generation`. Offsets index visible, non-empty records, not raw
node IDs. A continuation reads native semantics again and requires the same
epoch, view and revision. If semantics changed, start again at offset zero.
Earlier pages' refs remain usable only during that unchanged generation.
A new offset-zero observation supersedes that view's previous refs.

Refs are scoped to a session and view, never reassigned, and map to the exact
native epoch/view/node/revision handle. `semantics_perform` and `input_tap`
accept `ref` instead of `selector`/`handle`/coordinates. The adapter checks a fresh
snapshot before dispatch and the runtime still enforces native handle checks,
including updates between that check and the action. Unknown, stale, removed,
wrong-view and previous-session refs fail rather than retarget. Refresh instead
of retrying. A pointer tap uses bounds and hit testing; semantic activation is a
separate action, not a simulated coordinate tap.

Use `rlutter_semantics_find` for focused diagnostic lookup, or
`rlutter_semantics_snapshot` for full structured geometry, flags and handles.
These legacy diagnostic tools retain their original structured response format;
the compact hidden-subtree filtering applies to `ui_snapshot`, not those tools.
Ordinary observations and action acknowledgements contain text only, without a
duplicate `structuredContent` payload. Verify outcomes separately.

`rlutter_ui_screenshot` aliases the existing `rlutter_capture_screenshot`;
both explicitly deliver a PNG plus unchanged capture metadata. Use them for
appearance, layout, custom painting, semantics gaps or visual failures. Images
are never added automatically to ordinary snapshots/actions. Failure artifacts
may still be retained locally. Semantics and screenshots are separate
observations, not an atomic semantics/pixel pair. Attached monitors can use
compact snapshots but remain read-only and cannot capture images.

## Lifecycle

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

`tests/test_compact_mcp.py` compares full versus compact observations over the
same real form/dialog and long-list/custom-paint tasks, including pagination,
stale-action negative controls, visual fallback, tool schema overhead and total
wire costs. See [recorded evidence and limitations](evidence/compact-mcp/README.md).
Measured characters/bytes are not a model-token guarantee.

Normal Linux windows can explicitly opt into persistent observation in release,
profile or debug builds. Use `rlutter_session_attach(socket_path)` and
`rlutter_session_detach`; these sessions never own the application process.
See [Linux monitoring](monitoring.md) for both opt-in gates, supported tools,
error coverage, event cursors and reconnect behavior.
