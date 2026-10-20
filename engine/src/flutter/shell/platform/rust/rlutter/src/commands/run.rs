use anyhow::{Result, bail};

use super::Ctx;
use crate::{
    cli::RunArgs,
    device::{self, RunningApp},
    util::describe,
};

/// Builds and launches the app, then (debug) hands it to `flutter attach` for
/// hot reload, hot restart and DevTools, or (release) streams its output.
pub fn run_app(ctx: &Ctx, args: &RunArgs) -> Result<i32> {
    let mode = args.mode.resolve()?;
    let device = device::select(device::discover(), args.device.as_deref())?;
    eprintln!("Running on {}", device.description());

    let mut app = device.launch(ctx, mode, &args.target)?;
    let stop = std::sync::Arc::clone(&app.stop);
    ctrlc::set_handler(move || stop())
        .map_err(|e| anyhow::anyhow!("cannot install Ctrl-C handler: {e}"))?;

    let code = if mode.config().hot_reload {
        let result = attach(ctx, &*device, &app, &args.target);
        // `q` in attach only detaches; the app is ours to stop.
        (app.stop)();
        result?
    } else {
        let code = (app.wait)().unwrap_or(0);
        // The app has exited; stop is idempotent and also releases the
        // device-side helpers (e.g. the adb logcat process on Android).
        (app.stop)();
        code
    };
    Ok(code)
}

fn attach(ctx: &Ctx, device: &dyn device::Device, app: &RunningApp, target: &str) -> Result<i32> {
    let Some(uri) = &app.vm_service_uri else {
        bail!("the app did not report a Dart VM service to attach to");
    };
    let mut command = ctx.flutter.command(Some(&app.engine));
    command
        .args(["attach", "-d"])
        .arg(device.flutter_device_id())
        .arg("--debug-url")
        .arg(uri)
        .args(["-t", target])
        .current_dir(&app.project.dir);
    eprintln!("$ {}", describe(&command));
    let status = command.status()?;
    Ok(status.code().unwrap_or(1))
}
