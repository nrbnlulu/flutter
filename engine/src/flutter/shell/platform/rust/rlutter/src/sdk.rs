//! The per-project Rust-shell SDK under `.dart_tool/flutter_rs/sdk`.
//!
//! ```text
//! sdk/
//!   Cargo.toml, Cargo.lock      workspace manifest for the shell crates
//!   crates/, third_party/       shell sources (links into a local checkout or global cache, or unpacked)
//!   engine/src/out/<variant>/   engine-shaped directory for `flutter --local-engine`
//!   lib/<platform>-<mode>       link to the variant above; what runner-rs/build.rs links
//! ```
//!
//! Stock `flutter` accepts `--local-engine=<abs>/src/out/<variant>`, so a
//! directory of links into the real engine build makes it compile kernels
//! with the Dart SDK that matches the engine.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};

use crate::{
    abi::Abi,
    flutter::Flutter,
    mode::BuildMode,
    project::Project,
    util::{copy_dir, copy_if_changed, remove_path, replace_link},
};

const BETA_RELEASE_URL: &str = "https://github.com/nrbnlulu/flutter/releases/download/BETA";

/// What `flutter --local-engine` needs from an engine build, relative to its
/// output directory. Missing entries are skipped (a host engine has no
/// `clang_x64`, and so on); required ones are checked by [`ENGINE_REQUIRED`].
const ENGINE_ENTRIES: &[&str] = &[
    "engine-manifest.json",
    "dart-sdk",
    "flutter_patched_sdk",
    "frontend_server_aot.dart.snapshot",
    "gen_snapshot",
    "font-subset",
    "impellerc",
    "shader_lib",
    "libtessellator.so",
    "libtessellator.dylib",
    "gen/dart-pkg",
    // Android engines keep the host-run AOT compiler here.
    "clang_x64/gen_snapshot",
    "icudtl.dat",
    "libflutter_rust_engine.so",
    "libflutter_rust_engine.dylib",
    "flutter_rust_engine.dll",
    "flutter_rust_engine.dll.lib",
    "gen_snapshot.exe",
    "font-subset.exe",
    "impellerc.exe",
    "libtessellator.dll",
    // Standard Android embedding and its local Maven artifacts.
    "libflutter.so",
    "flutter.jar",
    "flutter_embedding_debug.jar",
    "flutter_embedding_debug.pom",
    "flutter_embedding_debug.maven-metadata.xml",
    "flutter_embedding_release.jar",
    "flutter_embedding_release.pom",
    "flutter_embedding_release.maven-metadata.xml",
    "arm64_v8a_debug.jar",
    "arm64_v8a_debug.pom",
    "arm64_v8a_debug.maven-metadata.xml",
    "arm64_v8a_release.jar",
    "arm64_v8a_release.pom",
    "arm64_v8a_release.maven-metadata.xml",
    // Debug bundles copy the VM snapshots from here.
    "gen/flutter/lib/snapshot",
];
pub fn host_engine_library() -> &'static str {
    if cfg!(windows) {
        "flutter_rust_engine.dll"
    } else if cfg!(target_os = "macos") {
        "libflutter_rust_engine.dylib"
    } else {
        "libflutter_rust_engine.so"
    }
}
/// The host's Flutter target platform name, also used for the BETA SDK
/// archive and the runner's `lib/<platform>-<mode>` link.
pub(crate) fn host_platform() -> &'static str {
    if cfg!(windows) {
        "windows-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else if cfg!(target_os = "macos") {
        "darwin-x64"
    } else {
        "linux-x64"
    }
}

/// GN output directory names for a host engine build of `mode`, most specific
/// first. macOS builds carry the CPU (and debug builds are usually
/// unoptimized), so `host_debug_unopt_arm64` is a debug candidate there.
fn host_engine_candidates(mode: &str) -> Vec<String> {
    let mut candidates = vec![format!("host_{mode}")];
    if cfg!(target_os = "macos") {
        let cpu = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        candidates.push(format!("host_{mode}_{cpu}"));
        if mode == "debug" {
            candidates.push(format!("host_debug_unopt_{cpu}"));
        }
    }
    candidates
}

