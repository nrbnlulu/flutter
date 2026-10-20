//! The Flutter application being built.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Deserialize)]
struct Pubspec {
    name: String,
    #[serde(rename = "rust-shell", default)]
    rust_shell: bool,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub dir: PathBuf,
    pub name: String,
    pub rust_shell: bool,
}

impl Project {
    pub fn load(dir: &Path) -> Result<Self> {
        let pubspec_path = dir.join("pubspec.yaml");
        let text = fs::read_to_string(&pubspec_path)
            .with_context(|| format!("cannot read {}", pubspec_path.display()))?;
        let pubspec: Pubspec = serde_norway::from_str(&text)
            .with_context(|| format!("invalid {}", pubspec_path.display()))?;
        Ok(Self {
            dir: dir.to_owned(),
            name: pubspec.name,
            rust_shell: pubspec.rust_shell,
        })
    }

    /// Finds the project containing the current directory.
    pub fn discover() -> Result<Self> {
        let start = env::current_dir()?;
        for dir in start.ancestors() {
            if dir.join("pubspec.yaml").is_file() {
                return Self::load(dir);
            }
        }
        bail!(
            "no pubspec.yaml found in {} or any parent directory",
            start.display()
        )
    }

    pub fn require_rust_shell(&self) -> Result<()> {
        if !self.rust_shell || !self.runner_dir().join("Cargo.toml").is_file() {
            bail!(
                "{} is not a Rust-shell project. Run `rlutter create` (in a new directory, or \
                 an existing Flutter project) to add the Rust shell.",
                self.name
            );
        }
        Ok(())
    }

    pub fn runner_dir(&self) -> PathBuf {
        self.dir.join("runner-rs")
    }

    pub fn flutter_rs_dir(&self) -> PathBuf {
        self.dir.join(".dart_tool").join("flutter_rs")
    }

    pub fn sdk_dir(&self) -> PathBuf {
        self.flutter_rs_dir().join("sdk")
    }

    /// Where rlutter writes its build outputs.
    pub fn build_dir(&self) -> PathBuf {
        self.dir.join("build").join("rlutter")
    }

    pub fn package_config(&self) -> PathBuf {
        self.dir.join(".dart_tool").join("package_config.json")
    }

    pub fn package_graph(&self) -> PathBuf {
        self.dir.join(".dart_tool").join("package_graph.json")
    }
}

/// Inserts `rust-shell: true` after the top-level `name:` line, keeping the
/// rest of the file (comments, ordering) untouched. Returns `None` if the key
/// is already present.
pub fn enable_rust_shell_in_pubspec(pubspec: &str) -> Result<Option<String>> {
    if pubspec.lines().any(|l| l.starts_with("rust-shell:")) {
        return Ok(None);
    }
    let mut out = String::with_capacity(pubspec.len() + 24);
    let mut inserted = false;
    for line in pubspec.split_inclusive('\n') {
        out.push_str(line);
        if !inserted && line.starts_with("name:") {
            if !line.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("rust-shell: true\n");
            inserted = true;
        }
    }
    if !inserted {
        bail!("pubspec.yaml has no top-level `name:`");
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    #[test]
    fn loads_name_and_rust_shell_flag() {
        let dir = tempdir("project_load");
        fs::write(
            dir.join("pubspec.yaml"),
            "name: demo\nrust-shell: true\nflutter:\n  uses-material-design: true\n",
        )
        .unwrap();
        let project = Project::load(&dir).unwrap();
        assert_eq!(project.name, "demo");
        assert!(project.rust_shell);
        assert!(
            project.require_rust_shell().is_err(),
            "runner-rs/Cargo.toml is missing"
        );
    }

    #[test]
    fn flag_defaults_to_false() {
        let dir = tempdir("project_default");
        fs::write(dir.join("pubspec.yaml"), "name: demo\n").unwrap();
        assert!(!Project::load(&dir).unwrap().rust_shell);
    }

    #[test]
    fn pubspec_patch_inserts_after_name_and_is_idempotent() {
        let original = "# header\nname: demo\ndescription: x\n";
        let patched = enable_rust_shell_in_pubspec(original).unwrap().unwrap();
        assert_eq!(
            patched,
            "# header\nname: demo\nrust-shell: true\ndescription: x\n"
        );
        assert!(enable_rust_shell_in_pubspec(&patched).unwrap().is_none());
    }

    #[test]
    fn pubspec_patch_handles_missing_trailing_newline() {
        let patched = enable_rust_shell_in_pubspec("name: demo").unwrap().unwrap();
        assert_eq!(patched, "name: demo\nrust-shell: true\n");
    }
}
