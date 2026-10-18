//! Enforces that shell crates reach the shared Vulkan queue only through
//! `SharedQueue`, so Impeller's queue mutex always covers wgpu submissions.

use std::{fs, path::Path};

const FORBIDDEN: &[&str] = &[
    "wgpu::Queue",
    "device_wait_idle",
    ".queue_submit(",
    "raw_queue()",
];

fn scan(dir: &Path, hits: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, hits);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = fs::read_to_string(&path).unwrap();
            for (number, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if let Some(pattern) = FORBIDDEN.iter().find(|p| code.contains(**p)) {
                    hits.push(format!("{}:{}: {pattern}", path.display(), number + 1));
                }
            }
        }
    }
}

#[test]
fn only_flutter_gpu_queue_touches_the_raw_queue() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut hits = Vec::new();
    for name in [
        "flutter-shell-wgpu",
        "flutter-shell-winit",
        "flutter-plugin-sdk",
    ] {
        let src = crates.join(name).join("src");
        if src.exists() {
            scan(&src, &mut hits);
        }
    }
    // `raw_queue()` only yields a handle value for Impeller's context
    // creation; no queue operation can be issued through it.
    hits.retain(|hit| !hit.ends_with("raw_queue()"));
    assert!(
        hits.is_empty(),
        "use flutter_gpu_queue::SharedQueue instead:\n{}",
        hits.join("\n")
    );
}
