//! Building the `cdylib`s that plugin discovery found (port of
//! `native_libraries.dart`): Rust packages loaded by name at runtime instead of
//! linked into the runner, e.g. flutter_rust_bridge crates.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};

use crate::{
    abi::Abi,
    android::Toolchain,
    mode::BuildMode,
    plugins::{NativeLibrary, read_native_libraries},
    project::Project,
    util::{remove_path, run},
};

/// A cross-compilation target: the ABI and the NDK that links for it.
pub type CrossTarget<'a> = Option<(Abi, &'a Toolchain)>;

/// Where built libraries are staged for `mode` (and `abi` when cross-compiling).
pub fn staging_dir(project: &Project, mode: BuildMode, abi: Option<Abi>) -> PathBuf {
    let base = project.flutter_rs_dir().join("native_libs");
    match abi {
        Some(abi) => base.join(abi.android_name()).join(mode.name()),
        None => base.join(mode.name()),
    }
}

fn cargo_command(
    library: &NativeLibrary,
    mode: BuildMode,
    target_dir: &Path,
    cross: CrossTarget,
) -> Command {
    let mut command = Command::new("cargo");
    command.arg("build");
    if mode.config().cargo_release {
        command.arg("--release");
    }
    if let Some((abi, toolchain)) = cross {
        command.arg("--target").arg(abi.rust_triple());
        command.envs(toolchain.cargo_env(abi));
    }
    command
        .arg("--manifest-path")
        .arg(&library.manifest_path)
        .arg("--target-dir")
        .arg(target_dir);
    if let Some(dir) = library.manifest_path.parent() {
        command.current_dir(dir);
    }
    if !library.rustflags.is_empty() {
        command.env("CARGO_ENCODED_RUSTFLAGS", library.rustflags.join("\u{1f}"));
    }
    command
}

/// Builds every discovered native library with the runner's profile and
/// stages the results. Returns the staged files.
pub fn build_all(project: &Project, mode: BuildMode, cross: CrossTarget) -> Result<Vec<PathBuf>> {
    let libraries = read_native_libraries(project)?;
    let staging = staging_dir(project, mode, cross.map(|(abi, _)| abi));
    // Drop libraries staged by an earlier build, including those whose plugin
    // has since been removed, so only what is built now gets bundled.
    remove_path(&staging)?;
    if libraries.is_empty() {
        return Ok(Vec::new());
    }
    let target_dir = project.flutter_rs_dir().join("native_libs").join("target");
    fs::create_dir_all(&staging)?;
    let mut staged = Vec::new();
    for library in &libraries {
        eprintln!(
            "Building native library {} ({})...",
            library.file_name(),
            library.dart_package
        );
        run(&mut cargo_command(library, mode, &target_dir, cross))?;
        let mut built_dir = target_dir.clone();
        if let Some((abi, _)) = cross {
            built_dir.push(abi.rust_triple());
        }
        let built = built_dir
            .join(mode.cargo_profile_dir())
            .join(library.file_name());
        if !built.is_file() {
            bail!(
                "Cargo did not produce {} for {}",
                built.display(),
                library.dart_package
            );
        }
        let destination = staging.join(library.file_name());
        fs::copy(&built, &destination)?;
        staged.push(destination);
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(rustflags: &[&str]) -> NativeLibrary {
        NativeLibrary {
            dart_package: "p".into(),
            manifest_path: PathBuf::from("/pkg/rust/Cargo.toml"),
            library_name: "p_ffi".into(),
            rustflags: rustflags.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn release_builds_use_the_release_profile_and_shared_target_dir() {
        let command = cargo_command(&library(&[]), BuildMode::Release, Path::new("/t"), None);
        assert_eq!(
            args(&command),
            [
                "build",
                "--release",
                "--manifest-path",
                "/pkg/rust/Cargo.toml",
                "--target-dir",
                "/t"
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/pkg/rust")));
    }

    #[test]
    fn debug_builds_omit_release() {
        let command = cargo_command(&library(&[]), BuildMode::Debug, Path::new("/t"), None);
        assert!(!args(&command).contains(&"--release".to_owned()));
    }

    #[test]
    fn rustflags_are_passed_encoded_with_unit_separators() {
        let command = cargo_command(
            &library(&["-Cx", "-Cy"]),
            BuildMode::Debug,
            Path::new("/t"),
            None,
        );
        let flags = command
            .get_envs()
            .find(|(k, _)| *k == "CARGO_ENCODED_RUSTFLAGS")
            .and_then(|(_, v)| v)
            .unwrap();
        assert_eq!(flags.to_string_lossy(), "-Cx\u{1f}-Cy");
    }

    #[test]
    fn cross_builds_target_the_abi_with_the_ndk_linker() {
        let toolchain = Toolchain {
            sdk: "/sdk".into(),
            ndk: "/ndk".into(),
            java_home: None,
        };
        let command = cargo_command(
            &library(&[]),
            BuildMode::Release,
            Path::new("/t"),
            Some((Abi::Arm64V8a, &toolchain)),
        );
        let a = args(&command);
        let at = a.iter().position(|x| x == "--target").unwrap();
        assert_eq!(a[at + 1], "aarch64-linux-android");
        let linker = command
            .get_envs()
            .find(|(k, _)| *k == "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER")
            .and_then(|(_, v)| v)
            .unwrap();
        assert!(
            linker
                .to_string_lossy()
                .ends_with("aarch64-linux-android24-clang")
        );
    }

    #[test]
    fn staging_is_separate_per_abi() {
        let project = Project {
            dir: "/p".into(),
            name: "demo".into(),
            rust_shell: true,
        };
        assert_eq!(
            staging_dir(&project, BuildMode::Debug, None),
            PathBuf::from("/p/.dart_tool/flutter_rs/native_libs/debug")
        );
        assert_eq!(
            staging_dir(&project, BuildMode::Release, Some(Abi::Arm64V8a)),
            PathBuf::from("/p/.dart_tool/flutter_rs/native_libs/arm64-v8a/release")
        );
    }

    #[test]
    fn stale_staged_libraries_are_removed_even_without_native_libraries() {
        let dir = crate::util::testing::tempdir("native_libs_stale");
        let project = Project {
            dir: dir.clone(),
            name: "demo".into(),
            rust_shell: true,
        };
        let staging = staging_dir(&project, BuildMode::Debug, None);
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("libgone.so"), "").unwrap();
        assert!(
            build_all(&project, BuildMode::Debug, None)
                .unwrap()
                .is_empty()
        );
        assert!(!staging.join("libgone.so").exists());
    }
}
