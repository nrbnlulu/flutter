// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_ANDROID_H_
#define FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_ANDROID_H_

#include "flutter/shell/platform/rust/cpp/rust_bridge.h"

#ifdef __cplusplus
#include <memory>
namespace flutter {
class AndroidContext;
class AndroidShellHolder;
std::shared_ptr<AndroidContext> RustAndroidContext(void* runtime);
bool HasRustAndroidRuntime();
void AttachRustAndroidRuntime(void* runtime, AndroidShellHolder& holder);
}  // namespace flutter
extern "C" {
#endif

// Private, lockstep Android embedding bridge. The caller retains the wgpu
// device until after FlutterEngine.destroy() and runtime destruction.
FLUTTER_RUST_SHELL_EXPORT void* FlutterRustAndroidCreate(
    FlutterRustVulkanContextData context);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidDestroy(void* runtime);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidPost(void* runtime,
                                                      void (*task)(void*),
                                                      void* data);
FLUTTER_RUST_SHELL_EXPORT int64_t FlutterRustAndroidRegisterTexture(
    void* runtime,
    FlutterRustExternalTextureCallbacks callbacks);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidMarkTexture(void* runtime,
                                                             int64_t id);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidUnregisterTexture(
    void* runtime,
    int64_t id,
    void (*complete)(void*),
    void* data);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidStop(void* runtime,
                                                      void (*complete)(void*),
                                                      void* data);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidLockQueue(void* runtime);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustAndroidUnlockQueue(void* runtime);
#ifdef __cplusplus
}
#endif
#endif  // FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_ANDROID_H_
