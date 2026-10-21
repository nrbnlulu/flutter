//! Build modes and every mode-dependent decision in one place.
//!
//! Profile mode is parsed (so the flag is stable) but rejected; adding it later
//! means adding a [`BuildMode`] variant and a [`ModeConfig`] row.

use anyhow::{Result, bail};
use clap::Args;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BuildMode {
    Debug,
    Release,
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
    /// Gradle task for the Android build.
    pub gradle_task: &'static str,
}

impl BuildMode {
    pub fn name(self) -> &'static str {
        match self {
            BuildMode::Debug => "debug",
            BuildMode::Release => "release",
        }
    }

    pub fn config(self) -> ModeConfig {
        match self {
            BuildMode::Debug => ModeConfig {
                cargo_release: false,
                dart_aot: false,
                plugins_include_dev: true,
                hot_reload: true,
                gradle_task: "assembleDebug",
            },
            BuildMode::Release => ModeConfig {
                cargo_release: true,
                dart_aot: true,
                plugins_include_dev: false,
                hot_reload: false,
                gradle_task: "assembleRelease",
            },
        }
    }

    /// Keep the debug runtime and debug assertions, but optimize native code.
    /// The generated runner is a workspace root: profiles from its shell SDK
    /// dependency are ignored by Cargo. Command-line config also fixes existing
    /// projects and independently built native plugin workspaces.
    pub fn cargo_args(self) -> &'static [&'static str] {
        if self.config().cargo_release {
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
        self.name()
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
    /// Not supported by the Rust shell yet.
    #[arg(long, group = "mode", hide = true)]
    profile: bool,
}

impl ModeArgs {
    pub fn resolve(&self) -> Result<BuildMode> {
        if self.profile {
            bail!("profile mode is not supported by the Rust shell yet; use --debug or --release");
        }
        Ok(if self.release {
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
    fn profile_is_rejected_with_a_clear_message() {
        let error = args(false, false, true).resolve().unwrap_err().to_string();
        assert!(error.contains("profile mode is not supported"), "{error}");
    }
}
