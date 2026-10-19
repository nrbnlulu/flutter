//! Locating the Android SDK, NDK and a JDK that Gradle can run on.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::abi::Abi;

/// Minimum API level, matching `minSdk` of the generated Gradle project and the
/// NDK clang wrappers used to link the runner.
pub const MIN_SDK: u32 = 24;

#[derive(Debug, Clone)]
pub struct Toolchain {
    pub sdk: PathBuf,
    pub ndk: PathBuf,
    /// `JAVA_HOME` for Gradle; `None` leaves Gradle to use `java` from `PATH`.
    pub java_home: Option<PathBuf>,
}

/// Where lookups read the environment and file system, so tests can fake them.
pub struct Probe<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub is_dir: &'a dyn Fn(&Path) -> bool,
    pub list_dir: &'a dyn Fn(&Path) -> Vec<String>,
    pub home: Option<PathBuf>,
}

impl Toolchain {
    pub fn discover() -> Result<Self> {
        let var = |name: &str| env::var(name).ok().filter(|v| !v.is_empty());
        let is_dir = |path: &Path| path.is_dir();
        let list_dir = |path: &Path| {
            fs::read_dir(path)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        Self::discover_with(&Probe {
            var: &var,
            is_dir: &is_dir,
            list_dir: &list_dir,
            home: env::var_os("HOME").map(PathBuf::from),
        })
    }

    pub fn discover_with(probe: &Probe) -> Result<Self> {
        let sdk = find_sdk(probe)?;
        let ndk = find_ndk(probe, &sdk)?;
        Ok(Self {
            sdk,
            ndk,
            java_home: find_java_home(probe),
        })
    }

    pub fn llvm_bin(&self) -> PathBuf {
        self.ndk
            .join("toolchains")
            .join("llvm")
            .join("prebuilt")
            .join("linux-x86_64")
            .join("bin")
    }

    pub fn clang(&self, abi: Abi) -> PathBuf {
        self.llvm_bin()
            .join(format!("{}{MIN_SDK}-clang", abi.ndk_clang_prefix()))
    }

    pub fn clang_cxx(&self, abi: Abi) -> PathBuf {
        self.llvm_bin()
            .join(format!("{}{MIN_SDK}-clang++", abi.ndk_clang_prefix()))
    }

    pub fn ar(&self) -> PathBuf {
        self.llvm_bin().join("llvm-ar")
    }

    pub fn adb(&self) -> PathBuf {
        self.sdk.join("platform-tools").join("adb")
    }

    /// Environment for `cargo build --target <triple>` so linking and any C
    /// dependencies use the NDK.
    pub fn cargo_env(&self, abi: Abi) -> Vec<(String, PathBuf)> {
        let suffix = abi.cc_env_suffix();
        vec![
            (format!("CC_{suffix}"), self.clang(abi)),
            (format!("CXX_{suffix}"), self.clang_cxx(abi)),
            (format!("AR_{suffix}"), self.ar()),
            (abi.cargo_linker_env(), self.clang(abi)),
        ]
    }
}

fn find_sdk(probe: &Probe) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|name| (probe.var)(name))
        .map(PathBuf::from)
        .collect();
    if let Some(home) = &probe.home {
        candidates.push(home.join("Android").join("Sdk"));
        candidates.push(home.join("Library").join("Android").join("sdk"));
    }
    candidates
        .into_iter()
        .find(|dir| (probe.is_dir)(&dir.join("platforms")))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot find the Android SDK; set ANDROID_HOME (looked for a `platforms` \
                 directory)"
            )
        })
}

