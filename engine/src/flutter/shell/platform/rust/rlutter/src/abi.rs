//! Android ABIs. Every Android step works on a list of these, so enabling one
//! (e.g. `x86_64` for emulators) means shipping its engine artifacts and adding
//! it to [`SUPPORTED_ABIS`], not changing code.

use clap::ValueEnum;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, ValueEnum)]
pub enum Abi {
    #[value(name = "android-arm64")]
    Arm64V8a,
    #[value(name = "android-x64")]
    X86_64,
    #[value(name = "android-arm")]
    ArmeabiV7a,
    #[value(name = "android-riscv64")]
    Riscv64,
}

/// ABIs with engine artifacts today.
pub const SUPPORTED_ABIS: &[Abi] = &[Abi::Arm64V8a];

impl Abi {
    pub const ALL: [Abi; 4] = [Abi::Arm64V8a, Abi::X86_64, Abi::ArmeabiV7a, Abi::Riscv64];

    /// Directory name under an APK's `lib/`, as reported by
    /// `ro.product.cpu.abi`.
    pub fn android_name(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "arm64-v8a",
            Abi::X86_64 => "x86_64",
            Abi::ArmeabiV7a => "armeabi-v7a",
            Abi::Riscv64 => "riscv64",
        }
    }

    pub fn from_android_name(name: &str) -> Option<Abi> {
        Abi::ALL.into_iter().find(|abi| abi.android_name() == name)
    }

    /// Flutter's `TargetPlatform` name, for `-dTargetPlatform=`.
    pub fn flutter_platform(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "android-arm64",
            Abi::X86_64 => "android-x64",
            Abi::ArmeabiV7a => "android-arm",
            Abi::Riscv64 => "android-riscv64",
        }
    }

    /// The suffix of engine build directories (`android_release_<this>`).
    pub fn engine_suffix(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "arm64",
            Abi::X86_64 => "x64",
            Abi::ArmeabiV7a => "arm",
            Abi::Riscv64 => "riscv64",
        }
    }

    pub fn rust_triple(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "aarch64-linux-android",
            Abi::X86_64 => "x86_64-linux-android",
            Abi::ArmeabiV7a => "armv7-linux-androideabi",
            Abi::Riscv64 => "riscv64-linux-android",
        }
    }

    /// Prefix of the NDK's `<prefix><api>-clang` wrappers, which differs from
    /// the Rust triple for 32-bit ARM.
    pub fn ndk_clang_prefix(self) -> &'static str {
        match self {
            Abi::ArmeabiV7a => "armv7a-linux-androideabi",
            other => other.rust_triple(),
        }
    }

    pub fn is_supported(self) -> bool {
        SUPPORTED_ABIS.contains(&self)
    }

    /// Cargo reads `CC_<triple with underscores>`.
    pub fn cc_env_suffix(self) -> String {
        self.rust_triple().replace('-', "_")
    }

    /// Cargo reads `CARGO_TARGET_<TRIPLE>_LINKER`.
    pub fn cargo_linker_env(self) -> String {
        format!(
            "CARGO_TARGET_{}_LINKER",
            self.cc_env_suffix().to_uppercase()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_names_round_trip() {
        for abi in Abi::ALL {
            assert_eq!(Abi::from_android_name(abi.android_name()), Some(abi));
        }
        assert_eq!(Abi::from_android_name("mips"), None);
    }

    #[test]
    fn arm64_maps_to_the_names_each_tool_expects() {
        let abi = Abi::Arm64V8a;
        assert_eq!(abi.flutter_platform(), "android-arm64");
        assert_eq!(abi.engine_suffix(), "arm64");
        assert_eq!(abi.rust_triple(), "aarch64-linux-android");
        assert_eq!(abi.ndk_clang_prefix(), "aarch64-linux-android");
        assert_eq!(abi.cc_env_suffix(), "aarch64_linux_android");
        assert_eq!(
            abi.cargo_linker_env(),
            "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
        );
    }

    #[test]
    fn armv7_uses_a_different_clang_prefix_than_its_rust_triple() {
        assert_eq!(Abi::ArmeabiV7a.rust_triple(), "armv7-linux-androideabi");
        assert_eq!(
            Abi::ArmeabiV7a.ndk_clang_prefix(),
            "armv7a-linux-androideabi"
        );
    }

    #[test]
    fn only_arm64_is_supported_for_now() {
        assert!(Abi::Arm64V8a.is_supported());
        assert!(Abi::ALL.iter().filter(|a| a.is_supported()).count() == 1);
    }
}
