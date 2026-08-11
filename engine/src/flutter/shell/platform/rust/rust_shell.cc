// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "flutter/shell/platform/rust/rust_shell.h"

#include <vector>

#include "flutter/common/constants.h"
#include "flutter/common/task_runners.h"
#include "flutter/fml/command_line.h"
#include "flutter/fml/mapping.h"
#include "flutter/fml/memory/ref_ptr.h"
#include "flutter/lib/ui/window/platform_message.h"
#include "flutter/lib/ui/window/view_focus.h"
#include "flutter/lib/ui/window/viewport_metrics.h"
#include "flutter/runtime/dart_vm.h"
#include "flutter/runtime/platform_data.h"
#include "flutter/shell/common/display.h"
#include "flutter/shell/common/run_configuration.h"
#include "flutter/shell/common/shell.h"
#include "flutter/shell/common/switches.h"
#include "flutter/shell/common/thread_host.h"
#include "flutter/shell/platform/common/engine_switches.h"
#include "flutter/shell/platform/rust/platform_view_rust.h"
#include "flutter/shell/platform/rust/rust_task_runner.h"
#include "flutter/shell/platform/rust/rust_vulkan_surface.h"

struct FlutterRustPlatformMessageResponseHandle {
  fml::RefPtr<flutter::PlatformMessageResponse> response;
};

namespace flutter {

namespace {

class RustPlatformMessageResponse final : public PlatformMessageResponse {
 public:
  RustPlatformMessageResponse(
      FlutterRustPlatformMessageResponseCallback callback,
      void* user_data)
      : callback_(callback), user_data_(user_data) {}

  void Complete(std::unique_ptr<fml::Mapping> data) override {
    if (!callback_) {
      return;
    }
    callback_(user_data_, data ? data->GetMapping() : nullptr,
              data ? data->GetSize() : 0);
  }

  void CompleteEmpty() override {
    if (callback_) {
      callback_(user_data_, nullptr, 0);
    }
  }

 private:
  ~RustPlatformMessageResponse() override = default;

  FlutterRustPlatformMessageResponseCallback callback_;
  void* user_data_;

