// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::{env, path::PathBuf, process::ExitCode};

use flutter_rust_shell_frb_fixture::register_application;
use flutter_shell_winit::{ShellConfig, run_application};

fn required_path(variable: &str) -> String {
    let path =
        PathBuf::from(env::var_os(variable).unwrap_or_else(|| panic!("{variable} must be set")));
    assert!(
        path.exists(),
        "{variable} does not exist: {}",
        path.display()
    );
    path.to_string_lossy().into_owned()
}

fn main() -> ExitCode {
    match run_application(
        ShellConfig {
            assets_path: required_path("FLUTTER_RUST_ASSETS_PATH"),
            icu_data_path: required_path("FLUTTER_RUST_ICU_DATA"),
            ..ShellConfig::default()
        },
        register_application,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("failed to run Flutter Rust FRB fixture: {error}");
            ExitCode::FAILURE
        }
    }
}
