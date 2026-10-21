//! The stock `flutter` tool, which rlutter delegates the Dart side to.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::sdk::Engine;

pub struct Flutter {
    root: PathBuf,
}

impl Flutter {
    /// Uses an explicit SDK override, or downloads our release SDK. Never
    /// discovers Flutter through PATH, FVM, or a build-time checkout path.
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        if let Some(root) = explicit {
            return Self::at(root);
        }
        let cache = env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .context("set HOME or XDG_CACHE_HOME for the rlutter SDK cache")?;
        Self::managed(&cache.join("rlutter"))
    }

    fn managed(cache: &Path) -> Result<Self> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            bail!(
                "the release Flutter SDK is only available for Linux x86_64; pass --flutter-root"
            );
        }
        let root = cache.join("flutter-BETA");
        if root.join(".rlutter-complete").is_file() {
            return Self::at(&root);
        }
        std::fs::create_dir_all(cache)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(cache.join("flutter.lock"))?;
        lock.lock()?;
        if root.join(".rlutter-complete").is_file() {
            return Self::at(&root);
        }
        let staging = cache.join(format!(".flutter-download-{}", std::process::id()));
        crate::util::remove_path(&staging)?;
        eprintln!("Downloading the rlutter Flutter SDK from the BETA release...");
        let result = (|| {
            crate::sdk::download_and_unpack(
                "https://github.com/nrbnlulu/flutter/releases/download/BETA/rlutter-flutter-linux-x64.tar.gz",
                &staging,
            )?;
            Self::at(&staging)?;
            for entry in [
                "bin/cache/dart-sdk/bin/dart",
                "bin/cache/flutter_tools.snapshot",
                ".git/HEAD",
            ] {
                if !staging.join(entry).is_file() {
                    bail!("the release Flutter SDK is incomplete (missing {entry})");
                }
            }
            std::fs::write(staging.join(".rlutter-complete"), "BETA\n")?;
            crate::util::remove_path(&root)?;
            std::fs::rename(&staging, &root)?;
            Self::at(&root)
        })();
        let _ = crate::util::remove_path(&staging);
        result
    }

    fn at(root: &Path) -> Result<Self> {
        let binary = root.join("bin").join("flutter");
        if !binary.is_file() {
            bail!(
                "{} is not a Flutter SDK (missing bin/flutter)",
                root.display()
            );
        }
        Ok(Self {
            root: root.to_owned(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A `flutter` invocation. With an [`Engine`], the global `--local-engine`
    /// flags make the tool compile with the Dart SDK matching the Rust shell's
    /// engine (anything else fails at launch with "Invalid SDK hash").
    pub fn command(&self, engine: Option<&Engine>) -> Command {
        let mut command = Command::new(self.root.join("bin").join("flutter"));
        command.env_remove("FLUTTER_ROOT");
        if self.root.join(".rlutter-complete").is_file() {
            // Flutter otherwise treats a fork's cached release version as
            // suspicious and recomputes it on every command.
            command.env("FLUTTER_GIT_URL", "https://github.com/nrbnlulu/flutter.git");
        }
        if let Some(engine) = engine {
            command
                .arg(format!("--local-engine={}", engine.target.display()))
                .arg(format!("--local-engine-host={}", engine.host.display()));
        }
        command
    }

    pub fn pub_get(&self, project_dir: &Path) -> Command {
        let mut command = self.command(None);
        command.args(["pub", "get"]).current_dir(project_dir);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;
    use std::fs;

    #[test]
    fn rejects_directories_without_a_flutter_binary() {
        let dir = tempdir("flutter_missing");
        assert!(Flutter::at(&dir).is_err());
    }

    #[test]
    fn completed_managed_sdk_is_reused_without_discovery_or_network() {
        let cache = tempdir("flutter_managed");
        let root = cache.join("flutter-BETA");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/flutter"), "").unwrap();
        fs::write(root.join(".rlutter-complete"), "BETA\n").unwrap();
        let flutter = Flutter::managed(&cache).unwrap();
        assert_eq!(flutter.root(), root);
        let command = flutter.command(None);
        assert!(command.get_envs().any(|(key, value)| {
            key == "FLUTTER_GIT_URL"
                && value
                    == Some(std::ffi::OsStr::new(
                        "https://github.com/nrbnlulu/flutter.git",
                    ))
        }));
    }

    #[test]
    fn local_engine_flags_come_before_the_subcommand() {
        let dir = tempdir("flutter_flags");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/flutter"), "").unwrap();
        let flutter = Flutter::at(&dir).unwrap();
        let engine = Engine {
            target: "/e/host_debug".into(),
            host: "/e/host_debug".into(),
        };
        let mut command = flutter.command(Some(&engine));
        command.arg("assemble");
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--local-engine=/e/host_debug",
                "--local-engine-host=/e/host_debug",
                "assemble"
            ]
        );
    }
}
