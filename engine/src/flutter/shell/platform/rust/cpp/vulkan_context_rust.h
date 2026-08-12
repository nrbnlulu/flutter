// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_CPP_VULKAN_CONTEXT_RUST_H_
#define FLUTTER_SHELL_PLATFORM_RUST_CPP_VULKAN_CONTEXT_RUST_H_

#include <memory>
#include <string>
#include <vector>

#include "impeller/renderer/context.h"

namespace flutter {

// Raw Vulkan objects borrowed from Rust/wgpu. Rust retains ownership and must
// keep every object alive until the returned Impeller context is destroyed.
struct RustVulkanContextData {
  void* get_instance_proc_addr = nullptr;
  void* instance = nullptr;
  void* physical_device = nullptr;
  void* device = nullptr;
  void* queue = nullptr;
  uint32_t queue_family_index = 0;
  std::vector<std::string> instance_extensions;
  std::vector<std::string> device_extensions;
};

std::shared_ptr<impeller::Context> CreateRustVulkanContext(
    RustVulkanContextData data);

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_CPP_VULKAN_CONTEXT_RUST_H_
