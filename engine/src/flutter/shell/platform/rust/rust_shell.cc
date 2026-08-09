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

namespace flutter {

std::unique_ptr<RustShell> RustShell::Create(
    fml::RefPtr<fml::TaskRunner> main_task_runner,
    RustVulkanContextData context_data,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustPlatformMessageCallbacks platform_message_callbacks,
    FlutterRustVsyncCallbacks vsync_callbacks,
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
        bool handled = false;
        if (platform_message_callbacks.handle_message) {
          const auto& channel = message->channel();
          const auto& data = message->data();
          handled = platform_message_callbacks.handle_message(
                        platform_message_callbacks.user_data,
                        reinterpret_cast<const uint8_t*>(channel.data()),
                        channel.size(), data.GetMapping(), data.GetSize()) != 0;
        }
        if (auto response = message->response()) {
          if (handled) {
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
  return std::unique_ptr<RustShell>(
      new RustShell(std::move(thread_host), std::move(presentation),
                    std::move(shell), std::move(settings)));
}

RustShell::RustShell(std::unique_ptr<ThreadHost> thread_host,
                     std::shared_ptr<RustVulkanPresentation> presentation,
                     std::unique_ptr<Shell> shell,
                     Settings settings)
    : thread_host_(std::move(thread_host)),
      presentation_(std::move(presentation)),
      shell_(std::move(shell)),
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

void RustShell::SetViewportMetrics(double width,
                                   double height,
                                   double pixel_ratio,
                                   double display_width,
                                   double display_height,
                                   double display_refresh_rate) {
  if (!shell_) {
    return;
  }
  std::vector<std::unique_ptr<Display>> displays;
  displays.push_back(std::make_unique<Display>(
      /*display_id=*/0, display_refresh_rate, display_width, display_height,
      pixel_ratio));
  shell_->OnDisplayUpdates(std::move(displays));
  auto platform_view = shell_->GetPlatformView();
  if (!platform_view) {
    return;
  }
  platform_view->SetViewportMetrics(
      kFlutterImplicitViewId, ViewportMetrics(pixel_ratio, width, height,
                                              /*p_physical_touch_slop=*/-1.0,
                                              /*display_id=*/0));
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
    FlutterRustShellSettings settings) {
  auto main_task_runner = flutter::RustTaskRunner::FromHandle(task_runner);
  if (!main_task_runner) {
    return nullptr;
  }
  auto shell = flutter::RustShell::Create(
      std::move(main_task_runner), ToContextData(context_data),
      presentation_callbacks, platform_message_callbacks, vsync_callbacks,
      ToSettings(settings));
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
    double width,
    double height,
    double pixel_ratio,
    double display_width,
    double display_height,
    double display_refresh_rate) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->SetViewportMetrics(
      width, height, pixel_ratio, display_width, display_height,
      display_refresh_rate);
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

extern "C" void FlutterRustShellOnVsync(void* shell,
                                        uint64_t frame_interval_nanos) {
  if (!shell) {
    return;
  }
  static_cast<flutter::RustShell*>(shell)->OnVsync(frame_interval_nanos);
}

extern "C" void FlutterRustShellDestroyShell(void* shell) {
  delete static_cast<flutter::RustShell*>(shell);
}
