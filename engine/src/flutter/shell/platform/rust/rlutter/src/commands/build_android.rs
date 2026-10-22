use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use super::Ctx;
use crate::{
    abi::{Abi, SUPPORTED_ABIS},
    android::Toolchain,
    android_project, android_runner, dart,
    mode::BuildMode,
    native_libs,
    project::Project,
    sdk::{self, Engine},
    util::{copy_dir, remove_path, run},
};

pub struct AndroidBuild {
    pub project: Project,
    pub apk: PathBuf,
    pub application_id: String,
    /// Engine layout of the first built ABI, for `flutter attach`.
    pub engine: Engine,
}

/// Everything the APK needs for one ABI.
struct AbiOutput {
    abi: Abi,
    engine: Engine,
    /// The compiled Dart assets (identical for every ABI).
    assets: PathBuf,
    /// The runner cdylib.
    runner_lib: PathBuf,
    /// AOT-compiled app (release only).
    app_so: Option<PathBuf>,
    native_libs: Vec<PathBuf>,
}

pub fn build_android(
    ctx: &Ctx,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
    abis: &[Abi],
    org: &str,
) -> Result<AndroidBuild> {
    check_abis(abis)?;
    let project = Project::discover()?;
    project.require_rust_shell()?;
    let toolchain = Toolchain::discover()?;

    super::prepare_sources(ctx, &project, mode, |project| {
        android_runner::ensure(project)?;
        android_project::ensure(project, org)?;
        Ok(())
    })?;
    let host = sdk::link_host(&project, &ctx.flutter, &ctx.sources, mode)?;

    let mut outputs: Vec<AbiOutput> = Vec::new();
    for &abi in abis {
        let engine = sdk::link_android(&project, &ctx.flutter, &ctx.sources, mode, abi, &host)?;
        outputs.push(build_abi(
            ctx,
            &project,
            &toolchain,
            mode,
            target,
            dart_defines,
            abi,
            engine,
        )?);
    }

    let stage = stage_dir(&project, mode);
    stage_android(&project, &stage, &outputs)?;
    run_gradle(&project, &toolchain, mode, &stage)?;

    let apk = android_project::apk_path(&project, mode.name());
    if !apk.is_file() {
        bail!("Gradle did not produce {}", apk.display());
    }
    eprintln!(
        "Built {}",
        apk.strip_prefix(&project.dir).unwrap_or(&apk).display()
    );
    let application_id = android_project::application_id(&project)?;
    let engine = outputs.swap_remove(0).engine;
    Ok(AndroidBuild {
        project,
        apk,
        application_id,
        engine,
    })
}

fn check_abis(abis: &[Abi]) -> Result<()> {
    if abis.is_empty() {
        bail!("no target platform given");
    }
    for abi in abis {
        if !abi.is_supported() {
            let supported: Vec<_> = SUPPORTED_ABIS
                .iter()
                .map(|a| a.flutter_platform())
                .collect();
            bail!(
                "{} is not supported yet: there are no Rust-shell engine artifacts for it. \
                 Supported: {}",
                abi.flutter_platform(),
                supported.join(", ")
            );
        }
    }
    Ok(())
}

pub fn stage_dir(project: &Project, mode: BuildMode) -> PathBuf {
    project.flutter_rs_dir().join("android").join(mode.name())
}

fn build_abi(
    ctx: &Ctx,
    project: &Project,
    toolchain: &Toolchain,
    mode: BuildMode,
    target: &str,
    dart_defines: &[String],
    abi: Abi,
    engine: Engine,
) -> Result<AbiOutput> {
    // The kernel is the same for every ABI; release also AOT-compiles per ABI.
    let bundle_dir = project.build_dir().join("android").join(mode.name());
    let bundle_dir = if mode.config().dart_aot {
        bundle_dir.join(abi.android_name())
    } else {
        bundle_dir
    };
    let bundle = dart::assemble_bundle(
        &ctx.flutter,
        project,
        &engine,
        mode,
        abi.flutter_platform(),
        target,
        dart_defines,
        &bundle_dir.join("flutter_assets"),
    )?;

    eprintln!("Building Rust shell runner for {}...", abi.android_name());
    run(&mut runner_cargo_command(project, toolchain, mode, abi))?;
    let runner_lib = project
        .runner_dir()
        .join("target")
        .join(abi.rust_triple())
        .join(mode.cargo_profile_dir())
        .join(format!("lib{}.so", project.name));
    if !runner_lib.is_file() {
        bail!("cargo did not produce {}", runner_lib.display());
    }
    let native_libs = native_libs::build_all(project, mode, Some((abi, toolchain)))?;
    Ok(AbiOutput {
        abi,
        engine,
        assets: bundle.assets,
        runner_lib,
        app_so: bundle.app_so,
        native_libs,
    })
}

