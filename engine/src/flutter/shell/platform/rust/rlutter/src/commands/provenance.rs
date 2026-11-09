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
pub fn manifest(
    root: &Path,
    sdk: &Path,
    runner: &Path,
    target: &str,
    mode: &str,
    defines: &[String],
    source: Value,
) -> Result<Value> {
    let directory = runner.parent().context("runner has no directory")?;
    let staged = fingerprint(
        directory,
        &[
            runner.file_name().unwrap().into(),
            "data".into(),
            "libs".into(),
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
