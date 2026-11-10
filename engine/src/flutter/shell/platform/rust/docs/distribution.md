# Headless SDK distribution and validation

The Linux SDK ships the agent crate, virtual host/offscreen code, both patched
Rust dependencies, the optional Dart bridge, fixture/scenario and matching engine
artifacts. `tools/package_sdk_sources.py` stages sources and emits sdk-manifest.json.
`--validate-engines` checks all three engine-manifest.json files against the Rust
ABI and expected build mode. `tools/package_engine_artifacts.sh` reads metadata
from the compiled library without starting a GPU or isolate. Runners also compare
the engine-compiled ABI before handing any callbacks or images to C++.

Supported runtime scope:

| Platform/mode | Native semantics/input/PNG | Optional bridge | Video |
| --- | --- | --- | --- |
| Linux x64 debug | Supported | Opt in | Optional FFmpeg/FFV1 |
| Linux x64 profile | Supported, real AOT | Opt in | Same protocol |
| Linux x64 release | Supported, real AOT | Opt in | Same protocol |
| Windows | Headless runtime gate pending | Not advertised | Not advertised |
| Android FlutterActivity | Outside this headless host | Not installed | Not installed |

Agent and verify builds explicitly define FLUTTER_RUST_AGENT=true for the bridge.
Ordinary release builds leave it disabled and create no control endpoint. Engine
shutdown, missing adapters, ABI mismatches, disk errors and unavailable encoders
remain explicit failures. The agent never substitutes a display server or another
renderer when Vulkan requirements are unavailable.

The BETA workflow runs GPU-free protocol/cache/codec/PNG tests on Linux and
Windows. The Linux debug lane selects llvmpipe explicitly, unsets display
variables, enables validation, retains strace socket evidence, and checks virtual
view lifecycle, plugin texture pixels, idle CPU, native input and decoded video.
The publish gate verifies a generated app using physical extracted source/engine
archives in debug, profile and release, with separate fresh-process positive and
negative controls. CI uploads reports on failure. Existing Android build and
native desktop task paths are retained.

`tests/test_packaged_agent.py` accepts --rlutter, --flutter-archive, --rust-archive
and a fresh --artifacts directory. Its temporary SDK and app are outside the
artifact directory; --work-dir retains them in a selected location for inspection.
No source-crate or engine links into the original checkout are accepted. The
scenario checks Unicode/RTL editing, a second view, focus, pointer activation,
persisted state, widget/app observations, healthy error hooks and correlated PNGs.
A separate ordinary-release launch checks that the bridge is disabled and no
agent socket is owned by the process. Passing reports retain the actual build
mode, engine ABI/revision, adapter and content hashes.

The Windows CPU job is deliberately not a Windows renderer certification. A
Windows runtime delivery still needs a native local transport implementation,
software/hardware Vulkan evidence, process-ownership tests and extracted release
packaging. Linux evidence cannot establish that gate. The public request schema
is independent of the local transport; Windows support must be advertised only
after those independent checks pass. An MCP adapter is deferred until the runtime
and distribution gates are stable; agents can already use NDJSON or scenarios.

Headless evidence certifies only its declared scenario and offscreen path. Keep
native compositor, IME, OS accessibility, Windows and Android tests for their own
behavior; a headless report is not a replacement for those lanes.
