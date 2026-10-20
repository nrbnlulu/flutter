//! Rust plugin discovery for a Rust-shell app (port of `rust_plugins.dart`).
//!
//! A Dart package with a `rust/Cargo.toml` is either
//! - a *plugin* (`[package.metadata.flutter] plugin = true` + `registrar`),
//!   linked into the runner through the generated section of
//!   `runner-rs/Cargo.toml` and `runner-rs/src/flutter_plugins.rs`, or
//! - a *native library* (any other `cdylib`, e.g. flutter_rust_bridge), built
//!   separately and loaded by name at runtime.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use cargo_metadata::{MetadataCommand, Package};
use serde::{Deserialize, Serialize};

use crate::{
    mode::BuildMode,
    project::Project,
    util::{normalize, remove_path, replace_link, write_if_changed},
};

pub const PLUGINS_BEGIN: &str = "# === BEGIN FLUTTER GENERATED RUST PLUGINS ===";
pub const PLUGINS_END: &str = "# === END FLUTTER GENERATED RUST PLUGINS ===";

/// Version of the tool-owned runner files (`build.rs`, `src/main.rs`). Keep in
/// sync with the `flutter-rust-runner-version` marker in the templates.
pub const RUNNER_VERSION: u32 = 6;
const RUNNER_VERSION_MARKER: &str = "// flutter-rust-runner-version: ";

pub const TEMPLATE_BUILD_RS: &str = include_str!("../templates/runner-rs/build.rs.tmpl");
pub const TEMPLATE_MAIN_RS: &str = include_str!("../templates/runner-rs/src/main.rs.tmpl");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustPlugin {
    pub dart_package: String,
    pub cargo_package: String,
    pub registrar: String,
    pub rust_dir: PathBuf,
}

impl RustPlugin {
    pub fn dependency_alias(&self) -> String {
        format!("flutter_rs_plugin_{}", self.dart_package)
    }
}

/// A `cdylib` the app loads with `dlopen` instead of linking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeLibrary {
    #[serde(rename = "package")]
    pub dart_package: String,
    #[serde(rename = "manifest")]
    pub manifest_path: PathBuf,
    #[serde(rename = "library")]
    pub library_name: String,
    #[serde(default)]
    pub rustflags: Vec<String>,
}

impl NativeLibrary {
    pub fn file_name(&self) -> String {
        format!("lib{}.so", self.library_name)
    }
}

#[derive(Debug, Default)]
pub struct Discovered {
    pub plugins: Vec<RustPlugin>,
    pub native_libraries: Vec<NativeLibrary>,
}

// --- Dart package metadata ---------------------------------------------------

#[derive(Deserialize)]
struct PackageGraph {
    roots: Vec<String>,
    packages: Vec<GraphPackage>,
}

#[derive(Deserialize)]
struct GraphPackage {
    name: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default, rename = "devDependencies")]
    dev_dependencies: Vec<String>,
}

#[derive(Deserialize)]
struct PackageConfig {
    packages: Vec<ConfigPackage>,
}

#[derive(Deserialize)]
struct ConfigPackage {
    name: String,
    #[serde(rename = "rootUri")]
    root_uri: String,
}

