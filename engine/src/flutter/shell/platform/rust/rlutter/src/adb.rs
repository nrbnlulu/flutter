//! A thin wrapper over `adb`.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    /// `device`, `unauthorized`, `offline`, ...
    pub state: String,
    pub model: Option<String>,
}

impl AdbDevice {
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }
}

/// Parses `adb devices -l`.
pub fn parse_devices(output: &str) -> Vec<AdbDevice> {
    output
        .lines()
        .skip_while(|line| !line.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let state = fields.next()?;
            if serial.starts_with('*') {
                // "* daemon not running" and friends.
                return None;
            }
            let model = fields
                .find_map(|f| f.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            Some(AdbDevice {
                serial: serial.to_owned(),
                state: state.to_owned(),
                model,
            })
        })
        .collect()
}

/// The local port printed by `adb forward tcp:0 tcp:<device port>`.
pub fn parse_forward_port(output: &str) -> Option<u16> {
    output.lines().find_map(|line| line.trim().parse().ok())
}

/// First pid printed by `pidof`.
pub fn parse_pid(output: &str) -> Option<u32> {
    output.split_whitespace().next()?.parse().ok()
}

/// Maps a VM service URI printed on the device to the forwarded host port.
pub fn rewrite_vm_service_uri(device_uri: &str, host_port: u16) -> Option<String> {
    let (scheme, rest) = device_uri.split_once("://")?;
    let path_start = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(path_start);
    let (host, _port) = authority.rsplit_once(':')?;
    Some(format!("{scheme}://{host}:{host_port}{path}"))
}

/// The port in a VM service URI.
pub fn uri_port(uri: &str) -> Option<u16> {
    let rest = uri.split_once("://")?.1;
    let authority = &rest[..rest.find('/').unwrap_or(rest.len())];
    authority.rsplit_once(':')?.1.parse().ok()
}

#[derive(Clone)]
pub struct Adb {
    path: PathBuf,
}

impl Adb {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `adb` from the SDK's platform-tools, else from `PATH`.
    pub fn locate(sdk_adb: PathBuf) -> Result<Self> {
        if sdk_adb.is_file() {
            return Ok(Self::new(sdk_adb));
        }
        which::which("adb")
            .map(Self::new)
            .context("cannot find adb; install the Android SDK platform-tools")
    }

    pub fn command(&self, serial: Option<&str>) -> Command {
        let mut command = Command::new(&self.path);
        if let Some(serial) = serial {
            command.args(["-s", serial]);
        }
        command
    }

    fn output(&self, mut command: Command) -> Result<String> {
        let output = command
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("cannot run {}", self.path.display()))?;
        if !output.status.success() {
            bail!(
                "adb failed: {}{}",
                String::from_utf8_lossy(&output.stderr).trim(),
                String::from_utf8_lossy(&output.stdout).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn devices(&self) -> Result<Vec<AdbDevice>> {
        let mut command = self.command(None);
        command.args(["devices", "-l"]);
        Ok(parse_devices(&self.output(command)?))
    }

    pub fn getprop(&self, serial: &str, property: &str) -> Result<String> {
        let mut command = self.command(Some(serial));
        command.args(["shell", "getprop", property]);
        Ok(self.output(command)?.trim().to_owned())
    }

    pub fn install(&self, serial: &str, apk: &std::path::Path) -> Result<()> {
        let mut command = self.command(Some(serial));
        command.args(["install", "-r"]).arg(apk);
        crate::util::run(&mut command)
    }

    pub fn start_activity(&self, serial: &str, component: &str) -> Result<()> {
        let mut command = self.command(Some(serial));
        command.args(["shell", "am", "start", "-n", component]);
        let output = self.output(command)?;
        // `am start` exits 0 even when the activity does not exist.
        if output.contains("Error") {
            bail!("cannot start {component}: {}", output.trim());
        }
        Ok(())
    }

    pub fn force_stop(&self, serial: &str, application_id: &str) {
        let mut command = self.command(Some(serial));
        command.args(["shell", "am", "force-stop", application_id]);
        let _ = self.output(command);
    }

    pub fn pid_of(&self, serial: &str, application_id: &str) -> Option<u32> {
        let mut command = self.command(Some(serial));
        command.args(["shell", "pidof", application_id]);
        self.output(command).ok().and_then(|out| parse_pid(&out))
    }

    /// Forwards a free host port to `device_port` and returns it.
    pub fn forward(&self, serial: &str, device_port: u16) -> Result<u16> {
        let mut command = self.command(Some(serial));
        command
            .args(["forward", "tcp:0"])
            .arg(format!("tcp:{device_port}"));
        let output = self.output(command)?;
        parse_forward_port(&output)
            .with_context(|| format!("unexpected output from `adb forward`: {output:?}"))
    }

    pub fn remove_forward(&self, serial: &str, host_port: u16) {
        let mut command = self.command(Some(serial));
        command
            .args(["forward", "--remove"])
            .arg(format!("tcp:{host_port}"));
        let _ = self.output(command);
    }

    /// `adb logcat` for one process, replaying what it already logged.
    pub fn logcat(&self, serial: &str, pid: u32) -> Command {
        let mut command = self.command(Some(serial));
        command
            .args(["logcat", "-v", "brief"])
            .arg(format!("--pid={pid}"));
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = "List of devices attached\n\
        emulator-5554          device product:sdk_gphone64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
        R58M123ABC             device usb:1-1 product:oriole model:Pixel_6 device:oriole transport_id:2\n\
        0123456789             unauthorized usb:1-2 transport_id:3\n\
        \n";

    #[test]
    fn devices_are_parsed_with_state_and_model() {
        let devices = parse_devices(DEVICES);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[1].serial, "R58M123ABC");
        assert_eq!(devices[1].model.as_deref(), Some("Pixel 6"));
        assert!(devices[1].is_ready());
        assert_eq!(devices[2].state, "unauthorized");
        assert!(!devices[2].is_ready());
        assert_eq!(devices[2].model, None);
    }

    #[test]
    fn daemon_chatter_is_not_a_device() {
        let output = "* daemon not running; starting now at tcp:5037\n* daemon started successfully\nList of devices attached\n\n";
        assert!(parse_devices(output).is_empty());
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn forward_and_pid_output_are_parsed() {
        assert_eq!(parse_forward_port("41873\n"), Some(41873));
        assert_eq!(parse_forward_port("error: closed"), None);
        assert_eq!(parse_pid("12345\n"), Some(12345));
        assert_eq!(parse_pid("12345 12399"), Some(12345));
        assert_eq!(parse_pid(""), None);
    }

    #[test]
    fn the_vm_service_uri_is_pointed_at_the_forwarded_port() {
        assert_eq!(
            rewrite_vm_service_uri("http://127.0.0.1:38241/aB3d=/", 51000),
            Some("http://127.0.0.1:51000/aB3d=/".to_owned())
        );
        assert_eq!(
            rewrite_vm_service_uri("http://127.0.0.1:38241", 51000),
            Some("http://127.0.0.1:51000".to_owned())
        );
        assert_eq!(rewrite_vm_service_uri("nonsense", 1), None);
        assert_eq!(uri_port("http://127.0.0.1:38241/aB3d=/"), Some(38241));
        assert_eq!(uri_port("http://127.0.0.1/aB3d=/"), None);
    }

    #[test]
    fn commands_are_scoped_to_the_serial() {
        let adb = Adb::new("/adb".into());
        let command = adb.logcat("S1", 77);
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-s", "S1", "logcat", "-v", "brief", "--pid=77"]);
    }
}
