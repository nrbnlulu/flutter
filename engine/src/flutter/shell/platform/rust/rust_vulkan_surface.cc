// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_vulkan_surface.h"

#include "flutter/shell/gpu/gpu_surface_vulkan_impeller.h"
#include "impeller/renderer/backend/vulkan/context_vk.h"
#include "impeller/renderer/backend/vulkan/queue_vk.h"

namespace flutter {

RustVulkanPresentation::RustVulkanPresentation(
    PFN_vkGetInstanceProcAddr get_instance_proc_addr,
    std::shared_ptr<impeller::Context> context,
    FlutterRustVulkanPresentationCallbacks callbacks)
    : vk_(fml::MakeRefCounted<vulkan::VulkanProcTable>(get_instance_proc_addr)),
      context_(std::move(context)),
      callbacks_(callbacks) {}

RustVulkanPresentation::~RustVulkanPresentation() = default;

bool RustVulkanPresentation::IsValid() const {
  return context_ && context_->IsValid() && callbacks_.acquire_image &&
         callbacks_.present_image;
}

std::unique_ptr<Surface> RustVulkanPresentation::CreateSurface() {
  if (!IsValid()) {
    return nullptr;
  }
  return std::make_unique<GPUSurfaceVulkanImpeller>(this, context_);
}

const vulkan::VulkanProcTable& RustVulkanPresentation::vk() {
  return *vk_;
}

FlutterVulkanImage RustVulkanPresentation::AcquireImage(const DlISize& size) {
  FlutterRustVulkanImage rust_image = {};
  if (acquire_semaphore_ != VK_NULL_HANDLE ||
      render_semaphore_ != VK_NULL_HANDLE || size.width <= 0 ||
      size.height <= 0 ||
      !callbacks_.acquire_image(callbacks_.user_data, size.width, size.height,
                                &rust_image)) {
    return {};
  }

  acquire_semaphore_ = reinterpret_cast<VkSemaphore>(
      static_cast<uintptr_t>(rust_image.acquire_semaphore));
  render_semaphore_ = reinterpret_cast<VkSemaphore>(
      static_cast<uintptr_t>(rust_image.render_semaphore));
  if (rust_image.image == 0u || acquire_semaphore_ == VK_NULL_HANDLE ||
      render_semaphore_ == VK_NULL_HANDLE) {
    acquire_semaphore_ = VK_NULL_HANDLE;
    render_semaphore_ = VK_NULL_HANDLE;
    return {};
  }

  // Consume the semaphore signalled by wgpu's acquisition submission before
  // any Impeller work is allowed to touch the borrowed swapchain image.
  impeller::vk::PipelineStageFlags wait_stage =
      impeller::vk::PipelineStageFlagBits::eColorAttachmentOutput;
  impeller::vk::Semaphore acquire_semaphore(acquire_semaphore_);
  impeller::vk::SubmitInfo submit_info;
  submit_info.setWaitSemaphores(acquire_semaphore);
  submit_info.setWaitDstStageMask(wait_stage);
  const auto& context = impeller::ContextVK::Cast(*context_);
  if (context.GetGraphicsQueue()->Submit(submit_info, {}) !=
      impeller::vk::Result::eSuccess) {
    acquire_semaphore_ = VK_NULL_HANDLE;
    render_semaphore_ = VK_NULL_HANDLE;
    return {};
  }
  return {.struct_size = sizeof(FlutterVulkanImage),
          .image = rust_image.image,
          .format = rust_image.format};
}

bool RustVulkanPresentation::PresentImage(VkImage image, VkFormat format) {
  if (render_semaphore_ == VK_NULL_HANDLE) {
    return false;
  }

  // This empty submission is ordered after Impeller's final image-layout
  // transition and makes that completion visible to the wgpu broker.
  impeller::vk::Semaphore render_semaphore(render_semaphore_);
  impeller::vk::SubmitInfo submit_info;
  submit_info.setSignalSemaphores(render_semaphore);
  const auto& context = impeller::ContextVK::Cast(*context_);
  if (context.GetGraphicsQueue()->Submit(submit_info, {}) !=
      impeller::vk::Result::eSuccess) {
    return false;
  }

  FlutterRustVulkanImage rust_image = {
      .image = reinterpret_cast<uint64_t>(image),
      .format = static_cast<uint32_t>(format),
      .acquire_semaphore = reinterpret_cast<uintptr_t>(acquire_semaphore_),
      .render_semaphore = reinterpret_cast<uintptr_t>(render_semaphore_),
  };
  acquire_semaphore_ = VK_NULL_HANDLE;
  render_semaphore_ = VK_NULL_HANDLE;
  return callbacks_.present_image(callbacks_.user_data, rust_image) != 0;
}

}  // namespace flutter