const ENGINE_REQUIRED: &[&str] = &[
    if cfg!(target_os = "macos") {
        "libflutter_rust_engine.dylib"
    } else {
        "libflutter_rust_engine.so"
    },
    "icudtl.dat",
];
fn host_bundle_modes() -> &'static [&'static str] {
    if cfg!(windows) {
        &["debug", "release"]
    } else {
        &["debug", "profile", "release"]
    }
}
fn host_required() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "flutter_rust_engine.dll",
            "flutter_rust_engine.dll.lib",
            "icudtl.dat",
        ]
    } else {
        ENGINE_REQUIRED
    }
}

/// Where the user's engine builds come from.
#[derive(Debug, Clone, Default)]
pub struct EngineSources {
    /// `<...>/engine/src`, overriding the checkout inside the Flutter SDK.
    pub local_engine_src: Option<PathBuf>,
}

/// An engine-shaped directory, as passed to `flutter --local-engine*`.
#[derive(Debug, Clone)]
pub struct Engine {
    pub target: PathBuf,
    pub host: PathBuf,
}

/// Makes the shell crates available in the project's SDK directory.
pub fn ensure_sources(project: &Project, flutter: &Flutter, sources: &EngineSources) -> Result<()> {
    let sdk = project.sdk_dir();
    fs::create_dir_all(&sdk)?;
    ensure_shell_sources(&sdk, flutter, sources)
}

/// The host engine layout; also the compiler side of every Android build.
pub fn link_host(
    project: &Project,
    flutter: &Flutter,
    sources: &EngineSources,
    mode: BuildMode,
) -> Result<Engine> {
    let variant = format!("host_{}", mode.name());
    // On macOS, local builds are also named host_<mode>_<cpu> or
    // host_debug_unopt_<cpu>; check all of them before falling back.
    let candidates = host_engine_candidates(mode.name());
    let fallback = if find_engine_build(&candidates, None, flutter, sources).is_some() {
        None
    } else {
        let project_engine = project.sdk_dir().join("lib").join(mode.name());
        if project_engine.join(host_engine_library()).is_file() {
            Some(project_engine)
        } else {
            let cache = crate::flutter::cache_dir()?;
            Some(
                ensure_managed_bundle(&cache, download_bundle)?
                    .join("lib")
                    .join(mode.name()),
            )
        }
    };
    let layout = link_variant(
        project,
        flutter,
        sources,
        &variant,
        &candidates,
        fallback,
        &format!("{}-{}", host_platform(), mode.name()),
    )?;
    Ok(Engine {
        target: layout.clone(),
        host: layout,
    })
}

/// The engine layout for one Android ABI, compiled against `host`.
pub fn link_android(
    project: &Project,
    flutter: &Flutter,
    sources: &EngineSources,
    mode: BuildMode,
    abi: Abi,
    host: &Engine,
) -> Result<Engine> {
    let suffix = abi.engine_suffix();
    let variant = format!("android_{}_{suffix}", mode.name());
    // Debug engines are usually built unoptimized.
    let mut candidates = vec![variant.clone()];
    if mode == BuildMode::Debug {
        candidates.push(format!("android_debug_unopt_{suffix}"));
    }
    // Without a local build, use the prebuilt engine from the BETA release.
    let fallback = if find_engine_build(&candidates, None, flutter, sources).is_some() {
        None
    } else {
        let project_engine = project
            .sdk_dir()
            .join("lib")
            .join(abi.flutter_platform())
            .join(mode.name());
        if android_bundle_complete(&project_engine, abi, mode) {
            Some(project_engine)
        } else {
            let cache = crate::flutter::cache_dir()?;
            Some(ensure_cached_android_bundle(&cache, abi, mode)?)
        }
    };
    let target = link_variant(
        project,
        flutter,
        sources,
        &variant,
        &candidates,
        fallback,
        &format!("{}-{}", abi.flutter_platform(), mode.name()),
    )?;
    Ok(Engine {
        target,
        host: host.host.clone(),
    })
}

