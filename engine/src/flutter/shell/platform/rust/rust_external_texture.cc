// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_external_texture.h"

#include <array>

#include "flutter/fml/logging.h"
#include "flutter/impeller/display_list/aiks_context.h"
#include "flutter/impeller/display_list/dl_image_impeller.h"
#include "flutter/impeller/renderer/backend/vulkan/context_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/formats_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/queue_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/texture_source_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/texture_vk.h"

namespace flutter {
namespace {

class BorrowedTextureSourceVK final : public impeller::TextureSourceVK {
 public:
  BorrowedTextureSourceVK(impeller::TextureDescriptor descriptor,
                          impeller::vk::Image image,
                          impeller::vk::ImageView image_view)
      : TextureSourceVK(std::move(descriptor)),
        image_(image),
        image_view_(image_view) {
    SetLayoutWithoutEncoding(impeller::vk::ImageLayout::eShaderReadOnlyOptimal);
  }

  impeller::vk::Image GetImage() const override { return image_; }
  impeller::vk::ImageView GetImageView() const override { return image_view_; }
  impeller::vk::ImageView GetRenderTargetView(uint32_t,
                                              uint32_t) const override {
    return image_view_;
  }
  bool IsSwapchainImage() const override { return false; }

 private:
  impeller::vk::Image image_;
  impeller::vk::ImageView image_view_;
};

bool SubmitSemaphore(impeller::ContextVK& context,
                     uint64_t semaphore_handle,
                     bool wait) {
  impeller::vk::Semaphore semaphore(
      reinterpret_cast<VkSemaphore>(static_cast<uintptr_t>(semaphore_handle)));
  impeller::vk::SubmitInfo submit_info;
  if (wait) {
    const std::array<impeller::vk::Semaphore, 1> semaphores = {semaphore};
    const std::array<impeller::vk::PipelineStageFlags, 1> wait_stages = {
        impeller::vk::PipelineStageFlagBits::eFragmentShader};
    submit_info.setWaitSemaphores(semaphores);
    submit_info.setWaitDstStageMask(wait_stages);
    return context.GetGraphicsQueue()->Submit(submit_info, {}) ==
           impeller::vk::Result::eSuccess;
  }
  submit_info.setSignalSemaphores(semaphore);
  return context.GetGraphicsQueue()->Submit(submit_info, {}) ==
         impeller::vk::Result::eSuccess;
}

}  // namespace

RustExternalTexture::RustExternalTexture(
    int64_t id,
    FlutterRustExternalTextureCallbacks callbacks)
    : Texture(id), callbacks_(callbacks) {}

RustExternalTexture::~RustExternalTexture() {
  ReleaseCurrentFrame(nullptr);
}

std::optional<RustExternalTexture::ResolvedFrame>
RustExternalTexture::ResolveFrame(PaintContext& context, const DlISize& size) {
  if (!context.aiks_context || !callbacks_.acquire_frame || size.width <= 0 ||
      size.height <= 0) {
    return std::nullopt;
  }
  FlutterRustExternalTextureFrame frame = {};
  if (!callbacks_.acquire_frame(callbacks_.user_data, size.width, size.height,
                                &frame)) {
    return std::nullopt;
  }
  if (frame.image == 0u || frame.image_view == 0u || frame.width == 0u ||
      frame.height == 0u || frame.acquire_semaphore == 0u ||
      frame.render_semaphore == 0u) {
    callbacks_.release_frame(callbacks_.user_data, frame);
    return std::nullopt;
  }

  auto context_ptr = context.aiks_context->GetContext();
  context_ = context_ptr;
  auto& context_vk = impeller::ContextVK::Cast(*context_ptr);
  if (!SubmitSemaphore(context_vk, frame.acquire_semaphore, true)) {
    callbacks_.release_frame(callbacks_.user_data, frame);
    return std::nullopt;
  }

  const auto format =
      impeller::ToPixelFormat(static_cast<impeller::vk::Format>(frame.format));
  if (format == impeller::PixelFormat::kUnknown) {
    ReleaseFrame(&context, ResolvedFrame{frame, nullptr});
    return std::nullopt;
  }
  impeller::TextureDescriptor descriptor;
  descriptor.format = format;
  descriptor.size = {static_cast<int64_t>(frame.width),
                     static_cast<int64_t>(frame.height)};
  auto source = std::make_shared<BorrowedTextureSourceVK>(
      descriptor,
      impeller::vk::Image(
          reinterpret_cast<VkImage>(static_cast<uintptr_t>(frame.image))),
      impeller::vk::ImageView(reinterpret_cast<VkImageView>(
          static_cast<uintptr_t>(frame.image_view))));
  auto texture = std::make_shared<impeller::TextureVK>(context_ptr, source);
  auto image = impeller::DlImageImpeller::Make(std::move(texture));
  if (!image) {
    ReleaseFrame(&context, ResolvedFrame{frame, nullptr});
    return std::nullopt;
  }
  return ResolvedFrame{frame, std::move(image)};
}

void RustExternalTexture::ReleaseFrame(PaintContext* context,
                                       ResolvedFrame frame) {
  auto context_ptr = context && context->aiks_context
                         ? context->aiks_context->GetContext()
                         : context_.lock();
  if (context_ptr && frame.frame.render_semaphore != 0u) {
    auto& context_vk = impeller::ContextVK::Cast(*context_ptr);
    if (!SubmitSemaphore(context_vk, frame.frame.render_semaphore, false)) {
      FML_LOG(ERROR) << "Could not signal Rust external texture release";
    }
  }
  if (callbacks_.release_frame) {
    callbacks_.release_frame(callbacks_.user_data, frame.frame);
  }
}

void RustExternalTexture::Paint(PaintContext& context,
                                const DlRect& bounds,
                                bool freeze,
                                const DlImageSampling sampling) {
  if (unregistered_) {
    return;
  }
  if (!freeze && new_frame_available_) {
    auto next =
        ResolveFrame(context, {static_cast<int32_t>(bounds.GetWidth()),
                               static_cast<int32_t>(bounds.GetHeight())});
    if (next) {
      ReleaseCurrentFrame(&context);
      current_frame_ = std::move(next);
      new_frame_available_ = false;
    }
  }
  if (current_frame_ && current_frame_->image) {
    context.canvas->DrawImageRect(
        current_frame_->image, DlRect::Make(current_frame_->image->GetBounds()),
        bounds, sampling, context.paint, DlSrcRectConstraint::kStrict);
  }
}

void RustExternalTexture::MarkNewFrameAvailable() {
  if (!unregistered_) {
    new_frame_available_ = true;
  }
}

void RustExternalTexture::OnTextureUnregistered() {
  unregistered_ = true;
  ReleaseCurrentFrame(nullptr);
}

void RustExternalTexture::OnGrContextCreated() {
  if (!unregistered_) {
    new_frame_available_ = true;
  }
}

void RustExternalTexture::OnGrContextDestroyed() {
  ReleaseCurrentFrame(nullptr);
  if (!unregistered_) {
    new_frame_available_ = true;
  }
}

void RustExternalTexture::ReleaseCurrentFrame(PaintContext* context) {
  if (!current_frame_) {
    return;
  }
  auto frame = std::move(*current_frame_);
  current_frame_.reset();
  ReleaseFrame(context, std::move(frame));
}

}  // namespace flutter
