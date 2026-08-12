// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_PLATFORM_VIEW_RUST_H_
#define FLUTTER_SHELL_PLATFORM_RUST_PLATFORM_VIEW_RUST_H_

#include <functional>
#include <memory>

#include "flutter/shell/common/platform_view.h"
#include "flutter/shell/platform/rust/cpp/rust_bridge.h"

namespace flutter {

class RustVsyncState;

// Converts one validated private-ABI event into an engine packet. Returns null
// for enum values that this engine revision does not understand.
std::unique_ptr<PointerDataPacket> CreateRustPointerDataPacket(
    const FlutterRustPointerEvent& event);

// Returns the framework-facing lifecycle string for a private-ABI value, or
// null when the value is not understood by this engine revision.
const char* GetRustLifecycleStateName(uint32_t state);

// Converts one validated private-ABI keyboard event into Flutter's key-data
// packet. Returns null for invalid types, identifiers, or character data.
std::unique_ptr<KeyDataPacket> CreateRustKeyDataPacket(
    const FlutterRustKeyEvent& event);

// The in-tree platform view used by the optional Rust shell.
//
// This class is deliberately a narrow C++ adapter. The Rust runtime owns
// windowing and platform services; future Rust FFI supplies the callbacks in
// Configuration. Keeping C++ inheritance on this side of the boundary avoids
// exposing Flutter's C++ ABI to Rust.
class PlatformViewRust final : public PlatformView {
 public:
  using CreateRenderingSurfaceCallback =
      std::function<std::unique_ptr<Surface>()>;
  using HandlePlatformMessageCallback =
      std::function<void(std::unique_ptr<PlatformMessage>)>;
  using UpdateSemanticsCallback = std::function<
      void(int64_t, SemanticsNodeUpdates, CustomAccessibilityActionUpdates)>;

  struct Configuration {
    // Invoked on Flutter's raster task runner.
    CreateRenderingSurfaceCallback create_rendering_surface;

    // Invoked on the merged Flutter UI/platform task runner.
    HandlePlatformMessageCallback handle_platform_message;

    // Invoked when Flutter publishes a semantics update.
    UpdateSemanticsCallback update_semantics;

    // Requests a compositor-aligned pulse from the Rust window host. When it
    // is absent CreateVSyncWaiter retains Flutter's timer fallback.
    FlutterRustVsyncCallbacks vsync_callbacks = {};
  };

  PlatformViewRust(Delegate& delegate,
                   const TaskRunners& task_runners,
                   Configuration configuration);

  ~PlatformViewRust() override;

  // |PlatformView|
  void HandlePlatformMessage(std::unique_ptr<PlatformMessage> message) override;

  // |PlatformView|
  void UpdateSemantics(int64_t view_id,
                       SemanticsNodeUpdates update,
                       CustomAccessibilityActionUpdates actions) override;

  // Delivers a compositor pulse requested by this platform view.
  void OnVsync(uint64_t frame_interval_nanos);

 private:
  // |PlatformView|
  std::unique_ptr<Surface> CreateRenderingSurface() override;

  // |PlatformView|
  std::unique_ptr<VsyncWaiter> CreateVSyncWaiter() override;

  Configuration configuration_;
  std::shared_ptr<RustVsyncState> vsync_state_;

  FML_DISALLOW_COPY_AND_ASSIGN(PlatformViewRust);
};

}  // namespace flutter

#endif  // FLUTTER_SHELL_PLATFORM_RUST_PLATFORM_VIEW_RUST_H_
