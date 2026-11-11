# Linux release monitor evidence

[Video: release app and MCP lifecycle checks](linux-release-monitor.mp4)

The video is a cropped screen recording of the real visible Wayland application
and the integration test output, captured on 2026-11-11. It shows release/AOT
attachment, live semantics/error coverage, app uptime while MCP is disconnected,
a deliberate framework error at twelve seconds, and successful reconnect/EOF/
detach checks. The crop removes the unrelated editor window. No UI frames were
synthesized. The integration test terminates its own fixture only after the
independent-process-lifetime assertions finish.

[Machine-readable MCP transcript](linux-release-monitor.json) includes the
application epoch, engine revision/build mode, healthy/error states and expected
rejection of input for the attached observation endpoint. The same release binary
also passed a separate integration run before recording. Vulkan validation was
enabled for both runs; the app log contained no validation errors.

Built with the source shell's `agent-monitor` Cargo feature and
`--release --dart-define=FLUTTER_RUST_AGENT=true`, using the BETA release engine
at `7c8b771b973cb1cd5b9000084077ff4a414e4b91`, ABI 16, and Rust 1.93.1.
The fixture source is `../../tests/monitor_fixture/main.dart`. The test command
was equivalent to:

```sh
VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation python tests/test_monitor.py \
  --runner /path/to/generated-app/runner-rs/target/release/monitor_proof \
  --rlutter /path/to/rebuilt/rlutter --artifacts /fresh/evidence/directory
```

This verifies a short real release session and reconnect behavior. It is not
an hours-long performance or durability measurement; history is bounded in memory
and an external monitoring process must save events and detect process death.
