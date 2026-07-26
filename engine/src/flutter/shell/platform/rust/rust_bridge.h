// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
#define FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// This private ABI is lockstep-versioned with the Flutter fork. It is not the
// Flutter Embedder API and is never exposed to application plugins.
#define FLUTTER_RUST_SHELL_ABI_VERSION 1u
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
void* FlutterRustShellCreateTaskRunner(
    FlutterRustTaskRunnerCallbacks callbacks);
int FlutterRustShellRunTask(void* task_runner, uint64_t task_baton);
void FlutterRustShellDestroyTaskRunner(void* task_runner);

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

// Creates the private engine-side half of one Rust-hosted Flutter
// application. `task_runner` must be a handle previously returned by
// FlutterRustShellCreateTaskRunner and is used as the merged UI/platform task
// runner; the caller retains ownership of it. Returns null on failure.
void* FlutterRustShellCreateShell(
    void* task_runner,
    FlutterRustVulkanContextData context_data,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustShellSettings settings);

// Starts the root isolate and attaches the Vulkan presentation surface. Must
// run on the merged Rust UI/platform task runner. Returns non-zero on
// success.
int FlutterRustShellRunShell(void* shell);

// Reports the implicit view's size to the running engine. Call once after
// FlutterRustShellRunShell succeeds and again on every resize; without this
// the root isolate has no valid view to schedule frames for. Must run on the
// merged Rust UI/platform task runner.
void FlutterRustShellSetViewportMetrics(void* shell,
                                        double width,
                                        double height,
                                        double pixel_ratio);

void FlutterRustShellDestroyShell(void* shell);

#ifdef __cplusplus
}
#endif

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
