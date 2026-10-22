//! The generated Gradle project in `android-rs/`, which packages the Rust
//! runner for Android. Created once and then owned by the user.

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

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
        "app/src/main/java/dev/flutter/rs/RustActivity.java",
        "../templates/android-rs/app/src/main/java/dev/flutter/rs/RustActivity.java.tmpl",
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

/// Flutter's Gradle plugin reads this file from the packaged project, which
/// may differ from the conventional `android/` project updated by pub get.
pub fn ensure_local_properties(project: &Project, flutter_root: &Path, sdk: &Path) -> Result<()> {
    let destination = android_dir(project).join("local.properties");
    let conventional = project.dir.join("android/local.properties");
    let existing = if destination.is_file() {
        fs::read_to_string(&destination)?
    } else if conventional.is_file() {
        fs::read_to_string(conventional)?
    } else {
        String::new()
    };
    let mut contents = String::new();
    for line in existing.lines() {
        if matches!(
            line.split_once('=').map(|(key, _)| key.trim()),
            Some("sdk.dir" | "flutter.sdk")
        ) {
            continue;
        }
        contents.push_str(line);
        contents.push('\n');
    }
    contents.push_str(&format!(
        "sdk.dir={}\nflutter.sdk={}\n",
        sdk.display(),
        flutter_root.display()
    ));
    crate::util::write_if_changed(&destination, &contents)?;
    Ok(())
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
        migrate_legacy(project)?;
        disable_release_shrinking(project)?;
        write_activity(project)?;
        expose_to_flutter(project)?;
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
    expose_to_flutter(project)?;
    Ok(true)
}

fn disable_release_shrinking(project: &Project) -> Result<()> {
    let path = android_dir(project).join("app/build.gradle");
    let mut text = fs::read_to_string(&path)?;
    if !text.contains("// rlutter: preserve Dart JNI plugin methods") {
        text.push_str("\nandroid {\n    buildTypes {\n        release {\n            // rlutter: preserve Dart JNI plugin methods\n            minifyEnabled false\n            shrinkResources false\n        }\n    }\n}\n");
        fs::write(path, text)?;
    }
    Ok(())
}

fn expose_to_flutter(project: &Project) -> Result<()> {
    let android = project.dir.join("android");
    if fs::symlink_metadata(&android).is_err() {
        symlink("android-rs", android)?;
    }
    Ok(())
}

/// Flutter writes its Java registrant under the conventional `android/`
/// directory. Existing Flutter apps may keep a separate Android project, so
/// copy the tool-generated source into the `android-rs/` app that we package.
pub fn sync_plugin_registrant(project: &Project) -> Result<()> {
    const REGISTRANT: &str = "app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java";
    let source = project.dir.join("android").join(REGISTRANT);
    let destination = android_dir(project).join(REGISTRANT);
    if source.is_file() {
        if source.canonicalize()? != destination.canonicalize().unwrap_or_default() {
            crate::util::write_if_changed(&destination, &fs::read_to_string(&source)?)?;
        }
    } else {
        let metadata = project.dir.join(".flutter-plugins-dependencies");
        if metadata.is_file() {
            anyhow::bail!(
                "Flutter did not generate {} for Android plugins; run `flutter pub get`",
                source.display()
            );
        }
        crate::util::write_if_changed(
            &destination,
            "package io.flutter.plugins;\npublic final class GeneratedPluginRegistrant {\n  public static void registerWith(io.flutter.embedding.engine.FlutterEngine engine) {}\n}\n",
        )?;
    }
    Ok(())
}

/// Tool-owned activity glue is refreshed on every build.
fn write_activity(project: &Project) -> Result<()> {
    let destination =
        android_dir(project).join("app/src/main/java/dev/flutter/rs/RustActivity.java");
    crate::util::write_if_changed(
        &destination,
        &include_str!(
            "../templates/android-rs/app/src/main/java/dev/flutter/rs/RustActivity.java.tmpl"
        )
        .replace("{{projectName}}", &project.name),
    )?;
    Ok(())
}

