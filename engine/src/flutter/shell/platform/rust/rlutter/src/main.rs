mod abi;
mod adb;
mod android;
mod android_project;
mod android_runner;
mod cli;
mod commands;
mod dart;
mod device;
mod flutter;
mod mode;
mod native_libs;
mod plugins;
mod project;
mod sdk;
mod templates;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::{
    cli::{BuildTarget, Cli, Command},
    commands::Ctx,
    flutter::Flutter,
    sdk::EngineSources,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    util::set_protocol_output(matches!(
        &cli.command,
        Command::Agent(_) | Command::Verify(_)
    ));
    let ctx = Ctx {
        flutter: Flutter::discover(cli.flutter_root.as_deref())?,
        sources: EngineSources {
            local_engine_src: cli.local_engine_src,
        },
    };
    match cli.command {
        command @ (Command::Dart(_) | Command::Lsp(_)) => {
            let is_lsp = matches!(command, Command::Lsp(_));
            let (Command::Dart(args) | Command::Lsp(args)) = command else {
                unreachable!()
            };
            let mut command = ctx.flutter.dart_command();
            if is_lsp {
                command.args(["language-server", "--protocol=lsp"]);
            }
            command.args(args.args);
            #[cfg(unix)]
            {
                Err(command.exec()).context("failed to start bundled Dart")
            }
            #[cfg(windows)]
            {
                std::process::exit(
                    command
                        .status()
                        .context("failed to start bundled Dart")?
                        .code()
                        .unwrap_or(1),
                );
            }
        }
        Command::Create(args) => commands::create::run_create(&ctx, &args),
        Command::Clean(args) => commands::clean::run_clean(&ctx, &args),
        Command::Build { target } => match target {
            BuildTarget::Linux(args) => commands::build::build_linux(
                &ctx,
                args.mode.resolve()?,
                &args.target,
                &args.defines.dart_defines,
            )
            .map(drop),
            BuildTarget::Windows(args) => commands::build::build_windows(
                &ctx,
                args.mode.resolve()?,
                &args.target,
                &args.defines.dart_defines,
            )
            .map(drop),
            BuildTarget::Android(args) => commands::build_android::build_android(
                &ctx,
                args.build.mode.resolve()?,
                &args.build.target,
                &args.build.defines.dart_defines,
                &args.target_platform,
                &args.org,
            )
            .map(drop),
        },
        Command::Agent(args) => commands::agent::run_stdio(&ctx, &args),
        Command::Verify(args) => commands::verify::run(&ctx, &args),
        Command::Run(args) => match commands::run::run_app(&ctx, &args)? {
            0 => Ok(()),
            code => std::process::exit(code),
        },
        Command::Vscode => commands::vscode::configure(&ctx),
        Command::Devices => {
            for device in device::discover() {
                println!("{} \u{2022} {}", device.id(), device.description());
            }
            for device in device::android::discover_all().1 {
                println!("{} \u{2022} not usable: {}", device.serial, device.state);
            }
            Ok(())
        }
    }
}
