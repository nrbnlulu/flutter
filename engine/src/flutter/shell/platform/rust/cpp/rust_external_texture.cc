// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/cpp/rust_external_texture.h"

#include <array>

#include "flutter/fml/build_config.h"
#include "flutter/fml/logging.h"
#include "flutter/impeller/display_list/aiks_context.h"
#include "flutter/impeller/display_list/dl_image_impeller.h"
#include "flutter/impeller/renderer/backend/vulkan/context_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/formats_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/queue_vk.h"
#if defined(FML_OS_ANDROID)
#include "flutter/impeller/renderer/backend/vulkan/android/ahb_texture_source_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/command_buffer_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/surface_context_vk.h"
#include "flutter/impeller/toolkit/android/hardware_buffer.h"
#endif
#include "flutter/impeller/renderer/backend/vulkan/texture_source_vk.h"
#include "flutter/impeller/renderer/backend/vulkan/texture_vk.h"

namespace flutter {
namespace {

std::shared_ptr<impeller::Context> TextureDeviceContext(
    const std::shared_ptr<impeller::Context>& context) {
#if defined(FML_OS_ANDROID)
  // Android paints through a SurfaceContextVK. Vulkan queue and texture
  // operations belong to its parent ContextVK.
  return impeller::SurfaceContextVK::Cast(*context).GetParent();
#else
  return context;
#endif
}

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

#if defined(FML_OS_ANDROID)
// The frame is already acquire-fence-ready. Explicitly acquire/release the
// foreign image ownership on the engine's shared, lock-serialized queue.
bool TransferHardwareBuffer(
    const std::shared_ptr<impeller::ContextVK>& context,
    const std::shared_ptr<impeller::AHBTextureSourceVK>& source,
    bool acquire,
    const std::shared_ptr<void>& lease) {
  auto command = context->CreateCommandBuffer();
  if (!command) {
    return false;
  }
  auto& command_vk = impeller::CommandBufferVK::Cast(*command);
  auto family = context->GetGraphicsQueue()->GetIndex().family;
  impeller::vk::ImageMemoryBarrier barrier;
  barrier.setImage(source->GetImage());
  barrier.setSubresourceRange(
      {impeller::vk::ImageAspectFlagBits::eColor, 0, 1, 0, 1});
  barrier.setSrcQueueFamilyIndex(acquire ? VK_QUEUE_FAMILY_FOREIGN_EXT
                                         : family);
  barrier.setDstQueueFamilyIndex(acquire ? family
                                         : VK_QUEUE_FAMILY_FOREIGN_EXT);
  barrier.setOldLayout(acquire
                           ? impeller::vk::ImageLayout::eUndefined
                           : impeller::vk::ImageLayout::eShaderReadOnlyOptimal);
  barrier.setNewLayout(acquire
                           ? impeller::vk::ImageLayout::eShaderReadOnlyOptimal
                           : impeller::vk::ImageLayout::eGeneral);
  barrier.setSrcAccessMask(acquire ? impeller::vk::AccessFlags{}
                                   : impeller::vk::AccessFlagBits::eShaderRead);
  barrier.setDstAccessMask(acquire ? impeller::vk::AccessFlagBits::eShaderRead
                                   : impeller::vk::AccessFlags{});
  command_vk.GetCommandBuffer().pipelineBarrier(
      acquire ? impeller::vk::PipelineStageFlagBits::eTopOfPipe
              : impeller::vk::PipelineStageFlagBits::eFragmentShader,
      acquire ? impeller::vk::PipelineStageFlagBits::eFragmentShader
              : impeller::vk::PipelineStageFlagBits::eBottomOfPipe,
      {}, {}, {}, barrier);
  // Retain the image and its decoder lease through the transfer fence too.
  return context->GetCommandQueue()
      ->Submit({command}, [source, lease](impeller::CommandBuffer::Status) {})
      .ok();
}

sk_sp<DlImage> ImportHardwareBuffer(
    const std::shared_ptr<impeller::ContextVK>& context,
    FlutterRustExternalTextureCallbacks callbacks,
    FlutterRustExternalTextureFrame frame) {
  // Frame-token release deliberately does not use the texture's user_data:
  // unregister can finish while submitted display lists still retain images.
  auto released_frame =
      std::make_shared<FlutterRustExternalTextureFrame>(frame);
  released_frame->format =
      UINT32_MAX;  // Failed imports trigger producer fallback.
  auto lease = std::shared_ptr<void>(
      reinterpret_cast<void*>(static_cast<uintptr_t>(frame.frame_token)),
      [callbacks, released_frame](void*) {
        callbacks.release_frame(nullptr, *released_frame);
      });
  auto* buffer = reinterpret_cast<AHardwareBuffer*>(
      static_cast<uintptr_t>(frame.hardware_buffer));
  auto desc = impeller::android::HardwareBuffer::Describe(buffer);
  if (!desc || desc->layers != 1 ||
      !(desc->usage & AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE) ||
      (desc->usage & AHARDWAREBUFFER_USAGE_PROTECTED_CONTENT) ||
      frame.crop[2] == 0 || frame.crop[3] == 0 ||
      uint64_t(frame.crop[0]) + frame.crop[2] > desc->width ||
      uint64_t(frame.crop[1]) + frame.crop[3] > desc->height) {
    return nullptr;
  }
  auto* imported = new impeller::AHBTextureSourceVK(context, buffer, *desc);
  if (!imported->IsValid()) {
    delete imported;
    return nullptr;
  }
  auto acquired = std::make_shared<bool>(false);
  auto source = std::shared_ptr<impeller::AHBTextureSourceVK>(
      imported, [weak = std::weak_ptr(context), lease, acquired](auto* image) {
        // Impeller tracks TextureVK through every GPU draw. This deleter runs
        // only after its last reader has completed, including cached frames.
        auto retiring = std::shared_ptr<impeller::AHBTextureSourceVK>(image);
        auto context = weak.lock();
        if (*acquired && context &&
            !TransferHardwareBuffer(context, retiring, false, lease)) {
          FML_LOG(ERROR) << "Could not release Android decoder image ownership";
        }
      });
  if (!TransferHardwareBuffer(context, source, true, lease)) {
    return nullptr;
  }
  *acquired = true;
  source->SetLayoutWithoutEncoding(
      impeller::vk::ImageLayout::eShaderReadOnlyOptimal);
  auto image = impeller::DlImageImpeller::Make(
      std::make_shared<impeller::TextureVK>(context, std::move(source)));
  if (image) {
    released_frame->format = 0;
  }
  return image;
}
#endif

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
#if defined(FML_OS_ANDROID)
  if (frame.hardware_buffer != 0u && frame.frame_token != 0u &&
      callbacks_.release_frame) {
    auto context_ptr = TextureDeviceContext(context.aiks_context->GetContext());
    context_ = context_ptr;
    auto image = ImportHardwareBuffer(
        std::static_pointer_cast<impeller::ContextVK>(context_ptr), callbacks_,
        frame);
    if (!image) {
      return std::nullopt;
    }
    if (!logged_hardware_buffer_) {
      FML_LOG(INFO) << "Rust Android hardware buffer imported: "
                    << frame.width << "x" << frame.height;
      logged_hardware_buffer_ = true;
    }
    return ResolvedFrame{frame, std::move(image),
                         DlRect::MakeXYWH(frame.crop[0], frame.crop[1],
                                          frame.crop[2], frame.crop[3])};
  }
#endif
  if (frame.image == 0u || frame.image_view == 0u || frame.width == 0u ||
      frame.height == 0u || frame.acquire_semaphore == 0u ||
      frame.render_semaphore == 0u) {
    callbacks_.release_frame(callbacks_.user_data, frame);
    return std::nullopt;
  }

  auto context_ptr = TextureDeviceContext(context.aiks_context->GetContext());
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
  if (frame.frame.hardware_buffer != 0u) {
    // Dropping the image retires its producer lease after GPU readers finish.
    return;
  }
  auto context_ptr =
      context && context->aiks_context
          ? TextureDeviceContext(context->aiks_context->GetContext())
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
        current_frame_->image,
        current_frame_->source_rect.value_or(
            DlRect::Make(current_frame_->image->GetBounds())),
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
