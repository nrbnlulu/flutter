// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_RUST_TASK_RUNNER_H_
#define FLUTTER_SHELL_PLATFORM_RUST_RUST_TASK_RUNNER_H_

#include <functional>
#include <mutex>
#include <unordered_map>

#include "flutter/fml/macros.h"
#include "flutter/fml/task_runner.h"
#include "flutter/shell/platform/rust/cpp/rust_bridge.h"

namespace flutter {

// A private task runner whose scheduling is driven by the Rust host event
// loop. The host receives opaque task batons and must return them through
// RunTask on the same thread that owns the runner.
class RustTaskRunner final : public fml::TaskRunner {
 public:
  struct DispatchTable {
    std::function<void(RustTaskRunner* task_runner,
                       uint64_t task_baton,
                       fml::TimePoint target_time)>
        schedule_task;
    std::function<bool()> runs_tasks_on_current_thread;
    std::function<void()> destruction_callback;
  };

  static fml::RefPtr<RustTaskRunner> Create(DispatchTable dispatch_table);

  // Creates a task runner whose dispatch table is supplied by the Rust host.
  // The callback table must outlive this runner's final destruction callback.
  static fml::RefPtr<RustTaskRunner> CreateForRustHost(
      FlutterRustTaskRunnerCallbacks callbacks);

  // Returns the runner owned by a handle previously returned by
  // FlutterRustShellCreateTaskRunner. The handle remains owned by its
  // original caller.
  static fml::RefPtr<RustTaskRunner> FromHandle(void* task_runner_handle);

  ~RustTaskRunner() override;

  // Called by the Rust event loop after the task's target time has elapsed.
  // Runs every registered task observer afterwards, mirroring
  // fml::MessageLoopImpl::FlushTasks: this is this runner's substitute for a
  // real fml::MessageLoop, and UIDartState relies on task observers running
  // after each task to flush the root isolate's microtask queue.
  bool RunTask(uint64_t task_baton);

  // Registers `callback` to run after every subsequent RunTask call. Suitable
  // for Settings::task_observer_add; the returned queue id is only a token
  // for the paired RemoveTaskObserver call and does not name a real
  // fml::MessageLoopTaskQueues queue.
  fml::TaskQueueId AddTaskObserver(intptr_t key, fml::closure callback);
  void RemoveTaskObserver(intptr_t key);

 private:
  explicit RustTaskRunner(DispatchTable dispatch_table);

  // |fml::TaskRunner|
  void PostTask(const fml::closure& task) override;
  void PostTaskForTime(const fml::closure& task,
                       fml::TimePoint target_time) override;
  void PostDelayedTask(const fml::closure& task, fml::TimeDelta delay) override;
  bool RunsTasksOnCurrentThread() override;
  fml::TaskQueueId GetTaskQueueId() override;

  DispatchTable dispatch_table_;
  std::mutex tasks_mutex_;
  uint64_t last_baton_ = 0;
  std::unordered_map<uint64_t, fml::closure> pending_tasks_;
  std::unordered_map<intptr_t, fml::closure> task_observers_;
  fml::TaskQueueId placeholder_id_;

  FML_FRIEND_MAKE_REF_COUNTED(RustTaskRunner);
  FML_DISALLOW_COPY_AND_ASSIGN(RustTaskRunner);
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_TASK_RUNNER_H_
