//! The Dart side of a build, delegated to `flutter assemble`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::{flutter::Flutter, mode::BuildMode, project::Project, sdk::Engine, util::run};

// `flutter assemble` bypasses FlutterCommand.getBuildInfo(), which normally
// adds enabled framework features to the Dart defines. The Rust shell exposes
// Flutter's windowing backend, so its bundles must always enable that API.
const WINDOWING_DEFINE: &str = "FLUTTER_ENABLED_FEATURE_FLAGS=windowing";

/// Standard base64 with padding, which is how the tool packs each Dart define.
fn base64(input: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.as_bytes().chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `DartDefines` build value: comma separated, each define base64 encoded.
pub(crate) fn encode_dart_defines(user: &[String]) -> String {
    std::iter::once(WINDOWING_DEFINE)
        .chain(user.iter().map(String::as_str))
        .map(base64)
        .collect::<Vec<_>>()
        .join(",")
}

pub struct Bundle {
    /// Directory holding the app's assets (`AssetManifest.bin`, fonts, and
    /// `kernel_blob.bin` in debug).
    pub assets: PathBuf,
    /// AOT-compiled app (profile and release).
    pub app_so: Option<PathBuf>,
}

/// `flutter assemble` arguments after the global flags. Debug ships a kernel;
/// release also AOT-compiles it. The generic `copy_flutter_bundle` is used
/// instead of the platform asset targets, which unpack the stock GTK embedder.
fn assemble_args(
    mode: BuildMode,
    platform: &str,
    target: &str,
    dart_defines: &[String],
    output: &Path,
) -> Vec<String> {
    let mut args = vec![
        "assemble".to_owned(),
        "--output".to_owned(),
        output.display().to_string(),
        format!("-dTargetPlatform={platform}"),
        format!("-dBuildMode={}", mode.name()),
        format!("-dTargetFile={target}"),
        // The deprecated -d option splits commas and silently discards later
        // defines. This alias preserves the complete base64 list as one value.
        format!("--DartDefines={}", encode_dart_defines(dart_defines)),
        "copy_flutter_bundle".to_owned(),
    ];
    if mode.config().dart_aot {
        if platform.starts_with("darwin") {
            // The stock aot_snapshot_* targets cannot build a darwin snapshot
            // outside an Xcode app (they need a CPU architecture and write
            // App.framework/App, not the declared app.so). Assemble only the
            // AOT kernel; `compile_macho` runs gen_snapshot on it.
            args.push("kernel_snapshot_program".to_owned());
        } else {
            args.push(format!("aot_snapshot_{}", mode.name()));
        }
    }
    args
}

/// Matches Flutter's macOS deployment target for App.framework.
const MACOS_MIN_VERSION: &str = "10.15";

/// The gen_snapshot arguments that turn the AOT kernel into a Mach-O dylib,
/// as the Flutter tool does for App.framework on macOS.
fn macho_args(app_dill: &Path, output: &Path) -> Vec<String> {
    vec![
        "--deterministic".to_owned(),
        "--snapshot_kind=app-aot-macho-dylib".to_owned(),
        format!("--macho={}", output.display()),
        format!("--macho-min-os-version={MACOS_MIN_VERSION}"),
        "--macho-install-name=@rpath/app.so".to_owned(),
        app_dill.display().to_string(),
    ]
}

/// Compiles the assembled AOT kernel into `app.so`, a Mach-O dylib the engine
/// loads with dlopen, and ad-hoc signs it: Apple Silicon refuses to map
/// unsigned code, and gen_snapshot (unlike the linker) does not sign.
fn compile_macho(engine: &Engine, project: &Project, output: &Path) -> Result<PathBuf> {
    let build = build_dir(project, output)?;
    let app_dill = build.join("app.dill");
    if !app_dill.is_file() {
        bail!("assemble did not produce {}", app_dill.display());
    }
    let app_so = build.join("app.so");
    let gen_snapshot = engine.host.join("gen_snapshot");
    run(Command::new(&gen_snapshot).args(macho_args(&app_dill, &app_so)))
        .with_context(|| format!("{} failed", gen_snapshot.display()))?;
    run(Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app_so))
    .context("cannot ad-hoc sign the AOT snapshot")?;
    Ok(app_so)
}

