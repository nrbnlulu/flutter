// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_EXTERNAL_TEXTURE_H_
#define FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_EXTERNAL_TEXTURE_H_

#include <optional>

#include "flutter/common/graphics/texture.h"
#include "flutter/shell/platform/rust/cpp/rust_bridge.h"

namespace impeller {
class Context;
}

namespace flutter {

// Adapts Rust-owned, wgpu-produced Vulkan images to Flutter's existing
// Texture registry. All methods except construction are called on the raster
// thread. Rust retains every Vulkan object's ownership.
class RustExternalTexture : public Texture {
 public:
  RustExternalTexture(int64_t id,
                      FlutterRustExternalTextureCallbacks callbacks);
  ~RustExternalTexture() override;

 protected:
  struct ResolvedFrame {
    FlutterRustExternalTextureFrame frame = {};
    sk_sp<DlImage> image;
  };

  // Virtual only to make the frame lifecycle independently testable without a
  // Vulkan device. Production instances use the implementations below.
  virtual std::optional<ResolvedFrame> ResolveFrame(PaintContext& context,
                                                    const DlISize& size);
  virtual void ReleaseFrame(PaintContext* context, ResolvedFrame frame);

 private:
  void Paint(PaintContext& context,
             const DlRect& bounds,
             bool freeze,
             const DlImageSampling sampling) override;
  void MarkNewFrameAvailable() override;
  void OnTextureUnregistered() override;
  void OnGrContextCreated() override;
  void OnGrContextDestroyed() override;

  void ReleaseCurrentFrame(PaintContext* context);

  FlutterRustExternalTextureCallbacks callbacks_;
  std::optional<ResolvedFrame> current_frame_;
  bool new_frame_available_ = true;
  bool unregistered_ = false;
  std::weak_ptr<impeller::Context> context_;

  FML_DISALLOW_COPY_AND_ASSIGN(RustExternalTexture);
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_EXTERNAL_TEXTURE_H_