  FML_FRIEND_MAKE_REF_COUNTED(RustPlatformMessageResponse);
  FML_DISALLOW_COPY_AND_ASSIGN(RustPlatformMessageResponse);
};

ViewportMetrics ToViewportMetrics(const FlutterRustViewMetrics& metrics) {
  ViewportMetrics result(metrics.pixel_ratio, metrics.width, metrics.height,
                         /*p_physical_touch_slop=*/-1.0, /*display_id=*/0);
  result.physical_min_width_constraint = metrics.min_width;
  result.physical_max_width_constraint = metrics.max_width;
  result.physical_min_height_constraint = metrics.min_height;
  result.physical_max_height_constraint = metrics.max_height;
  return result;
}

void CompleteViewOperation(FlutterRustViewOperationCallbacks callbacks,
                           FlutterRustViewId view_id,
                           bool success) {
  if (callbacks.complete) {
    callbacks.complete(callbacks.user_data, view_id, success ? 1 : 0);
  }
}

}  // namespace

std::unique_ptr<RustShell> RustShell::Create(
    fml::RefPtr<fml::TaskRunner> main_task_runner,
    RustVulkanContextData context_data,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustPlatformMessageCallbacks platform_message_callbacks,
    FlutterRustVsyncCallbacks vsync_callbacks,
    FlutterRustWindowingCallbacks windowing_callbacks,
    Settings settings) {
  if (!main_task_runner) {
    return nullptr;
  }
  // The winit-owned UI thread never installs an fml::MessageLoop, so
  // UIDartState's task-observer registration (used to flush the root
  // isolate's microtask queue after each task) has nowhere to go by default.
  // main_task_runner is always the RustTaskRunner obtained through
  // RustTaskRunner::FromHandle; route the callbacks to its own observer
  // registry, which RunTask drains after every task it executes.
  auto* rust_task_runner = static_cast<RustTaskRunner*>(main_task_runner.get());
  settings.task_observer_add = [rust_task_runner](
                                   intptr_t key, const fml::closure& callback) {
    return rust_task_runner->AddTaskObserver(key, callback);
  };
  settings.task_observer_remove = [rust_task_runner](fml::TaskQueueId queue_id,
                                                     intptr_t key) {
    rust_task_runner->RemoveTaskObserver(key);
  };
  auto get_instance_proc_addr = reinterpret_cast<PFN_vkGetInstanceProcAddr>(
      context_data.get_instance_proc_addr);
  auto context = CreateRustVulkanContext(std::move(context_data));
  auto presentation = std::make_shared<RustVulkanPresentation>(
      get_instance_proc_addr, std::move(context), presentation_callbacks);
  if (!presentation->IsValid()) {
    return nullptr;
  }

  auto thread_host = std::make_unique<ThreadHost>(
      "FlutterRust", ThreadHost::kRaster | ThreadHost::kIo);
  TaskRunners task_runners("FlutterRust", main_task_runner,
                           thread_host->raster_thread->GetTaskRunner(),
                           main_task_runner,
                           thread_host->io_thread->GetTaskRunner());
  PlatformViewRust::Configuration platform_view_configuration;
  platform_view_configuration.create_rendering_surface = [presentation] {
    return presentation->CreateSurface();
  };
  platform_view_configuration.handle_platform_message =
      [platform_message_callbacks](std::unique_ptr<PlatformMessage> message) {
        auto response_handle =
            message->response()
                ? std::make_unique<FlutterRustPlatformMessageResponseHandle>(
                      FlutterRustPlatformMessageResponseHandle{
                          message->response()})
                : nullptr;
        FlutterRustPlatformMessageDisposition disposition =
            kFlutterRustPlatformMessageUnhandled;
        if (platform_message_callbacks.handle_message) {
          const auto& channel = message->channel();
          const auto& data = message->data();
          disposition = platform_message_callbacks.handle_message(
              platform_message_callbacks.user_data,
              reinterpret_cast<const uint8_t*>(channel.data()), channel.size(),
              data.GetMapping(), data.GetSize(), response_handle.get());
        }
        if (disposition == kFlutterRustPlatformMessagePending &&
            response_handle) {
          response_handle.release();
          return;
        }
        if (auto response = message->response()) {
          if (disposition == kFlutterRustPlatformMessageSuccess) {
            constexpr char kSuccessEnvelope[] = "[null]";
            response->Complete(
                std::make_unique<fml::MallocMapping>(fml::MallocMapping::Copy(
                    kSuccessEnvelope, sizeof(kSuccessEnvelope) - 1)));
          } else {
            response->CompleteEmpty();
          }
        }
      };
  platform_view_configuration.vsync_callbacks = vsync_callbacks;
  auto shell = Shell::Create(
      PlatformData{}, task_runners, settings,
      [platform_view_configuration =
           std::move(platform_view_configuration)](Shell& shell) mutable {
        return std::make_unique<PlatformViewRust>(
            shell, shell.GetTaskRunners(),
            std::move(platform_view_configuration));
      },
      [](Shell& shell) { return std::make_unique<Rasterizer>(shell); });
  if (!shell || !shell->IsSetup()) {
    return nullptr;
  }
  return std::unique_ptr<RustShell>(new RustShell(
      std::move(thread_host), std::move(presentation), std::move(shell),
      windowing_callbacks, std::move(settings)));
}

RustShell::RustShell(std::unique_ptr<ThreadHost> thread_host,
                     std::shared_ptr<RustVulkanPresentation> presentation,
                     std::unique_ptr<Shell> shell,
                     FlutterRustWindowingCallbacks windowing_callbacks,
                     Settings settings)
    : thread_host_(std::move(thread_host)),
      presentation_(std::move(presentation)),
      shell_(std::move(shell)),
      windowing_callbacks_(windowing_callbacks),
      settings_(std::move(settings)) {}

RustShell::~RustShell() = default;

bool RustShell::IsValid() const {
  return shell_ && shell_->IsSetup() && presentation_->IsValid();
}

bool RustShell::Run() {
  if (!IsValid() || running_) {
    return false;
  }
  auto run_configuration = RunConfiguration::InferFromSettings(settings_);
  if (!run_configuration.IsValid()) {
    return false;
  }
  // Desktop framework initialization expects every engine to publish a
  // non-null identity through PlatformDispatcher.engineId. Keep the identity
  // stable for this RustShell's lifetime, matching the desktop embedders'
  // use of their engine object address.
  run_configuration.SetEngineId(reinterpret_cast<int64_t>(this));
  shell_->RunEngine(std::move(run_configuration));
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return false;
  }
  platform_view->NotifyCreated();
  running_ = true;
  return true;
}

