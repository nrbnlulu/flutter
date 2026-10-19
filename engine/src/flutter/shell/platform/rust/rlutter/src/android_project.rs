//! The generated Gradle project in `android-rs/`, which packages the Rust
//! runner for Android. Created once and then owned by the user.

use std::{fs, os::unix::fs::PermissionsExt};

use anyhow::{Context, Result};

use crate::project::Project;

pub const DEFAULT_ORG: &str = "com.example";

struct File {
    path: &'static str,
    contents: &'static [u8],
    templated: bool,
    executable: bool,
}

macro_rules! file {
    ($path:literal, $source:literal) => {
        File {
            path: $path,
            contents: include_bytes!($source),
            templated: false,
            executable: false,
        }
    };
    ($path:literal, $source:literal, template) => {
        File {
            path: $path,
            contents: include_bytes!($source),
            templated: true,
            executable: false,
        }
    };
}

const FILES: &[File] = &[
    file!(
        "settings.gradle",
        "../templates/android-rs/settings.gradle",
        template
    ),
    file!("build.gradle", "../templates/android-rs/build.gradle"),
    file!(
        "gradle.properties",
        "../templates/android-rs/gradle.properties"
    ),
    file!(".gitignore", "../templates/android-rs/.gitignore"),
    file!("gradlew.bat", "../templates/android-rs/gradlew.bat"),
    File {
        path: "gradlew",
        contents: include_bytes!("../templates/android-rs/gradlew"),
        templated: false,
        executable: true,
    },
    file!(
        "gradle/wrapper/gradle-wrapper.jar",
        "../templates/android-rs/gradle/wrapper/gradle-wrapper.jar"
    ),
    file!(
        "gradle/wrapper/gradle-wrapper.properties",
        "../templates/android-rs/gradle/wrapper/gradle-wrapper.properties"
    ),
    file!(
        "app/build.gradle",
        "../templates/android-rs/app/build.gradle.tmpl",
        template
    ),
    file!(
        "app/src/main/AndroidManifest.xml",
        "../templates/android-rs/app/src/main/AndroidManifest.xml.tmpl",
        template
    ),
    file!(
        "app/src/main/res/values/styles.xml",
        "../templates/android-rs/app/src/main/res/values/styles.xml"
    ),
];

pub fn android_dir(project: &Project) -> std::path::PathBuf {
    project.dir.join("android-rs")
}

/// `<org>.<project>.rs`, so the Rust shell can be installed next to the app
/// built from Flutter's own `android/` directory.
pub fn default_application_id(org: &str, project_name: &str) -> String {
    format!("{org}.{project_name}.rs")
}

/// Creates `android-rs/` unless it exists. Returns whether it was created.
pub fn ensure(project: &Project, org: &str) -> Result<bool> {
    let dir = android_dir(project);
    if dir.join("app").join("build.gradle").is_file() {
        return Ok(false);
    }
    let application_id = default_application_id(org, &project.name);
    for file in FILES {
        let destination = dir.join(file.path);
        fs::create_dir_all(destination.parent().expect("template paths have a parent"))?;
        if file.templated {
            let text = std::str::from_utf8(file.contents).context("template is not UTF-8")?;
            fs::write(
                &destination,
                text.replace("{{projectName}}", &project.name)
                    .replace("{{applicationId}}", &application_id),
            )?;
        } else {
            fs::write(&destination, file.contents)?;
        }
        if file.executable {
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(true)
}

/// The app's id, read from `app/build.gradle` (the user may have changed it).
pub fn application_id(project: &Project) -> Result<String> {
    let gradle = android_dir(project).join("app").join("build.gradle");
    let text =
        fs::read_to_string(&gradle).with_context(|| format!("cannot read {}", gradle.display()))?;
    parse_application_id(&text)
        .ok_or_else(|| anyhow::anyhow!("no applicationId found in {}", gradle.display()))
}

fn parse_application_id(gradle: &str) -> Option<String> {
    for line in gradle.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("applicationId") else {
            continue;
        };
        // Not `applicationIdSuffix` and friends.
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '=') {
            continue;
        }
        let value = rest.trim().trim_start_matches('=').trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

pub fn apk_path(project: &Project, mode: &str) -> std::path::PathBuf {
    android_dir(project)
        .join("app")
        .join("build")
        .join("outputs")
        .join("apk")
        .join(mode)
        .join(format!("app-{mode}.apk"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testing::tempdir;

    fn project(name: &str) -> Project {
        let dir = tempdir(name);
        fs::write(
            dir.join("pubspec.yaml"),
            "name: demo_app\nrust-shell: true\n",
        )
        .unwrap();
        Project::load(&dir).unwrap()
    }

    #[test]
    fn generated_project_is_filled_in_and_gradlew_is_executable() {
        let p = project("android_project_gen");
        assert!(ensure(&p, "dev.example").unwrap());
        let dir = android_dir(&p);
        let gradle = fs::read_to_string(dir.join("app/build.gradle")).unwrap();
        assert!(gradle.contains("applicationId \"dev.example.demo_app.rs\""));
        assert!(!gradle.contains("{{"));
        let manifest = fs::read_to_string(dir.join("app/src/main/AndroidManifest.xml")).unwrap();
        assert!(manifest.contains("android:value=\"demo_app\"") && !manifest.contains("{{"));
        assert!(
            fs::read_to_string(dir.join("settings.gradle"))
                .unwrap()
                .contains("demo_app_android")
        );
        let mode = fs::metadata(dir.join("gradlew"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
        assert!(
            fs::metadata(dir.join("gradle/wrapper/gradle-wrapper.jar"))
                .unwrap()
                .len()
                > 10_000
        );
        assert_eq!(application_id(&p).unwrap(), "dev.example.demo_app.rs");
    }

    #[test]
    fn an_existing_project_is_left_alone() {
        let p = project("android_project_keep");
        ensure(&p, "a.b").unwrap();
        let gradle = android_dir(&p).join("app/build.gradle");
        fs::write(&gradle, "applicationId 'my.custom.id'\n").unwrap();
        assert!(!ensure(&p, "other.org").unwrap());
        assert_eq!(application_id(&p).unwrap(), "my.custom.id");
    }

    #[test]
    fn application_id_parsing_accepts_gradle_spellings() {
        assert_eq!(
            parse_application_id("  applicationId \"a.b\"\n"),
            Some("a.b".into())
        );
        assert_eq!(
            parse_application_id("applicationId = 'a.b'"),
            Some("a.b".into())
        );
        assert_eq!(
            parse_application_id("applicationIdSuffix \".x\"\nname 'q'"),
            None
        );
        assert_eq!(
            parse_application_id("applicationIdSuffix \".x\"\napplicationId \"a.b\""),
            Some("a.b".into())
        );
        assert_eq!(parse_application_id("namespace \"a.b\""), None);
    }
}
