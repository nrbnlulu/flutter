//! Making a `runner-rs/` project buildable for Android.
//!
//! Projects start Linux-only. Building for Android adds, idempotently:
//! - a generated block of Android-only dependencies in `Cargo.toml`,
//! - `mod flutter_android;` in `src/lib.rs`, and
//! - the tool-owned `src/flutter_android.rs` entry point.
//!
//! The library is built as a `cdylib` for Android only (`cargo rustc
//! --crate-type`), so Linux builds do not gain an extra shared library.

use std::fs;

use anyhow::{Result, anyhow};
use cargo_metadata::MetadataCommand;

use crate::{
    plugins::{PLUGINS_BEGIN, PLUGINS_END},
    project::Project,
    util::write_if_changed,
};

pub const ANDROID_BEGIN: &str = "# === BEGIN FLUTTER GENERATED ANDROID RUNNER ===";
pub const ANDROID_END: &str = "# === END FLUTTER GENERATED ANDROID RUNNER ===";

const TEMPLATE: &str = include_str!("../templates/runner-rs/src/flutter_android.rs.tmpl");

const ANDROID_DEPENDENCIES: &str = r#"[target.'cfg(target_os = "android")'.dependencies]
android_logger = "=0.15.1"
log = "=0.4.33"
flutter-shell-android = { path = "../.dart_tool/flutter_rs/sdk/crates/flutter-shell-android" }"#;

const MODULE_LINE: &str = "mod flutter_android;";
const WINIT_DEPENDENCY: &str =
    "flutter-shell-winit = { path = \"../.dart_tool/flutter_rs/sdk/crates/flutter-shell-winit\" }";
const LINUX_DEPENDENCIES: &str = "[target.'cfg(target_os = \"linux\")'.dependencies]";

/// Moves older runners' desktop shell dependency out of Android's graph.
fn with_linux_only_winit(manifest: &str) -> String {
    if !manifest.lines().any(|line| line == WINIT_DEPENDENCY) {
        return manifest.to_owned();
    }
    let mut manifest = manifest.replace(&format!("{WINIT_DEPENDENCY}\n"), "");
    if manifest.contains(LINUX_DEPENDENCIES) {
        manifest = manifest.replacen(
            LINUX_DEPENDENCIES,
            &format!("{LINUX_DEPENDENCIES}\n{WINIT_DEPENDENCY}"),
            1,
        );
    } else {
        manifest.push_str(&format!("\n{LINUX_DEPENDENCIES}\n{WINIT_DEPENDENCY}\n"));
    }
    manifest
}

/// Fix runners generated while the plugin block followed the Linux table.
fn with_shared_plugins(manifest: &str) -> String {
    let (Some(linux), Some(begin), Some(end)) = (
        manifest.find(LINUX_DEPENDENCIES),
        manifest.find(PLUGINS_BEGIN),
        manifest.find(PLUGINS_END),
    ) else {
        return manifest.to_owned();
    };
    if begin < linux || end < begin {
        return manifest.to_owned();
    }
    if manifest[linux + LINUX_DEPENDENCIES.len()..begin]
        .lines()
        .any(|line| line.starts_with('['))
    {
        return manifest.to_owned();
    }
    let mut updated = manifest.to_owned();
    let block = updated[begin..end + PLUGINS_END.len()].to_owned();
    updated.replace_range(begin..end + PLUGINS_END.len(), "");
    let linux = updated
        .find(LINUX_DEPENDENCIES)
        .expect("table was retained");
    updated.insert_str(linux, &format!("{block}\n\n"));
    updated
}

/// `Cargo.toml` with the generated Android block present and current.
pub fn with_android_dependencies(manifest: &str) -> String {
    let block = format!("{ANDROID_BEGIN}\n{ANDROID_DEPENDENCIES}\n{ANDROID_END}");
    match (manifest.find(ANDROID_BEGIN), manifest.find(ANDROID_END)) {
        (Some(begin), Some(end)) if begin < end => {
            format!(
                "{}{block}{}",
                &manifest[..begin],
                &manifest[end + ANDROID_END.len()..]
            )
        }
        _ => {
            let separator = if manifest.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{manifest}{separator}{block}\n")
        }
    }
}

