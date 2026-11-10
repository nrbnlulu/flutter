pub mod agent;
pub mod build;
pub mod build_android;
pub mod clean;
pub mod create;
pub mod mcp;
mod provenance;
pub mod run;
pub mod verify;
pub mod vscode;

use anyhow::Result;

use crate::{
    flutter::Flutter,
    mode::BuildMode,
    plugins,
    project::Project,
    sdk::{self, Engine, EngineSources},
    util::run,
};

pub struct Ctx {
    pub flutter: Flutter,
    pub sources: EngineSources,
}

/// Resolves Dart packages, makes the shell crates available and brings the
/// runner project in line with the plugins. `adapt_runner` runs after the
/// shell crates exist but before plugins are refreshed (the Android builds use
/// it to prepare the runner).
pub fn prepare_sources(
    ctx: &Ctx,
    project: &Project,
    mode: BuildMode,
    adapt_runner: impl FnOnce(&Project) -> Result<()>,
) -> Result<()> {
    run(&mut ctx.flutter.pub_get(&project.dir))?;
    sdk::ensure_sources(project, &ctx.flutter, &ctx.sources)?;
    let update_hal = crate::android_runner::ensure_desktop(project)?;
    adapt_runner(project)?;
    plugins::refresh(project, mode)?;
    if update_hal {
        // Cargo resolves plugin paths even for a targeted HAL update. Refresh
        // plugin dependencies and symlinks before asking it to read the graph.
        run(std::process::Command::new("cargo")
            .args(["update", "--offline", "-p", "wgpu-hal"])
            .current_dir(project.runner_dir()))?;
    }
    Ok(())
}

/// Everything a desktop build needs before compiling: [`prepare_sources`] and the
/// host engine layout.
pub fn prepare(ctx: &Ctx, project: &Project, mode: BuildMode) -> Result<Engine> {
    prepare_sources(ctx, project, mode, |_| Ok(()))?;
    sdk::link_host(project, &ctx.flutter, &ctx.sources, mode)
}
