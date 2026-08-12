// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
#define FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_

#include <stdint.h>

#if defined(_WIN32)
#define FLUTTER_RUST_SHELL_EXPORT __declspec(dllexport)
#else
#define FLUTTER_RUST_SHELL_EXPORT __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

// This private ABI is lockstep-versioned with the Flutter fork. It is not the
// Flutter Embedder API and is never exposed to application plugins.
#define FLUTTER_RUST_SHELL_ABI_VERSION 8u
#define FLUTTER_RUST_PLUGIN_SDK_API_VERSION 1u

typedef struct FlutterRustShellAbi {
  uint32_t shell_abi_version;
  uint32_t plugin_sdk_api_version;
} FlutterRustShellAbi;

typedef int64_t FlutterRustViewId;
#define FLUTTER_RUST_IMPLICIT_VIEW_ID ((FlutterRustViewId)0)

typedef struct FlutterRustViewMetrics {
  double width;
  double height;
  double min_width;
  double max_width;
  double min_height;
  double max_height;
  double pixel_ratio;
  double display_width;
  double display_height;
  double display_refresh_rate;
} FlutterRustViewMetrics;

typedef void (*FlutterRustViewOperationCompleteCallback)(
    void* user_data,
    FlutterRustViewId view_id,
    int success);

typedef struct FlutterRustViewOperationCallbacks {
  void* user_data;
  FlutterRustViewOperationCompleteCallback complete;
} FlutterRustViewOperationCallbacks;

typedef struct FlutterRustRegularWindowRequest {
  int32_t has_size;
  double width;
  double height;
  const uint8_t* title;
  uint64_t title_length;
  int32_t resizable;
  int32_t has_constraints;
  double min_width;
  double min_height;
  double max_width;
  double max_height;
} FlutterRustRegularWindowRequest;

typedef struct FlutterRustDialogWindowRequest {
  FlutterRustRegularWindowRequest window;
  int32_t has_parent;
  FlutterRustViewId parent_view_id;
} FlutterRustDialogWindowRequest;

typedef enum FlutterRustPopupWindowKind {
  kFlutterRustPopupWindowKindTooltip = 0,
  kFlutterRustPopupWindowKindPopup = 1,
} FlutterRustPopupWindowKind;

typedef struct FlutterRustPopupWindowRequest {
  int32_t kind;
  FlutterRustViewId parent_view_id;
  double min_width;
  double min_height;
  double max_width;
  double max_height;
  double anchor_x;
  double anchor_y;
  double anchor_width;
  double anchor_height;
  int32_t parent_anchor;
  int32_t child_anchor;
  double offset_x;
  double offset_y;
  uint32_t constraint_adjustment;
} FlutterRustPopupWindowRequest;

typedef struct FlutterRustSatelliteWindowRequest {
  FlutterRustRegularWindowRequest window;
  FlutterRustViewId parent_view_id;
  int32_t has_anchor_rect;
  double anchor_x;
  double anchor_y;
  double anchor_width;
  double anchor_height;
  int32_t parent_anchor;
  int32_t child_anchor;
  double offset_x;
  double offset_y;
  uint32_t constraint_adjustment;
} FlutterRustSatelliteWindowRequest;

typedef struct FlutterRustWindowState {
  double width;
  double height;
  int32_t focused;
  int32_t maximized;
  int32_t minimized;
  int32_t fullscreen;
} FlutterRustWindowState;

typedef enum FlutterRustWindowEvent {
  kFlutterRustWindowEventStateChanged = 0,
  kFlutterRustWindowEventCloseRequested = 1,
  kFlutterRustWindowEventDestroyed = 2,
} FlutterRustWindowEvent;

typedef void (*FlutterRustWindowEventCallback)(FlutterRustViewId view_id,
                                               FlutterRustWindowEvent event);

typedef FlutterRustViewId (*FlutterRustCreateRegularWindowCallback)(
    void* user_data,
    const FlutterRustRegularWindowRequest* request);
