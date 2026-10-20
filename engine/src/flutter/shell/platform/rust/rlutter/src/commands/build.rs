use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use super::{Ctx, prepare};
use crate::{
    dart,
    mode::BuildMode,
    native_libs,
    project::Project,
    sdk::Engine,
    util::{copy_dir, run},
};

pub struct LinuxBuild {
    pub project: Project,
    pub engine: Engine,
    pub runner: PathBuf,
}

/// Builds the app for the Linux host: the Dart bundle, the runner, and the
/// native plugin libraries, staged next to the runner so it is relocatable.
pub fn build_linux(ctx: &Ctx, mode: BuildMode, target: &str) -> Result<LinuxBuild> {
    let project = Project::discover()?;
    project.require_rust_shell()?;
    let engine = prepare(ctx, &project, mode)?;

    let output = project
        .build_dir()
        .join("linux")
        .join(mode.name())
        .join("flutter_assets");
    let bundle = dart::assemble_bundle(
        &ctx.flutter,
        &project,
        &engine,
        mode,
        "linux-x64",
        target,
        &output,
    )?;
    if let Some(app_so) = &bundle.app_so {
        // The runner loads the AOT library from the assets directory.
        fs::copy(app_so, bundle.assets.join("app.so"))?;
    }

    eprintln!("Building Rust shell runner...");
    run(&mut runner_cargo_command(&project, mode))?;
    native_libs::build_all(&project, mode, None)?;

    let runner = runner_binary(&project, mode);
    stage_bundle(&project, mode, &engine, &bundle.assets, &runner)?;
    eprintln!(
        "Built {}",
        runner
            .strip_prefix(&project.dir)
            .unwrap_or(&runner)
            .display()
    );
    Ok(LinuxBuild {
        project,
        engine,
        runner,
    })
}

fn runner_cargo_command(project: &Project, mode: BuildMode) -> Command {
    let mut command = Command::new("cargo");
    command.arg("build");
    if mode.config().cargo_release {
        command.arg("--release");
    }
    if project.runner_dir().join("Cargo.lock").is_file() {
        command.arg("--locked");
    }
    command.current_dir(project.runner_dir());
    command
}

pub fn runner_binary(project: &Project, mode: BuildMode) -> PathBuf {
    project
        .runner_dir()
        .join("target")
        .join(mode.cargo_profile_dir())
        .join(&project.name)
}

/// Copies `flutter_assets` and `icudtl.dat` into `<runner dir>/data/`, and the
/// engine and native plugin libraries into `<runner dir>/libs/`, so the runner
/// finds them relative to its own executable (build.rs adds `$ORIGIN/libs` to
/// its rpath).
fn stage_bundle(
    project: &Project,
    mode: BuildMode,
    engine: &Engine,
    assets: &Path,
    runner: &Path,
) -> Result<()> {
    let runner_dir = runner
        .parent()
        .context("runner binary has no parent directory")?;
    let data = runner_dir.join("data");
    copy_dir(assets, &data.join("flutter_assets"))?;
    // Only needed to locate app.so while building.
    let _ = fs::remove_file(data.join("flutter_assets").join(".last_build_id"));
    fs::copy(engine.target.join("icudtl.dat"), data.join("icudtl.dat"))
        .context("cannot copy icudtl.dat")?;

    let libs = runner_dir.join("libs");
    fs::create_dir_all(&libs)?;
    fs::copy(
        engine.target.join("libflutter_rust_engine.so"),
        libs.join("libflutter_rust_engine.so"),
    )
    .context("cannot copy libflutter_rust_engine.so")?;
    let native = native_libs::staging_dir(project, mode, None);
    if native.is_dir() {
        for entry in fs::read_dir(&native)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::copy(entry.path(), libs.join(entry.file_name()))?;
            }
        }
    }
    if !runner.is_file() {
        bail!("cargo did not produce {}", runner.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    fn project(name: &str) -> Project {
        let dir = tempdir(name);
        fs::write(dir.join("pubspec.yaml"), "name: demo\nrust-shell: true\n").unwrap();
        Project::load(&dir).unwrap()
    }

    #[test]
    fn runner_binary_lives_in_the_cargo_profile_directory() {
        let p = project("build_binary");
        assert_eq!(
            runner_binary(&p, BuildMode::Debug),
            p.dir.join("runner-rs/target/debug/demo")
        );
        assert_eq!(
            runner_binary(&p, BuildMode::Release),
            p.dir.join("runner-rs/target/release/demo")
        );
    }

    #[test]
    fn cargo_is_locked_only_when_a_lockfile_exists() {
        let p = project("build_locked");
        fs::create_dir_all(p.runner_dir()).unwrap();
        let args = |c: &Command| {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args(&runner_cargo_command(&p, BuildMode::Release)),
            ["build", "--release"]
        );
        fs::write(p.runner_dir().join("Cargo.lock"), "").unwrap();
        assert_eq!(
            args(&runner_cargo_command(&p, BuildMode::Debug)),
            ["build", "--locked"]
        );
    }

    #[test]
    fn staging_lays_out_data_and_libs_next_to_the_runner() {
        let p = project("build_stage");
        let engine_dir = p.dir.join("engine");
        fs::create_dir_all(&engine_dir).unwrap();
        fs::write(engine_dir.join("icudtl.dat"), "icu").unwrap();
        fs::write(engine_dir.join("libflutter_rust_engine.so"), "engine").unwrap();
        let engine = Engine {
            target: engine_dir.clone(),
            host: engine_dir,
        };

        let assets = p.dir.join("assets");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("AssetManifest.bin"), "m").unwrap();
        fs::write(assets.join(".last_build_id"), "x").unwrap();

        let native = native_libs::staging_dir(&p, BuildMode::Debug, None);
        fs::create_dir_all(&native).unwrap();
        fs::write(native.join("libplug.so"), "p").unwrap();

        let runner = runner_binary(&p, BuildMode::Debug);
        fs::create_dir_all(runner.parent().unwrap()).unwrap();
        fs::write(&runner, "bin").unwrap();

        stage_bundle(&p, BuildMode::Debug, &engine, &assets, &runner).unwrap();
        let dir = runner.parent().unwrap();
        assert!(dir.join("data/flutter_assets/AssetManifest.bin").is_file());
        assert!(!dir.join("data/flutter_assets/.last_build_id").exists());
        assert_eq!(
            fs::read_to_string(dir.join("data/icudtl.dat")).unwrap(),
            "icu"
        );
        assert!(dir.join("libs/libflutter_rust_engine.so").is_file());
        assert!(dir.join("libs/libplug.so").is_file());
    }
}