/// `src/lib.rs` declaring the Android entry-point module.
pub fn with_android_module(lib_rs: &str) -> String {
    if lib_rs.lines().any(|l| l.trim() == MODULE_LINE) {
        return lib_rs.to_owned();
    }
    let declaration = format!("#[cfg(target_os = \"android\")]\n{MODULE_LINE}\n");
    match plugins_declaration_start(lib_rs) {
        Some(at) => format!("{}{declaration}{}", &lib_rs[..at], &lib_rs[at..]),
        None => {
            let separator = if lib_rs.ends_with('\n') || lib_rs.is_empty() {
                ""
            } else {
                "\n"
            };
            format!("{lib_rs}{separator}\n{declaration}")
        }
    }
}

/// Byte offset of the line where the `flutter_plugins` module declaration
/// starts: its first outer attribute or doc comment, or the `mod` line itself
/// (with any visibility) when it has none. Inserting there keeps both intact.
fn plugins_declaration_start(lib_rs: &str) -> Option<usize> {
    let lines: Vec<(usize, &str)> = lib_rs
        .split_inclusive('\n')
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some((start, line))
        })
        .collect();
    let declaration = lines
        .iter()
        .position(|(_, line)| is_plugins_declaration(line))?;
    let mut first = declaration;
    // Walk up over the attributes and doc comments that belong to the `mod`.
    // A multi-line attribute is recognised from its closing `]` line.
    while first > 0 {
        let line = lines[first - 1].1.trim();
        if line.starts_with("#[") || line.starts_with("///") {
            first -= 1;
        } else if line.ends_with(']') || line.ends_with(")]") {
            match lines[..first]
                .iter()
                .rposition(|(_, l)| l.trim_start().starts_with("#["))
            {
                Some(start) => first = start,
                None => break,
            }
        } else {
            break;
        }
    }
    Some(lines[first].0)
}

/// `[pub[(..)]] mod flutter_plugins;` on its own line.
fn is_plugins_declaration(line: &str) -> bool {
    let mut rest = line.trim();
    if let Some(after_pub) = rest.strip_prefix("pub") {
        rest = after_pub.trim_start();
        if rest.starts_with('(') {
            match rest.find(')') {
                Some(close) => rest = rest[close + 1..].trim_start(),
                None => return false,
            }
        }
    }
    rest == "mod flutter_plugins;"
}