typedef FlutterRustViewId (*FlutterRustCreateDialogWindowCallback)(
    void* user_data,
    const FlutterRustDialogWindowRequest* request);
typedef FlutterRustViewId (*FlutterRustCreatePopupWindowCallback)(
    void* user_data,
    const FlutterRustPopupWindowRequest* request);
typedef FlutterRustViewId (*FlutterRustCreateSatelliteWindowCallback)(
    void* user_data,
    const FlutterRustSatelliteWindowRequest* request);
typedef void (*FlutterRustDestroyWindowCallback)(void* user_data,
                                                 FlutterRustViewId view_id);
typedef int (*FlutterRustGetWindowStateCallback)(void* user_data,
                                                 FlutterRustViewId view_id,
                                                 FlutterRustWindowState* state);
typedef void (*FlutterRustSetWindowSizeCallback)(void* user_data,
                                                 FlutterRustViewId view_id,
                                                 double width,
                                                 double height);
typedef void (*FlutterRustSetWindowConstraintsCallback)(
    void* user_data,
    FlutterRustViewId view_id,
    int32_t has_constraints,
    double min_width,
    double min_height,
    double max_width,
    double max_height);
typedef void (*FlutterRustSetWindowTitleCallback)(void* user_data,
                                                  FlutterRustViewId view_id,
                                                  const uint8_t* title,
                                                  uint64_t title_length);
typedef void (*FlutterRustSetWindowFlagCallback)(void* user_data,
                                                 FlutterRustViewId view_id,
                                                 int32_t enabled);
typedef void (*FlutterRustSetWindowEventCallback)(
    void* user_data,
    FlutterRustWindowEventCallback callback);
typedef int (*FlutterRustSetWindowParentCallback)(
    void* user_data,
    FlutterRustViewId view_id,
    FlutterRustViewId parent_view_id);

typedef struct FlutterRustWindowingCallbacks {
  void* user_data;
  FlutterRustCreateRegularWindowCallback create_regular_window;
  FlutterRustCreateDialogWindowCallback create_dialog_window;
  FlutterRustCreatePopupWindowCallback create_popup_window;
  FlutterRustCreateSatelliteWindowCallback create_satellite_window;
  FlutterRustDestroyWindowCallback destroy_window;
  FlutterRustGetWindowStateCallback get_window_state;
  FlutterRustSetWindowSizeCallback set_window_size;
  FlutterRustSetWindowConstraintsCallback set_window_constraints;
  FlutterRustSetWindowTitleCallback set_window_title;
  FlutterRustDestroyWindowCallback activate_window;
  FlutterRustSetWindowFlagCallback set_window_maximized;
  FlutterRustSetWindowFlagCallback set_window_minimized;
  FlutterRustSetWindowFlagCallback set_window_fullscreen;
  FlutterRustSetWindowEventCallback set_window_event_callback;
  FlutterRustSetWindowParentCallback set_window_parent;
} FlutterRustWindowingCallbacks;

typedef enum FlutterRustViewFocusState {
  kFlutterRustViewFocusStateUnfocused = 0,
  kFlutterRustViewFocusStateFocused = 1,
} FlutterRustViewFocusState;

typedef enum FlutterRustViewFocusDirection {
  kFlutterRustViewFocusDirectionUndefined = 0,
  kFlutterRustViewFocusDirectionForward = 1,
  kFlutterRustViewFocusDirectionBackward = 2,
} FlutterRustViewFocusDirection;

// A callback table owned by the Rust host for one merged Flutter UI/platform
// task runner. Times are relative delays so C++ and Rust need not share a
// monotonic-clock epoch. `task_runner` and `task_baton` are opaque values that
// Rust returns to FlutterRustShellRunTask when the winit loop reaches them.
typedef void (*FlutterRustScheduleTaskCallback)(void* user_data,
                                                void* task_runner,
                                                uint64_t task_baton,
                                                uint64_t delay_nanos);
