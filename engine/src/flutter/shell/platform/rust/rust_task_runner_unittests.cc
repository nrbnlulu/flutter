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

struct RustHostState {
  std::optional<ScheduledTask> scheduled_task;
  uint64_t delay_nanos = 0;
  bool destroyed = false;
};

void ScheduleRustHostTask(void* user_data,
                          void* task_runner,
                          uint64_t task_baton,
                          uint64_t delay_nanos) {
  auto* state = static_cast<RustHostState*>(user_data);
  state->scheduled_task = {
      static_cast<RustTaskRunner*>(task_runner),
      task_baton,
      fml::TimePoint(),
  };
  state->delay_nanos = delay_nanos;
}

int RustHostRunsTasksOnCurrentThread(void*) {
  return 1;
}

void RustHostTaskRunnerDestroyed(void* user_data) {
  static_cast<RustHostState*>(user_data)->destroyed = true;
}

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

TEST(RustTaskRunnerTest, DispatchesTasksThroughPrivateRustCallbacks) {
  RustHostState state;
  {
    FlutterRustTaskRunnerCallbacks callbacks = {
        .user_data = &state,
        .schedule_task = ScheduleRustHostTask,
        .runs_tasks_on_current_thread = RustHostRunsTasksOnCurrentThread,
        .task_runner_destroyed = RustHostTaskRunnerDestroyed,
    };
    auto task_runner = RustTaskRunner::CreateForRustHost(callbacks);
    fml::RefPtr<fml::TaskRunner> task_runner_interface = task_runner;
    bool ran = false;

    task_runner_interface->PostTask([&ran] { ran = true; });

    ASSERT_TRUE(state.scheduled_task.has_value());
    EXPECT_TRUE(state.scheduled_task->task_runner->RunTask(
        state.scheduled_task->task_baton));
    EXPECT_TRUE(ran);
    EXPECT_TRUE(task_runner_interface->RunsTasksOnCurrentThread());
  }
  EXPECT_TRUE(state.destroyed);
}

TEST(RustTaskRunnerTest, OwnsOpaqueRunnerHandleForRustHost) {
  RustHostState state;
  FlutterRustTaskRunnerCallbacks callbacks = {
      .user_data = &state,
      .schedule_task = ScheduleRustHostTask,
      .runs_tasks_on_current_thread = RustHostRunsTasksOnCurrentThread,
      .task_runner_destroyed = RustHostTaskRunnerDestroyed,
  };
  void* task_runner = FlutterRustShellCreateTaskRunner(callbacks);
  ASSERT_NE(task_runner, nullptr);

  // FlutterRustShellRunTask must accept the same `task_runner` identity that
  // schedule_task forwards to Rust (RustTaskRunner::FromHandle's underlying
  // pointer), not the opaque handle returned by
  // FlutterRustShellCreateTaskRunner. Post through the real interface so this
  // exercises that identity end to end, the way the production Rust host
  // does. FromHandle returns its own retained reference, so it must go out of
  // scope before FlutterRustShellDestroyTaskRunner releases the handle's
  // reference, or the destruction callback below will not fire.
  {
    fml::RefPtr<fml::TaskRunner> task_runner_interface =
        RustTaskRunner::FromHandle(task_runner);
    bool ran = false;
    task_runner_interface->PostTask([&ran] { ran = true; });

    ASSERT_TRUE(state.scheduled_task.has_value());
    EXPECT_TRUE(FlutterRustShellRunTask(state.scheduled_task->task_runner,
                                        state.scheduled_task->task_baton));
    EXPECT_TRUE(ran);
    EXPECT_FALSE(FlutterRustShellRunTask(state.scheduled_task->task_runner,
                                         state.scheduled_task->task_baton));
  }

  FlutterRustShellDestroyTaskRunner(task_runner);
  EXPECT_TRUE(state.destroyed);
}

}  // namespace
}  // namespace testing
}  // namespace flutter
