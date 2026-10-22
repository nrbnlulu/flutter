// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "flutter/shell/platform/rust/cpp/rust_android.h"

#include <atomic>
#include <set>
#include "flutter/fml/synchronization/waitable_event.h"
#include "flutter/impeller/renderer/backend/vulkan/context_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/queue_vk.h"
#include "flutter/shell/platform/android/android_shell_holder.h"
#include "flutter/shell/platform/rust/cpp/rust_external_texture.h"
#include "flutter/shell/platform/rust/cpp/vulkan_context_rust.h"

namespace flutter {
namespace {
std::atomic_uint32_t g_active_rust_runtimes = 0;
class BorrowedAndroidContext final : public AndroidContext {
 public:
  explicit BorrowedAndroidContext(std::shared_ptr<impeller::Context> context)
      : AndroidContext(AndroidRenderingAPI::kImpellerVulkan) {
    SetImpellerContext(context);
  }
  bool IsValid() const override { return !!GetImpellerContext(); }
};
struct Runtime {
  std::shared_ptr<AndroidContext> context;
  fml::WeakPtr<PlatformViewAndroid> view;
  fml::RefPtr<fml::TaskRunner> platform;
  fml::RefPtr<fml::TaskRunner> raster;
  // Java FlutterRenderer allocates texture IDs upwards from zero.
  int64_t next_id = int64_t{1} << 40;
  std::set<int64_t> textures;
};
Runtime& Get(void* runtime) {
  return *static_cast<Runtime*>(runtime);
}
}  // namespace
std::shared_ptr<AndroidContext> RustAndroidContext(void* runtime) {
  return runtime ? Get(runtime).context : nullptr;
}
bool HasRustAndroidRuntime() {
  return g_active_rust_runtimes.load(std::memory_order_acquire) != 0;
}
void AttachRustAndroidRuntime(void* runtime, AndroidShellHolder& holder) {
  if (!runtime)
    return;
  auto& state = Get(runtime);
  state.view = holder.GetPlatformView();
  const auto& runners = holder.GetTaskRunners();
  state.platform = runners.GetPlatformTaskRunner();
  state.raster = runners.GetRasterTaskRunner();
}
}  // namespace flutter

extern "C" void* FlutterRustAndroidCreate(FlutterRustVulkanContextData data) {
  flutter::RustVulkanContextData vk;
  vk.get_instance_proc_addr = data.get_instance_proc_addr;
  vk.instance = data.instance;
  vk.physical_device = data.physical_device;
  vk.device = data.device;
  vk.queue = data.queue;
  vk.queue_family_index = data.queue_family_index;
  for (uint32_t i = 0; i < data.instance_extensions_count; ++i)
    vk.instance_extensions.emplace_back(data.instance_extensions[i]);
  for (uint32_t i = 0; i < data.device_extensions_count; ++i)
    vk.device_extensions.emplace_back(data.device_extensions[i]);
  auto context = flutter::CreateRustVulkanContext(std::move(vk));
  if (!context)
    return nullptr;
  auto runtime = std::make_unique<flutter::Runtime>();
  runtime->context =
      std::make_shared<flutter::BorrowedAndroidContext>(std::move(context));
  flutter::g_active_rust_runtimes.fetch_add(1, std::memory_order_release);
  return runtime.release();
}
extern "C" void FlutterRustAndroidDestroy(void* runtime) {
  delete static_cast<flutter::Runtime*>(runtime);
  flutter::g_active_rust_runtimes.fetch_sub(1, std::memory_order_release);
}
extern "C" void FlutterRustAndroidPost(void* runtime,
                                       void (*task)(void*),
                                       void* data) {
  // Registration starts only after nativeAttachRust supplied these runners.
  flutter::Get(runtime).platform->PostTask([task, data] { task(data); });
}
extern "C" int64_t FlutterRustAndroidRegisterTexture(
    void* runtime,
    FlutterRustExternalTextureCallbacks callbacks) {
  auto& state = flutter::Get(runtime);
  if (!state.view)
    return -1;
  const auto id = state.next_id++;
  state.textures.insert(id);
  state.view->RegisterTexture(
      std::make_shared<flutter::RustExternalTexture>(id, callbacks));
  return id;
}
extern "C" void FlutterRustAndroidMarkTexture(void* runtime, int64_t id) {
  auto& state = flutter::Get(runtime);
  if (state.view && state.textures.contains(id))
    state.view->MarkTextureFrameAvailable(id);
}
extern "C" void FlutterRustAndroidUnregisterTexture(void* runtime,
                                                    int64_t id,
                                                    void (*complete)(void*),
                                                    void* data) {
  auto& state = flutter::Get(runtime);
  if (state.view && state.textures.erase(id))
    state.view->UnregisterTexture(id);
  state.raster->PostTask([complete, data] { complete(data); });
}
extern "C" void FlutterRustAndroidStop(void* runtime,
                                       void (*complete)(void*),
                                       void* data) {
  auto& state = flutter::Get(runtime);
  if (!state.raster) {
    complete(data);
    return;
  }
  if (state.view)
    for (auto id : state.textures)
      state.view->UnregisterTexture(id);
  state.textures.clear();
  fml::AutoResetWaitableEvent done;
  fml::TaskRunner::RunNowOrPostTask(state.raster, [&done, complete, data] {
    complete(data);
    done.Signal();
  });
  done.Wait();
  state.view = {};
}
extern "C" void FlutterRustAndroidLockQueue(void* runtime) {
  impeller::ContextVK::Cast(
      *flutter::Get(runtime).context->GetImpellerContext())
      .GetGraphicsQueue()
      ->LockExternal();
}
extern "C" void FlutterRustAndroidUnlockQueue(void* runtime) {
  impeller::ContextVK::Cast(
      *flutter::Get(runtime).context->GetImpellerContext())
      .GetGraphicsQueue()
      ->UnlockExternal();
}
