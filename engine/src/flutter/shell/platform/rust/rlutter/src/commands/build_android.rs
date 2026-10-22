use super::Ctx;
use crate::{
    abi::{Abi, SUPPORTED_ABIS},
    android::Toolchain,
    android_project, android_runner, dart,
    mode::BuildMode,
    native_libs, plugins,
    project::Project,
    sdk::{self, Engine},
    util::{remove_path, run, write_if_changed},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub struct AndroidBuild {
    pub project: Project,
    pub apk: PathBuf,
    pub application_id: String,
    pub engine: Engine,
}

struct AbiOutput {
    abi: Abi,
    engine: Engine,
    runner_lib: PathBuf,
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
    // Flutter's pub get discovers Android plugins only for projects with an
    // android/ directory. Make the generated Android project visible first.
    android_project::ensure(&project, org)?;
    super::prepare_sources(ctx, &project, mode, |project| {
        android_runner::ensure(project)?;
        Ok(())
    })?;
    android_project::ensure_local_properties(&project, ctx.flutter.root(), &toolchain.sdk)?;
    android_project::sync_plugin_registrant(&project)?;
    let host = sdk::link_host(&project, &ctx.flutter, &ctx.sources, mode)?;
    let mut outputs = Vec::new();
    for &abi in abis {
        let engine = sdk::link_android(&project, &ctx.flutter, &ctx.sources, mode, abi, &host)?;
        eprintln!(
            "Building Rust Android plugins for {}...",
            abi.android_name()
        );
        run(&mut runner_cargo_command(&project, &toolchain, mode, abi))?;
        let runner_lib = project
            .runner_dir()
            .join("target")
            .join(abi.rust_triple())
            .join(mode.cargo_profile_dir())
            .join(format!("lib{}.so", project.name));
        if !runner_lib.is_file() {
            bail!("cargo did not produce {}", runner_lib.display());
        }
        let native_libs = native_libs::build_all(&project, mode, Some((abi, &toolchain)))?;
        outputs.push(AbiOutput {
            abi,
            engine,
            runner_lib,
            native_libs,
        });
    }
    let stage = stage_dir(&project, mode);
    stage_android(&project, &stage, &outputs)?;
    let prebuilt_native_script = write_prebuilt_native_script(&project, &stage)?;
    let engine = &outputs[0].engine;
    let repo = sdk::android_maven_repository(&project, engine, mode, abis[0])?;
    let mut command = gradle_command(&project, &toolchain, mode, &stage);
    if let Some(script) = prebuilt_native_script {
        command.arg("--init-script").arg(script);
    }
    command
        .arg(format!(
            "-PrlutterFlutterRoot={}",
            ctx.flutter.root().display()
        ))
        .arg(format!("-Plocal-engine-repo={}", repo.display()))
        .arg(format!("-Plocal-engine-build-mode={}", mode.name()))
        .arg(format!("-Plocal-engine-out={}", engine.target.display()))
        .arg(format!("-Plocal-engine-host-out={}", engine.host.display()))
        .arg(format!(
            "-Ptarget-platform={}",
            abis.iter()
                .map(|abi| abi.flutter_platform())
                .collect::<Vec<_>>()
                .join(",")
        ))
        .arg(format!("-Ptarget={target}"))
        .arg(format!(
            "-Pdart-defines={}",
            dart::encode_dart_defines(dart_defines)
        ))
        // An installed standalone SDK must never discover another Flutter on PATH.
        .env("FLUTTER_ROOT", ctx.flutter.root());
    run(&mut command)?;
    let apk = android_project::apk_path(&project, mode.name());
    if !apk.is_file() {
        bail!("Gradle did not produce {}", apk.display());
    }
    eprintln!(
        "Built {}",
        apk.strip_prefix(&project.dir).unwrap_or(&apk).display()
    );
    let application_id = android_project::application_id(&project)?;
    Ok(AndroidBuild {
        project,
        apk,
        application_id,
        engine: outputs.swap_remove(0).engine,
    })
}

fn check_abis(abis: &[Abi]) -> Result<()> {
    if abis.is_empty() {
        bail!("no target platform given");
    }
    for abi in abis {
        if !abi.is_supported() {
            bail!(
                "{} is not supported yet: there are no Rust-shell engine artifacts for it. Supported: {}",
                abi.flutter_platform(),
                SUPPORTED_ABIS
                    .iter()
                    .map(|a| a.flutter_platform())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}
pub fn stage_dir(project: &Project, mode: BuildMode) -> PathBuf {
    project.flutter_rs_dir().join("android").join(mode.name())
}
fn runner_cargo_command(
    project: &Project,
    toolchain: &Toolchain,
    mode: BuildMode,
    abi: Abi,
) -> Command {
    let mut command = Command::new("cargo");
    command
        .args([
            "rustc",
            "--lib",
            "--crate-type",
            "cdylib",
            "--target",
            abi.rust_triple(),
        ])
        .args(mode.cargo_args())
        .envs(toolchain.cargo_env(abi))
        .current_dir(project.runner_dir());
    command
}
fn stage_android(project: &Project, stage: &Path, outputs: &[AbiOutput]) -> Result<()> {
    remove_path(stage)?;
    for output in outputs {
        let libs = stage.join("jniLibs").join(output.abi.android_name());
        fs::create_dir_all(&libs)?;
        fs::copy(
            &output.runner_lib,
            libs.join(format!("lib{}.so", project.name)),
        )?;
        for native in &output.native_libs {
            fs::copy(
                native,
                libs.join(native.file_name().context("native library has no name")?),
            )?;
        }
    }
    // Flutter's Gradle plugin supplies libflutter.so, Dart assets/AOT and build
    // hook libraries. Staging those again would package duplicates or two VMs.
    Ok(())
}

/// CargoKit's Gradle task repeats native Rust builds that rlutter has already
/// staged. In particular, its Dart build tool cannot run inside the pub cache.
fn write_prebuilt_native_script(project: &Project, stage: &Path) -> Result<Option<PathBuf>> {
    let packages: BTreeSet<_> = plugins::read_native_libraries(project)?
        .into_iter()
        .map(|library| library.dart_package)
        .collect();
    if packages.is_empty() {
        return Ok(None);
    }
    let names = packages
        .iter()
        .map(|name| format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "// rlutter already staged these packages' Rust libraries.\n\
         def rlutterPrebuiltNativeProjects = [{names}]\n\
         gradle.beforeProject {{ project ->\n\
             if (rlutterPrebuiltNativeProjects.contains(project.name)) {{\n\
                 project.tasks.configureEach {{ task ->\n\
                     if (task.name.startsWith('cargokitCargoBuild')) task.enabled = false\n\
                 }}\n\
             }}\n\
         }}\n"
    );
    let path = stage.join("prebuilt-native.gradle");
    write_if_changed(&path, &script)?;
    Ok(Some(path))
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
        // The included Flutter Gradle build otherwise compiles against its
        // newest AGP API, which may differ from this project's pinned AGP.
        .arg("-Pflutter.internal.agpVersion=9.3.1")
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
        let error = check_abis(&[Abi::X86_64]).unwrap_err().to_string();
        assert!(error.contains("android-x64") && error.contains("Supported: android-arm64"));
        assert!(check_abis(&[]).is_err());
    }
    #[test]
    fn runner_is_built_as_a_cdylib_for_the_abi() {
        let p = project("android_cargo");
        let cmd = runner_cargo_command(&p, &toolchain(), BuildMode::Release, Abi::Arm64V8a);
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
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
            cmd.get_envs()
                .any(|(k, _)| k == "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER")
        );
    }
    #[test]
    fn gradle_gets_the_stage_dir_sdk_and_jdk() {
        let p = project("android_gradle");
        let cmd = gradle_command(
            &p,
            &toolchain(),
            BuildMode::Debug,
            Path::new("/stage/debug"),
        );
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                "assembleDebug",
                "-Pflutter.internal.agpVersion=9.3.1",
                "-PrlutterStageDir=/stage/debug"
            ]
        );
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == "JAVA_HOME" && v == Some(std::ffi::OsStr::new("/jbr")))
        );
    }
    #[test]
    fn prebuilt_native_script_targets_only_discovered_packages() {
        let p = project("android_prebuilt_native");
        let stage = stage_dir(&p, BuildMode::Debug);
        fs::create_dir_all(&stage).unwrap();
        assert!(write_prebuilt_native_script(&p, &stage).unwrap().is_none());
        fs::write(
            plugins::native_libraries_file(&p),
            r#"[{"package":"rhttp","manifest":"/rhttp/rust/Cargo.toml","library":"rhttp"}]"#,
        )
        .unwrap();
        let script = write_prebuilt_native_script(&p, &stage).unwrap().unwrap();
        let contents = fs::read_to_string(script).unwrap();
        assert!(contents.contains("['rhttp']"));
        assert!(contents.contains("project.name"));
        assert!(contents.contains("cargokitCargoBuild"));
    }
    #[test]
    fn staging_supplies_rust_libraries_without_a_second_engine_or_duplicate_assets() {
        let p = project("android_stage_embedding");
        let runner = p.dir.join("libdemo.so");
        fs::write(&runner, "runner").unwrap();
        let stage = stage_dir(&p, BuildMode::Debug);
        let output = AbiOutput {
            abi: Abi::Arm64V8a,
            engine: Engine {
                target: p.dir.clone(),
                host: p.dir.clone(),
            },
            runner_lib: runner,
            native_libs: vec![],
        };
        fs::create_dir_all(stage.join("assets")).unwrap();
        fs::write(stage.join("assets/stale"), "stale").unwrap();
        stage_android(&p, &stage, &[output]).unwrap();
        assert!(stage.join("jniLibs/arm64-v8a/libdemo.so").is_file());
        assert!(
            !stage
                .join("jniLibs/arm64-v8a/libflutter_rust_engine.so")
                .exists()
        );
        assert!(!stage.join("jniLibs/arm64-v8a/libflutter.so").exists());
        assert!(!stage.join("assets").exists());
    }
}
