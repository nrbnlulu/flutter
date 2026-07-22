// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_PLATFORM_VIEW_RUST_H_
#define FLUTTER_SHELL_PLATFORM_RUST_PLATFORM_VIEW_RUST_H_

#include <functional>
#include <memory>

#include "flutter/shell/common/platform_view.h"

namespace flutter {

// The in-tree platform view used by the optional Rust shell.
//
// This class is deliberately a narrow C++ adapter. The Rust runtime owns
// windowing and platform services; future Rust FFI supplies the callbacks in
// Configuration. Keeping C++ inheritance on this side of the boundary avoids
// exposing Flutter's C++ ABI to Rust.
class PlatformViewRust final : public PlatformView {
 public:
  using CreateRenderingSurfaceCallback =
      std::function<std::unique_ptr<Surface>()>;
  using HandlePlatformMessageCallback =
      std::function<void(std::unique_ptr<PlatformMessage>)>;
  using UpdateSemanticsCallback = std::function<
      void(int64_t, SemanticsNodeUpdates, CustomAccessibilityActionUpdates)>;

  struct Configuration {
    // Invoked on Flutter's raster task runner.
    CreateRenderingSurfaceCallback create_rendering_surface;

    // Invoked on the merged Flutter UI/platform task runner.
    HandlePlatformMessageCallback handle_platform_message;

    // Invoked when Flutter publishes a semantics update.
    UpdateSemanticsCallback update_semantics;
  };

  PlatformViewRust(Delegate& delegate,
                   const TaskRunners& task_runners,
                   Configuration configuration);

  ~PlatformViewRust() override;

  // |PlatformView|
  void HandlePlatformMessage(std::unique_ptr<PlatformMessage> message) override;

  // |PlatformView|
  void UpdateSemantics(int64_t view_id,
                       SemanticsNodeUpdates update,
                       CustomAccessibilityActionUpdates actions) override;

 private:
  // |PlatformView|
  std::unique_ptr<Surface> CreateRenderingSurface() override;

  Configuration configuration_;

  FML_DISALLOW_COPY_AND_ASSIGN(PlatformViewRust);
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_PLATFORM_VIEW_RUST_H_
