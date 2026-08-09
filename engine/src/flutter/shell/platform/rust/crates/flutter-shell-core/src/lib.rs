//! Private Rust runtime core for the in-tree Flutter Rust shell.
//!
//! Its C ABI is intentionally small and lockstep-versioned with this Flutter
//! fork. Plugins use `flutter-plugin-sdk`, never this crate directly.

// This crate owns the private C ABI, so exporting fixed C symbols is required.
// Keep unsafe operations forbidden by lint even though the `no_mangle` ABI
// attribute is explicitly marked unsafe in Rust edition 2024.
#![deny(unsafe_op_in_unsafe_fn)]

use flutter_plugin_sdk::PLUGIN_SDK_API_VERSION;
use std::ffi::c_void;

/// Version of the private Rust/C++ ABI.
pub const SHELL_ABI_VERSION: u32 = 2;

/// ABI information returned to C++ before it installs Rust callbacks.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlutterRustShellAbi {
    pub shell_abi_version: u32,
    pub plugin_sdk_api_version: u32,
}

/// Private callback table used to bind Flutter task scheduling to the Rust
/// host loop. It is ABI-compatible with `rust_bridge.h` and never exposed to
/// application plugins.
#[repr(C)]
pub struct FlutterRustTaskRunnerCallbacks {
    pub user_data: *mut c_void,
    pub schedule_task: Option<extern "C" fn(*mut c_void, *mut c_void, u64, u64)>,
    pub runs_tasks_on_current_thread: Option<extern "C" fn(*mut c_void) -> i32>,
    pub task_runner_destroyed: Option<extern "C" fn(*mut c_void)>,
}

/// Raw Vulkan objects borrowed from Rust/wgpu, ABI-compatible with
/// `FlutterRustVulkanContextData` in `rust_bridge.h`. The extension arrays are
/// borrowed only for the duration of the `FlutterRustShellCreateShell` call.
#[repr(C)]
pub struct FlutterRustVulkanContextData {
    pub get_instance_proc_addr: *mut c_void,
    pub instance: *mut c_void,
    pub physical_device: *mut c_void,
    pub device: *mut c_void,
    pub queue: *mut c_void,
    pub queue_family_index: u32,
    pub instance_extensions: *const *const std::ffi::c_char,
    pub instance_extensions_count: u32,
    pub device_extensions: *const *const std::ffi::c_char,
    pub device_extensions_count: u32,
}

/// A Vulkan swapchain image acquired by the Rust GPU broker, ABI-compatible
/// with `FlutterRustVulkanImage` in `rust_bridge.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FlutterRustVulkanImage {
    pub image: u64,
    pub format: u32,
    /// Binary semaphore signalled by wgpu after swapchain acquisition.
    pub acquire_semaphore: u64,
    /// Binary semaphore signalled by Impeller after its final image use.
    pub render_semaphore: u64,
}

/// ABI-compatible with `FlutterRustVulkanPresentationCallbacks`.
#[repr(C)]
pub struct FlutterRustVulkanPresentationCallbacks {
    pub user_data: *mut c_void,
    pub acquire_image:
        Option<extern "C" fn(*mut c_void, u32, u32, *mut FlutterRustVulkanImage) -> i32>,
    pub present_image: Option<extern "C" fn(*mut c_void, FlutterRustVulkanImage) -> i32>,
}

/// Paths borrowed only for the duration of the `FlutterRustShellCreateShell`
/// call; ABI-compatible with `FlutterRustShellSettings`.
#[repr(C)]
pub struct FlutterRustShellSettings {
    pub assets_path: *const std::ffi::c_char,
    pub icu_data_path: *const std::ffi::c_char,
}

/// Pointer phases accepted by the private engine bridge.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlutterRustPointerPhase {
    Cancel = 0,
    Add = 1,
    Remove = 2,
    Hover = 3,
    Down = 4,
    Move = 5,
    Up = 6,
}

/// Pointer device kinds accepted by the private engine bridge.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlutterRustPointerDeviceKind {
    Mouse = 0,
    Touch = 1,
}

/// Pointer signal kinds accepted by the private engine bridge.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlutterRustPointerSignalKind {
    None = 0,
    Scroll = 1,
}

/// One winit pointer event, ABI-compatible with `FlutterRustPointerEvent`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlutterRustPointerEvent {
    pub timestamp_micros: u64,
    pub phase: u32,
    pub device_kind: u32,
    pub signal_kind: u32,
    pub device: i64,
    pub physical_x: f64,
    pub physical_y: f64,
    pub scroll_delta_x: f64,
    pub scroll_delta_y: f64,
    pub buttons: i64,
}

/// Application lifecycle states accepted by the private engine bridge.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlutterRustLifecycleState {
    Detached = 0,
    Resumed = 1,
    Inactive = 2,
    Hidden = 3,
    Paused = 4,
}

/// Key event types accepted by the private engine bridge.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlutterRustKeyEventType {
    Down = 0,
    Up = 1,
    Repeat = 2,
}

/// Maximum UTF-8 payload carried inline by one private-ABI key event.
pub const FLUTTER_RUST_KEY_CHARACTER_CAPACITY: usize = 64;

/// One winit key event, ABI-compatible with `FlutterRustKeyEvent`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlutterRustKeyEvent {
    pub timestamp_micros: u64,
    pub event_type: u32,
    pub physical: u64,
    pub logical: u64,
    pub synthesized: i32,
    pub character_length: u32,
    pub character: [u8; FLUTTER_RUST_KEY_CHARACTER_CAPACITY],
}

/// Returns the ABI versions compiled into the Rust shell runtime.
#[unsafe(no_mangle)]
pub extern "C" fn FlutterRustShellGetAbi() -> FlutterRustShellAbi {
    FlutterRustShellAbi {
        shell_abi_version: SHELL_ABI_VERSION,
        plugin_sdk_api_version: PLUGIN_SDK_API_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_the_expected_abi_versions() {
        assert_eq!(
            FlutterRustShellGetAbi(),
            FlutterRustShellAbi {
                shell_abi_version: SHELL_ABI_VERSION,
                plugin_sdk_api_version: PLUGIN_SDK_API_VERSION,
            }
        );
    }
}
