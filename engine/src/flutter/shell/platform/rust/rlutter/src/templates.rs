//! Files of a generated `runner-rs/` project.

use crate::plugins::{TEMPLATE_BUILD_RS, TEMPLATE_MAIN_RS};

/// `(path relative to runner-rs/, contents)` for a new runner project.
pub fn runner_files(project_name: &str) -> Vec<(&'static str, String)> {
    let fill = |template: &str| template.replace("{{projectName}}", project_name);
    vec![
        (
            "Cargo.toml",
            fill(include_str!("../templates/runner-rs/Cargo.toml.tmpl")),
        ),
        ("build.rs", fill(TEMPLATE_BUILD_RS)),
        (
            "rust-toolchain.toml",
            fill(include_str!(
                "../templates/runner-rs/rust-toolchain.toml.tmpl"
            )),
        ),
        (
            "src/flutter_plugins.rs",
            fill(include_str!(
                "../templates/runner-rs/src/flutter_plugins.rs.tmpl"
            )),
        ),
        (
            "src/lib.rs",
            fill(include_str!("../templates/runner-rs/src/lib.rs.tmpl")),
        ),
        ("src/main.rs", fill(TEMPLATE_MAIN_RS)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_placeholder_is_filled() {
        for (path, contents) in runner_files("my_app") {
            assert!(!contents.contains("{{"), "{path} still has a placeholder");
        }
    }

    #[test]
    fn manifest_names_the_project_and_keeps_the_plugin_section() {
        let files = runner_files("my_app");
        let manifest = &files.iter().find(|(p, _)| *p == "Cargo.toml").unwrap().1;
        assert!(manifest.contains("name = \"my_app\""));
        assert!(manifest.contains(crate::plugins::PLUGINS_BEGIN));
        assert!(manifest.contains(crate::plugins::PLUGINS_END));
        assert!(
            manifest.find(crate::plugins::PLUGINS_END)
                < manifest.find("[target.'cfg(target_os = \"linux\")'.dependencies]")
        );
    }
}
