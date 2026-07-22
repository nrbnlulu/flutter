// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_task_runner.h"

#include <utility>

#include "flutter/fml/message_loop_impl.h"
#include "flutter/fml/message_loop_task_queues.h"

namespace flutter {

namespace {

uint64_t DelayNanosUntil(fml::TimePoint target_time) {
  const fml::TimeDelta delay = target_time - fml::TimePoint::Now();
  return delay.ToNanoseconds() > 0
             ? static_cast<uint64_t>(delay.ToNanoseconds())
             : 0u;
}

struct RustTaskRunnerHandle {
  explicit RustTaskRunnerHandle(fml::RefPtr<RustTaskRunner> runner)
      : runner(std::move(runner)) {}

  fml::RefPtr<RustTaskRunner> runner;
};

}  // namespace

fml::RefPtr<RustTaskRunner> RustTaskRunner::Create(
    DispatchTable dispatch_table) {
  return fml::MakeRefCounted<RustTaskRunner>(std::move(dispatch_table));
}

fml::RefPtr<RustTaskRunner> RustTaskRunner::CreateForRustHost(
    FlutterRustTaskRunnerCallbacks callbacks) {
  FML_DCHECK(callbacks.schedule_task);
  FML_DCHECK(callbacks.runs_tasks_on_current_thread);
  FML_DCHECK(callbacks.task_runner_destroyed);

  DispatchTable dispatch_table;
  dispatch_table.schedule_task = [callbacks](RustTaskRunner* task_runner,
                                             uint64_t task_baton,
                                             fml::TimePoint target_time) {
    callbacks.schedule_task(callbacks.user_data, task_runner, task_baton,
                            DelayNanosUntil(target_time));
  };
  dispatch_table.runs_tasks_on_current_thread = [callbacks] {
    return callbacks.runs_tasks_on_current_thread(callbacks.user_data) != 0;
  };
  dispatch_table.destruction_callback = [callbacks] {
    callbacks.task_runner_destroyed(callbacks.user_data);
  };
  return Create(std::move(dispatch_table));
}

RustTaskRunner::RustTaskRunner(DispatchTable dispatch_table)
    : TaskRunner(nullptr),
      dispatch_table_(std::move(dispatch_table)),
      placeholder_id_(fml::TaskQueueId::kInvalid) {
  FML_DCHECK(dispatch_table_.schedule_task);
  FML_DCHECK(dispatch_table_.runs_tasks_on_current_thread);
  FML_DCHECK(dispatch_table_.destruction_callback);
}

RustTaskRunner::~RustTaskRunner() {
  dispatch_table_.destruction_callback();
}

void RustTaskRunner::PostTask(const fml::closure& task) {
  PostTaskForTime(task, fml::TimePoint::Now());
}

void RustTaskRunner::PostTaskForTime(const fml::closure& task,
                                     fml::TimePoint target_time) {
  if (!task) {
    return;
  }

  uint64_t task_baton;
  {
    std::scoped_lock lock(tasks_mutex_);
    task_baton = ++last_baton_;
    pending_tasks_[task_baton] = task;
  }

  dispatch_table_.schedule_task(this, task_baton, target_time);
}

void RustTaskRunner::PostDelayedTask(const fml::closure& task,
                                     fml::TimeDelta delay) {
  PostTaskForTime(task, fml::TimePoint::Now() + delay);
}

bool RustTaskRunner::RunsTasksOnCurrentThread() {
  return dispatch_table_.runs_tasks_on_current_thread();
}

fml::TaskQueueId RustTaskRunner::GetTaskQueueId() {
  return placeholder_id_;
}

bool RustTaskRunner::RunTask(uint64_t task_baton) {
  fml::closure task;
  {
    std::scoped_lock lock(tasks_mutex_);
    const auto found = pending_tasks_.find(task_baton);
    if (found == pending_tasks_.end()) {
      return false;
    }
    task = found->second;
    pending_tasks_.erase(found);
  }

  task();
  return true;
}

}  // namespace flutter

extern "C" void* FlutterRustShellCreateTaskRunner(
    FlutterRustTaskRunnerCallbacks callbacks) {
  auto runner = flutter::RustTaskRunner::CreateForRustHost(callbacks);
  return new flutter::RustTaskRunnerHandle(std::move(runner));
}

extern "C" int FlutterRustShellRunTask(void* task_runner, uint64_t task_baton) {
  if (!task_runner) {
    return 0;
  }
  auto* handle = static_cast<flutter::RustTaskRunnerHandle*>(task_runner);
  return handle->runner->RunTask(task_baton) ? 1 : 0;
}

extern "C" void FlutterRustShellDestroyTaskRunner(void* task_runner) {
  delete static_cast<flutter::RustTaskRunnerHandle*>(task_runner);
}