void RustShell::SetViewportMetrics(FlutterRustViewId view_id,
                                   const FlutterRustViewMetrics& metrics) {
  if (!shell_) {
    return;
  }
  std::vector<std::unique_ptr<Display>> displays;
  displays.push_back(std::make_unique<Display>(
      /*display_id=*/0, metrics.display_refresh_rate, metrics.display_width,
      metrics.display_height, metrics.pixel_ratio));
  shell_->OnDisplayUpdates(std::move(displays));
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return;
  }
  platform_view->SetViewportMetrics(view_id, ToViewportMetrics(metrics));
}

void RustShell::AddView(
    FlutterRustViewId view_id,
    const FlutterRustViewMetrics& metrics,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustViewOperationCallbacks callbacks) {
  if (!shell_ || view_id <= kFlutterImplicitViewId) {
    CompleteViewOperation(callbacks, view_id, false);
    return;
  }
  if (!presentation_->RegisterView(view_id, presentation_callbacks)) {
    CompleteViewOperation(callbacks, view_id, false);
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    presentation_->UnregisterView(view_id);
    CompleteViewOperation(callbacks, view_id, false);
    return;
  }
  platform_view->AddView(
      view_id, ToViewportMetrics(metrics),
      [presentation = presentation_, callbacks, view_id](bool added) {
        if (!added) {
          presentation->UnregisterView(view_id);
        }
        CompleteViewOperation(callbacks, view_id, added);
      });
}

void RustShell::RemoveView(FlutterRustViewId view_id,
                           FlutterRustViewOperationCallbacks callbacks) {
  if (!shell_ || view_id <= kFlutterImplicitViewId) {
    CompleteViewOperation(callbacks, view_id, false);
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    CompleteViewOperation(callbacks, view_id, false);
    return;
  }
  platform_view->RemoveView(view_id, [presentation = presentation_, callbacks,
                                      view_id](bool removed) {
    if (removed) {
      presentation->UnregisterView(view_id);
    }
    CompleteViewOperation(callbacks, view_id, removed);
  });
}

void RustShell::SendViewFocusEvent(FlutterRustViewId view_id,
                                   uint32_t state,
                                   uint32_t direction) {
  if (!shell_ || state > static_cast<uint32_t>(ViewFocusState::kFocused) ||
      direction > static_cast<uint32_t>(ViewFocusDirection::kBackward)) {
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return;
  }
  platform_view->SendViewFocusEvent(
      ViewFocusEvent(view_id, static_cast<ViewFocusState>(state),
                     static_cast<ViewFocusDirection>(direction)));
}

void RustShell::SendPointerEvent(const FlutterRustPointerEvent& event) {
  if (!shell_) {
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return;
  }

  auto packet = CreateRustPointerDataPacket(event);
  if (packet) {
    platform_view->DispatchPointerDataPacket(std::move(packet));
  }
}

