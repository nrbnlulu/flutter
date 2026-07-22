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

FlutterRustShellAbi FlutterRustShellGetAbi(void);

// Creates and destroys the C++ half of a Rust-owned task runner. The returned
// handle is opaque to Rust; only the callback table's owner may destroy it.
void* FlutterRustShellCreateTaskRunner(
    FlutterRustTaskRunnerCallbacks callbacks);
int FlutterRustShellRunTask(void* task_runner, uint64_t task_baton);
void FlutterRustShellDestroyTaskRunner(void* task_runner);

#ifdef __cplusplus
}
#endif

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
