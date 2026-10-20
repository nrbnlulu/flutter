//! The stock `flutter` tool, which rlutter delegates the Dart side to.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};

use crate::sdk::Engine;

pub struct Flutter {
    root: PathBuf,
}

impl Flutter {
    /// Finds the Flutter SDK: `--flutter-root` / `$FLUTTER_ROOT`, else the
    /// `flutter` on `PATH`, else the checkout this tool was built from.
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        if let Some(root) = explicit {
            return Self::at(root);
        }
        if let Ok(binary) = which::which("flutter")
            && let Some(root) = binary.canonicalize()?.parent().and_then(Path::parent)
            && let Ok(flutter) = Self::at(root)
        {
            return Ok(flutter);
        }
        // <root>/engine/src/flutter/shell/platform/rust/rlutter
        if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(7)
            && let Ok(flutter) = Self::at(root)
        {
            return Ok(flutter);
        }
        bail!("cannot find the Flutter SDK; pass --flutter-root or set FLUTTER_ROOT")
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