/// Names of the packages the app depends on, transitively. Dev dependencies of
/// the app itself count only when `include_dev` is set (they never apply to
/// dependencies of dependencies).
fn dependency_names(graph: &PackageGraph, include_dev: bool) -> Result<BTreeSet<String>> {
    let by_name: BTreeMap<&str, &GraphPackage> = graph
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    let root = graph
        .roots
        .first()
        .ok_or_else(|| anyhow!("package_graph.json has no roots"))?;
    let root_package = by_name
        .get(root.as_str())
        .ok_or_else(|| anyhow!("package_graph.json does not describe {root}"))?;

    let mut queue: VecDeque<&str> = root_package
        .dependencies
        .iter()
        .map(String::as_str)
        .collect();
    if include_dev {
        queue.extend(root_package.dev_dependencies.iter().map(String::as_str));
    }
    let mut seen = BTreeSet::new();
    while let Some(name) = queue.pop_front() {
        if name == root || !seen.insert(name.to_owned()) {
            continue;
        }
        if let Some(package) = by_name.get(name) {
            queue.extend(package.dependencies.iter().map(String::as_str));
        }
    }
    Ok(seen)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolves a `package_config.json` `rootUri` (absolute `file://` URI, or a
/// path relative to the config file's directory).
fn resolve_root_uri(config_dir: &Path, root_uri: &str) -> PathBuf {
    match root_uri.strip_prefix("file://") {
        Some(path) => PathBuf::from(percent_decode(path)),
        None => normalize(&config_dir.join(percent_decode(root_uri))),
    }
}

fn dependency_dirs(project: &Project, mode: BuildMode) -> Result<Vec<(String, PathBuf)>> {
    let graph_path = project.package_graph();
    let graph: PackageGraph =
        serde_json::from_str(&fs::read_to_string(&graph_path).with_context(|| {
            format!(
                "cannot read {}; run `flutter pub get`",
                graph_path.display()
            )
        })?)
        .with_context(|| format!("invalid {}", graph_path.display()))?;
    let names = dependency_names(&graph, mode.config().plugins_include_dev)?;

    let config_path = project.package_config();
    let config: PackageConfig = serde_json::from_str(
        &fs::read_to_string(&config_path)
            .with_context(|| format!("cannot read {}", config_path.display()))?,
    )
    .with_context(|| format!("invalid {}", config_path.display()))?;
    let config_dir = config_path
        .parent()
        .expect("package_config.json has a parent");
    Ok(config
        .packages
        .into_iter()
        .filter(|p| names.contains(&p.name))
        .map(|p| (p.name, resolve_root_uri(config_dir, &p.root_uri)))
        .collect())
}

// --- Cargo metadata ----------------------------------------------------------

fn is_rust_path(path: &str) -> bool {
    !path.is_empty()
        && path.split("::").all(|segment| {
            let segment = segment.strip_prefix("r#").unwrap_or(segment);
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

enum RustPackage {
    Plugin(RustPlugin),
    Native(NativeLibrary),
    Other,
}

fn read_rust_package(dart_package: &str, rust_dir: &Path, manifest: &Path) -> Result<RustPackage> {
    let metadata = MetadataCommand::new()
        .manifest_path(manifest)
        .no_deps()
        .exec()
        .with_context(|| format!("unable to read {}", manifest.display()))?;
    let wanted = manifest.canonicalize()?;
    let package: &Package = metadata
        .packages
        .iter()
        .find(|p| {
            p.manifest_path
                .as_std_path()
                .canonicalize()
                .is_ok_and(|m| m == wanted)
        })
        .ok_or_else(|| anyhow!("cargo metadata did not describe {}", manifest.display()))?;
    let flutter = package.metadata.get("flutter");

    if flutter
        .and_then(|f| f.get("plugin"))
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        return read_native_library(dart_package, package, manifest, flutter);
    }
    let registrar = flutter
        .and_then(|f| f.get("registrar"))
        .and_then(|v| v.as_str())
        .filter(|r| is_rust_path(r))
        .ok_or_else(|| {
            anyhow!(
                "Rust plugin {dart_package} must provide a valid [package.metadata.flutter] registrar"
            )
        })?;
    Ok(RustPackage::Plugin(RustPlugin {
        dart_package: dart_package.to_owned(),
        cargo_package: package.name.to_string(),
        registrar: registrar.to_owned(),
        rust_dir: rust_dir.to_owned(),
    }))
}

fn read_native_library(
    dart_package: &str,
    package: &Package,
    manifest: &Path,
    flutter: Option<&serde_json::Value>,
) -> Result<RustPackage> {
    let Some(target) = package
        .targets
        .iter()
        .find(|t| t.crate_types.iter().any(|c| c.to_string() == "cdylib"))
    else {
        return Ok(RustPackage::Other);
    };
    let rustflags = match flutter.and_then(|f| f.get("rustflags")) {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_owned)).collect())
            .ok_or_else(|| {
                anyhow!(
                    "Rust package {dart_package} must provide [package.metadata.flutter] rustflags \
                     as a list of strings"
                )
            })?,
    };
    Ok(RustPackage::Native(NativeLibrary {
        dart_package: dart_package.to_owned(),
        manifest_path: manifest.to_owned(),
        library_name: target.name.replace('-', "_"),
        rustflags,
    }))
}

pub fn discover(project: &Project, mode: BuildMode) -> Result<Discovered> {
    let mut found = Discovered::default();
    for (name, dir) in dependency_dirs(project, mode)? {
        if name == project.name {
            continue;
        }
        let rust_dir = dir.join("rust");
        let manifest = rust_dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        match read_rust_package(&name, &rust_dir, &manifest)? {
            RustPackage::Plugin(plugin) => found.plugins.push(plugin),
            RustPackage::Native(library) => found.native_libraries.push(library),
            RustPackage::Other => {}
        }
    }
    found
        .plugins
        .sort_by(|a, b| a.dart_package.cmp(&b.dart_package));
    found
        .native_libraries
        .sort_by(|a, b| a.dart_package.cmp(&b.dart_package));
    Ok(found)
}

// --- Generated files ---------------------------------------------------------

/// Replaces the generated plugin section of `runner-rs/Cargo.toml`.
pub fn replace_generated_dependencies(manifest: &str, dependencies: &[String]) -> Result<String> {
    let invalid = || {
        anyhow!(
            "runner-rs/Cargo.toml must contain one valid Flutter generated Rust plugins section"
        )
    };
    let begin = manifest.find(PLUGINS_BEGIN).ok_or_else(invalid)?;
    let end = manifest.find(PLUGINS_END).ok_or_else(invalid)?;
    if begin >= end {
        return Err(invalid());
    }
    if manifest[begin + 1..].contains(PLUGINS_BEGIN) || manifest[end + 1..].contains(PLUGINS_END) {
        bail!("runner-rs/Cargo.toml contains more than one Flutter generated Rust plugins section");
    }
    let content_start = begin + PLUGINS_BEGIN.len();
    let generated = if dependencies.is_empty() {
        "\n".to_owned()
    } else {
        format!("\n{}\n", dependencies.join("\n"))
    };
    Ok(format!(
        "{}{generated}{}",
        &manifest[..content_start],
        &manifest[end..]
    ))
}

pub fn dependency_line(plugin: &RustPlugin) -> String {
    format!(
        "{} = {{ package = \"{}\", path = \"../.dart_tool/flutter_rs/plugins/{}\" }}",
        plugin.dependency_alias(),
        plugin.cargo_package,
        plugin.dart_package
    )
}

pub fn registrant(plugins: &[RustPlugin]) -> String {
    let mut out = String::from(
        "// Generated by Flutter. Do not edit.\n\n\
         use flutter_plugin_sdk::{PluginRegistrar, Result};\n\n\
         pub(crate) fn register_plugins(registrar: &mut PluginRegistrar) -> Result<()> {\n",
    );
    if plugins.is_empty() {
        out.push_str("    let _ = registrar;\n");
    }
    for plugin in plugins {
        out.push_str(&format!(
            "    {}::{}(registrar)?;\n",
            plugin.dependency_alias(),
            plugin.registrar
        ));
    }
    out.push_str("    Ok(())\n}\n");
    out
}

fn has_runner_version(file: &Path) -> bool {
    let marker = format!("{RUNNER_VERSION_MARKER}{RUNNER_VERSION}");
    fs::read_to_string(file).is_ok_and(|text| text.lines().any(|line| line.trim_end() == marker))
}

/// Rewrites the tool-owned runner files when their version marker is absent or
/// old, e.g. for projects created by an older tool.
fn regenerate_runner_if_outdated(project: &Project) -> Result<()> {
    let runner = project.runner_dir();
    let build_rs = runner.join("build.rs");
    let main_rs = runner.join("src").join("main.rs");
    if has_runner_version(&build_rs) && has_runner_version(&main_rs) {
        return Ok(());
    }
    write_if_changed(&build_rs, TEMPLATE_BUILD_RS)?;
    write_if_changed(
        &main_rs,
        &TEMPLATE_MAIN_RS.replace("{{projectName}}", &project.name),
    )?;
    eprintln!(
        "Regenerated runner-rs/build.rs and runner-rs/src/main.rs (runner version {RUNNER_VERSION})."
    );
    Ok(())
}

/// Brings the runner project in line with the app's current Rust plugins.
/// Requires the SDK ([`crate::sdk`]) and `flutter pub get` to have run.
pub fn refresh(project: &Project, mode: BuildMode) -> Result<Discovered> {
    let runner = project.runner_dir();
    let cargo_manifest = runner.join("Cargo.toml");
    if !cargo_manifest.is_file() {
        bail!("Rust-shell project is missing {}", cargo_manifest.display());
    }
    regenerate_runner_if_outdated(project)?;
    let found = discover(project, mode)?;
    write_native_libraries(project, &found.native_libraries)?;

    let links = project.flutter_rs_dir().join("plugins");
    fs::create_dir_all(&links)?;
    for plugin in &found.plugins {
        replace_link(&links.join(&plugin.dart_package), &plugin.rust_dir)?;
    }
    let keep: BTreeSet<&str> = found
        .plugins
        .iter()
        .map(|p| p.dart_package.as_str())
        .collect();
    for entry in fs::read_dir(&links)? {
        let entry = entry?;
        if !keep.contains(entry.file_name().to_string_lossy().as_ref()) {
            remove_path(&entry.path())?;
        }
    }

    let old = fs::read_to_string(&cargo_manifest)?;
    let lines: Vec<String> = found.plugins.iter().map(dependency_line).collect();
    write_if_changed(
        &cargo_manifest,
        &replace_generated_dependencies(&old, &lines)?,
    )?;
    write_if_changed(
        &runner.join("src").join("flutter_plugins.rs"),
        &registrant(&found.plugins),
    )?;
    if !found.plugins.is_empty() {
        validate_sdk_resolution(&cargo_manifest, &runner)?;
    }
    Ok(found)
}

/// Every plugin must resolve the same single `flutter-plugin-sdk`; two copies
/// would mean mismatched shell ABIs.
fn validate_sdk_resolution(manifest: &Path, runner: &Path) -> Result<()> {
    let metadata = MetadataCommand::new()
        .manifest_path(manifest)
        .current_dir(runner)
        .exec()
        .context("unable to resolve Rust-shell dependencies")?;
    let count = metadata
        .packages
        .iter()
        .filter(|p| p.name.as_str() == "flutter-plugin-sdk")
        .count();
    if count != 1 {
        bail!(
            "Rust-shell applications must resolve exactly one flutter-plugin-sdk package; Cargo \
             resolved {count}."
        );
    }
    Ok(())
}

// --- Native libraries --------------------------------------------------------

pub fn native_libraries_file(project: &Project) -> PathBuf {
    project.flutter_rs_dir().join("native_libs.json")
}

fn write_native_libraries(project: &Project, libraries: &[NativeLibrary]) -> Result<()> {
    let file = native_libraries_file(project);
    if libraries.is_empty() {
        return remove_path(&file);
    }
    write_if_changed(&file, &serde_json::to_string_pretty(libraries)?)?;
    Ok(())
}

pub fn read_native_libraries(project: &Project) -> Result<Vec<NativeLibrary>> {
    let file = native_libraries_file(project);
    if !file.is_file() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&fs::read_to_string(&file)?)
        .with_context(|| format!("invalid {}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(json: &str) -> PackageGraph {
        serde_json::from_str(json).unwrap()
    }

    const GRAPH: &str = r#"{
      "roots": ["app"],
      "packages": [
        {"name": "app", "dependencies": ["flutter", "plugin_a"], "devDependencies": ["dev_tool"]},
        {"name": "flutter", "dependencies": ["meta"]},
        {"name": "meta", "dependencies": []},
        {"name": "plugin_a", "dependencies": ["meta", "plugin_b"]},
        {"name": "plugin_b", "dependencies": []},
        {"name": "dev_tool", "dependencies": ["dev_only_dep"]},
        {"name": "dev_only_dep", "dependencies": []}
      ]
    }"#;

    #[test]
    fn debug_includes_dev_dependencies_transitively() {
        let names = dependency_names(&graph(GRAPH), true).unwrap();
        assert!(names.contains("dev_tool") && names.contains("dev_only_dep"));
        assert!(names.contains("plugin_b") && !names.contains("app"));
    }

    #[test]
    fn release_excludes_dev_only_packages() {
        let names = dependency_names(&graph(GRAPH), false).unwrap();
        assert!(!names.contains("dev_tool") && !names.contains("dev_only_dep"));
        assert!(names.contains("plugin_b") && names.contains("meta"));
    }

    #[test]
    fn root_uris_resolve_absolute_and_relative() {
        let config_dir = Path::new("/proj/.dart_tool");
        assert_eq!(
            resolve_root_uri(config_dir, "file:///home/me/.pub-cache/a%20b-1.0"),
            PathBuf::from("/home/me/.pub-cache/a b-1.0")
        );
        assert_eq!(resolve_root_uri(config_dir, "../"), PathBuf::from("/proj"));
        assert_eq!(
            resolve_root_uri(config_dir, "../../plugins/x"),
            PathBuf::from("/plugins/x")
        );
    }

    #[test]
    fn percent_decoding_survives_malformed_and_multibyte_input() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%é"), "%zz%é");
    }

    #[test]
    fn rust_paths_are_validated() {
        assert!(is_rust_path("my_plugin::register"));
        assert!(is_rust_path("r#type::go"));
        for bad in ["", "9lives", "a::", "a-b::c", "a b"] {
            assert!(!is_rust_path(bad), "{bad:?}");
        }
    }

    const MANIFEST: &str = "[dependencies]\nsdk = 1\n\n# === BEGIN FLUTTER GENERATED RUST PLUGINS ===\n# === END FLUTTER GENERATED RUST PLUGINS ===\n\n[patch]\n";

    #[test]
    fn generated_section_is_replaced_and_restored() {
        let lines = vec!["a = 1".to_owned(), "b = 2".to_owned()];
        let with = replace_generated_dependencies(MANIFEST, &lines).unwrap();
        assert!(with.contains("BEGIN FLUTTER GENERATED RUST PLUGINS ===\na = 1\nb = 2\n# === END"));
        assert!(with.starts_with("[dependencies]\nsdk = 1\n") && with.ends_with("[patch]\n"));
        assert_eq!(
            replace_generated_dependencies(&with, &[]).unwrap(),
            MANIFEST
        );
    }

    #[test]
    fn generated_section_must_be_unique_and_ordered() {
        assert!(replace_generated_dependencies("[dependencies]\n", &[]).is_err());
        let reversed = format!("{PLUGINS_END}\n{PLUGINS_BEGIN}\n");
        assert!(replace_generated_dependencies(&reversed, &[]).is_err());
        let twice = format!("{MANIFEST}{MANIFEST}");
        let error = replace_generated_dependencies(&twice, &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("more than one"), "{error}");
    }

    #[test]
    fn registrant_calls_each_plugin_in_order() {
        let plugin = |name: &str| RustPlugin {
            dart_package: name.to_owned(),
            cargo_package: format!("{name}-rs"),
            registrar: "register".to_owned(),
            rust_dir: PathBuf::from("/x"),
        };
        let text = registrant(&[plugin("a"), plugin("b")]);
        assert!(text.contains("flutter_rs_plugin_a::register(registrar)?;\n    flutter_rs_plugin_b::register(registrar)?;"));
        assert!(!text.contains("let _ = registrar"));
        assert!(registrant(&[]).contains("let _ = registrar;"));
        assert_eq!(
            dependency_line(&plugin("a")),
            "flutter_rs_plugin_a = { package = \"a-rs\", path = \"../.dart_tool/flutter_rs/plugins/a\" }"
        );
    }

    #[test]
    fn native_libraries_use_the_json_keys_the_dart_tool_wrote() {
        let text =
            r#"[{"package":"p","manifest":"/m/Cargo.toml","library":"p_ffi","rustflags":["-Cx"]}]"#;
        let libs: Vec<NativeLibrary> = serde_json::from_str(text).unwrap();
        assert_eq!(libs[0].file_name(), "libp_ffi.so");
        assert_eq!(libs[0].rustflags, ["-Cx"]);
    }

    #[test]
    fn templates_carry_the_current_runner_version() {
        let marker = format!("{RUNNER_VERSION_MARKER}{RUNNER_VERSION}");
        for template in [TEMPLATE_BUILD_RS, TEMPLATE_MAIN_RS] {
            assert!(template.lines().any(|l| l == marker));
        }
    }
}
