// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
#define FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_

#include <stdint.h>

#if defined(_WIN32)
#define FLUTTER_RUST_SHELL_EXPORT __declspec(dllexport)
#else
#define FLUTTER_RUST_SHELL_EXPORT __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

// This private ABI is lockstep-versioned with the Flutter fork. It is not the
// Flutter Embedder API and is never exposed to application plugins.
#define FLUTTER_RUST_SHELL_ABI_VERSION 2u
#define FLUTTER_RUST_PLUGIN_SDK_API_VERSION 1u

typedef struct FlutterRustShellAbi {
  uint32_t shell_abi_version;
  uint32_t plugin_sdk_api_version;
} FlutterRustShellAbi;

// A callback table owned by the Rust host for one merged Flutter UI/platform
// task runner. Times are relative delays so C++ and Rust need not share a
// monotonic-clock epoch. `task_runner` and `task_baton` are opaque values that
// Rust returns to FlutterRustShellRunTask when the winit loop reaches them.
typedef void (*FlutterRustScheduleTaskCallback)(void* user_data,
                                                void* task_runner,
                                                uint64_t task_baton,
                                                uint64_t delay_nanos);
typedef int (*FlutterRustRunsTasksOnCurrentThreadCallback)(void* user_data);
typedef void (*FlutterRustTaskRunnerDestroyedCallback)(void* user_data);

typedef struct FlutterRustTaskRunnerCallbacks {
  void* user_data;
  FlutterRustScheduleTaskCallback schedule_task;
  FlutterRustRunsTasksOnCurrentThreadCallback runs_tasks_on_current_thread;
  FlutterRustTaskRunnerDestroyedCallback task_runner_destroyed;
} FlutterRustTaskRunnerCallbacks;

// A Vulkan swapchain image acquired by the Rust GPU broker. The broker owns
// the image and all synchronization associated with it. C++ borrows it only
// for the duration of a Flutter frame.
typedef struct FlutterRustVulkanImage {
  uint64_t image;
  uint32_t format;
  // Binary semaphore signalled by wgpu once acquisition is complete. Impeller
  // consumes this wait before touching the image.
  uint64_t acquire_semaphore;
  // Binary semaphore signalled after Impeller's last submission. The broker
  // makes its final wgpu/present submission wait on it.
  uint64_t render_semaphore;
} FlutterRustVulkanImage;

typedef int (*FlutterRustAcquireVulkanImageCallback)(
    void* user_data,
    uint32_t width,
    uint32_t height,
    FlutterRustVulkanImage* image);
typedef int (*FlutterRustPresentVulkanImageCallback)(
    void* user_data,
    FlutterRustVulkanImage image);

typedef struct FlutterRustVulkanPresentationCallbacks {
  void* user_data;
  FlutterRustAcquireVulkanImageCallback acquire_image;
  FlutterRustPresentVulkanImageCallback present_image;
} FlutterRustVulkanPresentationCallbacks;

FlutterRustShellAbi FlutterRustShellGetAbi(void);

// Creates and destroys the C++ half of a Rust-owned task runner. The returned
// handle is opaque to Rust; only the callback table's owner may destroy it.
FLUTTER_RUST_SHELL_EXPORT void* FlutterRustShellCreateTaskRunner(
    FlutterRustTaskRunnerCallbacks callbacks);
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellRunTask(void* task_runner,
                                                      uint64_t task_baton);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellDestroyTaskRunner(
    void* task_runner);

// Raw Vulkan objects borrowed from Rust/wgpu for the lifetime of the shell
// they create. C-string arrays are borrowed only for the duration of the
// FlutterRustShellCreateShell call.
typedef struct FlutterRustVulkanContextData {
  void* get_instance_proc_addr;
  void* instance;
  void* physical_device;
  void* device;
  void* queue;
  uint32_t queue_family_index;
  const char* const* instance_extensions;
  uint32_t instance_extensions_count;
  const char* const* device_extensions;
  uint32_t device_extensions_count;
} FlutterRustVulkanContextData;

// Paths borrowed only for the duration of the FlutterRustShellCreateShell
// call; the engine copies what it needs.
typedef struct FlutterRustShellSettings {
  const char* assets_path;
  const char* icu_data_path;
} FlutterRustShellSettings;

