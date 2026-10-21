use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{abi::Abi, mode::ModeArgs};

#[derive(Parser, Debug)]
#[command(
    name = "rlutter",
    version,
    about = "Build and run Flutter apps on the Rust shell"
)]
pub struct Cli {
    /// Override the release-managed Flutter SDK.
    #[arg(long, global = true, env = "FLUTTER_ROOT", value_name = "DIR")]
    pub flutter_root: Option<PathBuf>,

    /// Engine checkout (the `src` directory) to take engine builds from.
    #[arg(long, global = true, value_name = "DIR")]
    pub local_engine_src: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a project, or add the Rust shell to an existing Flutter project.
    Create(CreateArgs),
    /// Build the app.
    Build {
        #[command(subcommand)]
        target: BuildTarget,
    },
    /// Build, launch and (in debug mode) hot reload the app on a device.
    Run(RunArgs),
    /// List the devices the app can run on.
    Devices,
    /// Remove build outputs.
    Clean(CleanArgs),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum RustPlatform {
    Linux,
    Android,
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Directory of the project to create or extend.
    pub dir: PathBuf,

    /// Rust shells to generate.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "linux")]
    pub rust_platforms: Vec<RustPlatform>,

    /// Organization, forwarded to `flutter create`.
    #[arg(long)]
    pub org: Option<String>,

    /// Project name, forwarded to `flutter create`.
    #[arg(long)]
    pub project_name: Option<String>,

    /// Arguments after `--` are passed to `flutter create` unchanged. Without
    /// `--platforms`, no Flutter platform directories are kept.
    #[arg(last = true, value_name = "FLUTTER_CREATE_ARGS")]
    pub flutter_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum BuildTarget {
    /// Build for the Linux host.
    Linux(BuildArgs),
    /// Build an Android APK.
    Android(AndroidBuildArgs),
}

/// `--dart-define`, as accepted by `flutter build` and `flutter run`.
#[derive(Args, Debug, Default, Clone)]
pub struct DartDefineArgs {
    /// Defines a compile-time Dart constant (`String.fromEnvironment`), as
    /// `KEY=VALUE`. May be repeated.
    #[arg(long = "dart-define", value_name = "KEY=VALUE")]
    pub dart_defines: Vec<String>,
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    #[command(flatten)]
    pub mode: ModeArgs,

    #[command(flatten)]
    pub defines: DartDefineArgs,

    /// Dart entrypoint.
    #[arg(short = 't', long, default_value = "lib/main.dart")]
    pub target: String,
}

#[derive(Args, Debug)]
pub struct AndroidBuildArgs {
    #[command(flatten)]
    pub build: BuildArgs,

    /// ABIs to build for, comma separated.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "android-arm64"
    )]
    pub target_platform: Vec<Abi>,

    /// Organization for the application id of a newly created `android-rs/`.
    #[arg(long, default_value = crate::android_project::DEFAULT_ORG)]
    pub org: String,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    #[command(flatten)]
    pub mode: ModeArgs,

    #[command(flatten)]
    pub defines: DartDefineArgs,

    /// Dart entrypoint.
    #[arg(short = 't', long, default_value = "lib/main.dart")]
    pub target: String,

    /// Device id from `rlutter devices`, or a platform (`android`, `linux`) to
    /// choose among its devices. Several candidates open a picker.
    #[arg(short = 'd', long)]
    pub device: Option<String>,
}

#[derive(Args, Debug)]
pub struct CleanArgs {
    /// Also run `flutter clean`, which deletes `.dart_tool` including the SDK.
    #[arg(long)]
    pub all: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("rlutter").chain(args.iter().copied()))
    }

    #[test]
    fn build_modes_are_mutually_exclusive() {
        assert!(parse(&["build", "linux", "--debug", "--release"]).is_err());
        assert!(parse(&["build", "linux", "--release"]).is_ok());
    }

    #[test]
    fn create_forwards_everything_after_the_separator() {
        let Command::Create(args) = parse(&[
            "create",
            "app",
            "--org",
            "com.x",
            "--",
            "--platforms=android",
            "--empty",
        ])
        .unwrap()
        .command
        else {
            panic!("not create")
        };
        assert_eq!(args.flutter_args, ["--platforms=android", "--empty"]);
        assert_eq!(args.org.as_deref(), Some("com.x"));
        assert_eq!(args.rust_platforms, [RustPlatform::Linux]);
    }

    #[test]
    fn dart_defines_can_be_repeated() {
        let Command::Run(args) = parse(&["run", "--dart-define=A=1", "--dart-define", "B=x=y"])
            .unwrap()
            .command
        else {
            panic!("not run")
        };
        assert_eq!(args.defines.dart_defines, ["A=1", "B=x=y"]);
    }

    #[test]
    fn rust_platforms_accept_a_list() {
        let Command::Create(args) = parse(&["create", "app", "--rust-platforms", "linux,android"])
            .unwrap()
            .command
        else {
            panic!("not create")
        };
        assert_eq!(
            args.rust_platforms,
            [RustPlatform::Linux, RustPlatform::Android]
        );
    }

    #[test]
    fn global_options_work_after_the_subcommand() {
        let cli = parse(&["clean", "--flutter-root", "/f"]).unwrap();
        assert_eq!(cli.flutter_root, Some(PathBuf::from("/f")));
    }
}
