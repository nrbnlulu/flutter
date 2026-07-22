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
pub const SHELL_ABI_VERSION: u32 = 1;

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
