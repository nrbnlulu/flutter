use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FLUTTER_RUST_ENGINE_DIR");
    let directory = PathBuf::from(
        env::var_os("FLUTTER_RUST_ENGINE_DIR")
            .expect("FLUTTER_RUST_ENGINE_DIR must name the matching GN output directory"),
    );
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let windows = target_os == "windows";
    let macos = target_os == "macos";
    let library = directory.join(if windows {
        "flutter_rust_engine.dll.lib"
    } else if macos {
        "libflutter_rust_engine.dylib"
    } else {
        "libflutter_rust_engine.so"
    });
    assert!(
        library.is_file(),
        "Missing engine library: {}",
        library.display()
    );
    println!("cargo:rerun-if-changed={}", library.display());
    println!("cargo:rustc-link-search=native={}", directory.display());
    if windows {
        println!("cargo:rustc-link-lib=dylib=flutter_rust_engine.dll");
    } else {
        println!("cargo:rustc-link-lib=dylib=flutter_rust_engine");
        if macos {
            // The engine dylib's install name is @rpath-relative. Search
            // beside the executable first so a staged copy uses its own
            // engine, then fall back to the GN output for development runs.
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", directory.display());
        if macos {
            // ld64 spells the dynamic-export flag differently from GNU ld.
            println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
        } else {
            println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
        }
    }
}