void RustShell::SendLifecycleEvent(uint32_t state) {
  if (!shell_) {
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  const char* state_name = GetRustLifecycleStateName(state);
  if (!platform_view || !state_name) {
    return;
  }
  const std::string state_string(state_name);
  platform_view->DispatchPlatformMessage(std::make_unique<PlatformMessage>(
      "flutter/lifecycle",
      fml::MallocMapping::Copy(state_string.data(), state_string.size()),
      fml::RefPtr<PlatformMessageResponse>()));
}

void RustShell::SendKeyEvent(const FlutterRustKeyEvent& event) {
  if (!shell_) {
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  auto packet = CreateRustKeyDataPacket(event);
  if (!platform_view || !packet) {
    return;
  }
  platform_view->DispatchPlatformMessage(std::make_unique<PlatformMessage>(
      "flutter/keydata",
      fml::MallocMapping::Copy(packet->data().data(), packet->data().size()),
      fml::RefPtr<PlatformMessageResponse>()));
}

void RustShell::SendPlatformMessage(const uint8_t* channel,
                                    uint64_t channel_size,
                                    const uint8_t* message,
                                    uint64_t message_size) {
  if (!shell_ || !channel || (!message && message_size != 0)) {
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return;
  }
  const std::string channel_string(reinterpret_cast<const char*>(channel),
                                   channel_size);
  platform_view->DispatchPlatformMessage(std::make_unique<PlatformMessage>(
      channel_string, fml::MallocMapping::Copy(message, message_size),
      fml::RefPtr<PlatformMessageResponse>()));
}

bool RustShell::SendPlatformMessageWithResponse(
    const uint8_t* channel,
    uint64_t channel_size,
    const uint8_t* message,
    uint64_t message_size,
    FlutterRustPlatformMessageResponseCallback callback,
    void* user_data) {
  if (!shell_ || !channel || (!message && message_size != 0) || !callback) {
    return false;
  }
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return false;
  }
  const std::string channel_string(reinterpret_cast<const char*>(channel),
                                   channel_size);
  platform_view->DispatchPlatformMessage(std::make_unique<PlatformMessage>(
      channel_string, fml::MallocMapping::Copy(message, message_size),
      fml::MakeRefCounted<RustPlatformMessageResponse>(callback, user_data)));
  return true;
}

void RustShell::OnVsync(uint64_t frame_interval_nanos) {
  if (!shell_) {
    return;
  }
  auto platform_view = shell_->GetPlatformView();
  if (platform_view) {
    static_cast<PlatformViewRust*>(platform_view.get())
        ->OnVsync(frame_interval_nanos);
  }
}

FlutterRustViewId RustShell::CreateRegularWindow(
    const FlutterRustRegularWindowRequest* request) {
  if (!request || !windowing_callbacks_.create_regular_window) {
    return -1;
  }
  return windowing_callbacks_.create_regular_window(
      windowing_callbacks_.user_data, request);
}

FlutterRustViewId RustShell::CreateDialogWindow(
    const FlutterRustDialogWindowRequest* request) {
  if (!request || !windowing_callbacks_.create_dialog_window) {
    return -1;
  }
  return windowing_callbacks_.create_dialog_window(
      windowing_callbacks_.user_data, request);
}

FlutterRustViewId RustShell::CreatePopupWindow(
    const FlutterRustPopupWindowRequest* request) {
  if (!request || !windowing_callbacks_.create_popup_window) {
    return -1;
  }
  return windowing_callbacks_.create_popup_window(
      windowing_callbacks_.user_data, request);
}

FlutterRustViewId RustShell::CreateSatelliteWindow(
    const FlutterRustSatelliteWindowRequest* request) {
  if (!request || !windowing_callbacks_.create_satellite_window) {
    return -1;
  }
  return windowing_callbacks_.create_satellite_window(
      windowing_callbacks_.user_data, request);
}

void RustShell::DestroyWindow(FlutterRustViewId view_id) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.destroy_window) {
    return;
  }
  windowing_callbacks_.destroy_window(windowing_callbacks_.user_data, view_id);
}

bool RustShell::GetWindowState(FlutterRustViewId view_id,
                               FlutterRustWindowState* state) {
  if (view_id <= kFlutterImplicitViewId || !state ||
      !windowing_callbacks_.get_window_state) {
    return false;
  }
  return windowing_callbacks_.get_window_state(windowing_callbacks_.user_data,
                                               view_id, state) != 0;
}

void RustShell::SetWindowSize(FlutterRustViewId view_id,
                              double width,
                              double height) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.set_window_size) {
    return;
  }
  windowing_callbacks_.set_window_size(windowing_callbacks_.user_data, view_id,
                                       width, height);
}

void RustShell::SetWindowConstraints(FlutterRustViewId view_id,
                                     int32_t has_constraints,
                                     double min_width,
                                     double min_height,
                                     double max_width,
                                     double max_height) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.set_window_constraints) {
    return;
  }
  windowing_callbacks_.set_window_constraints(
      windowing_callbacks_.user_data, view_id, has_constraints, min_width,
      min_height, max_width, max_height);
}

void RustShell::SetWindowTitle(FlutterRustViewId view_id,
                               const uint8_t* title,
                               uint64_t title_length) {
  if (view_id <= kFlutterImplicitViewId || (!title && title_length != 0) ||
      !windowing_callbacks_.set_window_title) {
    return;
  }
  windowing_callbacks_.set_window_title(windowing_callbacks_.user_data, view_id,
                                        title, title_length);
}

void RustShell::ActivateWindow(FlutterRustViewId view_id) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.activate_window) {
    return;
  }
  windowing_callbacks_.activate_window(windowing_callbacks_.user_data, view_id);
}