/// `ANDROID_NDK_HOME`, else the highest version under `<sdk>/ndk`.
fn find_ndk(probe: &Probe, sdk: &Path) -> Result<PathBuf> {
    for name in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"] {
        if let Some(dir) = (probe.var)(name).map(PathBuf::from)
            && (probe.is_dir)(&dir)
        {
            return Ok(dir);
        }
    }
    let ndk_root = sdk.join("ndk");
    let mut versions = (probe.list_dir)(&ndk_root);
    versions.sort_by_key(|v| version_key(v));
    match versions.pop() {
        Some(version) => Ok(ndk_root.join(version)),
        None => bail!(
            "no Android NDK found in {}; install one with the SDK manager or set ANDROID_NDK_HOME",
            ndk_root.display()
        ),
    }
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// Mirrors Flutter's lookup: Android Studio's bundled JDK, then `JAVA_HOME`.
/// The `java` on `PATH` is often too new for Gradle (JDK 27 is not supported
/// by the Gradle used here), which is why it comes last, via `None`.
fn find_java_home(probe: &Probe) -> Option<PathBuf> {
    let mut studios = vec![PathBuf::from("/opt/android-studio")];
    if let Some(home) = &probe.home {
        studios.push(home.join("android-studio"));
    }
    if let Some(dir) = (probe.var)("ANDROID_STUDIO_DIR") {
        studios.insert(0, PathBuf::from(dir));
    }
    for studio in studios {
        let jbr = studio.join("jbr");
        if (probe.is_dir)(&jbr) {
            return Some(jbr);
        }
    }
    (probe.var)("JAVA_HOME")
        .map(PathBuf::from)
        .filter(|dir| (probe.is_dir)(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    struct Fake {
        vars: HashMap<&'static str, &'static str>,
        dirs: HashSet<&'static str>,
        listings: HashMap<&'static str, Vec<&'static str>>,
    }

    impl Fake {
        fn discover(&self) -> Result<Toolchain> {
            let var = |n: &str| self.vars.get(n).map(|v| v.to_string());
            let is_dir = |p: &Path| self.dirs.contains(p.to_str().unwrap());
            let list = |p: &Path| {
                self.listings
                    .get(p.to_str().unwrap())
                    .map(|v| v.iter().map(|s| s.to_string()).collect())
                    .unwrap_or_default()
            };
            Toolchain::discover_with(&Probe {
                var: &var,
                is_dir: &is_dir,
                list_dir: &list,
                home: Some(PathBuf::from("/home/u")),
            })
        }
    }

    fn fake() -> Fake {
        Fake {
            vars: HashMap::from([("ANDROID_HOME", "/sdk")]),
            dirs: HashSet::from(["/sdk/platforms"]),
            listings: HashMap::from([(
                "/sdk/ndk",
                vec!["27.0.12077973", "28.2.13676358", "9.1.0"],
            )]),
        }
    }

    #[test]
    fn picks_the_highest_ndk_numerically() {
        let toolchain = fake().discover().unwrap();
        assert_eq!(toolchain.ndk, PathBuf::from("/sdk/ndk/28.2.13676358"));
    }

    #[test]
    fn explicit_ndk_wins_when_it_exists() {
        let mut f = fake();
        f.vars.insert("ANDROID_NDK_HOME", "/my/ndk");
        f.dirs.insert("/my/ndk");
        assert_eq!(f.discover().unwrap().ndk, PathBuf::from("/my/ndk"));
        f.dirs.remove("/my/ndk");
        assert_eq!(
            f.discover().unwrap().ndk,
            PathBuf::from("/sdk/ndk/28.2.13676358")
        );
    }

    #[test]
    fn missing_sdk_or_ndk_are_clear_errors() {
        let mut f = fake();
        f.dirs.clear();
        assert!(
            f.discover()
                .unwrap_err()
                .to_string()
                .contains("ANDROID_HOME")
        );
        let mut f = fake();
        f.listings.clear();
        assert!(
            f.discover()
                .unwrap_err()
                .to_string()
                .contains("no Android NDK")
        );
    }

    #[test]
    fn android_studio_jdk_beats_java_home() {
        let mut f = fake();
        f.vars.insert("JAVA_HOME", "/jdk");
        f.dirs.insert("/jdk");
        assert_eq!(f.discover().unwrap().java_home, Some(PathBuf::from("/jdk")));
        f.dirs.insert("/opt/android-studio/jbr");
        assert_eq!(
            f.discover().unwrap().java_home,
            Some(PathBuf::from("/opt/android-studio/jbr"))
        );
    }

    #[test]
    fn linker_and_cargo_env_use_the_api_level_wrappers() {
        let toolchain = fake().discover().unwrap();
        let clang = toolchain.clang(Abi::Arm64V8a);
        assert!(
            clang.ends_with("bin/aarch64-linux-android24-clang"),
            "{clang:?}"
        );
        let env: HashMap<_, _> = toolchain.cargo_env(Abi::Arm64V8a).into_iter().collect();
        assert_eq!(env["CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"], clang);
        assert!(env["AR_aarch64_linux_android"].ends_with("llvm-ar"));
        assert!(
            env.contains_key("CC_aarch64_linux_android")
                && env.contains_key("CXX_aarch64_linux_android")
        );
    }
}
