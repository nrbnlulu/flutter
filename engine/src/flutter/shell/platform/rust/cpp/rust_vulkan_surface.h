// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_VULKAN_SURFACE_H_
#define FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_VULKAN_SURFACE_H_

#include <memory>
#include <mutex>
#include <unordered_map>

#include "flutter/flow/surface.h"
#include "flutter/shell/gpu/gpu_surface_vulkan_delegate.h"
#include "flutter/shell/platform/rust/cpp/rust_bridge.h"
#include "flutter/vulkan/procs/vulkan_proc_table.h"
#include "impeller/renderer/context.h"

namespace flutter {

// Bridges Rust-owned swapchain acquisition and presentation to Impeller. This
// is deliberately an engine-private alternative to the embedder surface: the
// callback ABI is owned by this target and no Flutter Embedder entry point is
// involved.
class RustVulkanPresentation final : public GPUSurfaceVulkanDelegate {
 public:
  RustVulkanPresentation(PFN_vkGetInstanceProcAddr get_instance_proc_addr,
                         std::shared_ptr<impeller::Context> context,
                         FlutterRustVulkanPresentationCallbacks callbacks);
  ~RustVulkanPresentation() override;

  bool IsValid() const;
  std::unique_ptr<Surface> CreateSurface();
  bool RegisterView(FlutterRustViewId view_id,
                    FlutterRustVulkanPresentationCallbacks callbacks);
  bool UnregisterView(FlutterRustViewId view_id);

 private:
  // |GPUSurfaceVulkanDelegate|
  const vulkan::VulkanProcTable& vk() override;
  void SetActiveViewId(int64_t view_id) override;
  FlutterVulkanImage AcquireImage(const DlISize& size) override;
  bool PresentImage(VkImage image, VkFormat format) override;

  struct ViewPresentation {
    FlutterRustVulkanPresentationCallbacks callbacks;
    VkSemaphore acquire_semaphore = VK_NULL_HANDLE;
    VkSemaphore render_semaphore = VK_NULL_HANDLE;
  };

  fml::RefPtr<vulkan::VulkanProcTable> vk_;
  std::shared_ptr<impeller::Context> context_;
  mutable std::mutex views_mutex_;
  std::unordered_map<FlutterRustViewId, std::shared_ptr<ViewPresentation>>
      views_;
  FlutterRustViewId active_view_id_ = FLUTTER_RUST_IMPLICIT_VIEW_ID;
  std::shared_ptr<ViewPresentation> active_frame_;
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_VULKAN_SURFACE_H_
