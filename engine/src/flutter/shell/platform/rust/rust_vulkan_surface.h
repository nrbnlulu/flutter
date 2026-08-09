// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_RUST_VULKAN_SURFACE_H_
#define FLUTTER_SHELL_PLATFORM_RUST_RUST_VULKAN_SURFACE_H_

#include <memory>

#include "flutter/flow/surface.h"
#include "flutter/shell/gpu/gpu_surface_vulkan_delegate.h"
#include "flutter/shell/platform/rust/rust_bridge.h"
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

 private:
  // |GPUSurfaceVulkanDelegate|
  const vulkan::VulkanProcTable& vk() override;
  FlutterVulkanImage AcquireImage(const DlISize& size) override;
  bool PresentImage(VkImage image, VkFormat format) override;

  fml::RefPtr<vulkan::VulkanProcTable> vk_;
  std::shared_ptr<impeller::Context> context_;
  FlutterRustVulkanPresentationCallbacks callbacks_;
  VkSemaphore acquire_semaphore_ = VK_NULL_HANDLE;
  VkSemaphore render_semaphore_ = VK_NULL_HANDLE;
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_VULKAN_SURFACE_H_