/// Brings the runner up to date for Android builds.
pub fn ensure(project: &Project) -> Result<()> {
    let runner = project.runner_dir();
    let cargo_toml = runner.join("Cargo.toml");
    let lib_rs = runner.join("src").join("lib.rs");

    let manifest = fs::read_to_string(&cargo_toml)
        .map_err(|e| anyhow!("cannot read {}: {e}", cargo_toml.display()))?;
    let manifest_changed = write_if_changed(
        &cargo_toml,
        &with_android_dependencies(&with_shared_plugins(&with_linux_only_winit(&manifest))),
    )?;

    let lib = fs::read_to_string(&lib_rs)
        .map_err(|e| anyhow!("cannot read {}: {e}", lib_rs.display()))?;
    write_if_changed(&lib_rs, &with_android_module(&lib))?;
    write_if_changed(
        &runner.join("src").join("flutter_android.rs"),
        &TEMPLATE.replace("{{projectName}}", &project.name),
    )?;

    if manifest_changed {
        // Cargo.lock must know the new dependencies, or `cargo build --locked`
        // on any platform fails afterwards.
        MetadataCommand::new()
            .manifest_path(&cargo_toml)
            .exec()
            .map_err(|e| anyhow!("cannot resolve the Android runner dependencies: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[package]\nname = \"demo\"\n\n[lints.rust]\nx = 1\n";

    #[test]
    fn android_dependencies_are_appended_once() {
        let once = with_android_dependencies(MANIFEST);
        assert!(once.starts_with(MANIFEST) && once.contains("flutter-shell-android"));
        assert_eq!(with_android_dependencies(&once), once);
        assert_eq!(once.matches(ANDROID_BEGIN).count(), 1);
    }

    #[test]
    fn existing_runner_moves_winit_to_linux_only() {
        let old = format!("[dependencies]\n{WINIT_DEPENDENCY}\n\n[workspace]\n");
        let updated = with_linux_only_winit(&old);
        assert_eq!(updated.matches(WINIT_DEPENDENCY).count(), 1);
        assert!(updated.contains(&format!("{LINUX_DEPENDENCIES}\n{WINIT_DEPENDENCY}")));
        assert_eq!(with_linux_only_winit(&updated), updated);
    }

    #[test]
    fn plugin_dependencies_remain_shared_after_migration() {
        let old = format!(
            "[dependencies]\nflutter-plugin-sdk = \"x\"\n\n{LINUX_DEPENDENCIES}\n{WINIT_DEPENDENCY}\n\n{PLUGINS_BEGIN}\nplugin = \"x\"\n{PLUGINS_END}\n\n[patch.crates-io]\n"
        );
        let updated = with_shared_plugins(&old);
        assert!(updated.find(PLUGINS_BEGIN) < updated.find(LINUX_DEPENDENCIES));
        assert!(updated.contains("plugin = \"x\""));
        assert_eq!(with_shared_plugins(&updated), updated);
    }

    #[test]
    fn a_stale_block_is_replaced_in_place() {
        let stale = format!("a = 1\n{ANDROID_BEGIN}\nold = true\n{ANDROID_END}\nz = 2\n");
        let updated = with_android_dependencies(&stale);
        assert!(!updated.contains("old = true") && updated.contains("android_logger"));
        assert!(updated.starts_with("a = 1\n") && updated.ends_with("\nz = 2\n"));
    }

    #[test]
    fn manifest_without_trailing_newline_still_gets_a_valid_block() {
        let updated = with_android_dependencies("[package]\nname = \"demo\"");
        assert!(updated.contains("\"demo\"\n\n# === BEGIN"), "{updated}");
    }

    #[test]
    fn module_is_declared_before_the_plugins_module() {
        let lib = "// header\n\nmod flutter_plugins;\n\npub fn x() {}\n";
        let updated = with_android_module(lib);
        assert_eq!(
            updated,
            "// header\n\n#[cfg(target_os = \"android\")]\nmod flutter_android;\nmod flutter_plugins;\n\npub fn x() {}\n"
        );
        assert_eq!(with_android_module(&updated), updated);
    }

    #[test]
    fn module_is_declared_before_the_whole_plugins_declaration() {
        let declaration = |lib: &str| {
            let updated = with_android_module(lib);
            assert_eq!(with_android_module(&updated), updated, "idempotent");
            updated
        };
        let cfg = "#[cfg(target_os = \"android\")]\nmod flutter_android;\n";
        // Visibility must not be split from `mod`.
        assert_eq!(
            declaration("pub mod flutter_plugins;\n"),
            format!("{cfg}pub mod flutter_plugins;\n")
        );
        assert_eq!(
            declaration("pub(crate) mod flutter_plugins;\n"),
            format!("{cfg}pub(crate) mod flutter_plugins;\n")
        );
        // Attributes and docs stay attached to the plugins module.
        assert_eq!(
            declaration("use a;\n/// Plugins.\n#[allow(dead_code)]\npub mod flutter_plugins;\n"),
            format!("use a;\n{cfg}/// Plugins.\n#[allow(dead_code)]\npub mod flutter_plugins;\n")
        );
        assert_eq!(
            declaration(
                "#[cfg_attr(\n    feature = \"x\",\n    allow(dead_code)\n)]\nmod flutter_plugins;\n"
            ),
            format!(
                "{cfg}#[cfg_attr(\n    feature = \"x\",\n    allow(dead_code)\n)]\nmod flutter_plugins;\n"
            )
        );
        // A similarly named module is not the plugins module.
        assert!(declaration("mod flutter_plugins_extra;\n").ends_with(&format!("\n{cfg}")));
    }

    #[test]
    fn module_is_appended_when_the_plugins_module_is_gone() {
        let updated = with_android_module("pub fn x() {}");
        assert!(updated.ends_with("#[cfg(target_os = \"android\")]\nmod flutter_android;\n"));
        assert!(updated.starts_with("pub fn x() {}\n"));
    }

    #[test]
    fn template_fills_the_project_name_and_has_the_current_version() {
        assert!(TEMPLATE.contains("AndroidRuntime"));
        let marker = format!(
            "// flutter-rust-runner-version: {}",
            crate::plugins::RUNNER_VERSION
        );
        assert!(TEMPLATE.lines().any(|l| l == marker));
    }
}