void RustShell::SetWindowMaximized(FlutterRustViewId view_id, bool maximized) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.set_window_maximized) {
    return;
  }
  windowing_callbacks_.set_window_maximized(windowing_callbacks_.user_data,
                                            view_id, maximized ? 1 : 0);
}

void RustShell::SetWindowMinimized(FlutterRustViewId view_id, bool minimized) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.set_window_minimized) {
    return;
  }
  windowing_callbacks_.set_window_minimized(windowing_callbacks_.user_data,
                                            view_id, minimized ? 1 : 0);
}

void RustShell::SetWindowFullscreen(FlutterRustViewId view_id,
                                    bool fullscreen) {
  if (view_id <= kFlutterImplicitViewId ||
      !windowing_callbacks_.set_window_fullscreen) {
    return;
  }
  windowing_callbacks_.set_window_fullscreen(windowing_callbacks_.user_data,
                                             view_id, fullscreen ? 1 : 0);
}

void RustShell::SetWindowEventCallback(
    FlutterRustWindowEventCallback callback) {
  if (!windowing_callbacks_.set_window_event_callback) {
    return;
  }
  windowing_callbacks_.set_window_event_callback(windowing_callbacks_.user_data,
                                                 callback);
}

bool RustShell::SetWindowParent(FlutterRustViewId view_id,
                                FlutterRustViewId parent_view_id) {
  if (view_id <= kFlutterImplicitViewId ||
      parent_view_id < kFlutterImplicitViewId ||
      !windowing_callbacks_.set_window_parent) {
    return false;
  }
  return windowing_callbacks_.set_window_parent(windowing_callbacks_.user_data,
                                                view_id, parent_view_id) != 0;
}

}  // namespace flutter

namespace {

std::vector<std::string> ToStringVector(const char* const* values,
                                        uint32_t count) {
  std::vector<std::string> result;
  result.reserve(count);
  for (uint32_t i = 0; i < count; ++i) {
    result.emplace_back(values[i]);
  }
  return result;
}

flutter::RustVulkanContextData ToContextData(
    const FlutterRustVulkanContextData& data) {
  flutter::RustVulkanContextData result;
  result.get_instance_proc_addr = data.get_instance_proc_addr;
  result.instance = data.instance;
  result.physical_device = data.physical_device;
  result.device = data.device;
  result.queue = data.queue;
  result.queue_family_index = data.queue_family_index;
  result.instance_extensions =
      ToStringVector(data.instance_extensions, data.instance_extensions_count);
  result.device_extensions =
      ToStringVector(data.device_extensions, data.device_extensions_count);
  return result;
}

flutter::Settings ToSettings(const FlutterRustShellSettings& settings) {
  // Match the desktop shells' environment switch contract so a Flutter-tool
  // resident run can select a VM-service port, start paused, and pass the
  // usual debugger/profiling flags without adding them to application argv.
  std::vector<std::string> command_line_args = {"flutter_rust_shell"};
  auto environment_switches = flutter::GetSwitchesFromEnvironment();
  command_line_args.insert(command_line_args.end(),
                           environment_switches.begin(),
                           environment_switches.end());
  flutter::Settings result =
      flutter::SettingsFromCommandLine(fml::CommandLineFromIterators(
          command_line_args.begin(), command_line_args.end()));
  if (settings.assets_path) {
    result.assets_path = settings.assets_path;
  }
  if (settings.icu_data_path) {
    result.icu_data_path = settings.icu_data_path;
  }
  // Phase 0 only runs a JIT kernel snapshot; there is no AOT path yet.
  if (!flutter::DartVM::IsRunningPrecompiledCode()) {
    result.application_kernel_asset = "kernel_blob.bin";
  }
  // Settings::enable_impeller only defaults to true on Android/iOS; every
  // other platform, including Linux, defaults to false. Leaving it unset
  // makes the Dart-level paragraph/text layer build Skia-flavored DlText
  // objects while this shell's surface is Impeller-only, which crashes the
  // first time anything draws text.
  result.enable_impeller = true;
  // task_observer_add/remove are wired in RustShell::Create, which has the
  // concrete RustTaskRunner this Settings will run on.
  return result;
}

}  // namespace