/// Links the first existing build among `candidates` as `layout_name` under
/// `engine/src/out`, plus `lib/<lib_name>` for the runner's build.rs.
fn link_variant(
    project: &Project,
    flutter: &Flutter,
    sources: &EngineSources,
    layout_name: &str,
    candidates: &[String],
    fallback: Option<PathBuf>,
    lib_name: &str,
) -> Result<PathBuf> {
    let sdk = project.sdk_dir();
    let source = find_engine_build(candidates, fallback, flutter, sources).ok_or_else(|| {
        anyhow!(
            "no {layout_name} engine build found (looked for {} under {}). Build it first, or \
             pass --local-engine-src.",
            candidates.join(", "),
            engine_src(flutter, sources).join("out").display()
        )
    })?;
    let layout = sdk.join("engine").join("src").join("out").join(layout_name);
    link_engine_build(&source, &layout)?;

    // runner-rs/build.rs links lib/<platform>-<mode>/libflutter_rust_engine.so.
    replace_link(
        &sdk.join("lib").join(lib_name),
        &Path::new("..")
            .join("engine")
            .join("src")
            .join("out")
            .join(layout_name),
    )?;
    Ok(layout)
}

fn engine_src(flutter: &Flutter, sources: &EngineSources) -> PathBuf {
    sources
        .local_engine_src
        .clone()
        .unwrap_or_else(|| flutter.root().join("engine").join("src"))
}

fn local_shell_dir(flutter: &Flutter, sources: &EngineSources) -> PathBuf {
    engine_src(flutter, sources).join("flutter/shell/platform/rust")
}

fn find_engine_build(
    candidates: &[String],
    fallback: Option<PathBuf>,
    flutter: &Flutter,
    sources: &EngineSources,
) -> Option<PathBuf> {
    let out = engine_src(flutter, sources).join("out");
    candidates
        .iter()
        .map(|name| out.join(name))
        .chain(fallback)
        .find(|dir| {
            if candidates
                .first()
                .is_some_and(|name| name.starts_with("android_"))
            {
                dir.join("libflutter.so").is_file()
            } else {
                dir.join(host_engine_library()).is_file()
            }
        })
}

/// Fills `layout` with links to `source`'s engine files.
fn link_engine_build(source: &Path, layout: &Path) -> Result<()> {
    let source = source.canonicalize()?;
    fs::create_dir_all(layout)?;
    for entry in ENGINE_ENTRIES {
        let from = source.join(entry);
        if from.exists() {
            let destination = layout.join(entry);
            // The Windows AOT frontend's SDK-root validation rejects a directory
            // symlink here even though dart:io Directory.exists follows it.
            // Keep this small SDK directory physical; native libraries and the
            // large Dart SDK can retain their ordinary links.
            if cfg!(windows) && *entry == "flutter_patched_sdk" {
                if destination.is_symlink() {
                    remove_path(&destination)?;
                }
                copy_dir(&from, &destination)?;
            } else {
                replace_link(&destination, &from)?;
            }
        }
    }
    let required: &[&str] = if source.join("libflutter.so").is_file() {
        &["libflutter.so", "flutter.jar"]
    } else if cfg!(windows) {
        &[
            "flutter_rust_engine.dll",
            "flutter_rust_engine.dll.lib",
            "icudtl.dat",
        ]
    } else {
        ENGINE_REQUIRED
    };
    for entry in required {
        if !layout.join(entry).exists() {
            bail!("{} is missing {entry}", source.display());
        }
    }
    Ok(())
}

/// Makes `sdk` contain the shell crates, from the local checkout when there is
/// one and otherwise from the globally cached release bundle.
fn ensure_shell_sources(sdk: &Path, flutter: &Flutter, sources: &EngineSources) -> Result<()> {
    let cache = crate::flutter::cache_dir()?;
    ensure_shell_sources_with(sdk, &cache, flutter, sources, download_bundle)
}

fn sync_shell_sources(sdk: &Path, source: &Path) -> Result<()> {
    fs::create_dir_all(sdk)?;
    copy_if_changed(&source.join("Cargo.toml"), &sdk.join("Cargo.toml"))?;
    if source.join("Cargo.lock").is_file() {
        copy_if_changed(&source.join("Cargo.lock"), &sdk.join("Cargo.lock"))?;
    }
    for optional in ["rust-toolchain.toml", "sdk-manifest.json"] {
        if source.join(optional).is_file() {
            copy_if_changed(&source.join(optional), &sdk.join(optional))?;
        }
    }
    replace_link(&sdk.join("crates"), &source.join("crates"))?;
    replace_link(&sdk.join("third_party"), &source.join("third_party"))?;
    for dir in ["dart", "docs", "skills", "tests"] {
        if source.join(dir).is_dir() {
            replace_link(&sdk.join(dir), &source.join(dir))?;
        }
    }
    Ok(())
}

