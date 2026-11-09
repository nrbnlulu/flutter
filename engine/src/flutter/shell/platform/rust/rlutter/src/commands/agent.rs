//! Build/process ownership for the private local headless session.
use crate::{cli::AgentArgs, commands::Ctx};
use anyhow::{Context, Result, bail};

#[cfg(not(target_os = "linux"))]
pub fn run_stdio(_: &Ctx, _: &AgentArgs) -> Result<()> {
    bail!("headless agent sessions currently require Linux x64")
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use serde_json::json;
    use std::{
        fs::{self, OpenOptions},
        io::{self, Read, Write},
        net::Shutdown,
        os::unix::{fs::PermissionsExt, net::UnixStream},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        thread::{self, JoinHandle},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct Endpoint(PathBuf);
    impl Drop for Endpoint {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    pub struct Session {
        pub stream: UnixStream,
        child: Child,
        logs: Vec<JoinHandle<io::Result<()>>>,
        endpoint: Endpoint,
        pub artifacts: PathBuf,
    }
    fn forward(
        mut source: impl Read + Send + 'static,
        path: PathBuf,
    ) -> JoinHandle<io::Result<()>> {
        thread::spawn(move || {
            let mut log = OpenOptions::new().create_new(true).write(true).open(path)?;
            let mut buffer = [0; 8192];
            let mut failure = None;
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                if let Err(error) = log.write_all(&buffer[..count]) {
                    failure = Some(error);
                }
                let _ = io::stderr().lock().write_all(&buffer[..count]);
            }
            match failure {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    }
    impl Session {
        pub fn launch(
            ctx: &Ctx,
            mode: crate::mode::BuildMode,
            target: &str,
            defines: &[String],
            artifacts: &Path,
            size: (u32, u32),
            ratio: f64,
        ) -> Result<Self> {
            if !cfg!(target_arch = "x86_64") {
                bail!("headless agents currently require Linux x64");
            }
            if artifacts.exists() && fs::read_dir(artifacts)?.next().is_some() {
                bail!("artifacts directory must be fresh: {}", artifacts.display());
            }
            fs::create_dir_all(artifacts)?;
            let artifacts = artifacts.canonicalize()?;
            crate::util::set_protocol_output(true);
            crate::util::set_build_log(
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(artifacts.join("build.log"))?,
            );
            let build = super::super::build::build_linux(ctx, mode, target, defines)?;
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let endpoint = std::env::temp_dir()
                .join(format!("flutter-rs-agent-{}-{nonce:x}", std::process::id()));
            fs::create_dir(&endpoint)?;
            let endpoint = Endpoint(endpoint);
            fs::set_permissions(&endpoint.0, fs::Permissions::from_mode(0o700))?;
            let socket = endpoint.0.join("control.sock");
            let mut child = Command::new(&build.runner)
                .current_dir(&build.project.dir)
                .env("FLUTTER_RUST_HEADLESS", "1")
                .env(
                    "FLUTTER_RUST_HEADLESS_WIDTH",
                    (f64::from(size.0) / ratio).to_string(),
                )
                .env(
                    "FLUTTER_RUST_HEADLESS_HEIGHT",
                    (f64::from(size.1) / ratio).to_string(),
                )
                .env("FLUTTER_RUST_HEADLESS_PIXEL_RATIO", ratio.to_string())
                .env("FLUTTER_RUST_AGENT_SOCKET", &socket)
                .env(
                    "FLUTTER_RUST_AGENT_PARENT_PID",
                    std::process::id().to_string(),
                )
                .env("FLUTTER_RUST_AGENT_ENDPOINT_OWNED", "1")
                .env("FLUTTER_RUST_AGENT_ARTIFACTS", &artifacts)
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("launch {}", build.runner.display()))?;
            let logs = vec![
                forward(
                    child.stdout.take().unwrap(),
                    artifacts.join("app.stdout.log"),
                ),
                forward(
                    child.stderr.take().unwrap(),
                    artifacts.join("app.stderr.log"),
                ),
            ];
            let deadline = Instant::now() + Duration::from_secs(60);
            let stream = loop {
                match UnixStream::connect(&socket) {
                    Ok(stream) => break Ok(stream),
                    Err(error) => {
                        if let Some(status) = child.try_wait()? {
                            break Err(anyhow::anyhow!(
                                "runner exited before opening agent endpoint: {status}"
                            ));
                        }
                        if Instant::now() >= deadline {
                            break Err(error).context("agent endpoint startup timed out");
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                }
            };
            let stream = match stream {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    for log in logs {
                        let _ = log.join();
                    }
                    let _ = fs::remove_dir_all(&endpoint.0);
                    return Err(error);
                }
            };
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            Ok(Self {
                stream,
                child,
                logs,
                endpoint,
                artifacts,
            })
        }
        pub fn finish(&mut self) -> Result<()> {
            let _ = self.stream.shutdown(Shutdown::Both);
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = self.child.try_wait()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    self.child.kill()?;
                    let status = self.child.wait()?;
                    bail!("runner failed to shut down gracefully: {status}");
                }
                thread::sleep(Duration::from_millis(20));
            };
            for log in self.logs.drain(..) {
                log.join()
                    .map_err(|_| anyhow::anyhow!("application log worker panicked"))??;
            }
            if !status.success() {
                bail!(
                    "runner exited with {status}; see {}",
                    self.artifacts.display()
                );
            }
            match fs::remove_dir_all(&self.endpoint.0) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
            Ok(())
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.stream.shutdown(Shutdown::Both);
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            for log in self.logs.drain(..) {
                let _ = log.join();
            }
            let _ = fs::remove_dir_all(&self.endpoint.0);
        }
    }
    pub fn run_stdio(ctx: &Ctx, args: &AgentArgs) -> Result<()> {
        let mut session = match Session::launch(
            ctx,
            args.mode.resolve()?,
            &args.target,
            &args.defines.dart_defines,
            &args.artifacts,
            args.size,
            args.pixel_ratio,
        ) {
            Ok(session) => session,
            Err(error) => {
                println!(
                    "{}",
                    json!({"id":null,"error":{"code":"session_failed","message":error.to_string(),"data":{"artifacts":args.artifacts}}})
                );
                return Err(error);
            }
        };
        // The public stream carries JSON only. Application and engine output
        // is drained independently into stderr and session-owned files.
        let mut input = session.stream.try_clone()?;
        let _reader = thread::Builder::new()
            .name("agent-stdin".into())
            .spawn(move || {
                let result = io::copy(&mut io::stdin().lock(), &mut input);
                let _ = input.shutdown(Shutdown::Write);
                result
            })?;
        // Flush every relay chunk. io::copy may retain a short socket read in
        // stdout's buffer while waiting for another request, deadlocking RPC.
        let relay = (|| -> io::Result<()> {
            let mut output = io::stdout().lock();
            let mut buffer = [0u8; 8192];
            loop {
                let count = session.stream.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                output.write_all(&buffer[..count])?;
                output.flush()?;
            }
            Ok(())
        })();
        let finish = session.finish();
        relay?;
        let event = match &finish {
            Ok(()) => json!({"event":"session.exited","success":true}),
            Err(error) => {
                json!({"event":"session.exited","success":false,"message":error.to_string()})
            }
        };
        serde_json::to_writer(io::stdout().lock(), &event)?;
        io::stdout().lock().write_all(b"\n")?;
        finish
    }
}
#[cfg(target_os = "linux")]
pub use linux::{Session, run_stdio};