extern "C" void* FlutterRustShellCreateShell(
    void* task_runner,
    FlutterRustVulkanContextData context_data,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustPlatformMessageCallbacks platform_message_callbacks,
    FlutterRustVsyncCallbacks vsync_callbacks,
    FlutterRustWindowingCallbacks windowing_callbacks,
    FlutterRustShellSettings settings) {
  auto main_task_runner = flutter::RustTaskRunner::FromHandle(task_runner);
  if (!main_task_runner) {
    return nullptr;
  }
  auto shell = flutter::RustShell::Create(
      std::move(main_task_runner), ToContextData(context_data),
      presentation_callbacks, platform_message_callbacks, vsync_callbacks,
      windowing_callbacks, ToSettings(settings));
  if (!shell || !shell->IsValid()) {
    return nullptr;
  }
  return shell.release();
}

extern "C" int FlutterRustShellRunShell(void* shell) {
  if (!shell) {
    return 0;
  }
  return static_cast<flutter::RustShell*>(shell)->Run() ? 1 : 0;
}

extern "C" void FlutterRustShellSetViewportMetrics(
    void* shell,
    FlutterRustViewId view_id,
    FlutterRustViewMetrics metrics) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SetViewportMetrics(view_id, metrics);
}

extern "C" void FlutterRustShellAddView(
    void* shell,
    FlutterRustViewId view_id,
    FlutterRustViewMetrics metrics,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustViewOperationCallbacks callbacks) {
  if (!shell) {
    if (callbacks.complete) {
      callbacks.complete(callbacks.user_data, view_id, 0);
    }
    return;
  }
  static_cast<flutter::RustShell*>(shell)->AddView(
      view_id, metrics, presentation_callbacks, callbacks);
}

extern "C" void FlutterRustShellRemoveView(
    void* shell,
    FlutterRustViewId view_id,
    FlutterRustViewOperationCallbacks callbacks) {
  if (!shell) {
    if (callbacks.complete) {
      callbacks.complete(callbacks.user_data, view_id, 0);
    }
    return;
  }
  static_cast<flutter::RustShell*>(shell)->RemoveView(view_id, callbacks);
}

extern "C" void FlutterRustShellSendViewFocusEvent(void* shell,
                                                   FlutterRustViewId view_id,
                                                   uint32_t state,
                                                   uint32_t direction) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SendViewFocusEvent(view_id, state,
                                                              direction);
}

extern "C" void FlutterRustShellSendPointerEvent(
    void* shell,
    FlutterRustPointerEvent event) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SendPointerEvent(event);
}

extern "C" void FlutterRustShellSendLifecycleEvent(void* shell,
                                                   uint32_t state) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SendLifecycleEvent(state);
}

extern "C" void FlutterRustShellSendKeyEvent(void* shell,
                                             FlutterRustKeyEvent event) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SendKeyEvent(event);
}

extern "C" void FlutterRustShellSendPlatformMessage(void* shell,
                                                    const uint8_t* channel,
                                                    uint64_t channel_size,
                                                    const uint8_t* message,
                                                    uint64_t message_size) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SendPlatformMessage(
      channel, channel_size, message, message_size);
}

extern "C" int FlutterRustShellSendPlatformMessageWithResponse(
    void* shell,
    const uint8_t* channel,
    uint64_t channel_size,
    const uint8_t* message,
    uint64_t message_size,
    FlutterRustPlatformMessageResponseCallback callback,
    void* user_data) {
  if (!shell) {
    return 0;
  }
  return static_cast<flutter::RustShell*>(shell)
                 ->SendPlatformMessageWithResponse(channel, channel_size,
                                                   message, message_size,
                                                   callback, user_data)
             ? 1
             : 0;
}

extern "C" void FlutterRustShellCompletePlatformMessageResponse(
    FlutterRustPlatformMessageResponseHandle* response_handle,
    const uint8_t* response,
    uint64_t response_size) {
  std::unique_ptr<FlutterRustPlatformMessageResponseHandle> owned(
      response_handle);
  if (!owned || !owned->response || (!response && response_size != 0)) {
    return;
  }
  if (!response) {
    owned->response->CompleteEmpty();
    return;
  }
  owned->response->Complete(std::make_unique<fml::MallocMapping>(
      fml::MallocMapping::Copy(response, response_size)));
}

extern "C" void FlutterRustShellOnVsync(void* shell,
                                        uint64_t frame_interval_nanos) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->OnVsync(frame_interval_nanos);
}

