// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/platform_view_rust.h"
#include "flutter/shell/platform/rust/rust_bridge.h"

#include <memory>
#include <string>
#include <utility>

#include "flutter/common/settings.h"
#include "flutter/testing/testing.h"
#include "gtest/gtest.h"

namespace flutter {
namespace testing {
namespace {

class NoopDelegate final : public PlatformView::Delegate {
 public:
  void OnPlatformViewCreated(std::unique_ptr<Surface>) override {}
  void OnPlatformViewDestroyed() override {}
  void OnPlatformViewScheduleFrame() override {}
  void OnPlatformViewAddView(int64_t,
                             const ViewportMetrics&,
                             AddViewCallback) override {}
  void OnPlatformViewRemoveView(int64_t, RemoveViewCallback) override {}
  void OnPlatformViewSendViewFocusEvent(const ViewFocusEvent&) override {}
  void OnPlatformViewSetNextFrameCallback(const fml::closure&) override {}
  void OnPlatformViewSetViewportMetrics(int64_t,
                                        const ViewportMetrics&) override {}
  void OnPlatformViewDispatchPlatformMessage(
      std::unique_ptr<PlatformMessage>) override {}
  void OnPlatformViewDispatchPointerDataPacket(
      std::unique_ptr<PointerDataPacket>) override {}
  HitTestResponse OnPlatformViewHitTest(int64_t,
                                        const flutter::PointData) override {
    return {};
  }
  void OnPlatformViewDispatchSemanticsAction(int64_t,
                                             int32_t,
                                             SemanticsAction,
                                             fml::MallocMapping) override {}
  void OnPlatformViewSetSemanticsEnabled(bool) override {}
  void OnPlatformViewSetAccessibilityFeatures(int32_t) override {}
  void OnPlatformViewRegisterTexture(std::shared_ptr<Texture>) override {}
  void OnPlatformViewUnregisterTexture(int64_t) override {}
  void OnPlatformViewMarkTextureFrameAvailable(int64_t) override {}
  void LoadDartDeferredLibrary(intptr_t,
                               std::unique_ptr<const fml::Mapping>,
                               std::unique_ptr<const fml::Mapping>) override {}
  void LoadDartDeferredLibraryError(intptr_t,
                                    const std::string,
                                    bool) override {}
  void UpdateAssetResolverByType(std::unique_ptr<AssetResolver>,
                                 AssetResolver::AssetResolverType) override {}
  const Settings& OnPlatformViewGetSettings() const override {
    return settings_;
  }

 private:
  Settings settings_;
};

class TestResponse final : public PlatformMessageResponse {
 public:
  void Complete(std::unique_ptr<fml::Mapping>) override {
    completed_with_data_ = true;
  }

  void CompleteEmpty() override { completed_empty_ = true; }

  bool completed_with_data_ = false;
  bool completed_empty_ = false;
};

TaskRunners MakeTaskRunners() {
  return TaskRunners("PlatformViewRustTest", nullptr, nullptr, nullptr,
                     nullptr);
}

TEST(PlatformViewRustTest, ForwardsPlatformMessagesToRustCallback) {
  NoopDelegate delegate;
  bool received_message = false;
  PlatformViewRust::Configuration configuration;
  configuration.handle_platform_message =
      [&received_message](std::unique_ptr<PlatformMessage> message) {
        received_message = message != nullptr;
      };
  PlatformViewRust platform_view(delegate, MakeTaskRunners(),
                                 std::move(configuration));

  platform_view.HandlePlatformMessage(std::make_unique<PlatformMessage>(
      "test/channel", fml::RefPtr<PlatformMessageResponse>()));

  EXPECT_TRUE(received_message);
}

TEST(PlatformViewRustTest, CompletesMessagesWithoutRustCallback) {
  NoopDelegate delegate;
  auto response = fml::MakeRefCounted<TestResponse>();
  PlatformViewRust platform_view(delegate, MakeTaskRunners(), {});

  platform_view.HandlePlatformMessage(
      std::make_unique<PlatformMessage>("test/channel", response));

  EXPECT_TRUE(response->completed_empty_);
  EXPECT_FALSE(response->completed_with_data_);
}

TEST(PlatformViewRustTest, ForwardsSemanticsToRustCallback) {
  NoopDelegate delegate;
  int64_t received_view_id = -1;
  PlatformViewRust::Configuration configuration;
  configuration.update_semantics = [&received_view_id](
                                       int64_t view_id, SemanticsNodeUpdates,
                                       CustomAccessibilityActionUpdates) {
    received_view_id = view_id;
  };
  PlatformViewRust platform_view(delegate, MakeTaskRunners(),
                                 std::move(configuration));

  platform_view.UpdateSemantics(42, {}, {});

  EXPECT_EQ(received_view_id, 42);
}

TEST(PlatformViewRustTest, LinksThePrivateRustAbi) {
  const FlutterRustShellAbi abi = FlutterRustShellGetAbi();

  EXPECT_EQ(abi.shell_abi_version, FLUTTER_RUST_SHELL_ABI_VERSION);
  EXPECT_EQ(abi.plugin_sdk_api_version, FLUTTER_RUST_PLUGIN_SDK_API_VERSION);
}

}  // namespace
}  // namespace testing
}  // namespace flutter