fn ensure_shell_sources_with(
    sdk: &Path,
    cache: &Path,
    flutter: &Flutter,
    sources: &EngineSources,
    download: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let local = local_shell_dir(flutter, sources);
    if local.join("crates").is_dir() {
        // Always re-sync: crates from another checkout or bundle would not
        // match the engine library linked from this one (misaligned shell ABI).
        return sync_shell_sources(sdk, &local);
    }
    if release_bundle_complete(sdk) {
        return Ok(());
    }
    let bundle = ensure_managed_bundle(cache, download)?;
    sync_shell_sources(sdk, &bundle)
}

fn ensure_managed_bundle(
    cache: &Path,
    download: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let root = cache.join("rust-BETA");
    if release_bundle_complete(&root) {
        return Ok(root);
    }
    fs::create_dir_all(cache)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache.join("rust.lock"))?;
    lock.lock()?;
    if release_bundle_complete(&root) {
        return Ok(root);
    }
    let staging = cache.join(format!(".rust-download-{}", std::process::id()));
    remove_path(&staging)?;
    let result = (|| {
        download(&staging)?;
        if !release_bundle_complete(&staging) {
            bail!("the Flutter Rust shell BETA SDK archive is incomplete");
        }
        if let Ok(entries) = fs::read_dir(root.join("lib")) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("android-") {
                    let dest = staging.join("lib").join(&name);
                    if !dest.exists() {
                        let _ = fs::rename(entry.path(), dest);
                    }
                }
            }
        }
        remove_path(&root)?;
        fs::rename(&staging, &root)?;
        Ok(root)
    })();
    let _ = remove_path(&staging);
    result
}

// Sources alone may belong to a previous local-checkout setup or an interrupted
// download. Android also needs the Linux host compiler, in both build modes.
fn release_bundle_complete(sdk: &Path) -> bool {
    sdk.join("Cargo.toml").is_file()
        && sdk.join("crates/flutter-shell-agent/Cargo.toml").is_file()
        && sdk
            .join("crates/flutter-shell-winit/src/headless/recording.rs")
            .is_file()
        && sdk
            .join("dart/flutter_rust_agent/lib/flutter_rust_agent.dart")
            .is_file()
        && sdk.join("third_party/wgpu-hal/Cargo.toml").is_file()
        && sdk.join("crates/flutter-plugin-sdk/Cargo.toml").is_file()
        && sdk
            .join("crates/flutter-shell-android/Cargo.toml")
            .is_file()
        && !sdk.join("crates").is_symlink()
        && !sdk.join("third_party").is_symlink()
        && host_bundle_modes().iter().all(|mode| {
            let engine = sdk.join("lib").join(mode);
            host_required()
                .iter()
                .all(|entry| engine.join(entry).is_file())
                && engine
                    .join(if cfg!(windows) {
                        "dart-sdk/bin/dart.exe"
                    } else {
                        "dart-sdk/bin/dart"
                    })
                    .is_file()
                && engine.join("flutter_patched_sdk").is_dir()
                && engine.join("gen/dart-pkg/sky_engine").is_dir()
        })
}

fn download_bundle(sdk: &Path) -> Result<()> {
    eprintln!("Downloading the Flutter Rust shell BETA SDK...");
    remove_path(sdk)?;
    download_and_unpack(
        &format!("{BETA_RELEASE_URL}/flutter-rust-{}.tar.gz", host_platform()),
        sdk,
    )?;
    if !release_bundle_complete(sdk) {
        bail!("the Flutter Rust shell BETA SDK archive is incomplete");
    }
    Ok(())
}

/// The release asset holding the engine builds of one Android ABI: a
/// `<mode>/` directory per build mode, each shaped like an engine out dir.
fn android_bundle_url(abi: Abi) -> String {
    format!(
        "{BETA_RELEASE_URL}/flutter-rust-{}.tar.gz",
        abi.flutter_platform()
    )
}

