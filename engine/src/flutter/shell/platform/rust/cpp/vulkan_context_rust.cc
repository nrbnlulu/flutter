// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/cpp/vulkan_context_rust.h"

#include "flutter/fml/mapping.h"
#include "flutter/impeller/entity/vk/entity_shaders_vk.h"
#include "flutter/impeller/entity/vk/framebuffer_blend_shaders_vk.h"
#include "flutter/impeller/entity/vk/modern_shaders_vk.h"
#include "impeller/renderer/backend/vulkan/context_vk.h"

namespace flutter {

std::shared_ptr<impeller::Context> CreateRustVulkanContext(
    RustVulkanContextData data) {
  if (!data.get_instance_proc_addr || !data.instance || !data.physical_device ||
      !data.device || !data.queue) {
    return nullptr;
  }

  impeller::ContextVK::EmbedderData embedder_data;
  embedder_data.instance = reinterpret_cast<VkInstance>(data.instance);
  embedder_data.physical_device =
      reinterpret_cast<VkPhysicalDevice>(data.physical_device);
  embedder_data.device = reinterpret_cast<VkDevice>(data.device);
  embedder_data.queue_family_index = data.queue_family_index;
  embedder_data.queue = reinterpret_cast<VkQueue>(data.queue);
  embedder_data.instance_extensions = std::move(data.instance_extensions);
  embedder_data.device_extensions = std::move(data.device_extensions);

  impeller::ContextVK::Settings settings;
  settings.proc_address_callback =
      reinterpret_cast<PFN_vkGetInstanceProcAddr>(data.get_instance_proc_addr);
  settings.embedder_data = std::move(embedder_data);
  settings.shader_libraries_data = {
      std::make_shared<fml::NonOwnedMapping>(impeller_entity_shaders_vk_data,
                                             impeller_entity_shaders_vk_length),
      std::make_shared<fml::NonOwnedMapping>(impeller_modern_shaders_vk_data,
                                             impeller_modern_shaders_vk_length),
      std::make_shared<fml::NonOwnedMapping>(
          impeller_framebuffer_blend_shaders_vk_data,
          impeller_framebuffer_blend_shaders_vk_length),
  };
  return impeller::ContextVK::Create(std::move(settings));
}

}  // namespace flutter
