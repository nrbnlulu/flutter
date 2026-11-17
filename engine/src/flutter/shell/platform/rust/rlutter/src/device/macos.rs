use super::{Device, RunningApp};
use crate::{
    commands::{Ctx, build::build_macos},
    mode::BuildMode,
};
use anyhow::Result;

/// The Mac rlutter runs on (Apple Silicon or Intel).
pub struct MacosHost;
impl MacosHost {
    pub fn is_supported() -> bool {
        cfg!(all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        ))
    }
}
impl Device for MacosHost {
    fn id(&self) -> String {
        "macos".into()
    }
    fn platform(&self) -> &'static str {
        "macos"
    }
    fn description(&self) -> String {
        format!(
            "macOS desktop (Rust shell), {}",
            crate::sdk::host_platform()
        )
    }
    fn flutter_device_id(&self) -> String {
        "macos".into()
    }
    fn launch(
        &self,
        ctx: &Ctx,
        mode: BuildMode,
        target: &str,
        dart_defines: &[String],
    ) -> Result<RunningApp> {
        super::linux::launch_desktop(build_macos(ctx, mode, target, dart_defines)?, mode)
    }
}
