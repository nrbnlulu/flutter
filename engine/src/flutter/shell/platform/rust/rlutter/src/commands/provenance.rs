//! Content fingerprints for the built inputs and the staged executable/assets.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn files(path: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        output.push(path.to_owned());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        files(&entry.path(), output)?;
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("SHA-256 provenance needs coreutils sha256sum")?;
    child.stdin.take().unwrap().write_all(bytes)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("SHA-256 computation failed");
    }
    Ok(String::from_utf8(output.stdout)?
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned())
}
pub fn fingerprint(root: &Path, inputs: &[PathBuf]) -> Result<Value> {
    let mut paths = Vec::new();
    for input in inputs {
        files(&root.join(input), &mut paths)?;
    }
    paths.sort();
    paths.dedup();
    let mut hashes = Vec::new();
    for chunk in paths.chunks(128) {
        let output = Command::new("sha256sum").args(chunk).output()?;
        if !output.status.success() {
            bail!("cannot hash verification inputs");
        }
        hashes.extend_from_slice(&output.stdout);
    }
    Ok(json!({"sha256":digest(&hashes)?,"files":String::from_utf8(hashes)?}))
}
pub fn sources(root: &Path, target: &str) -> Result<Value> {
    fingerprint(
        root,
        &[
            "lib".into(),
            target.into(),
            "pubspec.yaml".into(),
            "runner-rs/src".into(),
            "runner-rs/Cargo.toml".into(),
            "runner-rs/build.rs".into(),
        ],
    )
}
fn git(root: &Path, args: &[&str]) -> Value {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| json!(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or(Value::Null)
}
#[allow(clippy::too_many_arguments)]
pub fn manifest(
    root: &Path,
    sdk: &Path,
    runner: &Path,
    assets: &Path,
    target: &str,
    mode: &str,
    defines: &[String],
    source: Value,
) -> Result<Value> {
    let directory = runner.parent().context("runner has no directory")?;
    let staged = fingerprint(
        Path::new(""),
        &[
            runner.to_path_buf(),
            directory.join("data").join("icudtl.dat"),
            directory.join("libs"),
            assets.to_path_buf(),
        ],
    )?;
    let dependencies = fingerprint(
        root,
        &[
            "pubspec.lock".into(),
            "runner-rs/Cargo.lock".into(),
            ".dart_tool/package_config.json".into(),
        ],
    )?;
    Ok(
        json!({"formatVersion":1,"freshProcess":true,"target":target,"mode":mode,"dartDefines":defines,"source":source,"dependencies":dependencies,"staged":staged,
        "projectRevision":git(root,&["rev-parse","HEAD"]),"dirtyTree":git(root,&["status","--porcelain","--untracked-files=normal"]),"sdkRevision":git(sdk,&["rev-parse","HEAD"]),"sdkDirtyTree":git(sdk,&["status","--porcelain","--untracked-files=normal"]),
        "environment":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"locale":std::env::var("LANG").ok(),"timezone":std::env::var("TZ").ok(),"fontconfig":std::env::var("FONTCONFIG_FILE").ok(),"display":false,"clock":"monotonic real time"}}),
    )
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    #[test]
    fn manifest_fingerprints_isolated_assets_instead_of_bundled_assets() {
        let root = tempdir("provenance_isolated");
        let runner_dir = root.join("runner-rs/target/debug");
        let bundled = runner_dir.join("data/flutter_assets");
        let libs = runner_dir.join("libs");
        let agent_assets = root.join("build/rlutter/linux/debug/agent/flutter_assets");
        fs::create_dir_all(&bundled).unwrap();
        fs::create_dir_all(&libs).unwrap();
        fs::create_dir_all(&agent_assets).unwrap();

        let runner = runner_dir.join("demo");
        fs::write(&runner, "runner").unwrap();
        fs::write(runner_dir.join("data/icudtl.dat"), "icu").unwrap();
        fs::write(libs.join("libflutter_rust_engine.so"), "engine").unwrap();
        fs::write(bundled.join("kernel_blob.bin"), "interactive-v1").unwrap();
        fs::write(agent_assets.join("kernel_blob.bin"), "agent-v1").unwrap();

        let m1 = manifest(
            &root,
            &root,
            &runner,
            &agent_assets,
            "lib/main.dart",
            "debug",
            &[],
            json!({}),
        )
        .unwrap();

        // Changing bundled interactive assets must not change agent staged fingerprint.
        fs::write(bundled.join("kernel_blob.bin"), "interactive-v2").unwrap();
        let m2 = manifest(
            &root,
            &root,
            &runner,
            &agent_assets,
            "lib/main.dart",
            "debug",
            &[],
            json!({}),
        )
        .unwrap();
        assert_eq!(m1["staged"]["sha256"], m2["staged"]["sha256"]);

        // Changing agent assets must change agent staged fingerprint.
        fs::write(agent_assets.join("kernel_blob.bin"), "agent-v2").unwrap();
        let m3 = manifest(
            &root,
            &root,
            &runner,
            &agent_assets,
            "lib/main.dart",
            "debug",
            &[],
            json!({}),
        )
        .unwrap();
        assert_ne!(m1["staged"]["sha256"], m3["staged"]["sha256"]);
    }
}
