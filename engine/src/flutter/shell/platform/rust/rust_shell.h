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
      FlutterRustVsyncCallbacks vsync_callbacks,
      FlutterRustWindowingCallbacks windowing_callbacks,
      Settings settings);

  ~RustShell();

  bool IsValid() const;

  // Starts the root isolate and attaches the Vulkan presentation surface.
  // Must run on the merged Rust UI/platform task runner.
  bool Run();

  // Reports one Flutter view's physical metrics.
  // Must run on the merged Rust UI/platform task runner.
  void SetViewportMetrics(FlutterRustViewId view_id,
                          const FlutterRustViewMetrics& metrics);

  // Adds/removes a non-implicit view in this engine. Completion is delivered
  // asynchronously on the merged Rust UI/platform runner.
  void AddView(FlutterRustViewId view_id,
               const FlutterRustViewMetrics& metrics,
               FlutterRustVulkanPresentationCallbacks presentation_callbacks,
               FlutterRustViewOperationCallbacks callbacks);
  void RemoveView(FlutterRustViewId view_id,
                  FlutterRustViewOperationCallbacks callbacks);

  void SendViewFocusEvent(FlutterRustViewId view_id,
                          uint32_t state,
                          uint32_t direction);

  // Dispatches one Rust-hosted pointer event to its target view.
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
  bool SendPlatformMessageWithResponse(
      const uint8_t* channel,
      uint64_t channel_size,
      const uint8_t* message,
      uint64_t message_size,
      FlutterRustPlatformMessageResponseCallback callback,
      void* user_data);

  // Delivers one compositor-aligned pulse to the platform view's waiter.
  void OnVsync(uint64_t frame_interval_nanos);

  FlutterRustViewId CreateRegularWindow(
      const FlutterRustRegularWindowRequest* request);
  FlutterRustViewId CreateDialogWindow(
      const FlutterRustDialogWindowRequest* request);
  FlutterRustViewId CreatePopupWindow(
      const FlutterRustPopupWindowRequest* request);
  FlutterRustViewId CreateSatelliteWindow(
      const FlutterRustSatelliteWindowRequest* request);
  void DestroyWindow(FlutterRustViewId view_id);
  bool GetWindowState(FlutterRustViewId view_id, FlutterRustWindowState* state);
  void SetWindowSize(FlutterRustViewId view_id, double width, double height);
  void SetWindowConstraints(FlutterRustViewId view_id,
                            int32_t has_constraints,
                            double min_width,
                            double min_height,
                            double max_width,
                            double max_height);
  void SetWindowTitle(FlutterRustViewId view_id,
                      const uint8_t* title,
                      uint64_t title_length);
  void ActivateWindow(FlutterRustViewId view_id);
  void SetWindowMaximized(FlutterRustViewId view_id, bool maximized);
  void SetWindowMinimized(FlutterRustViewId view_id, bool minimized);
  void SetWindowFullscreen(FlutterRustViewId view_id, bool fullscreen);
  void SetWindowEventCallback(FlutterRustWindowEventCallback callback);
  bool SetWindowParent(FlutterRustViewId view_id,
                       FlutterRustViewId parent_view_id);

 private:
  RustShell(std::unique_ptr<ThreadHost> thread_host,
            std::shared_ptr<RustVulkanPresentation> presentation,
            std::unique_ptr<Shell> shell,
            FlutterRustWindowingCallbacks windowing_callbacks,
            Settings settings);

  std::unique_ptr<ThreadHost> thread_host_;
  std::shared_ptr<RustVulkanPresentation> presentation_;
  std::unique_ptr<Shell> shell_;
  FlutterRustWindowingCallbacks windowing_callbacks_;
  Settings settings_;
  bool running_ = false;
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_SHELL_H_
