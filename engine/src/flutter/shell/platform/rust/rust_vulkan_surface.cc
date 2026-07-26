// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_vulkan_surface.h"

#include "flutter/shell/gpu/gpu_surface_vulkan_impeller.h"

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
  if (size.width <= 0 || size.height <= 0 ||
      !callbacks_.acquire_image(callbacks_.user_data, size.width, size.height,
                                &rust_image)) {
    return {};
  }
  return {.struct_size = sizeof(FlutterVulkanImage),
          .image = rust_image.image,
          .format = rust_image.format};
}

bool RustVulkanPresentation::PresentImage(VkImage image, VkFormat format) {
  return callbacks_.present_image(callbacks_.user_data,
                                  {.image = reinterpret_cast<uint64_t>(image),
                                   .format = static_cast<uint32_t>(format)}) !=
         0;
}

}  // namespace flutter
