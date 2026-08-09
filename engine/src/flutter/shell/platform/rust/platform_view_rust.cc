// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/platform_view_rust.h"

#include <algorithm>
#include <string>
#include <utility>

#include "flutter/common/constants.h"
#include "flutter/fml/logging.h"
#include "flutter/lib/ui/window/pointer_data.h"
#include "flutter/lib/ui/window/pointer_data_packet.h"

namespace flutter {

std::unique_ptr<PointerDataPacket> CreateRustPointerDataPacket(
    const FlutterRustPointerEvent& event) {
  PointerData data;
  data.Clear();
  data.embedder_id = 0;
  data.time_stamp = static_cast<int64_t>(event.timestamp_micros);
  switch (event.phase) {
    case kFlutterRustPointerPhaseCancel:
      data.change = PointerData::Change::kCancel;
      break;
    case kFlutterRustPointerPhaseAdd:
      data.change = PointerData::Change::kAdd;
      break;
    case kFlutterRustPointerPhaseRemove:
      data.change = PointerData::Change::kRemove;
      break;
    case kFlutterRustPointerPhaseHover:
      data.change = PointerData::Change::kHover;
      break;
    case kFlutterRustPointerPhaseDown:
      data.change = PointerData::Change::kDown;
      break;
    case kFlutterRustPointerPhaseMove:
      data.change = PointerData::Change::kMove;
      break;
    case kFlutterRustPointerPhaseUp:
      data.change = PointerData::Change::kUp;
      break;
    default:
      return nullptr;
  }
  switch (event.device_kind) {
    case kFlutterRustPointerDeviceKindMouse:
      data.kind = PointerData::DeviceKind::kMouse;
      break;
    case kFlutterRustPointerDeviceKindTouch:
      data.kind = PointerData::DeviceKind::kTouch;
      break;
    default:
      return nullptr;
  }
  switch (event.signal_kind) {
    case kFlutterRustPointerSignalKindNone:
      data.signal_kind = PointerData::SignalKind::kNone;
      break;
    case kFlutterRustPointerSignalKindScroll:
      data.signal_kind = PointerData::SignalKind::kScroll;
      break;
    default:
      return nullptr;
  }
  data.device = event.device;
  data.physical_x = event.physical_x;
  data.physical_y = event.physical_y;
  data.scroll_delta_x = event.scroll_delta_x;
  data.scroll_delta_y = event.scroll_delta_y;
  data.buttons = event.buttons;
  data.view_id = kFlutterImplicitViewId;

  auto packet = std::make_unique<PointerDataPacket>(1);
  packet->SetPointerData(0, data);
  return packet;
}

const char* GetRustLifecycleStateName(uint32_t state) {
  switch (state) {
    case kFlutterRustLifecycleStateDetached:
      return "AppLifecycleState.detached";
    case kFlutterRustLifecycleStateResumed:
      return "AppLifecycleState.resumed";
    case kFlutterRustLifecycleStateInactive:
      return "AppLifecycleState.inactive";
    case kFlutterRustLifecycleStateHidden:
      return "AppLifecycleState.hidden";
    case kFlutterRustLifecycleStatePaused:
      return "AppLifecycleState.paused";
    default:
      return nullptr;
  }
}

std::unique_ptr<KeyDataPacket> CreateRustKeyDataPacket(
    const FlutterRustKeyEvent& event) {
  if (event.physical == 0 || event.logical == 0 ||
      event.character_length > FLUTTER_RUST_KEY_CHARACTER_CAPACITY) {
    return nullptr;
  }
  KeyData data;
  data.Clear();
  data.timestamp = event.timestamp_micros;
  switch (event.event_type) {
    case kFlutterRustKeyEventTypeDown:
      data.type = KeyEventType::kDown;
      break;
    case kFlutterRustKeyEventTypeUp:
      data.type = KeyEventType::kUp;
      break;
    case kFlutterRustKeyEventTypeRepeat:
      data.type = KeyEventType::kRepeat;
      break;
    default:
      return nullptr;
  }
  data.physical = event.physical;
  data.logical = event.logical;
  data.synthesized = event.synthesized != 0;
  data.device_type = KeyEventDeviceType::kKeyboard;

  const auto character_length = static_cast<size_t>(event.character_length);
  if (std::find(event.character, event.character + character_length, '\0') !=
      event.character + character_length) {
    return nullptr;
  }
  const std::string character(reinterpret_cast<const char*>(event.character),
                              character_length);
  return std::make_unique<KeyDataPacket>(
      data, character.empty() ? nullptr : character.c_str());
}

PlatformViewRust::PlatformViewRust(Delegate& delegate,
                                   const TaskRunners& task_runners,
                                   Configuration configuration)
    : PlatformView(delegate, task_runners),
      configuration_(std::move(configuration)) {}

PlatformViewRust::~PlatformViewRust() = default;

void PlatformViewRust::HandlePlatformMessage(
    std::unique_ptr<PlatformMessage> message) {
  if (!message) {
    return;
  }

  if (configuration_.handle_platform_message) {
    configuration_.handle_platform_message(std::move(message));
    return;
  }

  if (auto response = message->response()) {
    response->CompleteEmpty();
  }
}

void PlatformViewRust::UpdateSemantics(
    int64_t view_id,
    SemanticsNodeUpdates update,
    CustomAccessibilityActionUpdates actions) {
  if (configuration_.update_semantics) {
    configuration_.update_semantics(view_id, std::move(update),
                                    std::move(actions));
  }
}

std::unique_ptr<Surface> PlatformViewRust::CreateRenderingSurface() {
  if (!configuration_.create_rendering_surface) {
    FML_LOG(ERROR) << "Rust platform view has no rendering surface callback.";
    return nullptr;
  }
  return configuration_.create_rendering_surface();
}

}  // namespace flutter
