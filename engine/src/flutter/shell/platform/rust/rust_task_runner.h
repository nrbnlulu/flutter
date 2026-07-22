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

  ~RustTaskRunner() override;

  // Called by the Rust event loop after the task's target time has elapsed.
  bool RunTask(uint64_t task_baton);

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
  fml::TaskQueueId placeholder_id_;

  FML_FRIEND_MAKE_REF_COUNTED(RustTaskRunner);
  FML_DISALLOW_COPY_AND_ASSIGN(RustTaskRunner);
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_TASK_RUNNER_H_
