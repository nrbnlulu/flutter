// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_task_runner.h"

#include <optional>

#include "flutter/testing/testing.h"
#include "gtest/gtest.h"

namespace flutter {
namespace testing {
namespace {

struct ScheduledTask {
  RustTaskRunner* task_runner;
  uint64_t task_baton;
  fml::TimePoint target_time;
};

RustTaskRunner::DispatchTable MakeDispatchTable(
    std::optional<ScheduledTask>* scheduled_task,
    bool* destroyed) {
  RustTaskRunner::DispatchTable dispatch_table;
  dispatch_table.schedule_task = [scheduled_task](RustTaskRunner* task_runner,
                                                  uint64_t task_baton,
                                                  fml::TimePoint target_time) {
    *scheduled_task = {task_runner, task_baton, target_time};
  };
  dispatch_table.runs_tasks_on_current_thread = [] { return true; };
  dispatch_table.destruction_callback = [destroyed] { *destroyed = true; };
  return dispatch_table;
}

TEST(RustTaskRunnerTest, RunsTaskReturnedByHostLoop) {
  std::optional<ScheduledTask> scheduled_task;
  bool destroyed = false;
  auto task_runner =
      RustTaskRunner::Create(MakeDispatchTable(&scheduled_task, &destroyed));
  fml::RefPtr<fml::TaskRunner> task_runner_interface = task_runner;
  bool ran = false;

  task_runner_interface->PostTask([&ran] { ran = true; });

  ASSERT_TRUE(scheduled_task.has_value());
  EXPECT_TRUE(scheduled_task->task_runner->RunTask(scheduled_task->task_baton));
  EXPECT_TRUE(ran);
  EXPECT_FALSE(
      scheduled_task->task_runner->RunTask(scheduled_task->task_baton));
}

TEST(RustTaskRunnerTest, DelegatesThreadAffinityToHostLoop) {
  std::optional<ScheduledTask> scheduled_task;
  bool destroyed = false;
  auto task_runner =
      RustTaskRunner::Create(MakeDispatchTable(&scheduled_task, &destroyed));
  fml::RefPtr<fml::TaskRunner> task_runner_interface = task_runner;

  EXPECT_TRUE(task_runner_interface->RunsTasksOnCurrentThread());
}

TEST(RustTaskRunnerTest, InvokesDestructionCallback) {
  std::optional<ScheduledTask> scheduled_task;
  bool destroyed = false;
  {
    auto task_runner =
        RustTaskRunner::Create(MakeDispatchTable(&scheduled_task, &destroyed));
  }

  EXPECT_TRUE(destroyed);
}

}  // namespace
}  // namespace testing
}  // namespace flutter
