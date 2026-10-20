//! The Dart side of a build, delegated to `flutter assemble`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{flutter::Flutter, mode::BuildMode, project::Project, sdk::Engine, util::run};

// `flutter assemble` bypasses FlutterCommand.getBuildInfo(), which normally
// adds enabled framework features to the Dart defines. The Rust shell exposes
// Flutter's windowing backend, so its bundles must always enable that API.
const WINDOWING_DART_DEFINES: &str = "RkxVVFRFUl9FTkFCTEVEX0ZFQVRVUkVfRkxBR1M9d2luZG93aW5n";

pub struct Bundle {
    /// Directory holding the app's assets (`AssetManifest.bin`, fonts, and
    /// `kernel_blob.bin` in debug).
    pub assets: PathBuf,
    /// AOT-compiled app (release only).
    pub app_so: Option<PathBuf>,
}

/// `flutter assemble` arguments after the global flags. Debug ships a kernel;
/// release also AOT-compiles it. The generic `copy_flutter_bundle` is used
/// instead of the platform asset targets, which unpack the stock GTK embedder.
fn assemble_args(mode: BuildMode, platform: &str, target: &str, output: &Path) -> Vec<String> {
    let mut args = vec![
        "assemble".to_owned(),
        "--output".to_owned(),
        output.display().to_string(),
        format!("-dTargetPlatform={platform}"),
        format!("-dBuildMode={}", mode.name()),
        format!("-dTargetFile={target}"),
        format!("-dDartDefines={WINDOWING_DART_DEFINES}"),
        "copy_flutter_bundle".to_owned(),
    ];
    if mode.config().dart_aot {
        args.push("aot_elf_release".to_owned());
    }
    args
}

/// Compiles the app's Dart code and assets for `platform` (a Flutter target
/// platform name such as `linux-x64`) into `output`.
pub fn assemble_bundle(
    flutter: &Flutter,
    project: &Project,
    engine: &Engine,
    mode: BuildMode,
    platform: &str,
    target: &str,
    output: &Path,
) -> Result<Bundle> {
    fs::create_dir_all(output)?;
    let mut command = flutter.command(Some(engine));
    command
        .args(assemble_args(mode, platform, target, output))
        .current_dir(&project.dir);
    run(&mut command)?;

    let app_so = if mode.config().dart_aot {
        Some(find_app_so(project, output)?)
    } else {
        None
    };
    Ok(Bundle {
        assets: output.to_owned(),
        app_so,
    })
}

/// `aot_elf_release` writes `app.so` into a build directory named by the
/// build id that assemble records in the output directory.
fn find_app_so(project: &Project, output: &Path) -> Result<PathBuf> {
    let id_file = output.join(".last_build_id");
    let id = fs::read_to_string(&id_file)
        .with_context(|| format!("assemble did not write {}", id_file.display()))?;
    let app_so = project
        .dir
        .join(".dart_tool")
        .join("flutter_build")
        .join(id.trim())
        .join("app.so");
    if !app_so.is_file() {
        bail!("assemble did not produce {}", app_so.display());
    }
    Ok(app_so)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    #[test]
    fn debug_ships_a_kernel_only() {
        let args = assemble_args(
            BuildMode::Debug,
            "linux-x64",
            "lib/main.dart",
            Path::new("/o"),
        );
        assert_eq!(
            args,
            [
                "assemble",
                "--output",
                "/o",
                "-dTargetPlatform=linux-x64",
                "-dBuildMode=debug",
                "-dTargetFile=lib/main.dart",
                "-dDartDefines=RkxVVFRFUl9FTkFCTEVEX0ZFQVRVUkVfRkxBR1M9d2luZG93aW5n",
                "copy_flutter_bundle"
            ]
        );
    }

    #[test]
    fn release_adds_the_aot_target() {
        let args = assemble_args(
            BuildMode::Release,
            "linux-x64",
            "lib/a.dart",
            Path::new("/o"),
        );
        assert!(args.contains(&"-dBuildMode=release".to_owned()));
        assert_eq!(
            &args[args.len() - 2..],
            ["copy_flutter_bundle", "aot_elf_release"]
        );
    }

    #[test]
    fn app_so_is_found_through_the_last_build_id() {
        let dir = tempdir("dart_app_so");
        fs::write(dir.join("pubspec.yaml"), "name: demo\n").unwrap();
        let project = Project::load(&dir).unwrap();
        let output = dir.join("out");
        fs::create_dir_all(&output).unwrap();
        assert!(find_app_so(&project, &output).is_err(), "no build id yet");
        fs::write(output.join(".last_build_id"), "abc123\n").unwrap();
        assert!(find_app_so(&project, &output).is_err(), "no app.so yet");
        let build = dir.join(".dart_tool/flutter_build/abc123");
        fs::create_dir_all(&build).unwrap();
        fs::write(build.join("app.so"), "").unwrap();
        assert_eq!(
            find_app_so(&project, &output).unwrap(),
            build.join("app.so")
        );
    }
}