typedef int (*FlutterRustRunsTasksOnCurrentThreadCallback)(void* user_data);
typedef void (*FlutterRustTaskRunnerDestroyedCallback)(void* user_data);

typedef struct FlutterRustTaskRunnerCallbacks {
  void* user_data;
  FlutterRustScheduleTaskCallback schedule_task;
  FlutterRustRunsTasksOnCurrentThreadCallback runs_tasks_on_current_thread;
  FlutterRustTaskRunnerDestroyedCallback task_runner_destroyed;
} FlutterRustTaskRunnerCallbacks;

// A Vulkan swapchain image acquired by the Rust GPU broker. The broker owns
// the image and all synchronization associated with it. C++ borrows it only
// for the duration of a Flutter frame.
typedef struct FlutterRustVulkanImage {
  uint64_t image;
  uint32_t format;
  // Binary semaphore signalled by wgpu once acquisition is complete. Impeller
  // consumes this wait before touching the image.
  uint64_t acquire_semaphore;
  // Binary semaphore signalled after Impeller's last submission. The broker
  // makes its final wgpu/present submission wait on it.
  uint64_t render_semaphore;
} FlutterRustVulkanImage;

typedef int (*FlutterRustAcquireVulkanImageCallback)(
    void* user_data,
    uint32_t width,
    uint32_t height,
    FlutterRustVulkanImage* image);
typedef int (*FlutterRustPresentVulkanImageCallback)(
    void* user_data,
    FlutterRustVulkanImage image);

typedef struct FlutterRustVulkanPresentationCallbacks {
  void* user_data;
  FlutterRustAcquireVulkanImageCallback acquire_image;
  FlutterRustPresentVulkanImageCallback present_image;
} FlutterRustVulkanPresentationCallbacks;

// One plugin-produced Vulkan texture frame. Rust owns the image and both
// semaphores until release_frame is called. The image must arrive in
// SHADER_READ_ONLY_OPTIMAL and remain alive until the render semaphore is
// consumed by the producer.
typedef struct FlutterRustExternalTextureFrame {
  uint64_t image;
  uint64_t image_view;
  uint32_t format;
  uint32_t width;
  uint32_t height;
  uint64_t acquire_semaphore;
  uint64_t render_semaphore;
} FlutterRustExternalTextureFrame;

typedef int (*FlutterRustAcquireExternalTextureFrameCallback)(
    void* user_data,
    uint32_t requested_width,
    uint32_t requested_height,
    FlutterRustExternalTextureFrame* frame);
typedef void (*FlutterRustReleaseExternalTextureFrameCallback)(
    void* user_data,
    FlutterRustExternalTextureFrame frame);

typedef struct FlutterRustExternalTextureCallbacks {
  void* user_data;
  FlutterRustAcquireExternalTextureFrameCallback acquire_frame;
  FlutterRustReleaseExternalTextureFrameCallback release_frame;
} FlutterRustExternalTextureCallbacks;

FlutterRustShellAbi FlutterRustShellGetAbi(void);

// Creates and destroys the C++ half of a Rust-owned task runner. The returned
// handle is opaque to Rust; only the callback table's owner may destroy it.
FLUTTER_RUST_SHELL_EXPORT void* FlutterRustShellCreateTaskRunner(
    FlutterRustTaskRunnerCallbacks callbacks);
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellRunTask(void* task_runner,
                                                      uint64_t task_baton);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellDestroyTaskRunner(
    void* task_runner);

// Raw Vulkan objects borrowed from Rust/wgpu for the lifetime of the shell
// they create. C-string arrays are borrowed only for the duration of the
// FlutterRustShellCreateShell call.
typedef struct FlutterRustVulkanContextData {
  void* get_instance_proc_addr;
  void* instance;
  void* physical_device;
  void* device;
  void* queue;
  uint32_t queue_family_index;
  const char* const* instance_extensions;
  uint32_t instance_extensions_count;
  const char* const* device_extensions;
  uint32_t device_extensions_count;
} FlutterRustVulkanContextData;

