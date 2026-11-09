# Reproducible verification

Build an application with stable semantics identifiers, add the optional
`dart/flutter_rust_agent` package as a path dependency, and call
`FlutterRustAgent.install()` before running the root widget. Installation is
inert unless compiled with `FLUTTER_RUST_AGENT=true`. Agent/verify builds opt in;
ordinary builds do not. The bridge chains existing FlutterError.onError and
PlatformDispatcher.onError handlers. Errors fail verification even if a previous
handler consumes them. Install early enough to observe application startup.

Run from a generated Rust-shell application:

```sh
rlutter verify test/agent/save_dialog.json --headless --pixel-ratio 1.25 \
  --report build/agent/result.json
```

The report and its sibling `result.artifacts` directory must be fresh. Every
invocation builds through Flutter's dependency graph, stages the matching engine,
and starts a new owned process. It records SHA-256 hashes of application inputs,
package locks/configuration, executable, engine and staged assets, along with
SDK/project revisions and dirty state. Source changes during building fail
explicitly. No hot reload/restart is offered; session epochs invalidate handles.

A scenario is a versioned JSON object with a name and ordered steps:

```json
{"version":1,"name":"Save value","steps":[
  {"name":"Wait for form","method":"verify.wait","params":{"viewId":1,"selector":{"identifier":"name"},"count":1}},
  {"name":"Click input","method":"input.tap","params":{"viewId":1,"selector":{"identifier":"name"}}},
  {"name":"Replace value","method":"input.replaceText","params":{"viewId":1,"text":"שלום 🌍 café"}},
  {"name":"Observe value","method":"verify.wait","params":{"viewId":1,"selector":{"identifier":"name"},"value":"שלום 🌍 café"}}
]}
```

Each step records its own response and duration. Dispatch does not imply that an
assertion passed. `verify.assert` checks once; `verify.wait` observes until its
monotonic timeout and may require stableMs. There is no global settling operation,
and mutations are never retried after timeout. Infinite unrelated animation does
not block predicates. Protocol wait/assert methods are also available directly.

Native assertions default to kind=semantics, with viewId/selector and count,
exists, value, enabled or focused. A property requires one match. kind=windows
uses a where object matching viewId/title/kind/parentViewId/focused/ready/visible/
removing, plus count or exists. Empty results are useful for disappearance checks.
Multiple results never silently choose a control.

The bridge supports kind=widget with viewId/query/equals. Queries select a string
ValueKey (`key`) and/or runtime widget type (`type`), then property=count (default),
text (Text only) or enabled (EditableText only). Counts include mounted widgets;
physical visibility still requires native semantics, hit testing and captures.
Runtime type names may change with obfuscation; prefer ValueKey. Obscured editing
text is not exposed as a public bridge property. kind=app queries a name and
arguments registered through FlutterRustAgent.registerPredicate. Predicates return
JSON observations on the actual UI isolate; arbitrary evaluation is unsupported.

Scenarios require structured framework errors by default. A deliberately native
scenario may set require_framework_errors=false; the report records that reduced
diagnostic scope. Native semantics/input/capture continue to work without the
bridge. Capability discovery reports actual attachment and build mode.

Failed predicates return last observations and diagnostics. The runner retains
window/semantics snapshots and requests completed per-view PNGs at the end of the
run. Engine/control-channel loss and nonzero exit status cannot produce a pass.
Reports distinguish assertion_failed from infrastructure_failed. A step may use
expected_error for a narrow operation error; process/app failures cannot be masked.

The packaged fixture's `test/agent/save_dialog.json` edits Unicode text, opens a
second window, checks text/focus, confirms, checks removal and saved state, queries
a ValueKey and app predicate, and captures evidence. The same scenario with
`--dart-define=AGENT_REGRESSION=true` must fail. `AGENT_STARTUP_ERROR=true` and the
throw_error button verify framework error handling. `tests/test_verify.py` runs all
four controls through the CLI and compares built fingerprints. Choose identifiers
and predicates that exercise the behavior changed by your patch, and retain a
negative control; a passing report only covers its declared scenario.
