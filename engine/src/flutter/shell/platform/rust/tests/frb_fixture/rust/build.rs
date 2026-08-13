// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FLUTTER_RUST_ENGINE_DIR");
    if env::var_os("CARGO_FEATURE_RUNNER").is_none() {
        return;
    }
    let engine_dir = PathBuf::from(
        env::var_os("FLUTTER_RUST_ENGINE_DIR")
            .expect("FLUTTER_RUST_ENGINE_DIR must name the engine output directory"),
    );
    let engine_library = engine_dir.join("libflutter_rust_engine.so");
    assert!(
        engine_library.is_file(),
        "Flutter Rust engine library is missing: {}",
        engine_library.display()
    );
    println!("cargo:rerun-if-changed={}", engine_library.display());
    println!("cargo:rustc-link-search=native={}", engine_dir.display());
    println!("cargo:rustc-link-lib=dylib=flutter_rust_engine");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", engine_dir.display());
    println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
}
