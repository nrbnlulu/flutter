// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_external_texture.h"

#include <vector>

#include "flutter/display_list/dl_builder.h"
#include "flutter/testing/testing.h"
#include "gtest/gtest.h"

namespace flutter {
namespace testing {
namespace {

class TestImage final : public DlImage {
 public:
  explicit TestImage(DlISize size) : size_(size) {}
  Type GetImageType() const override { return Type::kImpeller; }
  bool isTextureBacked() const override { return true; }
  DlColorSpace GetColorSpace() const override { return DlColorSpace::kSRGB; }
  bool isOpaque() const override { return false; }
  bool isUIThreadSafe() const override { return false; }
  DlISize GetSize() const override { return size_; }
  size_t GetApproximateByteSize() const override { return 0u; }

 private:
  DlISize size_;
};

class TestRustExternalTexture final : public RustExternalTexture {
 public:
  explicit TestRustExternalTexture(int64_t id) : RustExternalTexture(id, {}) {}

  int acquire_count = 0;
  std::vector<uint64_t> released_images;
  bool fail_next_acquire = false;

 protected:
  std::optional<ResolvedFrame> ResolveFrame(PaintContext&,
                                            const DlISize& size) override {
    acquire_count++;
    if (fail_next_acquire) {
      fail_next_acquire = false;
      return std::nullopt;
    }
    FlutterRustExternalTextureFrame frame = {};
    frame.image = static_cast<uint64_t>(acquire_count);
    frame.width = static_cast<uint32_t>(size.width);
    frame.height = static_cast<uint32_t>(size.height);
    return ResolvedFrame{frame, sk_make_sp<TestImage>(size)};
  }

  void ReleaseFrame(PaintContext*, ResolvedFrame frame) override {
    released_images.push_back(frame.frame.image);
  }
};

Texture::PaintContext MakePaintContext(DisplayListBuilder& builder) {
  Texture::PaintContext context;
  context.canvas = &builder;
  return context;
}

TEST(RustExternalTextureTest, RotatesOnlyOnNotifiedUnfrozenFrames) {
  auto texture = std::make_shared<TestRustExternalTexture>(41);
  DisplayListBuilder builder;
  auto context = MakePaintContext(builder);
  const DlRect bounds = DlRect::MakeWH(64, 32);
  Texture& registered = *texture;

  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  EXPECT_EQ(texture->acquire_count, 1);

  registered.MarkNewFrameAvailable();
  registered.Paint(context, bounds, true, DlImageSampling::kLinear);
  EXPECT_EQ(texture->acquire_count, 1);

  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  EXPECT_EQ(texture->acquire_count, 2);
  EXPECT_EQ(texture->released_images, std::vector<uint64_t>({1}));

  registered.OnTextureUnregistered();
  EXPECT_EQ(texture->released_images, std::vector<uint64_t>({1, 2}));
}

TEST(RustExternalTextureTest, RetriesFailedAcquireWithoutDroppingLastFrame) {
  auto texture = std::make_shared<TestRustExternalTexture>(42);
  DisplayListBuilder builder;
  auto context = MakePaintContext(builder);
  const DlRect bounds = DlRect::MakeWH(16, 16);
  Texture& registered = *texture;

  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  registered.MarkNewFrameAvailable();
  texture->fail_next_acquire = true;
  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  EXPECT_TRUE(texture->released_images.empty());

  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  EXPECT_EQ(texture->acquire_count, 3);
  EXPECT_EQ(texture->released_images, std::vector<uint64_t>({1}));
  registered.OnTextureUnregistered();
}

TEST(RustExternalTextureTest, ContextLossAndUnregisterAreIdempotent) {
  auto texture = std::make_shared<TestRustExternalTexture>(43);
  DisplayListBuilder builder;
  auto context = MakePaintContext(builder);
  const DlRect bounds = DlRect::MakeWH(8, 8);
  Texture& registered = *texture;

  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  registered.OnGrContextDestroyed();
  registered.OnGrContextDestroyed();
  EXPECT_EQ(texture->released_images, std::vector<uint64_t>({1}));

  registered.OnGrContextCreated();
  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  EXPECT_EQ(texture->acquire_count, 2);
  registered.OnTextureUnregistered();
  registered.OnTextureUnregistered();
  registered.MarkNewFrameAvailable();
  registered.Paint(context, bounds, false, DlImageSampling::kLinear);
  EXPECT_EQ(texture->released_images, std::vector<uint64_t>({1, 2}));
  EXPECT_EQ(texture->acquire_count, 2);
}

}  // namespace
}  // namespace testing
}  // namespace flutter
