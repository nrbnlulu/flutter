// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_vulkan_surface.h"

#include "flutter/shell/gpu/gpu_surface_vulkan_impeller.h"
#include "impeller/renderer/backend/vulkan/command_buffer_vk.h"
#include "impeller/renderer/backend/vulkan/context_vk.h"
#include "impeller/renderer/backend/vulkan/queue_vk.h"

namespace flutter {

RustVulkanPresentation::RustVulkanPresentation(
    PFN_vkGetInstanceProcAddr get_instance_proc_addr,
    std::shared_ptr<impeller::Context> context,
    FlutterRustVulkanPresentationCallbacks callbacks)
    : vk_(fml::MakeRefCounted<vulkan::VulkanProcTable>(get_instance_proc_addr)),
      context_(std::move(context)) {
  views_.emplace(FLUTTER_RUST_IMPLICIT_VIEW_ID,
                 std::make_shared<ViewPresentation>(ViewPresentation{
                     .callbacks = callbacks,
                 }));
}

RustVulkanPresentation::~RustVulkanPresentation() = default;

bool RustVulkanPresentation::IsValid() const {
  std::scoped_lock lock(views_mutex_);
  const auto implicit = views_.find(FLUTTER_RUST_IMPLICIT_VIEW_ID);
  return context_ && context_->IsValid() && implicit != views_.end() &&
         implicit->second->callbacks.acquire_image &&
         implicit->second->callbacks.present_image;
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

bool RustVulkanPresentation::RegisterView(
    FlutterRustViewId view_id,
    FlutterRustVulkanPresentationCallbacks callbacks) {
  if (view_id <= FLUTTER_RUST_IMPLICIT_VIEW_ID || !callbacks.acquire_image ||
      !callbacks.present_image) {
    return false;
  }
  std::scoped_lock lock(views_mutex_);
  return views_
      .emplace(view_id, std::make_shared<ViewPresentation>(ViewPresentation{
                            .callbacks = callbacks,
                        }))
      .second;
}

bool RustVulkanPresentation::UnregisterView(FlutterRustViewId view_id) {
  if (view_id <= FLUTTER_RUST_IMPLICIT_VIEW_ID) {
    return false;
  }
  std::scoped_lock lock(views_mutex_);
  return views_.erase(view_id) == 1u;
}

void RustVulkanPresentation::SetActiveViewId(int64_t view_id) {
  std::scoped_lock lock(views_mutex_);
  active_view_id_ = view_id;
}

FlutterVulkanImage RustVulkanPresentation::AcquireImage(const DlISize& size) {
  std::shared_ptr<ViewPresentation> view;
  {
    std::scoped_lock lock(views_mutex_);
    const auto found = views_.find(active_view_id_);
    if (found == views_.end() || active_frame_) {
      return {};
    }
    view = found->second;
  }

  FlutterRustVulkanImage rust_image = {};
  if (view->acquire_semaphore != VK_NULL_HANDLE ||
      view->render_semaphore != VK_NULL_HANDLE || size.width <= 0 ||
      size.height <= 0 ||
      !view->callbacks.acquire_image(view->callbacks.user_data, size.width,
                                     size.height, &rust_image)) {
    return {};
  }

  view->acquire_semaphore = reinterpret_cast<VkSemaphore>(
      static_cast<uintptr_t>(rust_image.acquire_semaphore));
  view->render_semaphore = reinterpret_cast<VkSemaphore>(
      static_cast<uintptr_t>(rust_image.render_semaphore));
  if (rust_image.image == 0u || view->acquire_semaphore == VK_NULL_HANDLE ||
      view->render_semaphore == VK_NULL_HANDLE) {
    view->acquire_semaphore = VK_NULL_HANDLE;
    view->render_semaphore = VK_NULL_HANDLE;
    return {};
  }

  // Consume the semaphore signalled by wgpu's acquisition submission before
  // any Impeller work is allowed to touch the borrowed swapchain image.
  impeller::vk::PipelineStageFlags wait_stage =
      impeller::vk::PipelineStageFlagBits::eColorAttachmentOutput;
  impeller::vk::Semaphore acquire_semaphore(view->acquire_semaphore);
  impeller::vk::SubmitInfo submit_info;
  submit_info.setWaitSemaphores(acquire_semaphore);
  submit_info.setWaitDstStageMask(wait_stage);
  const auto& context = impeller::ContextVK::Cast(*context_);
  if (context.GetGraphicsQueue()->Submit(submit_info, {}) !=
      impeller::vk::Result::eSuccess) {
    view->acquire_semaphore = VK_NULL_HANDLE;
    view->render_semaphore = VK_NULL_HANDLE;
    return {};
  }
  {
    std::scoped_lock lock(views_mutex_);
    active_frame_ = std::move(view);
  }
  return {.struct_size = sizeof(FlutterVulkanImage),
          .image = rust_image.image,
          .format = rust_image.format};
}

bool RustVulkanPresentation::PresentImage(VkImage image, VkFormat format) {
  std::shared_ptr<ViewPresentation> view;
  {
    std::scoped_lock lock(views_mutex_);
    view = active_frame_;
  }
  if (!view || view->render_semaphore == VK_NULL_HANDLE) {
    return false;
  }

  // GPUSurfaceVulkanImpeller returns external images in
  // COLOR_ATTACHMENT_OPTIMAL. Wgpu's surface tracker expects its borrowed
  // swapchain image back in PRESENT_SRC_KHR before its final handoff pass.
  const auto& context = impeller::ContextVK::Cast(*context_);
  auto command_buffer = context.CreateCommandBuffer();
  if (!command_buffer) {
    return false;
  }
  auto vk_command_buffer =
      impeller::CommandBufferVK::Cast(*command_buffer).GetCommandBuffer();
  impeller::vk::ImageMemoryBarrier barrier;
  barrier.srcAccessMask = impeller::vk::AccessFlagBits::eColorAttachmentWrite;
  barrier.dstAccessMask = {};
  barrier.oldLayout = impeller::vk::ImageLayout::eColorAttachmentOptimal;
  barrier.newLayout = impeller::vk::ImageLayout::ePresentSrcKHR;
  barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
  barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
  barrier.image = image;
  barrier.subresourceRange.aspectMask =
      impeller::vk::ImageAspectFlagBits::eColor;
  barrier.subresourceRange.baseMipLevel = 0u;
  barrier.subresourceRange.levelCount = 1u;
  barrier.subresourceRange.baseArrayLayer = 0u;
  barrier.subresourceRange.layerCount = 1u;
  vk_command_buffer.pipelineBarrier(
      impeller::vk::PipelineStageFlagBits::eColorAttachmentOutput,
      impeller::vk::PipelineStageFlagBits::eBottomOfPipe, {}, nullptr, nullptr,
      barrier);
  if (!context.GetCommandQueue()->Submit({command_buffer}).ok()) {
    return false;
  }

  // This submission is ordered after the Rust-specific layout transition and
  // makes that completion visible to the wgpu broker.
  impeller::vk::Semaphore render_semaphore(view->render_semaphore);
  impeller::vk::SubmitInfo submit_info;
  submit_info.setSignalSemaphores(render_semaphore);
  if (context.GetGraphicsQueue()->Submit(submit_info, {}) !=
      impeller::vk::Result::eSuccess) {
    return false;
  }

  FlutterRustVulkanImage rust_image = {
      .image = reinterpret_cast<uint64_t>(image),
      .format = static_cast<uint32_t>(format),
      .acquire_semaphore = reinterpret_cast<uintptr_t>(view->acquire_semaphore),
      .render_semaphore = reinterpret_cast<uintptr_t>(view->render_semaphore),
  };
  view->acquire_semaphore = VK_NULL_HANDLE;
  view->render_semaphore = VK_NULL_HANDLE;
  {
    std::scoped_lock lock(views_mutex_);
    active_frame_.reset();
  }
  return view->callbacks.present_image(view->callbacks.user_data, rust_image) !=
         0;
}

}  // namespace flutter
