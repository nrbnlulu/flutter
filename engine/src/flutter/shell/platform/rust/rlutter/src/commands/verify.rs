//! Fresh-process scenario execution; actions and observations remain distinct.
use super::Ctx;
use crate::cli::VerifyArgs;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    version: u32,
    name: String,
    steps: Vec<Step>,
    #[serde(default = "yes")]
    require_framework_errors: bool,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    name: String,
    method: String,
    #[serde(default = "object")]
    params: Value,
    expected_error: Option<String>,
}
fn object() -> Value {
    json!({})
}
fn load(path: &Path) -> Result<Scenario> {
    let bytes = fs::read(path)?;
    if bytes.len() > 1024 * 1024 {
        bail!("scenario exceeds 1 MiB");
    }
    let scenario: Scenario =
        serde_json::from_slice(&bytes).context("invalid verification scenario")?;
    if scenario.version != 1
        || scenario.name.is_empty()
        || scenario.steps.is_empty()
        || scenario.steps.len() > 1000
    {
        bail!("scenario needs version 1, a name and 1..=1000 steps");
    }
    for step in &scenario.steps {
        if step.name.is_empty()
            || !step.params.is_object()
            || step.method.starts_with("session.")
            || step.method == "request.cancel"
        {
            bail!("invalid scenario step {}", step.name);
        }
        if step.expected_error.as_ref().is_some_and(|code| {
            ![
                "not_found",
                "ambiguous_selector",
                "stale_handle",
                "unsupported_capability",
                "assertion_failed",
                "timeout",
                "not_interactable",
                "view_closed",
            ]
            .contains(&code.as_str())
        }) {
            bail!("expected_error must name a narrow operation error");
        }
    }
    Ok(scenario)
}

#[cfg(target_os = "linux")]
struct Client {
    input: BufReader<std::os::unix::net::UnixStream>,
    output: std::os::unix::net::UnixStream,
    id: u64,
}
#[cfg(target_os = "linux")]
impl Client {
    fn new(stream: &std::os::unix::net::UnixStream) -> Result<Self> {
        Ok(Self {
            input: BufReader::new(stream.try_clone()?),
            output: stream.try_clone()?,
            id: 0,
        })
    }
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        self.id += 1;
        let timeout = params
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(5000)
            .min(120000);
        self.input
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(timeout + 5000)))?;
        let message = json!({"id":self.id,"method":method,"params":params});
        serde_json::to_writer(&mut self.output, &message)?;
        self.output.write_all(b"\n")?;
        loop {
            let mut line = Vec::new();
            self.input
                .by_ref()
                .take(16 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)?;
            if line.is_empty() {
                bail!("process_exit: runner closed its control channel");
            }
            if line.len() > 16 * 1024 * 1024 {
                bail!("invalid_request: oversized runner response");
            }
            let response: Value = serde_json::from_slice(&line)?;
            if response.get("event").is_some() {
                continue;
            }
            if response["id"] != self.id {
                bail!("invalid_request: mismatched response ID");
            }
            return Ok(response);
        }
    }
    fn success(&mut self, method: &str, params: Value) -> Result<Value> {
        let response = self.rpc(method, params)?;
        if let Some(error) = response.get("error") {
            bail!("{error}");
        }
        Ok(response["result"].clone())
    }
    fn evidence(&mut self, directory: &Path) -> Value {
        let mut evidence = json!({});
        match self.success("windows.list", json!({})) {
            Ok(windows) => {
                let mut views = Vec::new();
                for view in windows["views"].as_array().into_iter().flatten() {
                    let id = &view["viewId"];
                    let semantics = self.rpc("semantics.snapshot", json!({"viewId":id}));
                    let capture =
                        self.rpc("capture.screenshot", json!({"viewId":id,"timeoutMs":1500}));
                    if semantics
                        .as_ref()
                        .map_or(true, |v| v.get("error").is_some())
                        || capture.as_ref().map_or(true, |v| v.get("error").is_some())
                    {
                        evidence["incomplete"] = json!(true);
                    }
                    views.push(json!({"viewId":id,"semantics":semantics.unwrap_or_else(|e|json!({"infrastructureError":e.to_string()})),"capture":capture.unwrap_or_else(|e|json!({"infrastructureError":e.to_string()}))}));
                }
                evidence["windows"] = windows;
                evidence["views"] = json!(views);
            }
            Err(error) => {
                evidence["missing"] = json!(error.to_string());
                evidence["incomplete"] = json!(true);
            }
        }
        evidence["diagnostics"] = self
            .rpc("diagnostics.snapshot", json!({}))
            .ok()
            .unwrap_or(Value::Null);
        let path = directory.join("observations.json");
        if let Err(error) = fs::write(&path, serde_json::to_vec_pretty(&evidence).unwrap()) {
            evidence["writeError"] = json!(error.to_string());
        }
        evidence
    }
}

#[cfg(not(target_os = "linux"))]
pub fn run(_: &Ctx, _: &VerifyArgs) -> Result<()> {
    bail!("verification currently requires Linux x64");
}

