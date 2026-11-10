use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{abi::Abi, mode::ModeArgs};

#[derive(Parser, Debug)]
#[command(
    name = "rlutter",
    version,
    about = "Build, run, and verify Flutter apps on the Rust shell",
    after_help = "Headless workflows (Linux x64, Vulkan required):\n  run --headless        Launch an app for development\n  agent --headless      Control an app through NDJSON over stdio\n  mcp --headless        Expose app control as MCP tools over stdio\n  verify --headless     Execute a JSON scenario and write a report\n\nRun rlutter <COMMAND> --help for examples and required options."
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
    /// Build and supervise a display-free verification session over stdio.
    Agent(AgentArgs),
    /// Expose headless app control as MCP tools over stdio (lazy build/launch).
    #[command(
        after_help = "Example (from the application directory; DIR must be fresh):\n  rlutter mcp --headless --stdio --artifacts build/agent/mcp-001\n\nConfigure your MCP client to launch this command in the app directory.\nDiscovery does not build; the first tool call starts the owned headless app.\nUse rlutter_windows_list, rlutter_input_tap, rlutter_verify_wait and\nrlutter_capture_screenshot. Finish with rlutter_session_shutdown.\nRequires Linux x64 and Vulkan. See docs/mcp.md in the Rust-shell source SDK."
    )]
    Mcp(AgentArgs),
    /// Build a fresh app process and execute a versioned verification scenario.
    Verify(VerifyArgs),
    /// Generate a VS Code workspace with one-button Linux debugging.
    Vscode,
    /// List the devices the app can run on.
    Devices,
    /// Run the bundled Dart CLI, forwarding all arguments.
    Dart(PassthroughArgs),
    /// Start the bundled Dart language server over stdio.
    Lsp(PassthroughArgs),
    /// Remove build outputs.
    Clean(CleanArgs),
}

#[derive(Args, Debug)]
#[command(disable_help_flag = true)]
pub struct PassthroughArgs {
    /// Arguments passed unchanged to Dart or the language server.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<std::ffi::OsString>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum RustPlatform {
    Linux,
    Windows,
    Android,
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Directory of the project to create or extend.
    pub dir: PathBuf,

    /// Rust shells to generate.
    #[arg(long, value_enum, value_delimiter = ',', default_value = if cfg!(windows) { "windows" } else { "linux" })]
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
    /// Build for the Windows x64 host.
    Windows(BuildArgs),
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
#[command(
    after_help = "Example (from the application directory):\n  rlutter run -d linux --headless --size 1280x800 --pixel-ratio 1.25\n\nHeadless rendering requires Linux x64 and Vulkan; no display server is needed.\nFor agent control use `rlutter agent`; for assertions and reports use `rlutter verify`."
)]
pub struct RunArgs {
    /// Run without a display server or native windows (Linux x64).
    #[arg(long)]
    pub headless: bool,

    /// Initial physical viewport dimensions.
    #[arg(long, default_value = "1280x800", value_name = "WIDTHxHEIGHT", value_parser = parse_headless_size, requires = "headless")]
    pub size: (u32, u32),

    /// Logical-to-physical pixel scale.
    #[arg(long, default_value = "1", value_name = "RATIO", value_parser = parse_pixel_ratio, requires = "headless")]
    pub pixel_ratio: f64,

    #[command(flatten)]
    pub mode: ModeArgs,

    #[command(flatten)]
    pub defines: DartDefineArgs,

    /// Dart entrypoint.
    #[arg(short = 't', long, default_value = "lib/main.dart")]
    pub target: String,

    /// Write VM service details for a desktop editor debugger.
    #[arg(long, value_name = "FILE")]
    pub vm_service_info_file: Option<PathBuf>,

    /// Device id from `rlutter devices`, or a platform (`android`, `linux`, `windows`) to
    /// choose among its devices. Several candidates open a picker.
    #[arg(short = 'd', long)]
    pub device: Option<String>,
}

#[derive(Args, Debug, Clone)]
#[command(
    after_help = "Example (from the application directory; DIR must be fresh):\n  rlutter agent --headless --stdio --artifacts build/agent/session-001\n\nRequires Linux x64 and Vulkan. Keep stdin open and send one JSON request per line.\nFirst request:\n  {\"id\":1,\"method\":\"session.handshake\",\"params\":{\"protocolVersion\":1}}\nThen use windows.list to discover view IDs, semantics.snapshot, input.tap,\nverify.wait, and capture.screenshot. Request session.shutdown when finished.\nStdout contains only protocol messages; build/app logs go to stderr and artifacts.\nStructured framework error checks require FlutterRustAgent.install() in the app.\nSee docs/agent-protocol.md and docs/verification.md in the Rust-shell source SDK."
)]
pub struct AgentArgs {
    #[command(flatten)]
    pub mode: ModeArgs,
    #[command(flatten)]
    pub defines: DartDefineArgs,
    /// Dart entrypoint.
    #[arg(short = 't', long, default_value = "lib/main.dart")]
    pub target: String,
    /// Run without a display server or native windows (Linux x64).
    #[arg(long, required = true)]
    pub headless: bool,
    /// Exchange newline-delimited JSON requests and responses over stdin/stdout.
    #[arg(long, required = true)]
    pub stdio: bool,
    /// Fresh directory for build/app logs and captured artifacts.
    #[arg(long, value_name = "DIR")]
    pub artifacts: PathBuf,
    /// Initial physical viewport dimensions.
    #[arg(long,default_value="1280x800",value_name="WIDTHxHEIGHT",value_parser=parse_headless_size)]
    pub size: (u32, u32),
    /// Logical-to-physical pixel scale.
    #[arg(long,default_value="1",value_name="RATIO",value_parser=parse_pixel_ratio)]
    pub pixel_ratio: f64,
}

