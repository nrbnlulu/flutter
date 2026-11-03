//! A workspace keeps editor setup separate from an application's existing JSONC settings.
use std::{fs::OpenOptions, io::Write, path::Path};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::Ctx;
use crate::project::Project;

pub fn configure(ctx: &Ctx) -> Result<()> {
    let project = Project::load(&std::env::current_dir()?)?;
    let executable = std::env::current_exe()?;
    let workspace = workspace(
        &executable,
        &std::fs::canonicalize(ctx.flutter.root())?,
        ctx.sources.local_engine_src.as_deref(),
    )?;
    let path = project.dir.join("rlutter.code-workspace");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "create {} (existing workspaces are left untouched)",
                path.display()
            )
        })?;
    file.write_all(&serde_json::to_vec_pretty(&workspace)?)?;
    eprintln!(
        "Open {} in VS Code, select `rlutter (Linux)`, and press F5.",
        path.display()
    );
    Ok(())
}

fn workspace(executable: &Path, flutter_root: &Path, engine_src: Option<&Path>) -> Result<Value> {
    let mut args = vec![
        "--flutter-root".to_owned(),
        flutter_root.display().to_string(),
    ];
    if let Some(src) = engine_src {
        args.extend([
            "--local-engine-src".to_owned(),
            std::fs::canonicalize(src)?.display().to_string(),
        ]);
    }
    args.extend(
        [
            "run",
            "-d",
            "linux",
            "--vm-service-info-file",
            "${workspaceFolder}/.dart_tool/rlutter-vm-service.json",
        ]
        .map(str::to_owned),
    );
    Ok(json!({
        "folders": [{"path": "."}],
        "extensions": {"recommendations": ["Dart-Code.flutter"]},
        "settings": {"dart.flutterSdkPath": flutter_root},
        "launch": {
            "version": "0.2.0",
            "configurations": [{
                "name": "rlutter (Linux)",
                "type": "dart",
                "request": "attach",
                "debuggerType": "dart",
                "cwd": "${workspaceFolder}",
                "vmServiceInfoFile": "${workspaceFolder}/.dart_tool/rlutter-vm-service.json",
                "deleteServiceInfoFile": false,
                "preLaunchTask": "rlutter: run Linux"
            }]
        },
        "tasks": {
            "version": "2.0.0",
            "tasks": [{
                "label": "rlutter: run Linux",
                "type": "process",
                "command": executable,
                "args": args,
                "options": {"cwd": "${workspaceFolder}"},
                "isBackground": true,
                "presentation": {"reveal": "always", "panel": "dedicated", "focus": false},
                "problemMatcher": {
                    "owner": "rlutter",
                    "pattern": {"regexp": r"^(.+):(\d+): (.+)$", "file": 1, "line": 2, "message": 3},
                    "background": {
                        "activeOnStart": true,
                        "beginsPattern": "^Running on ",
                        "endsPattern": "^rlutter: debugger ready$"
                    }
                }
            }]
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_and_task_share_service_file_and_preserve_paths_with_spaces() {
        let w = workspace(
            Path::new("/tools with spaces/rlutter"),
            Path::new("/sdk with spaces"),
            None,
        )
        .unwrap();
        let task = &w["tasks"]["tasks"][0];
        let launch = &w["launch"]["configurations"][0];
        assert_eq!(task["command"], "/tools with spaces/rlutter");
        assert_eq!(task["args"][1], "/sdk with spaces");
        assert_eq!(
            task["args"].as_array().unwrap().last().unwrap(),
            &launch["vmServiceInfoFile"]
        );
        assert_eq!(task["label"], launch["preLaunchTask"]);
        assert_eq!(launch["debuggerType"], "dart");
        assert_eq!(task["type"], "process");
    }
}
