//! Private Rust runtime core for the in-tree Flutter Rust shell.
//!
//! Its C ABI is intentionally small and lockstep-versioned with this Flutter
//! fork. Plugins use `flutter-plugin-sdk`, never this crate directly.

// This crate owns the private C ABI, so exporting fixed C symbols is required.
// Keep unsafe operations forbidden by lint even though the `no_mangle` ABI
// attribute is explicitly marked unsafe in Rust edition 2024.
#![deny(unsafe_op_in_unsafe_fn)]

use flutter_plugin_sdk::PLUGIN_SDK_API_VERSION;

/// Version of the private Rust/C++ ABI.
pub const SHELL_ABI_VERSION: u32 = 1;

/// ABI information returned to C++ before it installs Rust callbacks.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlutterRustShellAbi {
    pub shell_abi_version: u32,
    pub plugin_sdk_api_version: u32,
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