// Paths borrowed only for the duration of the FlutterRustShellCreateShell
// call; the engine copies what it needs.
typedef struct FlutterRustShellSettings {
  const char* assets_path;
  const char* icu_data_path;
} FlutterRustShellSettings;

typedef struct FlutterRustPlatformMessageResponseHandle
    FlutterRustPlatformMessageResponseHandle;

typedef enum FlutterRustPlatformMessageDisposition {
  kFlutterRustPlatformMessageUnhandled = 0,
  kFlutterRustPlatformMessageSuccess = 1,
  kFlutterRustPlatformMessagePending = 2,
} FlutterRustPlatformMessageDisposition;

typedef void (*FlutterRustPlatformMessageResponseCallback)(
    void* user_data,
    const uint8_t* response,
    uint64_t response_size);

// Framework-to-host platform messages. All byte pointers and the response
// handle are borrowed only for the duration of the callback. Returning
// Pending transfers ownership of a non-null response handle to Rust.
typedef FlutterRustPlatformMessageDisposition (
    *FlutterRustHandlePlatformMessageCallback)(
    void* user_data,
    const uint8_t* channel,
    uint64_t channel_size,
    const uint8_t* message,
    uint64_t message_size,
    FlutterRustPlatformMessageResponseHandle* response_handle);

typedef struct FlutterRustPlatformMessageCallbacks {
  void* user_data;
  FlutterRustHandlePlatformMessageCallback handle_message;
} FlutterRustPlatformMessageCallbacks;

// Requests that the Rust window host wake Flutter on the next compositor
// frame. A null callback selects Flutter's timer-based fallback waiter.
typedef void (*FlutterRustRequestVsyncCallback)(void* user_data);

typedef struct FlutterRustVsyncCallbacks {
  void* user_data;
  FlutterRustRequestVsyncCallback request_vsync;
} FlutterRustVsyncCallbacks;

// One pointer event produced by the Rust window host. Numeric enum values are
// translated explicitly on the C++ side rather than relying on Flutter's
// internal enum layout across the C ABI.
typedef enum FlutterRustPointerPhase {
  kFlutterRustPointerPhaseCancel = 0,
  kFlutterRustPointerPhaseAdd = 1,
  kFlutterRustPointerPhaseRemove = 2,
  kFlutterRustPointerPhaseHover = 3,
  kFlutterRustPointerPhaseDown = 4,
  kFlutterRustPointerPhaseMove = 5,
  kFlutterRustPointerPhaseUp = 6,
} FlutterRustPointerPhase;

typedef enum FlutterRustPointerDeviceKind {
  kFlutterRustPointerDeviceKindMouse = 0,
  kFlutterRustPointerDeviceKindTouch = 1,
} FlutterRustPointerDeviceKind;

typedef enum FlutterRustPointerSignalKind {
  kFlutterRustPointerSignalKindNone = 0,
  kFlutterRustPointerSignalKindScroll = 1,
} FlutterRustPointerSignalKind;

typedef struct FlutterRustPointerEvent {
  FlutterRustViewId view_id;
  uint64_t timestamp_micros;
  uint32_t phase;
  uint32_t device_kind;
  uint32_t signal_kind;
  int64_t device;
  double physical_x;
  double physical_y;
  double scroll_delta_x;
  double scroll_delta_y;
  int64_t buttons;
} FlutterRustPointerEvent;

// Lifecycle values are translated explicitly into Flutter's framework-facing
// strings by C++; Rust does not send string pointers across the ABI.
typedef enum FlutterRustLifecycleState {
  kFlutterRustLifecycleStateDetached = 0,
  kFlutterRustLifecycleStateResumed = 1,
  kFlutterRustLifecycleStateInactive = 2,
  kFlutterRustLifecycleStateHidden = 3,
  kFlutterRustLifecycleStatePaused = 4,
} FlutterRustLifecycleState;

