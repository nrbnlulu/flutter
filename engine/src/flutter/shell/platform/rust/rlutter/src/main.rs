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

use anyhow::Result;
use clap::Parser;

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
    let ctx = Ctx {
        flutter: Flutter::discover(cli.flutter_root.as_deref())?,
        sources: EngineSources {
            local_engine_src: cli.local_engine_src,
        },
    };
    match cli.command {
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
        Command::Run(args) => match commands::run::run_app(&ctx, &args)? {
            0 => Ok(()),
            code => std::process::exit(code),
        },
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
