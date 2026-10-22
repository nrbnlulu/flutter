use std::{
    process::Stdio,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use super::{Device, RunningApp, forward_output};
use crate::{
    abi::{Abi, SUPPORTED_ABIS},
    adb::{self, Adb, AdbDevice},
    android::Toolchain,
    android_project::DEFAULT_ORG,
    commands::{Ctx, build_android::build_android},
    mode::BuildMode,
};

const ACTIVITY: &str = "dev.flutter.rs.RustActivity";
const PID_TIMEOUT: Duration = Duration::from_secs(20);
const VM_SERVICE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct AndroidDevice {
    adb: Adb,
    serial: String,
    model: Option<String>,
    abi_name: String,
}

/// Ready devices, and the ones adb sees but cannot use (unauthorized, offline).
pub fn discover_all() -> (Vec<AndroidDevice>, Vec<AdbDevice>) {
    let Ok(toolchain) = Toolchain::discover() else {
        return (Vec::new(), Vec::new());
    };
    let Ok(adb) = Adb::locate(toolchain.adb()) else {
        return (Vec::new(), Vec::new());
    };
    let Ok(all) = adb.devices() else {
        return (Vec::new(), Vec::new());
    };
    let (ready, unready): (Vec<_>, Vec<_>) = all.into_iter().partition(AdbDevice::is_ready);
    let devices = ready
        .into_iter()
        .map(|d| AndroidDevice {
            abi_name: adb
                .getprop(&d.serial, "ro.product.cpu.abi")
                .unwrap_or_default(),
            adb: adb.clone(),
            serial: d.serial,
            model: d.model,
        })
        .collect();
    (devices, unready)
}

impl AndroidDevice {
    fn abi(&self) -> Result<Abi> {
        let Some(abi) = Abi::from_android_name(&self.abi_name) else {
            bail!(
                "cannot tell the CPU architecture of {} ({:?})",
                self.serial,
                self.abi_name
            );
        };
        if !abi.is_supported() {
            let supported: Vec<_> = SUPPORTED_ABIS.iter().map(|a| a.android_name()).collect();
            bail!(
                "{} is {}, but the Rust shell only has engine artifacts for {} so far",
                self.serial,
                abi.android_name(),
                supported.join(", ")
            );
        }
        Ok(abi)
    }
}

impl Device for AndroidDevice {
    fn id(&self) -> String {
        self.serial.clone()
    }

    fn platform(&self) -> &'static str {
        "android"
    }

    fn description(&self) -> String {
        format!(
            "{} (android, {})",
            self.model.as_deref().unwrap_or("Android device"),
            self.abi_name
        )
    }

    fn flutter_device_id(&self) -> String {
        self.serial.clone()
    }

    fn launch(
        &self,
        ctx: &Ctx,
        mode: BuildMode,
        target: &str,
        dart_defines: &[String],
    ) -> Result<RunningApp> {
        let abi = self.abi()?;
        let build = build_android(ctx, mode, target, dart_defines, &[abi], DEFAULT_ORG)?;
        let (adb, serial, app_id) = (self.adb.clone(), self.serial.clone(), build.application_id);

        eprintln!("Installing {}...", build.apk.display());
        adb.install(&serial, &build.apk)?;
        adb.force_stop(&serial, &app_id);
        adb.start_activity(&serial, &format!("{app_id}/{ACTIVITY}"))?;

        let pid = wait_for_pid(&adb, &serial, &app_id)?;
        let mut logcat = adb
            .logcat(&serial, pid)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .context("cannot start adb logcat")?;
        let (uri_tx, uri_rx) = mpsc::channel();
        forward_output(logcat.stdout.take().expect("piped"), false, uri_tx);

        let forwarded: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
        let logcat = Arc::new(Mutex::new(logcat));
        let stop: Arc<dyn Fn() + Send + Sync> = {
            let (adb, serial, app_id) = (adb.clone(), serial.clone(), app_id.clone());
            let (forwarded, logcat) = (Arc::clone(&forwarded), Arc::clone(&logcat));
            Arc::new(move || {
                let _ = logcat.lock().expect("logcat lock").kill();
                if let Some(port) = forwarded.lock().expect("forward lock").take() {
                    adb.remove_forward(&serial, port);
                }
                adb.force_stop(&serial, &app_id);
            })
        };

        let vm_service_uri = if mode.config().hot_reload {
            match uri_rx.recv_timeout(VM_SERVICE_TIMEOUT) {
                Ok(device_uri) => {
                    let port = adb::uri_port(&device_uri)
                        .with_context(|| format!("no port in {device_uri}"))?;
                    let host_port = adb.forward(&serial, port)?;
                    *forwarded.lock().expect("forward lock") = Some(host_port);
                    adb::rewrite_vm_service_uri(&device_uri, host_port)
                }
                Err(_) => {
                    stop();
                    bail!("the app did not report a Dart VM service within {VM_SERVICE_TIMEOUT:?}");
                }
            }
        } else {
            None
        };

        Ok(RunningApp {
            project: build.project,
            engine: build.engine,
            vm_service_uri,
            stop,
            wait: Box::new(move || {
                // The app is gone once its process is, for a couple of polls in a row.
                let mut misses = 0;
                while misses < 2 {
                    misses = if adb.pid_of(&serial, &app_id).is_some() {
                        0
                    } else {
                        misses + 1
                    };
                    thread::sleep(Duration::from_secs(1));
                }
                Some(0)
            }),
        })
    }
}

fn wait_for_pid(adb: &Adb, serial: &str, app_id: &str) -> Result<u32> {
    let deadline = Instant::now() + PID_TIMEOUT;
    loop {
        if let Some(pid) = adb.pid_of(serial, app_id) {
            return Ok(pid);
        }
        if Instant::now() > deadline {
            bail!("{app_id} did not start on {serial}; check `adb logcat`");
        }
        thread::sleep(Duration::from_millis(200));
    }
}