/// `cargo rustc --crate-type cdylib` builds the runner as a shared library for
/// Android only, leaving its `Cargo.toml` (and Linux builds) alone.
fn runner_cargo_command(
    project: &Project,
    toolchain: &Toolchain,
    mode: BuildMode,
    abi: Abi,
) -> Command {
    let mut command = Command::new("cargo");
    command.args([
        "rustc",
        "--lib",
        "--crate-type",
        "cdylib",
        "--target",
        abi.rust_triple(),
    ]);
    command.args(mode.cargo_args());
    command.envs(toolchain.cargo_env(abi));
    command.current_dir(project.runner_dir());
    command
}

/// Lays out what Gradle packages:
///
/// ```text
/// <stage>/jniLibs/<abi>/{lib<name>.so, libflutter_rust_engine.so, libapp.so, plugins}
/// <stage>/assets/{flutter_assets/, icudtl.dat, rlutter_assets.txt, rlutter_build_id}
/// ```
fn stage_android(project: &Project, stage: &Path, outputs: &[AbiOutput]) -> Result<()> {
    let first = outputs.first().context("no ABI was built")?;

    remove_path(stage)?;
    for output in outputs {
        let libs = stage.join("jniLibs").join(output.abi.android_name());
        fs::create_dir_all(&libs)?;
        fs::copy(
            &output.runner_lib,
            libs.join(format!("lib{}.so", project.name)),
        )?;
        fs::copy(
            output.engine.target.join("libflutter_rust_engine.so"),
            libs.join("libflutter_rust_engine.so"),
        )
        .context("cannot copy libflutter_rust_engine.so")?;
        if let Some(app_so) = &output.app_so {
            fs::copy(app_so, libs.join("libapp.so"))?;
        }
        for native in &output.native_libs {
            fs::copy(
                native,
                libs.join(native.file_name().context("native library has no name")?),
            )?;
        }
        // Code assets from build hooks (`DynamicLoadingBundled`) land in
        // `native_assets/jniLibs/lib/<abi>/`; the Flutter Gradle plugin would move them into
        // the APK's `lib/<abi>/`, so do the same or `dlopen` can't find them.
        let hook_libs = output
            .assets
            .join("native_assets/jniLibs/lib")
            .join(output.abi.android_name());
        if hook_libs.is_dir() {
            for entry in fs::read_dir(&hook_libs)? {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "so") {
                    let dest = libs.join(path.file_name().context("native library has no name")?);
                    if !dest.exists() {
                        fs::copy(&path, dest)?;
                    }
                }
            }
        }
    }

    let assets = stage.join("assets");
    let flutter_assets = assets.join("flutter_assets");
    copy_dir(&first.assets, &flutter_assets)?;
    // Already staged under jniLibs/; don't package the libraries twice.
    let _ = fs::remove_dir_all(flutter_assets.join("native_assets/jniLibs"));
    // Build bookkeeping, and the AOT library, which ships as libapp.so.
    let _ = fs::remove_file(flutter_assets.join(".last_build_id"));
    let _ = fs::remove_file(flutter_assets.join("app.so"));
    fs::copy(
        first.engine.target.join("icudtl.dat"),
        assets.join("icudtl.dat"),
    )
    .context("cannot copy icudtl.dat")?;
    fs::write(
        assets.join("rlutter_assets.txt"),
        asset_list(&flutter_assets)?,
    )?;
    fs::write(assets.join("rlutter_build_id"), build_id())?;
    Ok(())
}