typedef enum FlutterRustKeyEventType {
  kFlutterRustKeyEventTypeDown = 0,
  kFlutterRustKeyEventTypeUp = 1,
  kFlutterRustKeyEventTypeRepeat = 2,
} FlutterRustKeyEventType;

#define FLUTTER_RUST_KEY_CHARACTER_CAPACITY 64u

// Text is stored inline to keep keyboard delivery value-only. character_length
// is a byte length and must not exceed FLUTTER_RUST_KEY_CHARACTER_CAPACITY.
typedef struct FlutterRustKeyEvent {
  uint64_t timestamp_micros;
  uint32_t event_type;
  uint64_t physical;
  uint64_t logical;
  int32_t synthesized;
  uint32_t character_length;
  uint8_t character[FLUTTER_RUST_KEY_CHARACTER_CAPACITY];
} FlutterRustKeyEvent;

// Creates the private engine-side half of one Rust-hosted Flutter
// application. `task_runner` must be a handle previously returned by
// FlutterRustShellCreateTaskRunner and is used as the merged UI/platform task
// runner; the caller retains ownership of it. Returns null on failure.
FLUTTER_RUST_SHELL_EXPORT void* FlutterRustShellCreateShell(
    void* task_runner,
    FlutterRustVulkanContextData context_data,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustPlatformMessageCallbacks platform_message_callbacks,
    FlutterRustVsyncCallbacks vsync_callbacks,
    FlutterRustWindowingCallbacks windowing_callbacks,
    FlutterRustShellSettings settings);

// Dart FFI entry points backing Flutter's WindowController on the Rust shell.
FLUTTER_RUST_SHELL_EXPORT FlutterRustViewId FlutterRustShellWindowCreateRegular(
    int64_t engine_id,
    const FlutterRustRegularWindowRequest* request);
FLUTTER_RUST_SHELL_EXPORT FlutterRustViewId FlutterRustShellWindowCreateDialog(
    int64_t engine_id,
    const FlutterRustDialogWindowRequest* request);
FLUTTER_RUST_SHELL_EXPORT FlutterRustViewId
FlutterRustShellWindowCreatePopup(int64_t engine_id,
                                  const FlutterRustPopupWindowRequest* request);
FLUTTER_RUST_SHELL_EXPORT FlutterRustViewId
FlutterRustShellWindowCreateSatellite(
    int64_t engine_id,
    const FlutterRustSatelliteWindowRequest* request);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowDestroy(
    int64_t engine_id,
    FlutterRustViewId view_id);
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellWindowGetState(
    int64_t engine_id,
    FlutterRustViewId view_id,
    FlutterRustWindowState* state);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetSize(
    int64_t engine_id,
    FlutterRustViewId view_id,
    double width,
    double height);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetConstraints(
    int64_t engine_id,
    FlutterRustViewId view_id,
    int32_t has_constraints,
    double min_width,
    double min_height,
    double max_width,
    double max_height);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetTitle(
    int64_t engine_id,
    FlutterRustViewId view_id,
    const uint8_t* title,
    uint64_t title_length);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowActivate(
    int64_t engine_id,
    FlutterRustViewId view_id);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetMaximized(
    int64_t engine_id,
    FlutterRustViewId view_id,
    int32_t maximized);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetMinimized(
    int64_t engine_id,
    FlutterRustViewId view_id,
    int32_t minimized);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetFullscreen(
    int64_t engine_id,
    FlutterRustViewId view_id,
    int32_t fullscreen);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellWindowSetEventCallback(
    int64_t engine_id,
    FlutterRustWindowEventCallback callback);
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellWindowSetParent(
    int64_t engine_id,
    FlutterRustViewId view_id,
    FlutterRustViewId parent_view_id);