#[derive(Args, Debug)]
#[command(
    after_help = "Example (from the application directory; output paths must be fresh):\n  rlutter verify test/agent/save_dialog.json --headless --report build/agent/run-001/result.json\n\nRequires Linux x64 and Vulkan. Builds and launches a fresh app, runs ordered JSON\nsteps, and writes a report plus result.artifacts beside it. Failures exit nonzero.\nScenarios use version 1, a name, and steps with name/method/params. By default,\ninstall the flutter_rust_agent Dart bridge before runApp to capture framework errors.\nSee docs/verification.md in the Rust-shell source SDK for the schema and examples."
)]
pub struct VerifyArgs {
    /// Versioned JSON verification scenario to execute.
    #[arg(value_name = "SCENARIO")]
    pub scenario: PathBuf,
    #[command(flatten)]
    pub mode: ModeArgs,
    #[command(flatten)]
    pub defines: DartDefineArgs,
    /// Dart entrypoint.
    #[arg(short, long, default_value = "lib/main.dart")]
    pub target: String,
    /// Run without a display server or native windows (Linux x64).
    #[arg(long, required = true)]
    pub headless: bool,
    /// Fresh JSON report path; evidence goes in a sibling <stem>.artifacts directory.
    #[arg(long, value_name = "FILE")]
    pub report: PathBuf,
    /// Initial physical viewport dimensions.
    #[arg(long,default_value="1280x800",value_name="WIDTHxHEIGHT",value_parser=parse_headless_size)]
    pub size: (u32, u32),
    /// Logical-to-physical pixel scale.
    #[arg(long,default_value="1",value_name="RATIO",value_parser=parse_pixel_ratio)]
    pub pixel_ratio: f64,
}

fn parse_headless_size(value: &str) -> Result<(u32, u32), String> {
    let (width, height) = value.split_once('x').ok_or("size must be WIDTHxHEIGHT")?;
    let width = width.parse::<u32>().map_err(|_| "invalid width")?;
    let height = height.parse::<u32>().map_err(|_| "invalid height")?;
    if width == 0 || height == 0 {
        return Err("dimensions must be positive".into());
    }
    Ok((width, height))
}
fn parse_pixel_ratio(value: &str) -> Result<f64, String> {
    let value = value.parse::<f64>().map_err(|_| "invalid pixel ratio")?;
    if !value.is_finite() || value <= 0. {
        return Err("pixel ratio must be finite and positive".into());
    }
    Ok(value)
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
    fn headless_options_validate_dimensions_and_scale() {
        assert!(parse(&["run"]).is_ok());
        for value in ["0x800", "800x0", "800", "NaNx800"] {
            assert!(parse(&["run", "--headless", "--size", value]).is_err());
        }
        for value in ["0", "NaN", "inf"] {
            assert!(parse(&["run", "--headless", "--pixel-ratio", value]).is_err());
        }
        assert!(parse(&["run", "--size", "800x600"]).is_err());
        let Command::Run(args) = parse(&[
            "run",
            "--headless",
            "--size",
            "801x601",
            "--pixel-ratio",
            "1.25",
        ])
        .unwrap()
        .command
        else {
            panic!()
        };
        assert!(args.headless);
        assert_eq!(args.size, (801, 601));
        assert_eq!(args.pixel_ratio, 1.25);
    }

    #[test]
    fn dart_and_lsp_forward_flags_and_arguments() {
        for (name, arguments) in [
            ("dart", vec!["--help"]),
            (
                "dart",
                vec!["run", "tool.dart", "--", "--flutter-root", "other"],
            ),
            ("lsp", vec!["--client-id=editor", "--help"]),
            ("lsp", vec![]),
        ] {
            let mut input = vec!["--flutter-root", "/sdk", name];
            input.extend(&arguments);
            let cli = parse(&input).unwrap();
            assert_eq!(cli.flutter_root, Some(PathBuf::from("/sdk")));
            let (Command::Dart(args) | Command::Lsp(args)) = cli.command else {
                panic!("not a passthrough command")
            };
            assert_eq!(
                args.args,
                arguments
                    .iter()
                    .map(std::ffi::OsString::from)
                    .collect::<Vec<_>>()
            );
        }
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
        assert_eq!(
            args.rust_platforms,
            [if cfg!(windows) {
                RustPlatform::Windows
            } else {
                RustPlatform::Linux
            }]
        );
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
