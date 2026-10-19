use std::{fs, path::Path};

use anyhow::{Context, Result};

use super::{Ctx, prepare_sources};
use crate::{
    android_project::{self, DEFAULT_ORG},
    android_runner,
    cli::{CreateArgs, RustPlatform},
    mode::BuildMode,
    project::{Project, enable_rust_shell_in_pubspec},
    sdk,
    templates::runner_files,
    util::{remove_path, run},
};

pub fn run_create(ctx: &Ctx, args: &CreateArgs) -> Result<()> {
    let dir = std::path::absolute(&args.dir)?;
    if !dir.join("pubspec.yaml").is_file() {
        flutter_create(ctx, args, &dir)?;
    }

    let project = Project::load(&dir)?;
    let pubspec_path = dir.join("pubspec.yaml");
    let pubspec = fs::read_to_string(&pubspec_path)?;
    if let Some(patched) = enable_rust_shell_in_pubspec(&pubspec)? {
        fs::write(&pubspec_path, patched)?;
    }

    write_runner(&project)?;
    ignore_build_outputs(&dir)?;

    // Best effort: the project exists either way, and `rlutter build` repeats this.
    let project = Project::load(&dir)?;
    let android = args.rust_platforms.contains(&RustPlatform::Android);
    let org = args.org.as_deref().unwrap_or(DEFAULT_ORG);
    let linux = args.rust_platforms.contains(&RustPlatform::Linux);
    let prepared = prepare_sources(ctx, &project, BuildMode::Debug, |project| {
        if android {
            android_runner::ensure(project)?;
            android_project::ensure(project, org)?;
        }
        Ok(())
    })
    .and_then(|()| {
        // So `cargo build` works in runner-rs/ right away.
        if linux {
            sdk::link_host(&project, &ctx.flutter, &ctx.sources, BuildMode::Debug)?;
        }
        Ok(())
    });
    match prepared {
        Ok(()) => eprintln!(
            "Created Rust-shell project {}. Run `rlutter run` inside it.",
            project.name
        ),
        Err(error) => eprintln!(
            "Created Rust-shell project {}, but could not prepare it yet: {error:#}\n\
             Fix that and run `rlutter build`.",
            project.name
        ),
    }
    Ok(())
}

fn forwards_platforms(flutter_args: &[String]) -> bool {
    flutter_args
        .iter()
        .any(|a| a == "--platforms" || a.starts_with("--platforms="))
}

/// Delegates to `flutter create`, so the Rust shell can sit next to Flutter's
/// own platform directories. Stock create insists on at least one platform; if
/// the user asked for none, create `linux` and drop it again.
fn flutter_create(ctx: &Ctx, args: &CreateArgs, dir: &Path) -> Result<()> {
    let keep_native_shells = forwards_platforms(&args.flutter_args);
    let mut command = ctx.flutter.command(None);
    command.arg("create");
    if let Some(org) = &args.org {
        command.args(["--org", org]);
    }
    if let Some(name) = &args.project_name {
        command.args(["--project-name", name]);
    }
    if !keep_native_shells {
        command.arg("--platforms=linux");
    }
    command.args(&args.flutter_args).arg(dir);
    run(&mut command)?;
    if !keep_native_shells {
        remove_path(&dir.join("linux"))?;
        let metadata = dir.join(".metadata");
        if let Ok(text) = fs::read_to_string(&metadata) {
            fs::write(&metadata, strip_platform_from_metadata(&text, "linux"))?;
        }
    }
    Ok(())
}

/// Drops `platform`'s entry from `.metadata`'s migration list, so a later
/// `flutter create .` does not bring back a directory we removed.
fn strip_platform_from_metadata(metadata: &str, platform: &str) -> String {
    let entry = format!("- platform: {platform}");
    let mut out = String::with_capacity(metadata.len());
    let mut skipping_indent: Option<usize> = None;
    for line in metadata.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        if let Some(entry_indent) = skipping_indent {
            // The entry's own fields are indented deeper than its dash.
            if !line.trim().is_empty() && indent > entry_indent {
                continue;
            }
            skipping_indent = None;
        }
        if line.trim() == entry {
            skipping_indent = Some(indent);
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Writes `runner-rs/`, leaving an existing runner untouched.
fn write_runner(project: &Project) -> Result<()> {
    let runner = project.runner_dir();
    if runner.join("Cargo.toml").exists() {
        eprintln!("runner-rs/ already exists; leaving it as is.");
        return Ok(());
    }
    for (path, contents) in runner_files(&project.name) {
        let file = runner.join(path);
        fs::create_dir_all(file.parent().expect("template paths have a parent"))?;
        fs::write(&file, contents).with_context(|| format!("write {}", file.display()))?;
    }
    Ok(())
}

fn ignore_build_outputs(dir: &Path) -> Result<()> {
    let gitignore = dir.join(".gitignore");
    let existing = fs::read_to_string(&gitignore).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == "/runner-rs/target/") {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::write(
        &gitignore,
        format!("{existing}{separator}/runner-rs/target/\n"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    #[test]
    fn detects_forwarded_platforms() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(forwards_platforms(&args(&["--platforms=android,web"])));
        assert!(forwards_platforms(&args(&[
            "--empty",
            "--platforms",
            "android"
        ])));
        assert!(!forwards_platforms(&args(&[
            "--empty",
            "--description=platforms"
        ])));
    }

    #[test]
    fn metadata_entry_for_the_removed_platform_is_dropped() {
        let metadata = "migration:\n  platforms:\n    - platform: root\n      create_revision: a\n    - platform: linux\n      create_revision: b\n      base_revision: c\n    - platform: web\n      create_revision: d\n\nunmanaged_files:\n  - 'lib/main.dart'\n";
        assert_eq!(
            strip_platform_from_metadata(metadata, "linux"),
            "migration:\n  platforms:\n    - platform: root\n      create_revision: a\n    - platform: web\n      create_revision: d\n\nunmanaged_files:\n  - 'lib/main.dart'\n"
        );
        assert_eq!(strip_platform_from_metadata(metadata, "windows"), metadata);
    }

    #[test]
    fn gitignore_is_extended_once() {
        let dir = tempdir("create_gitignore");
        fs::write(dir.join(".gitignore"), "/build/").unwrap();
        ignore_build_outputs(&dir).unwrap();
        ignore_build_outputs(&dir).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "/build/\n/runner-rs/target/\n"
        );
    }

    #[test]
    fn existing_runner_is_not_overwritten() {
        let dir = tempdir("create_runner");
        fs::write(dir.join("pubspec.yaml"), "name: demo\n").unwrap();
        let project = Project::load(&dir).unwrap();
        write_runner(&project).unwrap();
        fs::write(project.runner_dir().join("src/lib.rs"), "// mine").unwrap();
        write_runner(&project).unwrap();
        assert_eq!(
            fs::read_to_string(project.runner_dir().join("src/lib.rs")).unwrap(),
            "// mine"
        );
        assert!(project.runner_dir().join("build.rs").is_file());
    }
}
