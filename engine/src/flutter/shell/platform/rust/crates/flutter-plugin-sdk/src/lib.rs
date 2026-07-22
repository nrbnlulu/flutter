//! Public, semantically versioned Rust API for Flutter Rust-shell plugins.
//!
//! The SDK deliberately exposes no Flutter C++ or Impeller types. Those remain
//! behind the private, lockstep engine bridge.

#![forbid(unsafe_code)]

/// The source compatibility version of this SDK.
pub const PLUGIN_SDK_API_VERSION: u32 = 1;

/// Errors returned while registering a Rust-shell plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginError {
    /// The plugin requires an SDK capability not provided by this shell build.
    Unsupported,
}

/// A convenient result type for plugin registration.
pub type Result<T> = core::result::Result<T, PluginError>;

/// The shell-owned registration context passed to every plugin.
///
/// Capabilities are added here as the shell implements them. Keeping this type
/// opaque prevents plugins from depending on private engine handles.
#[derive(Default)]
pub struct PluginRegistrar {
    _private: (),
}

impl PluginRegistrar {
    /// Creates the registrar used by the shell during application startup.
    ///
    /// This is public only so the private shell runtime can construct the
    /// registrar across crate boundaries; plugin code should only receive it
    /// from [`FlutterRustPlugin::register`].
    #[doc(hidden)]
    pub fn for_shell() -> Self {
        Self { _private: () }
    }
}

/// A source-linked plugin compiled into the application's Rust aggregate.
pub trait FlutterRustPlugin: Send + Sync + 'static {
    /// Registers the plugin's platform services, FRB APIs, and textures.
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlugin;

    impl FlutterRustPlugin for TestPlugin {
        fn register(&self, _registrar: &mut PluginRegistrar) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn plugins_register_with_the_shell_registrar() {
        let mut registrar = PluginRegistrar::for_shell();
        TestPlugin.register(&mut registrar).unwrap();
    }
}