/// Every file under `dir`, relative and `/`-separated, one per line.
fn asset_list(dir: &Path) -> Result<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                walk(base, &entry.path(), out)?;
            } else {
                let relative = entry.path().strip_prefix(base)?.to_owned();
                out.push(
                    relative
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    Ok(files.join("\n") + "\n")
}

/// Changes on every staging, so the app re-extracts its assets after each install.
fn build_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

fn run_gradle(
    project: &Project,
    toolchain: &Toolchain,
    mode: BuildMode,
    stage: &Path,
) -> Result<()> {
    let mut command = gradle_command(project, toolchain, mode, stage);
    run(&mut command)
}

fn gradle_command(
    project: &Project,
    toolchain: &Toolchain,
    mode: BuildMode,
    stage: &Path,
) -> Command {
    let dir = android_project::android_dir(project);
    let mut command = Command::new(dir.join("gradlew"));
    command
        .arg(mode.config().gradle_task)
        .arg(format!("-PrlutterStageDir={}", stage.display()))
        .env("ANDROID_HOME", &toolchain.sdk)
        .current_dir(dir);
    if let Some(java_home) = &toolchain.java_home {
        command.env("JAVA_HOME", java_home);
    }
    command
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

    fn toolchain() -> Toolchain {
        Toolchain {
            sdk: "/sdk".into(),
            ndk: "/ndk".into(),
            java_home: Some("/jbr".into()),
        }
    }

    #[test]
    fn unsupported_abis_are_refused_with_the_supported_list() {
        assert!(check_abis(&[Abi::Arm64V8a]).is_ok());
        let error = check_abis(&[Abi::Arm64V8a, Abi::X86_64])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("android-x64") && error.contains("Supported: android-arm64"),
            "{error}"
        );
        assert!(check_abis(&[]).is_err());
    }

    #[test]
    fn runner_is_built_as_a_cdylib_for_the_abi() {
        let p = project("android_cargo");
        let command = runner_cargo_command(&p, &toolchain(), BuildMode::Release, Abi::Arm64V8a);
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "rustc",
                "--lib",
                "--crate-type",
                "cdylib",
                "--target",
                "aarch64-linux-android",
                "--release"
            ]
        );
        assert!(
            command
                .get_envs()
                .any(|(k, _)| k == "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER")
        );
        assert_eq!(command.get_current_dir(), Some(p.runner_dir().as_path()));
    }

    #[test]
    fn gradle_gets_the_stage_dir_sdk_and_jdk() {
        let p = project("android_gradle");
        let stage = Path::new("/stage/debug");
        let command = gradle_command(&p, &toolchain(), BuildMode::Debug, stage);
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["assembleDebug", "-PrlutterStageDir=/stage/debug"]);
        let env: std::collections::HashMap<_, _> = command
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert_eq!(env["ANDROID_HOME"], "/sdk");
        assert_eq!(env["JAVA_HOME"], "/jbr");
    }

    #[test]
    fn asset_list_is_sorted_relative_and_slash_separated() {
        let dir = tempdir("android_asset_list");
        fs::create_dir_all(dir.join("fonts/sub")).unwrap();
        fs::write(dir.join("b.txt"), "").unwrap();
        fs::write(dir.join("a.txt"), "").unwrap();
        fs::write(dir.join("fonts/sub/f.ttf"), "").unwrap();
        assert_eq!(asset_list(&dir).unwrap(), "a.txt\nb.txt\nfonts/sub/f.ttf\n");
    }

    #[test]
    fn staging_lays_out_libs_and_assets_for_gradle() {
        let p = project("android_stage");
        let engine_dir = p.dir.join("engine");
        fs::create_dir_all(&engine_dir).unwrap();
        fs::write(engine_dir.join("libflutter_rust_engine.so"), "engine").unwrap();
        fs::write(engine_dir.join("icudtl.dat"), "icu").unwrap();
        let runner = p.dir.join("libdemo.so");
        fs::write(&runner, "runner").unwrap();
        let app_so = p.dir.join("app.so");
        fs::write(&app_so, "aot").unwrap();
        let plugin = p.dir.join("libplug.so");
        fs::write(&plugin, "plug").unwrap();

        let bundle = p.dir.join("bundle");
        fs::create_dir_all(bundle.join("fonts")).unwrap();
        fs::write(bundle.join("AssetManifest.bin"), "m").unwrap();
        fs::write(bundle.join("fonts/a.ttf"), "f").unwrap();
        fs::write(bundle.join(".last_build_id"), "x").unwrap();
        fs::write(bundle.join("app.so"), "dup").unwrap();

        let stage = stage_dir(&p, BuildMode::Release);
        let output = AbiOutput {
            abi: Abi::Arm64V8a,
            assets: bundle,
            engine: Engine {
                target: engine_dir.clone(),
                host: engine_dir,
            },
            runner_lib: runner,
            app_so: Some(app_so),
            native_libs: vec![plugin],
        };
        stage_android(&p, &stage, &[output]).unwrap();

        let libs = stage.join("jniLibs/arm64-v8a");
        for lib in [
            "libdemo.so",
            "libflutter_rust_engine.so",
            "libapp.so",
            "libplug.so",
        ] {
            assert!(libs.join(lib).is_file(), "{lib}");
        }
        let assets = stage.join("assets");
        assert!(assets.join("flutter_assets/fonts/a.ttf").is_file());
        assert!(!assets.join("flutter_assets/.last_build_id").exists());
        assert!(!assets.join("flutter_assets/app.so").exists());
        assert_eq!(
            fs::read_to_string(assets.join("icudtl.dat")).unwrap(),
            "icu"
        );
        assert_eq!(
            fs::read_to_string(assets.join("rlutter_assets.txt")).unwrap(),
            "AssetManifest.bin\nfonts/a.ttf\n"
        );
        assert!(
            !fs::read_to_string(assets.join("rlutter_build_id"))
                .unwrap()
                .trim()
                .is_empty()
        );
    }
}
