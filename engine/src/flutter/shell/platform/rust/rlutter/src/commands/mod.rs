pub mod build;
pub mod build_android;
pub mod clean;
pub mod create;
pub mod run;

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
    adapt_runner(project)?;
    plugins::refresh(project, mode)?;
    Ok(())
}

/// Everything a Linux build needs before compiling: [`prepare_sources`] and the
/// host engine layout.
pub fn prepare(ctx: &Ctx, project: &Project, mode: BuildMode) -> Result<Engine> {
    prepare_sources(ctx, project, mode, |_| Ok(()))?;
    sdk::link_host(project, &ctx.flutter, &ctx.sources, mode)
}
