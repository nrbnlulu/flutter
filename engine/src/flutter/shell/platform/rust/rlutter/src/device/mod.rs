//! Places an app can run: the Linux host now, Android devices next.

pub mod android;
pub mod linux;

use std::{
    io::{BufRead, BufReader, IsTerminal, Read},
    sync::{Arc, mpsc::Sender},
    thread::{self, JoinHandle},
};

use anyhow::{Result, bail};

use crate::{commands::Ctx, mode::BuildMode, project::Project, sdk::Engine};

/// An app started by [`Device::launch`].
pub struct RunningApp {
    pub project: Project,
    /// Engine layout the app was built against; `flutter attach` needs it to
    /// compile the same Dart SDK version.
    pub engine: Engine,
    /// Where the Dart VM service listens (debug mode only).
    pub vm_service_uri: Option<String>,
    /// Stops the app. Safe to call from a signal handler and more than once.
    pub stop: Arc<dyn Fn() + Send + Sync>,
    /// Blocks until the app exits and returns its exit code, if it has one.
    pub wait: Box<dyn FnMut() -> Option<i32>>,
}

pub trait Device {
    /// The id used with `-d`.
    fn id(&self) -> String;
    /// `linux` or `android`; `-d <platform>` selects among a platform's devices.
    fn platform(&self) -> &'static str;
    fn description(&self) -> String;
    /// The `-d` value `flutter attach` knows this device by.
    fn flutter_device_id(&self) -> String;
    /// Builds the app for this device and starts it. In debug mode this
    /// returns once the Dart VM service is known.
    fn launch(
        &self,
        ctx: &Ctx,
        mode: BuildMode,
        target: &str,
        dart_defines: &[String],
    ) -> Result<RunningApp>;
}

pub fn discover() -> Vec<Box<dyn Device>> {
    let mut devices: Vec<Box<dyn Device>> = Vec::new();
    if linux::LinuxHost::is_supported() {
        devices.push(Box::new(linux::LinuxHost));
    }
    let (android, _) = android::discover_all();
    devices.extend(android.into_iter().map(|d| Box::new(d) as Box<dyn Device>));
    devices
}

/// Platform names accepted by `-d` as a filter.
const PLATFORMS: [&str; 2] = ["linux", "android"];

/// Picks the device to run on. `requested` is an exact device id, or a
/// platform name (`android`) to choose among that platform's devices. Several
/// candidates are resolved by an arrow-key picker on a terminal, and are an
/// error otherwise.
pub fn select(devices: Vec<Box<dyn Device>>, requested: Option<&str>) -> Result<Box<dyn Device>> {
    let candidates: Vec<Candidate> = devices
        .iter()
        .map(|d| Candidate {
            id: d.id(),
            platform: d.platform(),
        })
        .collect();
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let index = match choose(&candidates, requested, interactive)? {
        Choice::Index(index) => index,
        Choice::Ask(indices) => ask(&devices, &indices)?,
    };
    Ok(devices.into_iter().nth(index).expect("choice is in range"))
}

