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

/// First pid printed by `pidof`.
pub fn parse_pid(output: &str) -> Option<u32> {
    output.split_whitespace().next()?.parse().ok()
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
    fn pid_output_is_parsed() {
        assert_eq!(parse_pid("12345\n"), Some(12345));
        assert_eq!(parse_pid("12345 12399"), Some(12345));
        assert_eq!(parse_pid(""), None);
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
