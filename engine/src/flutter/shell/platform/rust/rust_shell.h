// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_RUST_SHELL_H_
#define FLUTTER_SHELL_PLATFORM_RUST_RUST_SHELL_H_

#include <memory>

#include "flutter/common/settings.h"
#include "flutter/fml/task_runner.h"
#include "flutter/shell/platform/rust/rust_bridge.h"
#include "flutter/shell/platform/rust/vulkan_context_rust.h"

namespace flutter {

class Shell;
struct ThreadHost;
class RustVulkanPresentation;

// Owns the private engine-side half of one Rust-hosted Flutter application.
// It is created and destroyed on the winit thread. Rust owns the task runner,
// Vulkan objects, and presentation callbacks for the entire lifetime here.
class RustShell final {
 public:
  static std::unique_ptr<RustShell> Create(
      fml::RefPtr<fml::TaskRunner> main_task_runner,
      RustVulkanContextData context_data,
      FlutterRustVulkanPresentationCallbacks presentation_callbacks,
      FlutterRustPlatformMessageCallbacks platform_message_callbacks,
      Settings settings);

  ~RustShell();

  bool IsValid() const;

  // Starts the root isolate and attaches the Vulkan presentation surface.
  // Must run on the merged Rust UI/platform task runner.
  bool Run();

  // Reports the implicit view's size to the running engine. Without this the
  // root isolate's widget binding has no valid view to schedule frames for,
  // so PlatformView::NotifyCreated alone is not enough to see any content.
  // Must run on the merged Rust UI/platform task runner.
  void SetViewportMetrics(double width,
                          double height,
                          double pixel_ratio,
                          double display_width,
                          double display_height,
                          double display_refresh_rate);

  // Dispatches one Rust-hosted pointer event to the implicit view.
  void SendPointerEvent(const FlutterRustPointerEvent& event);

  // Dispatches a private-ABI lifecycle state on flutter/lifecycle.
  void SendLifecycleEvent(uint32_t state);

  // Dispatches a private-ABI physical keyboard event.
  void SendKeyEvent(const FlutterRustKeyEvent& event);

  // Dispatches one encoded platform message to the Flutter framework.
  void SendPlatformMessage(const uint8_t* channel,
                           uint64_t channel_size,
                           const uint8_t* message,
                           uint64_t message_size);

 private:
  RustShell(std::unique_ptr<ThreadHost> thread_host,
            std::shared_ptr<RustVulkanPresentation> presentation,
            std::unique_ptr<Shell> shell,
            Settings settings);

  std::unique_ptr<ThreadHost> thread_host_;
  std::shared_ptr<RustVulkanPresentation> presentation_;
  std::unique_ptr<Shell> shell_;
  Settings settings_;
  bool running_ = false;
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_SHELL_H_
