// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/platform_view_rust.h"

#include <utility>

#include "flutter/fml/logging.h"

namespace flutter {

PlatformViewRust::PlatformViewRust(Delegate& delegate,
                                   const TaskRunners& task_runners,
                                   Configuration configuration)
    : PlatformView(delegate, task_runners),
      configuration_(std::move(configuration)) {}

PlatformViewRust::~PlatformViewRust() = default;

void PlatformViewRust::HandlePlatformMessage(
    std::unique_ptr<PlatformMessage> message) {
  if (!message) {
    return;
  }

  if (configuration_.handle_platform_message) {
    configuration_.handle_platform_message(std::move(message));
    return;
  }

  if (auto response = message->response()) {
    response->CompleteEmpty();
  }
}

void PlatformViewRust::UpdateSemantics(
    int64_t view_id,
    SemanticsNodeUpdates update,
    CustomAccessibilityActionUpdates actions) {
  if (configuration_.update_semantics) {
    configuration_.update_semantics(view_id, std::move(update),
                                    std::move(actions));
  }
}

std::unique_ptr<Surface> PlatformViewRust::CreateRenderingSurface() {
  if (!configuration_.create_rendering_surface) {
    FML_LOG(ERROR) << "Rust platform view has no rendering surface callback.";
    return nullptr;
  }
  return configuration_.create_rendering_surface();
}

}  // namespace flutter
