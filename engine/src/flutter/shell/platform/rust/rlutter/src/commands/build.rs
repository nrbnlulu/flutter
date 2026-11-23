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
    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub assets: PathBuf,
}

enum AssetStaging {
    Bundled,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Agent,
}

/// Builds the app for the Linux host: the Dart bundle, the runner, and the
/// native plugin libraries, staged next to the runner so it is relocatable.
pub fn build_linux(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
) -> Result<DesktopBuild> {
    build_desktop(
        ctx,
        mode,
        target,
        dart_defines,
        "linux",
        Project::discover()?,
    )
}

pub fn build_macos(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
) -> Result<DesktopBuild> {
    build_desktop(
        ctx,
        mode,
        target,
        dart_defines,
        "macos",
        Project::discover()?,
    )
}

pub fn build_windows(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
) -> Result<DesktopBuild> {
    build_desktop(
        ctx,
        mode,
        target,
        dart_defines,
        "windows",
        Project::discover()?,
    )
}

pub(super) fn build_desktop(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
    platform: &str,
    project: Project,
) -> Result<DesktopBuild> {
    build_desktop_with_staging(
        ctx,
        mode,
        target,
        dart_defines,
        platform,
        project,
        AssetStaging::Bundled,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn build_desktop_agent(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
    platform: &str,
    project: Project,
) -> Result<DesktopBuild> {
    build_desktop_with_staging(
        ctx,
        mode,
        target,
        dart_defines,
        platform,
        project,
        AssetStaging::Agent,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn agent_asset_dirs(project: &Project, platform: &str, mode: BuildMode) -> (PathBuf, PathBuf) {
    let base = project
        .build_dir()
        .join(platform)
        .join(mode.name())
        .join("agent");
    // Assemble writes in place via Flutter's copySync; stage into a sibling
    // directory via atomic copy_dir so concurrent agent runs never truncate an
    // active mapping, and never touch `<runner dir>/data/flutter_assets`.
    (base.join("assemble"), base.join("flutter_assets"))
}

fn build_desktop_with_staging(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
    platform: &str,
    project: Project,
    staging: AssetStaging,
) -> Result<DesktopBuild> {
    let supported_arch = if platform == "macos" {
        cfg!(any(target_arch = "aarch64", target_arch = "x86_64"))
    } else {
        cfg!(target_arch = "x86_64")
    };
    if std::env::consts::OS != platform || !supported_arch {
        bail!("{platform} desktop builds require a native {platform} host");
    }
    project.require_rust_shell()?;
    let engine = prepare(ctx, &project, mode)?;

    let runner = runner_binary(&project, mode);
    let runner_dir = runner
        .parent()
        .context("runner binary has no parent directory")?;
    let (output, staged_assets) = match staging {
        AssetStaging::Bundled => (
            project
                .build_dir()
                .join(platform)
                .join(mode.name())
                .join("flutter_assets"),
            runner_dir.join("data").join("flutter_assets"),
        ),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        AssetStaging::Agent => agent_asset_dirs(&project, platform, mode),
    };
    let bundle = dart::assemble_bundle(
        &ctx.flutter,
        &project,
        &engine,
        mode,
        crate::sdk::host_platform(),
        target,
        dart_defines,
        &output,
    )?;
    if let Some(app_so) = &bundle.app_so {
        // The runner loads the AOT library from the assets directory.
        copy_if_changed(app_so, &bundle.assets.join("app.so"))?;
    }

    eprintln!("Building Rust shell runner...");
    run(&mut runner_cargo_command(&project, mode))?;
    native_libs::build_all(&project, mode, None)?;

    stage_bundle(
        &project,
        mode,
        &engine,
        &bundle.assets,
        &runner,
        &staged_assets,
    )?;
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
        assets: staged_assets,
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

/// Copies `flutter_assets` into `staged_assets`, `icudtl.dat` into
/// `<runner dir>/data/`, and the engine and native plugin libraries into
/// `<runner dir>/libs/`, so the runner finds them relative to its own
/// executable (build.rs adds `$ORIGIN/libs` to its rpath).
fn stage_bundle(
    project: &Project,
    mode: BuildMode,
    engine: &Engine,
    assets: &Path,
    runner: &Path,
    staged_assets: &Path,
) -> Result<()> {
    let runner_dir = runner
        .parent()
        .context("runner binary has no parent directory")?;
    let data = runner_dir.join("data");
    copy_dir(assets, staged_assets)?;
    // Only needed to locate app.so while building.
    let _ = fs::remove_file(staged_assets.join(".last_build_id"));
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

        let dir = runner.parent().unwrap();
        let bundled_assets = dir.join("data/flutter_assets");
        stage_bundle(
            &p,
            BuildMode::Debug,
            &engine,
            &assets,
            &runner,
            &bundled_assets,
        )
        .unwrap();
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
        stage_bundle(
            &p,
            BuildMode::Debug,
            &engine,
            &assets,
            &runner,
            &bundled_assets,
        )
        .unwrap();
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

    #[test]
    fn isolated_agent_staging_preserves_bundled_flutter_assets() {
        let p = project("build_stage_agent");
        let engine_dir = p.dir.join("engine");
        fs::create_dir_all(&engine_dir).unwrap();
        fs::write(engine_dir.join("icudtl.dat"), "icu").unwrap();
        fs::write(engine_dir.join(crate::sdk::host_engine_library()), "engine").unwrap();
        let engine = Engine {
            target: engine_dir.clone(),
            host: engine_dir,
        };

        let runner = runner_binary(&p, BuildMode::Debug);
        let dir = runner.parent().unwrap();
        fs::create_dir_all(dir).unwrap();
        fs::write(&runner, "bin").unwrap();

        let app_assets = p.dir.join("app_assets");
        fs::create_dir_all(&app_assets).unwrap();
        fs::write(app_assets.join("kernel_blob.bin"), "interactive-app").unwrap();
        let bundled_assets = dir.join("data/flutter_assets");
        stage_bundle(
            &p,
            BuildMode::Debug,
            &engine,
            &app_assets,
            &runner,
            &bundled_assets,
        )
        .unwrap();

        let (assemble, agent_assets) = agent_asset_dirs(&p, "linux", BuildMode::Debug);
        assert_ne!(assemble, agent_assets);
        assert_ne!(agent_assets, bundled_assets);
        fs::create_dir_all(&assemble).unwrap();
        fs::write(assemble.join("kernel_blob.bin"), "agent-smoke-target").unwrap();
        fs::write(assemble.join(".last_build_id"), "id").unwrap();
        stage_bundle(
            &p,
            BuildMode::Debug,
            &engine,
            &assemble,
            &runner,
            &agent_assets,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(bundled_assets.join("kernel_blob.bin")).unwrap(),
            "interactive-app"
        );
        assert_eq!(
            fs::read_to_string(agent_assets.join("kernel_blob.bin")).unwrap(),
            "agent-smoke-target"
        );
        assert!(!agent_assets.join(".last_build_id").exists());
        assert_eq!(
            fs::read_to_string(dir.join("data/icudtl.dat")).unwrap(),
            "icu"
        );
    }
}
