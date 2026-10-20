use anyhow::Result;

use super::Ctx;
use crate::{
    cli::CleanArgs,
    project::Project,
    util::{remove_path, run},
};

pub fn run_clean(ctx: &Ctx, args: &CleanArgs) -> Result<()> {
    let project = Project::discover()?;
    // Keep `.dart_tool/flutter_rs/sdk`: it is expensive to download again.
    let flutter_rs = project.flutter_rs_dir();
    for path in [
        project.runner_dir().join("target"),
        project.dir.join("build"),
        flutter_rs.join("native_libs"),
        flutter_rs.join("native_libs.json"),
        flutter_rs.join("plugins"),
        flutter_rs.join("android"),
        project.dir.join("android-rs").join("build"),
        project.dir.join("android-rs").join("app").join("build"),
        project.dir.join("android-rs").join(".gradle"),
    ] {
        remove_path(&path)?;
    }
    if args.all {
        let mut command = ctx.flutter.command(None);
        command.arg("clean").current_dir(&project.dir);
        run(&mut command)?;
    }
    eprintln!("Cleaned {}.", project.name);
    Ok(())
}