extern "C" FlutterRustViewId FlutterRustShellWindowCreateRegular(
    int64_t engine_id,
    const FlutterRustRegularWindowRequest* request) {
  if (engine_id == 0) {
    return -1;
  }
  return reinterpret_cast<flutter::RustShell*>(engine_id)->CreateRegularWindow(
      request);
}

extern "C" FlutterRustViewId FlutterRustShellWindowCreateDialog(
    int64_t engine_id,
    const FlutterRustDialogWindowRequest* request) {
  if (engine_id == 0) {
    return -1;
  }
  return reinterpret_cast<flutter::RustShell*>(engine_id)->CreateDialogWindow(
      request);
}

extern "C" FlutterRustViewId FlutterRustShellWindowCreatePopup(
    int64_t engine_id,
    const FlutterRustPopupWindowRequest* request) {
  if (engine_id == 0) {
    return -1;
  }
  return reinterpret_cast<flutter::RustShell*>(engine_id)->CreatePopupWindow(
      request);
}

extern "C" FlutterRustViewId FlutterRustShellWindowCreateSatellite(
    int64_t engine_id,
    const FlutterRustSatelliteWindowRequest* request) {
  if (engine_id == 0) {
    return -1;
  }
  return reinterpret_cast<flutter::RustShell*>(engine_id)
      ->CreateSatelliteWindow(request);
}

extern "C" void FlutterRustShellWindowDestroy(int64_t engine_id,
                                              FlutterRustViewId view_id) {
  if (engine_id == 0) {
    return;
  }
  reinterpret_cast<flutter::RustShell*>(engine_id)->DestroyWindow(view_id);
}

extern "C" int FlutterRustShellWindowGetState(int64_t engine_id,
                                              FlutterRustViewId view_id,
                                              FlutterRustWindowState* state) {
  if (engine_id == 0) {
    return 0;
  }
  return reinterpret_cast<flutter::RustShell*>(engine_id)->GetWindowState(
             view_id, state)
             ? 1
             : 0;
}

extern "C" void FlutterRustShellWindowSetSize(int64_t engine_id,
                                              FlutterRustViewId view_id,
                                              double width,
                                              double height) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowSize(
        view_id, width, height);
  }
}

extern "C" void FlutterRustShellWindowSetConstraints(int64_t engine_id,
                                                     FlutterRustViewId view_id,
                                                     int32_t has_constraints,
                                                     double min_width,
                                                     double min_height,
                                                     double max_width,
                                                     double max_height) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowConstraints(
        view_id, has_constraints, min_width, min_height, max_width, max_height);
  }
}

extern "C" void FlutterRustShellWindowSetTitle(int64_t engine_id,
                                               FlutterRustViewId view_id,
                                               const uint8_t* title,
                                               uint64_t title_length) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowTitle(
        view_id, title, title_length);
  }
}

extern "C" void FlutterRustShellWindowActivate(int64_t engine_id,
                                               FlutterRustViewId view_id) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->ActivateWindow(view_id);
  }
}

extern "C" void FlutterRustShellWindowSetMaximized(int64_t engine_id,
                                                   FlutterRustViewId view_id,
                                                   int32_t maximized) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowMaximized(
        view_id, maximized != 0);
  }
}

extern "C" void FlutterRustShellWindowSetMinimized(int64_t engine_id,
                                                   FlutterRustViewId view_id,
                                                   int32_t minimized) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowMinimized(
        view_id, minimized != 0);
  }
}

extern "C" void FlutterRustShellWindowSetFullscreen(int64_t engine_id,
                                                    FlutterRustViewId view_id,
                                                    int32_t fullscreen) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowFullscreen(
        view_id, fullscreen != 0);
  }
}

extern "C" void FlutterRustShellWindowSetEventCallback(
    int64_t engine_id,
    FlutterRustWindowEventCallback callback) {
  if (engine_id != 0) {
    reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowEventCallback(
        callback);
  }
}

extern "C" int FlutterRustShellWindowSetParent(
    int64_t engine_id,
    FlutterRustViewId view_id,
    FlutterRustViewId parent_view_id) {
  if (engine_id == 0) {
    return 0;
  }
  return reinterpret_cast<flutter::RustShell*>(engine_id)->SetWindowParent(
             view_id, parent_view_id)
             ? 1
             : 0;
}

extern "C" void FlutterRustShellDestroyShell(void* shell) {
  delete static_cast<flutter::RustShell*>(shell);
}