// One pointer event produced by the Rust window host. Numeric enum values are
// translated explicitly on the C++ side rather than relying on Flutter's
// internal enum layout across the C ABI.
typedef enum FlutterRustPointerPhase {
  kFlutterRustPointerPhaseCancel = 0,
  kFlutterRustPointerPhaseAdd = 1,
  kFlutterRustPointerPhaseRemove = 2,
  kFlutterRustPointerPhaseHover = 3,
  kFlutterRustPointerPhaseDown = 4,
  kFlutterRustPointerPhaseMove = 5,
  kFlutterRustPointerPhaseUp = 6,
} FlutterRustPointerPhase;

typedef enum FlutterRustPointerDeviceKind {
  kFlutterRustPointerDeviceKindMouse = 0,
  kFlutterRustPointerDeviceKindTouch = 1,
} FlutterRustPointerDeviceKind;

typedef enum FlutterRustPointerSignalKind {
  kFlutterRustPointerSignalKindNone = 0,
  kFlutterRustPointerSignalKindScroll = 1,
} FlutterRustPointerSignalKind;

typedef struct FlutterRustPointerEvent {
  uint64_t timestamp_micros;
  uint32_t phase;
  uint32_t device_kind;
  uint32_t signal_kind;
  int64_t device;
  double physical_x;
  double physical_y;
  double scroll_delta_x;
  double scroll_delta_y;
  int64_t buttons;
} FlutterRustPointerEvent;

// Lifecycle values are translated explicitly into Flutter's framework-facing
// strings by C++; Rust does not send string pointers across the ABI.
typedef enum FlutterRustLifecycleState {
  kFlutterRustLifecycleStateDetached = 0,
  kFlutterRustLifecycleStateResumed = 1,
  kFlutterRustLifecycleStateInactive = 2,
  kFlutterRustLifecycleStateHidden = 3,
  kFlutterRustLifecycleStatePaused = 4,
} FlutterRustLifecycleState;

typedef enum FlutterRustKeyEventType {
  kFlutterRustKeyEventTypeDown = 0,
  kFlutterRustKeyEventTypeUp = 1,
  kFlutterRustKeyEventTypeRepeat = 2,
} FlutterRustKeyEventType;

#define FLUTTER_RUST_KEY_CHARACTER_CAPACITY 64u

// Text is stored inline to keep keyboard delivery value-only. character_length
// is a byte length and must not exceed FLUTTER_RUST_KEY_CHARACTER_CAPACITY.
typedef struct FlutterRustKeyEvent {
  uint64_t timestamp_micros;
  uint32_t event_type;
  uint64_t physical;
  uint64_t logical;
  int32_t synthesized;
  uint32_t character_length;
  uint8_t character[FLUTTER_RUST_KEY_CHARACTER_CAPACITY];
} FlutterRustKeyEvent;

// Creates the private engine-side half of one Rust-hosted Flutter
// application. `task_runner` must be a handle previously returned by
// FlutterRustShellCreateTaskRunner and is used as the merged UI/platform task
// runner; the caller retains ownership of it. Returns null on failure.
FLUTTER_RUST_SHELL_EXPORT void* FlutterRustShellCreateShell(
    void* task_runner,
    FlutterRustVulkanContextData context_data,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustShellSettings settings);

// Starts the root isolate and attaches the Vulkan presentation surface. Must
// run on the merged Rust UI/platform task runner. Returns non-zero on
// success.
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellRunShell(void* shell);

// Reports the implicit view's size to the running engine. Call once after
// FlutterRustShellRunShell succeeds and again on every resize; without this
// the root isolate has no valid view to schedule frames for. Must run on the
// merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSetViewportMetrics(
    void* shell,
    double width,
    double height,
    double pixel_ratio,
    double display_width,
    double display_height,
    double display_refresh_rate);

// Dispatches one mouse or touch event to the implicit Flutter view. Must run
// on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendPointerEvent(
    void* shell,
    FlutterRustPointerEvent event);

// Reports one application lifecycle transition. Unknown numeric enum values
// are ignored. Must run on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendLifecycleEvent(
    void* shell,
    uint32_t state);

// Dispatches one physical keyboard event to Flutter's key-data channel. Must
// run on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendKeyEvent(
    void* shell,
    FlutterRustKeyEvent event);

FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellDestroyShell(void* shell);

#ifdef __cplusplus
}
#endif

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