#[cfg(target_os = "linux")]
pub fn run(ctx: &Ctx, args: &VerifyArgs) -> Result<()> {
    let scenario = load(&args.scenario)?;
    if args.report.exists() {
        bail!(
            "report destination already exists: {}",
            args.report.display()
        );
    }
    if let Some(parent) = args.report.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let directory = args.report.with_extension("artifacts");
    let started = Instant::now();
    let mut report = json!({"scenarioVersion":1,"scenario":scenario.name,"status":"infrastructure_failed","steps":[],"artifacts":directory,"size":args.size,"pixelRatio":args.pixel_ratio});
    let result = (|| -> Result<()> {
        let mut session = super::agent::Session::launch(
            ctx,
            args.mode.resolve()?,
            &args.target,
            &args.defines.dart_defines,
            &directory,
            args.size,
            args.pixel_ratio,
        )?;
        let mut client = Client::new(&session.stream)?;
        report["session"] = client.success("session.handshake", json!({"protocolVersion":1}))?;
        if scenario.require_framework_errors {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let capabilities = client.success("session.capabilities", json!({}))?;
                if capabilities["capabilities"]["frameworkErrors"] == true {
                    report["session"] = capabilities;
                    break;
                }
                if Instant::now() >= deadline {
                    bail!(
                        "unsupported_capability: verification requires the opt-in Dart bridge for structured framework errors; set require_framework_errors=false only for an explicitly native-only scenario"
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        report["frameworkErrorsRequired"] = json!(scenario.require_framework_errors);
        let mut failed = false;
        let mut infrastructure_failed = false;
        for step in scenario.steps {
            let start = Instant::now();
            let response = client.rpc(&step.method, step.params.clone());
            let record = match response {
                Ok(response) => {
                    let passed = if let Some(expected) = &step.expected_error {
                        response["error"]["code"].as_str() == Some(expected)
                    } else {
                        response.get("result").is_some()
                    };
                    json!({"name":step.name,"method":step.method,"elapsedMicros":start.elapsed().as_micros() as u64,"passed":passed,"response":response,"expectedError":step.expected_error})
                }
                Err(error) => {
                    json!({"name":step.name,"method":step.method,"elapsedMicros":start.elapsed().as_micros() as u64,"passed":false,"infrastructureError":error.to_string()})
                }
            };
            failed = record["passed"] != true;
            let predicate_failure = record["response"]["error"]["code"] == "assertion_failed"
                || (step.method == "verify.wait"
                    && record["response"]["error"]["code"] == "timeout");
            infrastructure_failed |=
                failed && (record.get("infrastructureError").is_some() || !predicate_failure);
            report["steps"].as_array_mut().unwrap().push(record);
            if failed {
                break;
            }
        }
        report["observations"] = client.evidence(&session.artifacts);
        infrastructure_failed |= report["observations"]["incomplete"] == true
            || report["observations"].get("writeError").is_some();
        failed |= infrastructure_failed;
        // Health and a zero exit status are mandatory even after all predicates pass.
        let health = client.success("session.health", json!({}));
        let shutdown = client.success("session.shutdown", json!({}));
        let finished = session.finish();
        if let Err(error) = health.and(shutdown).map(drop).and(finished) {
            report["error"] = json!(error.to_string());
            bail!("verification infrastructure failed: {error}");
        }
        report["status"] = json!(if infrastructure_failed {
            "infrastructure_failed"
        } else if failed {
            "assertion_failed"
        } else {
            "passed"
        });
        if failed {
            bail!(
                "scenario predicate/action failed; see {}",
                args.report.display()
            );
        }
        Ok(())
    })();
    if let Err(error) = &result
        && report.get("error").is_none()
    {
        report["error"] = json!(error.to_string());
    }
    report["elapsedMicros"] = json!(started.elapsed().as_micros() as u64);
    let provenance = directory.join("build-manifest.json");
    report["build"] = fs::read(provenance)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(Value::Null);
    if let Ok(bytes) = fs::read(directory.join("failure.json")) {
        report["runnerFailure"] = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    }
    let mut artifacts = Vec::new();
    fn collect(directory: &Path, artifacts: &mut Vec<Value>) {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                if let Ok(kind) = entry.file_type() {
                    if kind.is_dir() {
                        collect(&entry.path(), artifacts);
                    } else if kind.is_file()
                        && let Ok(metadata) = entry.metadata()
                    {
                        artifacts.push(json!({"path":entry.path(),"bytes":metadata.len()}));
                    }
                }
            }
        }
    }
    collect(&directory, &mut artifacts);
    report["manifest"] = json!(artifacts);
    fs::write(&args.report, serde_json::to_vec_pretty(&report)?)?;
    eprintln!(
        "{}: {}",
        report["status"].as_str().unwrap(),
        args.report.display()
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scenario_rejects_unknown_fields_and_broad_expected_errors() {
        assert!(
            serde_json::from_str::<Scenario>(r#"{"version":1,"name":"x","steps":[],"typo":true}"#)
                .is_err()
        );
        let path = std::env::temp_dir().join(format!(
            "flutter-agent-scenario-{}.json",
            std::process::id()
        ));
        fs::write(&path,r#"{"version":1,"name":"x","steps":[{"name":"mask crash","method":"verify.wait","expected_error":"process_exit"}]}"#).unwrap();
        assert!(load(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
