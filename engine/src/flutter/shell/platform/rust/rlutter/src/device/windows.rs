use anyhow::Result;
use super::{Device, RunningApp};
use crate::{commands::{Ctx, build::build_windows}, mode::BuildMode};

pub struct WindowsHost;
impl WindowsHost {
    pub fn is_supported() -> bool { cfg!(all(target_os = "windows", target_arch = "x86_64")) }
}
impl Device for WindowsHost {
    fn id(&self) -> String { "windows".into() }
    fn platform(&self) -> &'static str { "windows" }
    fn description(&self) -> String { "Windows desktop (Rust shell), windows-x64".into() }
    fn flutter_device_id(&self) -> String { "windows".into() }
    fn launch(&self, ctx: &Ctx, mode: BuildMode, target: &str, dart_defines: &[String]) -> Result<RunningApp> {
        super::linux::launch_desktop(build_windows(ctx, mode, target, dart_defines)?, mode)
    }
}