/// Compiles the app's Dart code and assets for `platform` (a Flutter target
/// platform name such as `linux-x64`) into `output`.
#[allow(clippy::too_many_arguments)]
pub fn assemble_bundle(
    flutter: &Flutter,
    project: &Project,
    engine: &Engine,
    mode: BuildMode,
    platform: &str,
    target: &str,
    dart_defines: &[String],
    output: &Path,
) -> Result<Bundle> {
    fs::create_dir_all(output)?;
    let mut command = flutter.command(Some(engine));
    command
        .args(assemble_args(mode, platform, target, dart_defines, output))
        .current_dir(&project.dir);
    run(&mut command)?;

    let app_so = if !mode.config().dart_aot {
        None
    } else if platform.starts_with("darwin") {
        Some(compile_macho(engine, project, output)?)
    } else {
        Some(find_app_so(project, output)?)
    };
    Ok(Bundle {
        assets: output.to_owned(),
        app_so,
    })
}

/// The assemble build directory, named by the build id that assemble records
/// in the output directory.
fn build_dir(project: &Project, output: &Path) -> Result<PathBuf> {
    let id_file = output.join(".last_build_id");
    let id = fs::read_to_string(&id_file)
        .with_context(|| format!("assemble did not write {}", id_file.display()))?;
    Ok(project
        .dir
        .join(".dart_tool")
        .join("flutter_build")
        .join(id.trim()))
}

/// `aot_snapshot_release` writes `app.so` into the build directory.
fn find_app_so(project: &Project, output: &Path) -> Result<PathBuf> {
    let app_so = build_dir(project, output)?.join("app.so");
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
            &[],
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
                "--DartDefines=RkxVVFRFUl9FTkFCTEVEX0ZFQVRVUkVfRkxBR1M9d2luZG93aW5n",
                "copy_flutter_bundle"
            ]
        );
    }

    #[test]
    fn base64_matches_the_known_windowing_value() {
        assert_eq!(
            base64(WINDOWING_DEFINE),
            "RkxVVFRFUl9FTkFCTEVEX0ZFQVRVUkVfRkxBR1M9d2luZG93aW5n"
        );
        assert_eq!(base64("a"), "YQ==");
        assert_eq!(base64("ab"), "YWI=");
    }

    #[test]
    fn user_defines_follow_the_windowing_define() {
        let args = assemble_args(
            BuildMode::Debug,
            "linux-x64",
            "lib/main.dart",
            &["A=1".to_owned()],
            Path::new("/o"),
        );
        assert!(args.contains(
            &"--DartDefines=RkxVVFRFUl9FTkFCTEVEX0ZFQVRVUkVfRkxBR1M9d2luZG93aW5n,QT0x".to_owned()
        ));
    }

    #[test]
    fn release_adds_the_aot_target() {
        let args = assemble_args(
            BuildMode::Release,
            "linux-x64",
            "lib/a.dart",
            &[],
            Path::new("/o"),
        );
        assert!(args.contains(&"-dBuildMode=release".to_owned()));
        assert_eq!(
            &args[args.len() - 2..],
            ["copy_flutter_bundle", "aot_snapshot_release"]
        );
    }

    #[test]
    fn darwin_assembles_the_aot_kernel_and_compiles_a_macho_dylib() {
        let args = assemble_args(
            BuildMode::Release,
            "darwin-arm64",
            "lib/main.dart",
            &[],
            Path::new("/o"),
        );
        assert_eq!(
            &args[args.len() - 2..],
            ["copy_flutter_bundle", "kernel_snapshot_program"]
        );
        let macho = macho_args(Path::new("/b/app.dill"), Path::new("/b/app.so"));
        assert!(macho.contains(&"--snapshot_kind=app-aot-macho-dylib".to_owned()));
        assert!(macho.contains(&"--macho=/b/app.so".to_owned()));
        assert_eq!(macho.last().unwrap(), "/b/app.dill");
    }

    #[test]
    fn profile_uses_the_profile_aot_compiler_target() {
        let args = assemble_args(
            BuildMode::Profile,
            "linux-x64",
            "lib/main.dart",
            &[],
            Path::new("/o"),
        );
        assert!(args.contains(&"-dBuildMode=profile".to_owned()));
        assert_eq!(args.last().unwrap(), "aot_snapshot_profile");
        assert!(!args.contains(&"aot_snapshot_release".to_owned()));
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