fn ensure_cached_android_bundle(cache: &Path, abi: Abi, mode: BuildMode) -> Result<PathBuf> {
    let root = cache.join("rust-BETA");
    let engine = root
        .join("lib")
        .join(abi.flutter_platform())
        .join(mode.name());
    if android_bundle_complete(&engine, abi, mode) {
        return Ok(engine);
    }
    fs::create_dir_all(cache)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache.join("rust.lock"))?;
    lock.lock()?;
    ensure_android_bundle(&root, abi, mode)
}

/// Returns `<sdk>/lib/<platform>/<mode>`, the prebuilt Android engine,
/// downloading the ABI's bundle first unless it is already there.
fn ensure_android_bundle(sdk: &Path, abi: Abi, mode: BuildMode) -> Result<PathBuf> {
    let bundle = sdk.join("lib").join(abi.flutter_platform());
    let engine = bundle.join(mode.name());
    if android_bundle_complete(&engine, abi, mode) {
        return Ok(engine);
    }
    let url = android_bundle_url(abi);
    eprintln!(
        "Downloading the {} engine from the BETA release...",
        abi.flutter_platform()
    );
    remove_path(&bundle)?;
    download_and_unpack(&url, &bundle)?;
    if !android_bundle_complete(&engine, abi, mode) {
        bail!(
            "{url} has an incomplete {} Android embedding; rebuild/publish the Android release bundle",
            mode.name()
        );
    }
    Ok(engine)
}

fn android_bundle_complete(engine: &Path, abi: Abi, mode: BuildMode) -> bool {
    let native = format!("{}_{}", abi.android_name().replace('-', "_"), mode.name());
    let embedding = format!("flutter_embedding_{}", mode.name());
    [
        "libflutter.so",
        "flutter.jar",
        "flutter_patched_sdk/platform_strong.dill",
    ]
    .iter()
    .all(|entry| engine.join(entry).is_file())
        && [&native, &embedding].iter().all(|artifact| {
            ["jar", "pom", "maven-metadata.xml"]
                .iter()
                .all(|extension| engine.join(format!("{artifact}.{extension}")).is_file())
        })
        && (mode.config().dart_aot
            || ["vm_isolate_snapshot.bin", "isolate_snapshot.bin"]
                .iter()
                .all(|name| engine.join("gen/flutter/lib/snapshot").join(name).is_file()))
        && (!mode.config().dart_aot || engine.join("clang_x64/gen_snapshot").is_file())
}

/// Local Maven artifacts must come from the same fork build as libflutter.so;
/// an upstream embedding jar has neither the matching Dart VM nor Rust hooks.
pub fn android_maven_repository(
    project: &Project,
    engine: &Engine,
    mode: BuildMode,
    abi: Abi,
) -> Result<PathBuf> {
    if !android_bundle_complete(&engine.target, abi, mode) {
        bail!(
            "{} is missing Android embedding artifacts. Build flutter/shell/platform/android:android_jar, :pom_embedding and :pom_libflutter before running rlutter",
            engine.target.display()
        );
    }
    let repository = project
        .flutter_rs_dir()
        .join("android")
        .join(format!("maven-{}", mode.name()));
    remove_path(&repository)?;
    for artifact in [
        format!("flutter_embedding_{}", mode.name()),
        format!("{}_{}", abi.android_name().replace('-', "_"), mode.name()),
    ] {
        let pom = fs::read_to_string(engine.target.join(format!("{artifact}.pom")))?;
        let version = pom
            .split_once("<version>")
            .and_then(|(_, rest)| rest.split_once("</version>"))
            .map(|(v, _)| v.trim())
            .context("Android engine POM has no version")?;
        if version.is_empty()
            || !version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            bail!("invalid Android engine artifact version {version:?}");
        }
        let directory = repository.join("io/flutter").join(&artifact);
        for extension in ["jar", "pom"] {
            replace_link(
                &directory
                    .join(version)
                    .join(format!("{artifact}-{version}.{extension}")),
                &engine.target.join(format!("{artifact}.{extension}")),
            )?;
        }
        replace_link(
            &directory.join("maven-metadata.xml"),
            &engine.target.join(format!("{artifact}.maven-metadata.xml")),
        )?;
    }
    Ok(repository)
}

