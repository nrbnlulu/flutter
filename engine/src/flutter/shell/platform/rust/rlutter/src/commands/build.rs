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
    util::{copy_dir, copy_if_changed, retain_files, run},
};

pub struct DesktopBuild {
    pub project: Project,
    pub engine: Engine,
    pub runner: PathBuf,
}

/// Builds the app for the Linux host: the Dart bundle, the runner, and the
/// native plugin libraries, staged next to the runner so it is relocatable.
pub fn build_linux(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
) -> Result<DesktopBuild> {
    build_desktop(ctx, mode, target, dart_defines, "linux")
}

pub fn build_windows(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
) -> Result<DesktopBuild> {
    build_desktop(ctx, mode, target, dart_defines, "windows")
}

fn build_desktop(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
    platform: &str,
) -> Result<DesktopBuild> {
    if std::env::consts::OS != platform || !cfg!(target_arch = "x86_64") {
        bail!("{platform} desktop builds require a native {platform} x64 host");
    }
    let project = Project::discover()?;
    project.require_rust_shell()?;
    let engine = prepare(ctx, &project, mode)?;

    let output = project
        .build_dir()
        .join(platform)
        .join(mode.name())
        .join("flutter_assets");
    let bundle = dart::assemble_bundle(
        &ctx.flutter,
        &project,
        &engine,
        mode,
        &format!("{platform}-x64"),
        target,
        dart_defines,
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
    Ok(DesktopBuild {
        project,
        engine,
        runner,
    })
}

fn runner_cargo_command(project: &Project, mode: BuildMode) -> Command {
    let mut command = Command::new("cargo");
    command.env("FLUTTER_RS_BUILD_MODE", mode.name());
    command.arg("build");
    command.args(mode.cargo_args());
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
        .join(format!("{}{}", project.name, std::env::consts::EXE_SUFFIX))
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
    copy_if_changed(&engine.target.join("icudtl.dat"), &data.join("icudtl.dat"))
        .context("cannot copy icudtl.dat")?;

    // Keep Windows DLLs beside the executable; Linux uses its rpath directory.
    let libs = if cfg!(windows) {
        runner_dir.to_owned()
    } else {
        runner_dir.join("libs")
    };
    fs::create_dir_all(&libs)?;
    copy_if_changed(
        &engine.target.join(crate::sdk::host_engine_library()),
        &libs.join(crate::sdk::host_engine_library()),
    )
    .context("cannot copy libflutter_rust_engine.so")?;
    let mut keep = vec![libs.join(crate::sdk::host_engine_library())];
    let native = native_libs::staging_dir(project, mode, None);
    if native.is_dir() {
        for entry in fs::read_dir(&native)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let destination = libs.join(entry.file_name());
                copy_if_changed(&entry.path(), &destination)?;
                keep.push(destination);
            }
        }
    }
    if cfg!(windows) {
        // The executable directory also contains Cargo outputs and application data.
        // Prune only DLLs previously staged by this tool.
        let record = runner_dir.join(".rlutter-dlls.json");
        let previous: Vec<PathBuf> = match fs::read_to_string(&record) {
            Ok(s) => serde_json::from_str(&s).context("invalid staged DLL record")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).context("cannot read staged DLL record"),
        };
        for name in previous {
            if name.components().count() == 1 && name.extension().is_some_and(|ext| ext == "dll") {
                let path = runner_dir.join(name);
                if !keep.contains(&path) {
                    crate::util::remove_path(&path)?;
                }
            }
        }
        let names: Vec<PathBuf> = keep
            .iter()
            .filter_map(|p| p.file_name().map(PathBuf::from))
            .collect();
        fs::write(record, serde_json::to_vec(&names)?)?;
    } else {
        retain_files(&libs, &keep)?;
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
    fn profile_runner_selects_the_profile_engine_and_separate_output() {
        let p = project("build_profile");
        let command = runner_cargo_command(&p, BuildMode::Profile);
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "FLUTTER_RS_BUILD_MODE"
                    && value == Some(std::ffi::OsStr::new("profile")))
        );
        assert_eq!(
            runner_binary(&p, BuildMode::Profile),
            p.dir.join(format!(
                "runner-rs/target/flutter-profile/demo{}",
                std::env::consts::EXE_SUFFIX
            ))
        );
    }

    #[test]
    fn runner_binary_lives_in_the_cargo_profile_directory() {
        let p = project("build_binary");
        assert_eq!(
            runner_binary(&p, BuildMode::Debug),
            p.dir.join(format!(
                "runner-rs/target/debug/demo{}",
                std::env::consts::EXE_SUFFIX
            ))
        );
        assert_eq!(
            runner_binary(&p, BuildMode::Release),
            p.dir.join(format!(
                "runner-rs/target/release/demo{}",
                std::env::consts::EXE_SUFFIX
            ))
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
        let mut expected = vec!["build"];
        expected.extend(BuildMode::Debug.cargo_args());
        expected.push("--locked");
        assert_eq!(args(&runner_cargo_command(&p, BuildMode::Debug)), expected);
    }

    #[test]
    fn staging_lays_out_data_and_libs_next_to_the_runner() {
        let p = project("build_stage");
        let engine_dir = p.dir.join("engine");
        fs::create_dir_all(&engine_dir).unwrap();
        fs::write(engine_dir.join("icudtl.dat"), "icu").unwrap();
        fs::write(engine_dir.join(crate::sdk::host_engine_library()), "engine").unwrap();
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
        fs::write(native.join("plug.dll"), "p").unwrap();

        let runner = runner_binary(&p, BuildMode::Debug);
        fs::create_dir_all(runner.parent().unwrap()).unwrap();
        fs::write(&runner, "bin").unwrap();
        fs::write(runner.parent().unwrap().join("user.txt"), "keep").unwrap();

        stage_bundle(&p, BuildMode::Debug, &engine, &assets, &runner).unwrap();
        let dir = runner.parent().unwrap();
        let libs = if cfg!(windows) {
            dir.to_owned()
        } else {
            dir.join("libs")
        };
        let staged_engine = libs.join(crate::sdk::host_engine_library());
        let sentinel = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        fs::OpenOptions::new()
            .write(true)
            .open(&staged_engine)
            .unwrap()
            .set_modified(sentinel)
            .unwrap();
        fs::remove_file(native.join("plug.dll")).unwrap();
        stage_bundle(&p, BuildMode::Debug, &engine, &assets, &runner).unwrap();
        assert_eq!(
            fs::metadata(staged_engine).unwrap().modified().unwrap(),
            sentinel
        );
        assert!(!libs.join("plug.dll").exists());
        assert_eq!(fs::read_to_string(&runner).unwrap(), "bin");
        assert!(dir.join("user.txt").is_file());
        assert!(dir.join("data/flutter_assets/AssetManifest.bin").is_file());
        assert!(!dir.join("data/flutter_assets/.last_build_id").exists());
        assert_eq!(
            fs::read_to_string(dir.join("data/icudtl.dat")).unwrap(),
            "icu"
        );
        assert!(libs.join(crate::sdk::host_engine_library()).is_file());
    }
}
