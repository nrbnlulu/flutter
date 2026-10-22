//! The per-project Rust-shell SDK under `.dart_tool/flutter_rs/sdk`.
//!
//! ```text
//! sdk/
//!   Cargo.toml, Cargo.lock      workspace manifest for the shell crates
//!   crates/, third_party/       shell sources (links into a local checkout, or unpacked)
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
use indicatif::{ProgressBar, ProgressStyle};
use flate2::read::GzDecoder;

use crate::{
    abi::Abi,
    flutter::Flutter,
    mode::BuildMode,
    project::Project,
    util::{copy_if_changed, remove_path, replace_link},
};

const BETA_RELEASE_URL: &str = "https://github.com/nrbnlulu/flutter/releases/download/BETA";
const BETA_BUNDLE_URL: &str =
    "https://github.com/nrbnlulu/flutter/releases/download/BETA/flutter-rust-linux-x64.tar.gz";

/// What `flutter --local-engine` needs from an engine build, relative to its
/// output directory. Missing entries are skipped (a host engine has no
/// `clang_x64`, and so on); required ones are checked by [`ENGINE_REQUIRED`].
const ENGINE_ENTRIES: &[&str] = &[
    "dart-sdk",
    "flutter_patched_sdk",
    "frontend_server_aot.dart.snapshot",
    "gen_snapshot",
    "font-subset",
    "impellerc",
    "shader_lib",
    "libtessellator.so",
    "gen/dart-pkg",
    // Android engines keep the host-run AOT compiler here.
    "clang_x64/gen_snapshot",
    "icudtl.dat",
    "libflutter_rust_engine.so",
    // Debug bundles copy the VM snapshots from here.
    "gen/flutter/lib/snapshot",
];
const ENGINE_REQUIRED: &[&str] = &["libflutter_rust_engine.so", "icudtl.dat"];

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
    let layout = link_variant(
        project,
        flutter,
        sources,
        &variant,
        std::slice::from_ref(&variant),
        // Layout of the downloaded bundle: one directory per mode.
        Some(project.sdk_dir().join("lib").join(mode.name())),
        &format!("linux-x64-{}", mode.name()),
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
        Some(ensure_android_bundle(&project.sdk_dir(), abi, mode)?)
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
        .find(|dir| dir.join("libflutter_rust_engine.so").is_file())
}

/// Fills `layout` with links to `source`'s engine files.
fn link_engine_build(source: &Path, layout: &Path) -> Result<()> {
    let source = source.canonicalize()?;
    fs::create_dir_all(layout)?;
    for entry in ENGINE_ENTRIES {
        let from = source.join(entry);
        if from.exists() {
            replace_link(&layout.join(entry), &from)?;
        }
    }
    for entry in ENGINE_REQUIRED {
        if !layout.join(entry).exists() {
            bail!("{} is missing {entry}", source.display());
        }
    }
    Ok(())
}

/// Makes `sdk` contain the shell crates, from the local checkout when there is
/// one and otherwise from the downloaded bundle.
fn ensure_shell_sources(sdk: &Path, flutter: &Flutter, sources: &EngineSources) -> Result<()> {
    let local = local_shell_dir(flutter, sources);
    if local.join("crates").is_dir() {
        // Always re-sync: crates from another checkout or bundle would not
        // match the engine library linked from this one (misaligned shell ABI).
        copy_if_changed(&local.join("Cargo.toml"), &sdk.join("Cargo.toml"))?;
        copy_if_changed(&local.join("Cargo.lock"), &sdk.join("Cargo.lock"))?;
        replace_link(&sdk.join("crates"), &local.join("crates"))?;
        replace_link(&sdk.join("third_party"), &local.join("third_party"))?;
        return Ok(());
    }
    let complete = sdk.join("Cargo.toml").is_file()
        && sdk.join("crates/flutter-plugin-sdk/Cargo.toml").is_file();
    if complete {
        return Ok(());
    }
    download_bundle(sdk)
}

fn download_bundle(sdk: &Path) -> Result<()> {
    eprintln!("Downloading the Flutter Rust shell BETA SDK...");
    remove_path(sdk)?;
    download_and_unpack(BETA_BUNDLE_URL, sdk)?;
    if !sdk.join("Cargo.toml").is_file() || !sdk.join("crates/flutter-plugin-sdk").is_dir() {
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

/// Returns `<sdk>/lib/<platform>/<mode>`, the prebuilt Android engine,
/// downloading the ABI's bundle first unless it is already there.
fn ensure_android_bundle(sdk: &Path, abi: Abi, mode: BuildMode) -> Result<PathBuf> {
    let bundle = sdk.join("lib").join(abi.flutter_platform());
    let engine = bundle.join(mode.name());
    if engine.join("libflutter_rust_engine.so").is_file() {
        return Ok(engine);
    }
    let url = android_bundle_url(abi);
    eprintln!(
        "Downloading the {} engine from the BETA release...",
        abi.flutter_platform()
    );
    remove_path(&bundle)?;
    download_and_unpack(&url, &bundle)?;
    if !engine.join("libflutter_rust_engine.so").is_file() {
        bail!("{url} has no {} engine", mode.name());
    }
    Ok(engine)
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
            "icudtl.dat",
            "gen_snapshot",
            "font-subset",
            "impellerc",
        ] {
            fs::write(dir.join(file), "").unwrap();
        }
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
        assert_eq!(
            ensure_android_bundle(&root, Abi::Arm64V8a, BuildMode::Release).unwrap(),
            engine
        );
    }

    #[test]
    fn layout_links_present_entries_and_skips_absent_ones() {
        let root = tempdir("sdk_layout");
        let (source, layout) = (root.join("host_debug"), root.join("sdk/out/host_debug"));
        fake_engine(&source);
        link_engine_build(&source, &layout).unwrap();
        for present in [
            "libflutter_rust_engine.so",
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
        assert!(
            error.to_string().contains("libflutter_rust_engine.so"),
            "{error}"
        );
    }

    fn flutter_at(root: &Path) -> Flutter {
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/flutter"), "").unwrap();
        Flutter::discover(Some(root)).unwrap()
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