pub(crate) fn download_and_unpack(url: &str, dest: &Path) -> Result<()> {
    let response = ureq::get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let progress = match total {
        Some(total) => ProgressBar::new(total),
        None => ProgressBar::new_spinner(),
    };
    progress.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=> "),
    );
    fs::create_dir_all(dest)?;
    let result = tar::Archive::new(GzDecoder::new(
        progress.wrap_read(response.into_body().into_reader()),
    ))
    .unpack(dest)
    .with_context(|| format!("cannot unpack {url}"));
    progress.finish_and_clear();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    fn fake_engine(dir: &Path) {
        fs::create_dir_all(dir.join("gen/flutter/lib/snapshot")).unwrap();
        fs::create_dir_all(dir.join("flutter_patched_sdk")).unwrap();
        for file in [
            "libflutter_rust_engine.so",
            "libflutter_rust_engine.dylib",
            "flutter_rust_engine.dll",
            "flutter_rust_engine.dll.lib",
            "libflutter.so",
            "flutter.jar",
            "icudtl.dat",
            "gen_snapshot",
            "font-subset",
            "impellerc",
        ] {
            fs::write(dir.join(file), "").unwrap();
        }
    }

    fn fake_release_bundle(sdk: &Path) {
        fs::create_dir_all(sdk.join("third_party/wgpu-hal")).unwrap();
        fs::write(sdk.join("third_party/wgpu-hal/Cargo.toml"), "").unwrap();
        fs::create_dir_all(sdk.join("crates/flutter-shell-agent")).unwrap();
        fs::create_dir_all(sdk.join("crates/flutter-shell-winit/src/headless")).unwrap();
        fs::create_dir_all(sdk.join("dart/flutter_rust_agent/lib")).unwrap();
        fs::write(sdk.join("crates/flutter-shell-agent/Cargo.toml"), "").unwrap();
        fs::write(
            sdk.join("crates/flutter-shell-winit/src/headless/recording.rs"),
            "",
        )
        .unwrap();
        fs::write(
            sdk.join("dart/flutter_rust_agent/lib/flutter_rust_agent.dart"),
            "",
        )
        .unwrap();
        fs::create_dir_all(sdk.join("crates/flutter-plugin-sdk")).unwrap();
        fs::create_dir_all(sdk.join("crates/flutter-shell-android")).unwrap();
        fs::write(sdk.join("crates/flutter-shell-android/Cargo.toml"), "").unwrap();
        fs::write(sdk.join("Cargo.toml"), "").unwrap();
        fs::write(sdk.join("crates/flutter-plugin-sdk/Cargo.toml"), "").unwrap();
        for mode in ["debug", "profile", "release"] {
            let engine = sdk.join("lib").join(mode);
            fake_engine(&engine);
            fs::create_dir_all(engine.join("dart-sdk/bin")).unwrap();
            fs::write(
                engine.join(if cfg!(windows) {
                    "dart-sdk/bin/dart.exe"
                } else {
                    "dart-sdk/bin/dart"
                }),
                "",
            )
            .unwrap();
            fs::create_dir_all(engine.join("gen/dart-pkg/sky_engine")).unwrap();
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_distribution_requires_debug_and_release_import_libraries() {
        let root = tempdir("windows_release_bundle");
        fake_release_bundle(&root);
        remove_path(&root.join("lib/profile")).unwrap();
        assert!(release_bundle_complete(&root));
        remove_path(&root.join("lib/release/flutter_rust_engine.dll.lib")).unwrap();
        assert!(!release_bundle_complete(&root));
    }

    #[test]
    fn incomplete_release_cache_is_refreshed_before_linking_host() {
        let root = tempdir("sdk_incomplete_release");
        let cache = root.join("cache");
        let cached_sdk = cache.join("rust-BETA");
        let sdk = root.join("sdk");
        let flutter = flutter_at(&root.join("flutter"));
        fake_release_bundle(&cached_sdk);
        // Reproduce a cache with valid sources but no host debug engine.
        remove_path(&cached_sdk.join("lib/debug")).unwrap();
        ensure_shell_sources_with(&sdk, &cache, &flutter, &EngineSources::default(), |dest| {
            fake_release_bundle(dest);
            Ok(())
        })
        .unwrap();
        assert!(release_bundle_complete(&cached_sdk));
        assert!(sdk.join("crates/flutter-plugin-sdk/Cargo.toml").is_file());
        let source = find_engine_build(
            &["host_debug".to_owned()],
            Some(cached_sdk.join("lib/debug")),
            &flutter,
            &EngineSources::default(),
        )
        .unwrap();
        link_engine_build(&source, &sdk.join("engine/src/out/host_debug")).unwrap();
        let sdk2 = root.join("sdk2");
        ensure_shell_sources_with(&sdk2, &cache, &flutter, &EngineSources::default(), |_| {
            panic!("a complete global bundle must not be downloaded again")
        })
        .unwrap();
        assert!(sdk2.join("crates/flutter-plugin-sdk/Cargo.toml").is_file());
    }

    #[test]
    fn release_cache_rejects_sources_linked_to_another_checkout() {
        let root = tempdir("sdk_stale_sources");
        let sdk = root.join("sdk");
        fake_release_bundle(&sdk);
        fs::rename(sdk.join("crates"), root.join("old-crates")).unwrap();
        replace_link(&sdk.join("crates"), &root.join("old-crates")).unwrap();
        assert!(!release_bundle_complete(&sdk));
    }

    #[test]
    fn android_bundles_are_named_after_the_target_platform() {
        assert_eq!(
            android_bundle_url(Abi::Arm64V8a),
            "https://github.com/nrbnlulu/flutter/releases/download/BETA/flutter-rust-android-arm64.tar.gz"
        );
    }

    #[test]
    fn a_present_android_bundle_is_not_downloaded_again() {
        let root = tempdir("sdk_android_bundle");
        let engine = root.join("lib/android-arm64/release");
        fake_engine(&engine);
        fs::write(engine.join("flutter_patched_sdk/platform_strong.dill"), "").unwrap();
        fs::create_dir_all(engine.join("clang_x64")).unwrap();
        fs::write(engine.join("clang_x64/gen_snapshot"), "").unwrap();
        for artifact in ["arm64_v8a_release", "flutter_embedding_release"] {
            for extension in ["jar", "pom", "maven-metadata.xml"] {
                fs::write(engine.join(format!("{artifact}.{extension}")), "").unwrap();
            }
        }
        assert_eq!(
            ensure_android_bundle(&root, Abi::Arm64V8a, BuildMode::Release).unwrap(),
            engine
        );
    }

    #[test]
    fn debug_android_bundle_requires_flutter_snapshots() {
        let root = tempdir("sdk_android_debug_snapshots");
        let engine = root.join("debug");
        fake_engine(&engine);
        fs::write(engine.join("flutter_patched_sdk/platform_strong.dill"), "").unwrap();
        for artifact in ["arm64_v8a_debug", "flutter_embedding_debug"] {
            for extension in ["jar", "pom", "maven-metadata.xml"] {
                fs::write(engine.join(format!("{artifact}.{extension}")), "").unwrap();
            }
        }
        assert!(!android_bundle_complete(
            &engine,
            Abi::Arm64V8a,
            BuildMode::Debug
        ));
        for name in ["vm_isolate_snapshot.bin", "isolate_snapshot.bin"] {
            fs::write(engine.join("gen/flutter/lib/snapshot").join(name), "").unwrap();
        }
        assert!(android_bundle_complete(
            &engine,
            Abi::Arm64V8a,
            BuildMode::Debug
        ));
    }

    #[test]
    fn layout_links_present_entries_and_skips_absent_ones() {
        let root = tempdir("sdk_layout");
        let (source, layout) = (root.join("host_debug"), root.join("sdk/out/host_debug"));
        fake_engine(&source);
        fs::write(
            source.join("flutter_patched_sdk/platform_strong.dill"),
            "platform",
        )
        .unwrap();
        link_engine_build(&source, &layout).unwrap();
        if cfg!(windows) {
            assert!(!layout.join("flutter_patched_sdk").is_symlink());
        }
        assert_eq!(
            fs::read_to_string(layout.join("flutter_patched_sdk/platform_strong.dill")).unwrap(),
            "platform"
        );
        for present in [
            "libflutter_rust_engine.so",
            "flutter_rust_engine.dll",
            "flutter_rust_engine.dll.lib",
            "icudtl.dat",
            "gen_snapshot",
            "font-subset",
            "impellerc",
            "flutter_patched_sdk",
            "gen/flutter/lib/snapshot",
        ] {
            assert!(layout.join(present).exists(), "{present}");
        }
        assert!(!layout.join("dart-sdk").exists());
    }

    #[test]
    fn layout_requires_the_engine_library() {
        let root = tempdir("sdk_layout_missing");
        let source = root.join("host_debug");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("icudtl.dat"), "").unwrap();
        let error = link_engine_build(&source, &root.join("layout")).unwrap_err();
        assert!(error.to_string().contains(host_engine_library()), "{error}");
    }

    fn flutter_at(root: &Path) -> Flutter {
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join(if cfg!(windows) {
                "bin/flutter.bat"
            } else {
                "bin/flutter"
            }),
            "",
        )
        .unwrap();
        Flutter::discover(Some(root)).unwrap()
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_finds_cpu_suffixed_and_unoptimized_host_builds() {
        let root = tempdir("sdk_macos_variants");
        let src = root.join("engine/src");
        let flutter = flutter_at(&root.join("flutter"));
        let sources = EngineSources {
            local_engine_src: Some(src.clone()),
        };
        let cpu = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        let unopt = src.join(format!("out/host_debug_unopt_{cpu}"));
        fake_engine(&unopt);
        assert_eq!(
            find_engine_build(&host_engine_candidates("debug"), None, &flutter, &sources),
            Some(unopt)
        );
        let release = src.join(format!("out/host_release_{cpu}"));
        fake_engine(&release);
        assert_eq!(
            find_engine_build(&host_engine_candidates("release"), None, &flutter, &sources),
            Some(release)
        );
        assert_eq!(host_engine_library(), "libflutter_rust_engine.dylib");
    }

    #[test]
    fn engine_build_search_prefers_the_checkout_over_the_bundle() {
        let root = tempdir("sdk_search");
        let (src, bundle) = (root.join("engine/src"), root.join("sdk/lib/debug"));
        fake_engine(&src.join("out/host_debug"));
        fake_engine(&bundle);
        let flutter = flutter_at(&root.join("flutter"));
        let names = ["host_debug".to_owned()];
        let sources = EngineSources {
            local_engine_src: Some(src.clone()),
        };
        let found = find_engine_build(&names, Some(bundle.clone()), &flutter, &sources);
        assert_eq!(found, Some(src.join("out/host_debug")));
        let none = EngineSources {
            local_engine_src: Some(root.join("nowhere")),
        };
        let found = find_engine_build(&names, Some(bundle.clone()), &flutter, &none);
        assert_eq!(found, Some(bundle));
    }

    #[test]
    fn unoptimized_debug_android_builds_are_found_under_their_own_name() {
        let root = tempdir("sdk_android_search");
        let src = root.join("engine/src");
        fake_engine(&src.join("out/android_debug_unopt_arm64"));
        let flutter = flutter_at(&root.join("flutter"));
        let sources = EngineSources {
            local_engine_src: Some(src.clone()),
        };
        let names = [
            "android_debug_arm64".to_owned(),
            "android_debug_unopt_arm64".to_owned(),
        ];
        assert_eq!(
            find_engine_build(&names, None, &flutter, &sources),
            Some(src.join("out/android_debug_unopt_arm64"))
        );
        assert_eq!(
            find_engine_build(&names[..1], None, &flutter, &sources),
            None
        );
    }

    #[test]
    fn android_layout_links_the_aot_compiler_when_present() {
        let root = tempdir("sdk_android_layout");
        let (source, layout) = (root.join("android_release_arm64"), root.join("layout"));
        fake_engine(&source);
        fs::create_dir_all(source.join("clang_x64")).unwrap();
        fs::write(source.join("clang_x64/gen_snapshot"), "").unwrap();
        link_engine_build(&source, &layout).unwrap();
        assert!(layout.join("clang_x64/gen_snapshot").exists());
    }
}