// Starts the root isolate and attaches the Vulkan presentation surface. Must
// run on the merged Rust UI/platform task runner. Returns non-zero on
// success.
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellRunShell(void* shell);

// Reports one view's size to the running engine. Call for the implicit view
// after FlutterRustShellRunShell succeeds and again whenever any view resizes.
// Must run on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSetViewportMetrics(
    void* shell,
    FlutterRustViewId view_id,
    FlutterRustViewMetrics metrics);

// Adds/removes a non-implicit view in the shared engine. Completion is
// asynchronous and returns on the merged Rust UI/platform task runner. The
// callback owner must remain alive until completion.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellAddView(
    void* shell,
    FlutterRustViewId view_id,
    FlutterRustViewMetrics metrics,
    FlutterRustVulkanPresentationCallbacks presentation_callbacks,
    FlutterRustViewOperationCallbacks callbacks);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellRemoveView(
    void* shell,
    FlutterRustViewId view_id,
    FlutterRustViewOperationCallbacks callbacks);

// Reports native focus changes for one Flutter view.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendViewFocusEvent(
    void* shell,
    FlutterRustViewId view_id,
    uint32_t state,
    uint32_t direction);

// Dispatches one mouse or touch event to the event's target Flutter view.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendPointerEvent(
    void* shell,
    FlutterRustPointerEvent event);

// Reports one application lifecycle transition. Unknown numeric enum values
// are ignored. Must run on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendLifecycleEvent(
    void* shell,
    uint32_t state);

// Dispatches one physical keyboard event to Flutter's key-data channel. Must
// run on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendKeyEvent(
    void* shell,
    FlutterRustKeyEvent event);

// Dispatches an encoded platform message to the Flutter framework. The byte
// slices are copied before this function returns.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellSendPlatformMessage(
    void* shell,
    const uint8_t* channel,
    uint64_t channel_size,
    const uint8_t* message,
    uint64_t message_size);

// Dispatches a platform message and invokes callback once with the framework's
// encoded response. Returns zero if the message could not be dispatched.
FLUTTER_RUST_SHELL_EXPORT int FlutterRustShellSendPlatformMessageWithResponse(
    void* shell,
    const uint8_t* channel,
    uint64_t channel_size,
    const uint8_t* message,
    uint64_t message_size,
    FlutterRustPlatformMessageResponseCallback callback,
    void* user_data);

// Completes and releases a one-shot framework-to-host response retained after
// a platform-message callback returned Pending.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellCompletePlatformMessageResponse(
    FlutterRustPlatformMessageResponseHandle* response_handle,
    const uint8_t* response,
    uint64_t response_size);

// Delivers one compositor-aligned pulse. The interval is the active monitor's
// nominal refresh period; C++ supplies its own monotonic timestamp so clock
// epochs never cross the private ABI.
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellOnVsync(
    void* shell,
    uint64_t frame_interval_nanos);

// Registers a plugin-produced Vulkan texture with Flutter's existing texture
// registry. Returns a positive engine-generated texture ID, or -1 on failure.
// All three calls must run on the merged Rust UI/platform task runner.
FLUTTER_RUST_SHELL_EXPORT int64_t FlutterRustShellRegisterExternalTexture(
    void* shell,
    FlutterRustExternalTextureCallbacks callbacks);
FLUTTER_RUST_SHELL_EXPORT void
FlutterRustShellMarkExternalTextureFrameAvailable(void* shell,
                                                  int64_t texture_id);
FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellUnregisterExternalTexture(
    void* shell,
    int64_t texture_id);

FLUTTER_RUST_SHELL_EXPORT void FlutterRustShellDestroyShell(void* shell);

#ifdef __cplusplus
}
#endif

#endif  // FLUTTER_SHELL_PLATFORM_RUST_RUST_BRIDGE_H_
