// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/cpp/rust_shell.h"

#include <cmath>

#include "flutter/fml/mapping.h"
#include "flutter/shell/common/engine.h"
#include "flutter/shell/common/shell.h"
#include "flutter/shell/platform/rust/cpp/platform_view_rust.h"
#include "flutter/shell/version/version.h"
#include "rapidjson/stringbuffer.h"
#include "rapidjson/writer.h"

namespace flutter {
namespace {
using Writer = rapidjson::Writer<rapidjson::StringBuffer>;
void String(Writer& w, const char* key, const std::string& value) {
  w.Key(key);
  w.String(value.data(), value.size());
}
void Integer(Writer& w, const char* key, int64_t value) {
  w.Key(key);
  w.Int64(value);
}
void Boolean(Writer& w, const char* key, bool value) {
  w.Key(key);
  w.Bool(value);
}
void Double(Writer& w, const char* key, double value) {
  w.Key(key);
  if (std::isfinite(value)) {
    w.Double(value);
  } else {
    w.Null();
  }
}
void Matrix(Writer& w, const char* key, const SkM44& value) {
  w.Key(key);
  w.StartArray();
  for (int row = 0; row < 4; row++) {
    for (int col = 0; col < 4; col++) {
      auto component = value.rc(row, col);
      if (std::isfinite(component)) {
        w.Double(component);
      } else {
        w.Null();
      }
    }
  }
  w.EndArray();
}
void Integers(Writer& w, const char* key, const std::vector<int32_t>& values) {
  w.Key(key);
  w.StartArray();
  for (auto value : values) {
    w.Int(value);
  }
  w.EndArray();
}
void Node(Writer& w, const SemanticsNode& n) {
  w.StartObject();
  Integer(w, "id", n.id);
  Integer(w, "actions", n.actions);
  Integer(w, "role", static_cast<int32_t>(n.role));
  Integer(w, "validationResult", static_cast<int32_t>(n.validationResult));
  Integer(w, "textDirection", n.textDirection);
  Integer(w, "textSelectionBase", n.textSelectionBase);
  Integer(w, "textSelectionExtent", n.textSelectionExtent);
  Integer(w, "maxValueLength", n.maxValueLength);
  Integer(w, "currentValueLength", n.currentValueLength);
  Integer(w, "platformViewId", n.platformViewId);
  Integer(w, "traversalParent", n.traversalParent);
  Integer(w, "scrollChildren", n.scrollChildren);
  Integer(w, "scrollIndex", n.scrollIndex);
  Integer(w, "headingLevel", n.headingLevel);
  Double(w, "scrollPosition", n.scrollPosition);
  Double(w, "scrollExtentMax", n.scrollExtentMax);
  Double(w, "scrollExtentMin", n.scrollExtentMin);
  String(w, "identifier", n.identifier);
  String(w, "label", n.label);
  String(w, "value", n.value);
  String(w, "hint", n.hint);
  String(w, "tooltip", n.tooltip);
  String(w, "minValue", n.minValue);
  String(w, "maxValue", n.maxValue);
  String(w, "increasedValue", n.increasedValue);
  String(w, "decreasedValue", n.decreasedValue);
  String(w, "linkUrl", n.linkUrl);
  String(w, "locale", n.locale);
  w.Key("rect");
  w.StartArray();
  for (auto value :
       {n.rect.left(), n.rect.top(), n.rect.right(), n.rect.bottom()}) {
    if (std::isfinite(value)) {
      w.Double(value);
    } else {
      w.Null();
    }
  }
  w.EndArray();
  Matrix(w, "transform", n.transform);
  Matrix(w, "hitTestTransform", n.hitTestTransform);
  Integers(w, "children", n.childrenInTraversalOrder);
  Integers(w, "hitTestChildren", n.childrenInHitTestOrder);
  Integers(w, "customActions", n.customAccessibilityActions);
  w.Key("flags");
  w.StartObject();
#define TRISTATE(name) Integer(w, #name, static_cast<int32_t>(n.flags.name))
  TRISTATE(isChecked);
  TRISTATE(isSelected);
  TRISTATE(isEnabled);
  TRISTATE(isToggled);
  TRISTATE(isExpanded);
  TRISTATE(isRequired);
  TRISTATE(isFocused);
#undef TRISTATE
#define FLAG(name) Boolean(w, #name, n.flags.name)
  FLAG(isButton);
  FLAG(isTextField);
  FLAG(isInMutuallyExclusiveGroup);
  FLAG(isHeader);
  FLAG(isObscured);
  FLAG(scopesRoute);
  FLAG(namesRoute);
  FLAG(isHidden);
  FLAG(isImage);
  FLAG(isLiveRegion);
  FLAG(hasImplicitScrolling);
  FLAG(isMultiline);
  FLAG(isReadOnly);
  FLAG(isLink);
  FLAG(isSlider);
  FLAG(isKeyboardKey);
  FLAG(isAccessibilityFocusBlocked);
#undef FLAG
  w.EndObject();
  w.EndObject();
}
}  // namespace

void RustShell::SetSemanticsCallbacks(FlutterRustSemanticsCallbacks callbacks) {
  if (auto view = shell_->GetPlatformView()) {
    static_cast<PlatformViewRust*>(view.get())
        ->SetSemanticsUpdateCallback(
            [callbacks](int64_t view_id, SemanticsNodeUpdates nodes,
                        CustomAccessibilityActionUpdates actions) {
              if (!callbacks.update) {
                return;
              }
              rapidjson::StringBuffer buffer;
              Writer w(buffer);
              w.StartObject();
              w.Key("nodes");
              w.StartArray();
              for (const auto& [id, node] : nodes) {
                Node(w, node);
              }
              w.EndArray();
              w.Key("customActions");
              w.StartArray();
              for (const auto& [id, action] : actions) {
                w.StartObject();
                Integer(w, "id", id);
                Integer(w, "overrideId", action.overrideId);
                String(w, "label", action.label);
                String(w, "hint", action.hint);
                w.EndObject();
              }
              w.EndArray();
              w.EndObject();
              callbacks.update(
                  callbacks.user_data, view_id,
                  reinterpret_cast<const uint8_t*>(buffer.GetString()),
                  buffer.GetSize());
            });
  }
}
void RustShell::SetSemanticsEnabled(bool enabled) {
  if (auto view = shell_->GetPlatformView()) {
    view->SetSemanticsEnabled(enabled);
  }
}
bool RustShell::PerformSemanticsAction(int64_t view_id,
                                       int32_t node_id,
                                       int32_t action,
                                       const uint8_t* args,
                                       uint64_t length) {
  if (length > 1024 * 1024 || (length != 0 && !args) || action <= 0 ||
      (action & (action - 1)) != 0) {
    return false;
  }
  if (auto view = shell_->GetPlatformView()) {
    view->DispatchSemanticsAction(view_id, node_id,
                                  static_cast<SemanticsAction>(action),
                                  fml::MallocMapping::Copy(args, length));
    return true;
  }
  return false;
}
uint64_t RustShell::RequestObservationFrame() {
  if (auto engine = shell_->GetEngine()) {
    auto boundary = engine->GetUIFrameNumber();
    engine->MarkAllViewsNeedRender();
    engine->ScheduleFrame(true);
    return boundary;
  }
  return 0;
}
}  // namespace flutter

extern "C" void FlutterRustShellSetSemanticsCallbacks(
    void* shell,
    FlutterRustSemanticsCallbacks callbacks) {
  if (shell) {
    static_cast<flutter::RustShell*>(shell)->SetSemanticsCallbacks(callbacks);
  }
}
extern "C" void FlutterRustShellSetSemanticsEnabled(void* shell,
                                                    int32_t enabled) {
  if (shell) {
    static_cast<flutter::RustShell*>(shell)->SetSemanticsEnabled(enabled != 0);
  }
}
extern "C" int FlutterRustShellPerformSemanticsAction(void* shell,
                                                      int64_t view_id,
                                                      int32_t node_id,
                                                      int32_t action,
                                                      const uint8_t* args,
                                                      uint64_t length) {
  return shell &&
         static_cast<flutter::RustShell*>(shell)->PerformSemanticsAction(
             view_id, node_id, action, args, length);
}
extern "C" uint64_t FlutterRustShellRequestObservationFrame(void* shell) {
  return shell ? static_cast<flutter::RustShell*>(shell)
                     ->RequestObservationFrame()
               : 0;
}

extern "C" const char* FlutterRustShellGetEngineRevision(void) {
  return flutter::GetFlutterEngineVersion();
}
