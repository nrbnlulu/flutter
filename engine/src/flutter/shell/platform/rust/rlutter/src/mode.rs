//! Build modes and every mode-dependent decision in one place.
//!
//! Profile uses AOT with a VM service and an optimized native runner.

use anyhow::Result;
use clap::Args;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BuildMode {
    Debug,
    Release,
    Profile,
}

/// What a [`BuildMode`] means for each stage of a build or run.
#[derive(Copy, Clone, Debug)]
pub struct ModeConfig {
    /// `cargo build --release`.
    pub cargo_release: bool,
    /// Dart code is AOT-compiled to `app.so` instead of shipped as a kernel.
    pub dart_aot: bool,
    /// Dev-only Dart dependencies may provide Rust plugins.
    pub plugins_include_dev: bool,
    /// `rlutter run` attaches `flutter attach` (hot reload, DevTools).
    pub hot_reload: bool,
    pub vm_service: bool,
    /// Gradle task for the Android build.
    pub gradle_task: &'static str,
}

impl BuildMode {
    pub fn name(self) -> &'static str {
        match self {
            BuildMode::Debug => "debug",
            BuildMode::Release => "release",
            BuildMode::Profile => "profile",
        }
    }

    pub fn config(self) -> ModeConfig {
        match self {
            BuildMode::Debug => ModeConfig {
                cargo_release: false,
                dart_aot: false,
                plugins_include_dev: true,
                hot_reload: true,
                vm_service: true,
                gradle_task: "assembleDebug",
            },
            BuildMode::Profile => ModeConfig {
                cargo_release: true,
                dart_aot: true,
                plugins_include_dev: false,
                hot_reload: false,
                vm_service: true,
                gradle_task: "assembleProfile",
            },
            BuildMode::Release => ModeConfig {
                cargo_release: true,
                dart_aot: true,
                plugins_include_dev: false,
                hot_reload: false,
                vm_service: false,
                gradle_task: "assembleRelease",
            },
        }
    }

    /// Keep the debug runtime and debug assertions, but optimize native code.
    /// The generated runner is a workspace root: profiles from its shell SDK
    /// dependency are ignored by Cargo. Command-line config also fixes existing
    /// projects and independently built native plugin workspaces.
    pub fn cargo_args(self) -> &'static [&'static str] {
        if self == Self::Profile {
            &[
                "--profile",
                "flutter-profile",
                "--config",
                "profile.flutter-profile.inherits=\"release\"",
                "--config",
                "profile.flutter-profile.debug=true",
            ]
        } else if self.config().cargo_release {
            &["--release"]
        } else {
            &[
                "--config",
                "profile.dev.opt-level=1",
                "--config",
                "profile.dev.package.\"*\".opt-level=3",
            ]
        }
    }

    /// Directory Cargo puts artifacts in (`target/<this>/`).
    pub fn cargo_profile_dir(self) -> &'static str {
        if self == Self::Profile {
            "flutter-profile"
        } else {
            self.name()
        }
    }
}

#[derive(Args, Debug, Default, Clone)]
pub struct ModeArgs {
    /// Build a debug (JIT, hot reload) app. This is the default.
    #[arg(long, group = "mode")]
    debug: bool,
    /// Build a release (AOT) app.
    #[arg(long, group = "mode")]
    release: bool,
    /// Build a Linux profile (AOT with DevTools) app.
    #[arg(long, group = "mode")]
    profile: bool,
}

impl ModeArgs {
    pub fn resolve(&self) -> Result<BuildMode> {
        Ok(if self.profile {
            BuildMode::Profile
        } else if self.release {
            BuildMode::Release
        } else {
            BuildMode::Debug
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(debug: bool, release: bool, profile: bool) -> ModeArgs {
        ModeArgs {
            debug,
            release,
            profile,
        }
    }

    #[test]
    fn defaults_to_debug() {
        assert_eq!(
            args(false, false, false).resolve().unwrap(),
            BuildMode::Debug
        );
        assert_eq!(
            args(true, false, false).resolve().unwrap(),
            BuildMode::Debug
        );
    }

    #[test]
    fn release_is_aot_without_hot_reload() {
        let mode = args(false, true, false).resolve().unwrap();
        assert_eq!(mode, BuildMode::Release);
        let config = mode.config();
        assert!(config.cargo_release && config.dart_aot && !config.hot_reload);
        assert!(!config.plugins_include_dev);
    }

    #[test]
    fn debug_optimizes_native_code_without_switching_to_aot() {
        let mode = BuildMode::Debug;
        let config = mode.config();
        assert!(!config.cargo_release && !config.dart_aot && config.hot_reload);
        assert_eq!(mode.cargo_profile_dir(), "debug");
        assert_eq!(
            mode.cargo_args(),
            [
                "--config",
                "profile.dev.opt-level=1",
                "--config",
                "profile.dev.package.\"*\".opt-level=3",
            ]
        );
        assert_eq!(BuildMode::Release.cargo_args(), ["--release"]);
    }

    #[test]
    fn profile_is_aot_with_a_service_and_separate_native_outputs() {
        let mode = args(false, false, true).resolve().unwrap();
        assert_eq!(mode, BuildMode::Profile);
        let config = mode.config();
        assert!(config.dart_aot && config.vm_service && !config.hot_reload);
        assert!(!config.plugins_include_dev);
        assert_eq!(mode.cargo_profile_dir(), "flutter-profile");
    }
}