/// Upgrades generated GameActivity projects while retaining application IDs,
/// permissions, signing configuration and other user edits. Back up each
/// changed configuration before the one-time migration.
fn migrate_legacy(project: &Project) -> Result<()> {
    let dir = android_dir(project);
    let manifest_path = dir.join("app/src/main/AndroidManifest.xml");
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_default();
    if !manifest.contains("com.google.androidgamesdk.GameActivity") {
        return Ok(());
    }
    let backup = dir.join(".rlutter-gameactivity-backup");
    for file in [
        "settings.gradle",
        "build.gradle",
        "app/build.gradle",
        "app/src/main/AndroidManifest.xml",
        "gradle.properties",
        "gradle/wrapper/gradle-wrapper.properties",
    ] {
        let from = dir.join(file);
        let to = backup.join(file);
        if !to.exists() {
            fs::create_dir_all(to.parent().unwrap())?;
            fs::copy(from, to)?;
        }
    }
    let ignore = dir.join(".gitignore");
    let mut ignore_text = fs::read_to_string(&ignore).unwrap_or_default();
    for entry in [
        ".rlutter-gameactivity-backup/",
        "app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java",
    ] {
        if !ignore_text.lines().any(|line| line == entry) {
            ignore_text.push_str(&format!("\n{entry}\n"));
        }
    }
    fs::write(ignore, ignore_text)?;
    let settings_path = dir.join("settings.gradle");
    let settings = fs::read_to_string(&settings_path)?;
    let settings = settings.replacen("pluginManagement {", "pluginManagement {\n    def flutterSdkPath = providers.gradleProperty(\"rlutterFlutterRoot\").get()\n    includeBuild(\"${flutterSdkPath}/packages/flutter_tools/gradle\")", 1);
    let settings = settings.replacen("\n}\n", "\n}\nplugins {\n    id \"dev.flutter.flutter-plugin-loader\" version \"1.0.0\"\n    id \"com.android.application\" version \"9.3.1\" apply false\n    id \"org.jetbrains.kotlin.android\" version \"2.4.20\" apply false\n}\n", 1);
    fs::write(settings_path, settings)?;
    let root = dir.join("build.gradle");
    let root_text = fs::read_to_string(&root)?;
    let root_text = root_text
        .lines()
        .filter(|line| !line.contains("id \"com.android.application\" version"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&root, format!("{root_text}\n"))?;
    let wrapper = dir.join("gradle/wrapper/gradle-wrapper.properties");
    fs::write(
        &wrapper,
        fs::read_to_string(&wrapper)?.replace("gradle-9.0.0-bin.zip", "gradle-9.5.0-bin.zip"),
    )?;
    let properties = dir.join("gradle.properties");
    let mut properties_text = fs::read_to_string(&properties)?;
    for flag in ["android.newDsl=false", "android.builtInKotlin=false"] {
        if !properties_text.contains(flag) {
            properties_text.push_str(&format!("\n{flag}\n"));
        }
    }
    fs::write(properties, properties_text)?;
    let app = dir.join("app/build.gradle");
    let gradle = fs::read_to_string(&app)?;
    let gradle = gradle.replacen("id \"com.android.application\"", "id \"com.android.application\"\n    id \"org.jetbrains.kotlin.android\"\n    id \"dev.flutter.flutter-gradle-plugin\"", 1);
    let gradle = gradle
        .lines()
        .filter(|line| {
            !line.contains("assets.srcDirs")
                && !line.contains("androidx.games:games-activity")
                && !line.contains("force \"org.jetbrains.kotlin:")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let gradle = gradle.replacen("android {", "android {\n    ndkVersion flutter.ndkVersion\n    compileOptions {\n        sourceCompatibility JavaVersion.VERSION_17\n        targetCompatibility JavaVersion.VERSION_17\n    }\n    kotlinOptions { jvmTarget = \"17\" }", 1);
    fs::write(app, format!("{gradle}\nflutter {{ source \"../..\" }}\n"))?;
    let mut manifest = manifest.replace(
        "com.google.androidgamesdk.GameActivity",
        "dev.flutter.rs.RustActivity",
    );
    // Keep all user permissions and activity attributes.
    if let Some(start) = manifest
        .find("            <meta-data\n                android:name=\"android.app.lib_name\"")
        && let Some(end) = manifest[start..].find("/>")
    {
        manifest.replace_range(start..start + end + 2, "");
    }
    manifest = manifest.replace("</application>", "    <meta-data android:name=\"flutterEmbedding\" android:value=\"2\" />\n    </application>");
    fs::write(manifest_path, manifest)?;
    eprintln!(
        "Migrated android-rs to FlutterActivity; previous configuration saved in {}",
        backup.display()
    );
    Ok(())
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
        assert!(gradle.contains("minifyEnabled false"));
        assert!(!gradle.contains("{{"));
        let manifest = fs::read_to_string(dir.join("app/src/main/AndroidManifest.xml")).unwrap();
        assert!(manifest.contains("dev.flutter.rs.RustActivity") && !manifest.contains("{{"));
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
        assert_eq!(
            fs::read_link(p.dir.join("android")).unwrap(),
            std::path::Path::new("android-rs")
        );
    }

    #[test]
    fn local_properties_are_created_and_keep_other_flutter_values() {
        let p = project("android_local_properties");
        ensure(&p, "dev.example").unwrap();
        let path = android_dir(&p).join("local.properties");
        fs::write(&path, "flutter.sdk=/old\nflutter.versionCode=42\n").unwrap();
        ensure_local_properties(&p, Path::new("/flutter"), Path::new("/sdk")).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents,
            "flutter.versionCode=42\nsdk.dir=/sdk\nflutter.sdk=/flutter\n"
        );
        ensure_local_properties(&p, Path::new("/flutter"), Path::new("/sdk")).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), contents);

        let existing_android = project("android_local_properties_existing_project");
        let conventional = existing_android.dir.join("android/local.properties");
        fs::create_dir_all(conventional.parent().unwrap()).unwrap();
        fs::write(&conventional, "flutter.versionName=2.0\nflutter.sdk=/old\n").unwrap();
        ensure(&existing_android, "dev.example").unwrap();
        ensure_local_properties(&existing_android, Path::new("/flutter"), Path::new("/sdk"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(android_dir(&existing_android).join("local.properties")).unwrap(),
            "flutter.versionName=2.0\nsdk.dir=/sdk\nflutter.sdk=/flutter\n"
        );
    }

    #[test]
    fn an_existing_project_keeps_its_id_and_preserves_jni_methods() {
        let p = project("android_project_keep");
        ensure(&p, "a.b").unwrap();
        let gradle = android_dir(&p).join("app/build.gradle");
        fs::write(&gradle, "applicationId 'my.custom.id'\n").unwrap();
        assert!(!ensure(&p, "other.org").unwrap());
        assert!(!ensure(&p, "other.org").unwrap());
        assert_eq!(application_id(&p).unwrap(), "my.custom.id");
        let gradle = fs::read_to_string(&gradle).unwrap();
        assert_eq!(
            gradle
                .matches("// rlutter: preserve Dart JNI plugin methods")
                .count(),
            1
        );
    }

    #[test]
    fn conventional_android_project_registrant_is_packaged_by_android_rs() {
        let p = project("android_plugin_registrant");
        let source = p
            .dir
            .join("android/app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "package io.flutter.plugins; // native plugin\n").unwrap();
        ensure(&p, "com.example").unwrap();
        sync_plugin_registrant(&p).unwrap();
        let destination = android_dir(&p)
            .join("app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java");
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            fs::read_to_string(&source).unwrap()
        );
        assert!(!p.dir.join("android").is_symlink());
    }

    #[test]
    fn no_plugin_project_gets_an_empty_registrant() {
        let p = project("android_no_plugin_registrant");
        ensure(&p, "com.example").unwrap();
        sync_plugin_registrant(&p).unwrap();
        assert!(
            android_dir(&p)
                .join("app/src/main/java/io/flutter/plugins/GeneratedPluginRegistrant.java")
                .is_file()
        );
    }

    #[test]
    fn gameactivity_migration_keeps_user_configuration_and_its_backup() {
        let p = project("android_project_migrate");
        let dir = android_dir(&p);
        fs::create_dir_all(dir.join("app/src/main")).unwrap();
        fs::create_dir_all(dir.join("gradle/wrapper")).unwrap();
        fs::write(
            dir.join("settings.gradle"),
            "pluginManagement {\n    repositories { google() }\n}\ninclude(\":app\")\n",
        )
        .unwrap();
        fs::write(
            dir.join("build.gradle"),
            "plugins { id \"com.android.application\" version \"8.5.2\" apply false }\n",
        )
        .unwrap();
        fs::write(dir.join("gradle.properties"), "android.useAndroidX=true\n").unwrap();
        fs::write(
            dir.join("gradle/wrapper/gradle-wrapper.properties"),
            "distributionUrl=https\\://services.gradle.org/distributions/gradle-9.0.0-bin.zip\n",
        )
        .unwrap();
        fs::write(dir.join("app/build.gradle"), "plugins {\n    id \"com.android.application\"\n}\nandroid {\n    defaultConfig {\n        applicationId \"custom.example\"\n    }\n    sourceSets { main { jniLibs.srcDirs = [\"${rlutterStageDir}/jniLibs\"] } }\n}\ndependencies { implementation \"androidx.games:games-activity:4.4.0\" }\n").unwrap();
        fs::write(dir.join("app/src/main/AndroidManifest.xml"), "<manifest><uses-permission android:name=\"android.permission.CAMERA\"/><application><activity android:name=\"com.google.androidgamesdk.GameActivity\" /></application></manifest>\n").unwrap();
        assert!(!ensure(&p, "unused").unwrap());
        let manifest = fs::read_to_string(dir.join("app/src/main/AndroidManifest.xml")).unwrap();
        assert!(manifest.contains("android.permission.CAMERA"));
        assert!(manifest.contains("dev.flutter.rs.RustActivity"));
        assert_eq!(application_id(&p).unwrap(), "custom.example");
        assert!(
            fs::read_to_string(dir.join("settings.gradle"))
                .unwrap()
                .contains("com.android.application\" version \"9.3.1")
        );
        assert!(
            !fs::read_to_string(dir.join("build.gradle"))
                .unwrap()
                .contains("com.android.application")
        );
        assert!(
            fs::read_to_string(dir.join("gradle.properties"))
                .unwrap()
                .contains("android.newDsl=false")
        );
        assert!(
            fs::read_to_string(dir.join("gradle/wrapper/gradle-wrapper.properties"))
                .unwrap()
                .contains("gradle-9.5.0-bin.zip")
        );
        assert!(
            fs::read_to_string(
                dir.join(".rlutter-gameactivity-backup/app/src/main/AndroidManifest.xml")
            )
            .unwrap()
            .contains("GameActivity")
        );
        assert!(!ensure(&p, "unused").unwrap());
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
