// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_task_runner.h"

#include <utility>

#include "flutter/fml/message_loop_impl.h"
#include "flutter/fml/message_loop_task_queues.h"

namespace flutter {

fml::RefPtr<RustTaskRunner> RustTaskRunner::Create(
    DispatchTable dispatch_table) {
  return fml::MakeRefCounted<RustTaskRunner>(std::move(dispatch_table));
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