struct Candidate {
    id: String,
    platform: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
enum Choice {
    Index(usize),
    /// Let the user pick among these indices.
    Ask(Vec<usize>),
}

fn choose(devices: &[Candidate], requested: Option<&str>, interactive: bool) -> Result<Choice> {
    let ids = || {
        devices
            .iter()
            .map(|d| d.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if devices.is_empty() {
        bail!("no devices found; connect one or check `rlutter devices`");
    }

    let pool: Vec<usize> = match requested {
        None => (0..devices.len()).collect(),
        Some(requested) => {
            if let Some(index) = devices.iter().position(|d| d.id == requested) {
                return Ok(Choice::Index(index));
            }
            if !PLATFORMS.contains(&requested) {
                bail!(
                    "no device with id `{requested}`; available: {} (or a platform: {})",
                    ids(),
                    PLATFORMS.join(", ")
                );
            }
            let matching: Vec<usize> = (0..devices.len())
                .filter(|&i| devices[i].platform == requested)
                .collect();
            if matching.is_empty() {
                bail!("no {requested} device found; available: {}", ids());
            }
            matching
        }
    };

    match pool.as_slice() {
        [only] => Ok(Choice::Index(*only)),
        _ if interactive => Ok(Choice::Ask(pool)),
        _ => bail!(
            "several devices are available ({}); pass one with -d",
            pool.iter()
                .map(|&i| devices[i].id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Arrow-key picker over `indices`; returns the chosen index into `devices`.
fn ask(devices: &[Box<dyn Device>], indices: &[usize]) -> Result<usize> {
    let options: Vec<String> = indices
        .iter()
        .map(|&i| format!("{} ({})", devices[i].description(), devices[i].id()))
        .collect();
    match inquire::Select::new("Choose a device", options)
        .with_help_message("↑↓ to move, enter to select, esc to cancel")
        .raw_prompt()
    {
        Ok(choice) => Ok(indices[choice.index]),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => bail!("no device chosen"),
        Err(error) => Err(error.into()),
    }
}

const VM_SERVICE_PREFIX: &str = "The Dart VM service is listening on ";

/// Extracts the VM service URI from an app output line.
pub fn parse_vm_service_uri(line: &str) -> Option<String> {
    let rest = line.split_once(VM_SERVICE_PREFIX)?.1.trim();
    let uri = rest.split_whitespace().next()?;
    uri.starts_with("http").then(|| uri.to_owned())
}

/// Copies `reader` to our stdout/stderr line by line, reporting the VM
/// service URI on `uri` when the app prints it.
pub fn forward_output<R: Read + Send + 'static>(
    reader: R,
    to_stderr: bool,
    uri: Sender<String>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if to_stderr {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
            if let Some(found) = parse_vm_service_uri(&line) {
                let _ = uri.send(found);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices(spec: &[(&str, &'static str)]) -> Vec<Candidate> {
        spec.iter()
            .map(|(id, platform)| Candidate {
                id: id.to_string(),
                platform,
            })
            .collect()
    }

    fn mixed() -> Vec<Candidate> {
        devices(&[
            ("linux", "linux"),
            ("R58M", "android"),
            ("emu-1", "android"),
        ])
    }

    #[test]
    fn an_exact_id_wins() {
        assert_eq!(
            choose(&mixed(), Some("emu-1"), false).unwrap(),
            Choice::Index(2)
        );
        // `linux` is both an id and a platform; the host is meant.
        assert_eq!(
            choose(&mixed(), Some("linux"), true).unwrap(),
            Choice::Index(0)
        );
    }

    #[test]
    fn a_platform_filters_and_a_single_match_needs_no_question() {
        let one_phone = devices(&[("linux", "linux"), ("R58M", "android")]);
        assert_eq!(
            choose(&one_phone, Some("android"), false).unwrap(),
            Choice::Index(1)
        );
    }

    #[test]
    fn a_platform_with_several_devices_asks_only_among_them() {
        assert_eq!(
            choose(&mixed(), Some("android"), true).unwrap(),
            Choice::Ask(vec![1, 2])
        );
        let error = choose(&mixed(), Some("android"), false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("R58M, emu-1") && !error.contains("linux"),
            "{error}"
        );
    }

    #[test]
    fn a_platform_without_devices_says_so() {
        let host_only = devices(&[("linux", "linux")]);
        let error = choose(&host_only, Some("android"), true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no android device found"), "{error}");
    }

    #[test]
    fn an_unknown_request_lists_ids_and_platforms() {
        let error = choose(&mixed(), Some("nope"), true)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("nope") && error.contains("R58M") && error.contains("android"),
            "{error}"
        );
    }

    #[test]
    fn without_a_request_one_device_is_taken_and_several_ask_on_a_terminal() {
        assert_eq!(
            choose(&devices(&[("linux", "linux")]), None, false).unwrap(),
            Choice::Index(0)
        );
        assert_eq!(
            choose(&mixed(), None, true).unwrap(),
            Choice::Ask(vec![0, 1, 2])
        );
        let error = choose(&mixed(), None, false).unwrap_err().to_string();
        assert!(error.contains("pass one with -d"), "{error}");
    }

    #[test]
    fn no_devices_is_an_error() {
        assert!(choose(&[], None, true).is_err());
        assert!(choose(&[], Some("linux"), true).is_err());
    }
}
