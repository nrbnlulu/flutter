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
    let windows = env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows";
    let engine_library = engine_dir.join(if windows {
        "flutter_rust_engine.dll.lib"
    } else {
        "libflutter_rust_engine.so"
    });
    assert!(
        engine_library.is_file(),
        "Flutter Rust engine library is missing: {}",
        engine_library.display()
    );
    println!("cargo:rerun-if-changed={}", engine_library.display());
    println!("cargo:rustc-link-search=native={}", engine_dir.display());
    if windows {
        println!("cargo:rustc-link-lib=dylib=flutter_rust_engine.dll");
        // ExternalLibrary.process() requires PE exports; no_mangle alone does
        // not export rlib symbols from an MSVC executable. This list matches
        // flutter_rust_bridge 2.12.0's MultiPackageCBinding and generated wire.
        for symbol in [
            "store_dart_post_cobject",
            "frb_pde_ffi_dispatcher_primary",
            "frb_pde_ffi_dispatcher_sync",
            "frb_dart_fn_deliver_output",
            "frb_get_rust_content_hash",
            "frb_dart_opaque_dart2rust_encode",
            "frb_dart_opaque_drop_thread_box_persistent_handle",
            "frb_dart_opaque_rust2dart_decode",
            "frb_rust_vec_u8_new",
            "frb_rust_vec_u8_resize",
            "frb_rust_vec_u8_free",
            "frb_init_frb_dart_api_dl",
            "frb_free_wire_sync_rust2dart_dco",
            "frb_free_wire_sync_rust2dart_sse",
            "frb_create_shutdown_callback",
        ] {
            println!("cargo:rustc-link-arg-bin=flutter-rust-shell-frb-fixture=/EXPORT:{symbol}");
        }
    } else {
        println!("cargo:rustc-link-lib=dylib=flutter_rust_engine");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", engine_dir.display());
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
    }
}
