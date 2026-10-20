use std::{
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use super::{Device, RunningApp, forward_output};
use crate::{commands::Ctx, commands::build::build_linux, mode::BuildMode};

const VM_SERVICE_TIMEOUT: Duration = Duration::from_secs(60);

/// The machine rlutter runs on.
pub struct LinuxHost;

impl LinuxHost {
    pub fn is_supported() -> bool {
        cfg!(all(target_os = "linux", target_arch = "x86_64"))
    }
}

impl Device for LinuxHost {
    fn id(&self) -> String {
        "linux".to_owned()
    }

    fn platform(&self) -> &'static str {
        "linux"
    }

    fn description(&self) -> String {
        "Linux desktop (Rust shell), linux-x64".to_owned()
    }

    fn flutter_device_id(&self) -> String {
        // Stock `flutter attach` only accepts this device for a project without
        // a `linux/` directory when it is named explicitly.
        "linux".to_owned()
    }

    fn launch(&self, ctx: &Ctx, mode: BuildMode, target: &str) -> Result<RunningApp> {
        let build = build_linux(ctx, mode, target)?;
        eprintln!("Launching {}...", build.runner.display());
        let mut child = Command::new(&build.runner)
            .current_dir(&build.project.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("cannot start {}", build.runner.display()))?;

        let (uri_tx, uri_rx) = mpsc::channel();
        forward_output(child.stdout.take().expect("piped"), false, uri_tx.clone());
        forward_output(child.stderr.take().expect("piped"), true, uri_tx);

        let child = Arc::new(Mutex::new(child));
        let stop: Arc<dyn Fn() + Send + Sync> = {
            let child = Arc::clone(&child);
            Arc::new(move || {
                let mut child = child.lock().expect("child lock");
                if matches!(child.try_wait(), Ok(None)) {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            })
        };

        let vm_service_uri = if mode.config().hot_reload {
            match uri_rx.recv_timeout(VM_SERVICE_TIMEOUT) {
                Ok(uri) => Some(uri),
                Err(_) => {
                    stop();
                    bail!("the app did not report a Dart VM service within {VM_SERVICE_TIMEOUT:?}");
                }
            }
        } else {
            None
        };

        let waiter = Arc::clone(&child);
        Ok(RunningApp {
            project: build.project,
            engine: build.engine,
            vm_service_uri,
            stop,
            wait: Box::new(move || {
                loop {
                    // Poll instead of blocking in `wait` so `stop` can take the lock.
                    if let Ok(Some(status)) = waiter.lock().expect("child lock").try_wait() {
                        return status.code();
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }),
        })
    }
}
