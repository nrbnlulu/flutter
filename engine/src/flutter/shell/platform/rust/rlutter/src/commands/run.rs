use anyhow::{Result, bail};
use std::{
    io::{BufRead, BufReader},
    process::Stdio,
};

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
    if let Some(path) = &args.vm_service_info_file {
        if !mode.config().hot_reload || device.platform() != "linux" {
            bail!("--vm-service-info-file requires a Linux debug run");
        }
        crate::util::remove_path(path)?;
    }
    eprintln!("Running on {}", device.description());

    let mut app = device.launch(ctx, mode, &args.target, &args.defines.dart_defines)?;
    let stop = std::sync::Arc::clone(&app.stop);
    ctrlc::set_handler(move || stop())
        .map_err(|e| anyhow::anyhow!("cannot install Ctrl-C handler: {e}"))?;

    let code = if mode.config().hot_reload {
        let result = attach(
            ctx,
            &*device,
            &app,
            &args.target,
            &args.defines.dart_defines,
            args.vm_service_info_file.as_deref(),
        );
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

fn attach(
    ctx: &Ctx,
    device: &dyn device::Device,
    app: &RunningApp,
    target: &str,
    dart_defines: &[String],
    service_info_file: Option<&std::path::Path>,
) -> Result<i32> {
    let Some(uri) = &app.vm_service_uri else {
        bail!("the app did not report a Dart VM service to attach to");
    };
    let mut command = ctx.flutter.command(Some(&app.engine));
    command
        // Attach builds hot-reload and hot-restart kernels through the normal
        // Flutter command path, where this enables the matching Dart feature.
        .env("FLUTTER_WINDOWING", "true")
        .args(["attach", "-d"])
        .arg(device.flutter_device_id())
        .arg("--debug-url")
        .arg(uri)
        .args(["-t", target])
        .args(dart_defines.iter().map(|d| format!("--dart-define={d}")))
        .current_dir(&app.project.dir);
    eprintln!("$ {}", describe(&command));
    let Some(path) = service_info_file else {
        return Ok(command.status()?.code().unwrap_or(1));
    };
    // Publish Flutter's DDS URI, whose source mappings allow editor breakpoints
    // to resolve. The raw engine service URI lacks these mappings.
    let mut child = command.stdout(Stdio::piped()).spawn()?;
    let mut published = false;
    let result = (|| -> Result<()> {
        for line in BufReader::new(child.stdout.take().expect("piped")).lines() {
            let line = line?;
            println!("{line}");
            if let Some(uri) = debugger_uri(&line) {
                write_service_info(path, Some(uri))?;
                published = true;
                eprintln!("rlutter: debugger ready");
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    if status.success() && !published {
        bail!("Flutter attach did not report its debugger service URI");
    }
    Ok(status.code().unwrap_or(1))
}

fn debugger_uri(line: &str) -> Option<&str> {
    let line = line.strip_prefix("A Dart VM Service on ")?;
    let (_, uri) = line.split_once(" is available at: ")?;
    let uri = uri.trim();
    uri.starts_with("http://").then_some(uri)
}

fn write_service_info(path: &std::path::Path, uri: Option<&str>) -> Result<()> {
    let Some(uri) = uri else {
        bail!("the app did not report a Dart VM service");
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec(&serde_json::json!({"uri": uri}))?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debugger_uri_uses_flutter_dds_instead_of_the_raw_engine_service() {
        assert_eq!(
            debugger_uri(
                "A Dart VM Service on Linux is available at: http://127.0.0.1:1234/auth=/"
            ),
            Some("http://127.0.0.1:1234/auth=/")
        );
        assert_eq!(
            debugger_uri("The Dart VM service is listening on http://127.0.0.1:4321/raw=/"),
            None
        );
        assert_eq!(
            debugger_uri("The Flutter DevTools debugger and profiler on Linux is available at:"),
            None
        );
    }

    #[test]
    fn service_info_uses_the_debug_adapter_format_and_creates_parents() {
        let path = crate::util::testing::tempdir("service_info").join("nested/service.json");
        write_service_info(&path, Some("http://127.0.0.1:1234/auth=/")).unwrap();
        let info: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(info["uri"], "http://127.0.0.1:1234/auth=/");
        assert!(write_service_info(&path, None).is_err());
    }
}
