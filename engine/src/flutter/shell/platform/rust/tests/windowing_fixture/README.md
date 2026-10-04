# Native multi-view lifecycle regression

Run `task test-rust-shell-windowing` from the Flutter checkout in an active
desktop session with `VK_LAYER_KHRONOS_validation` installed.

The fixture continuously animates the implicit view while creating three
regular windows at a time, resizing them, and removing them. It waits for
native destruction callbacks before beginning the next cycle. After 60
removals it exits with three additional views alive, testing whole-shell
teardown as well. The Python harness requires completion, ongoing primary
presentation, a clean exit, and no Vulkan or framework diagnostics.
