use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=FLUTTER_RUST_ENGINE_DIR");
    let directory = PathBuf::from(
        env::var_os("FLUTTER_RUST_ENGINE_DIR")
            .expect("FLUTTER_RUST_ENGINE_DIR must name the matching GN output directory"),
    );
    let windows = env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows";
    let library = directory.join(if windows {
        "flutter_rust_engine.dll.lib"
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
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", directory.display());
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic");
    }
}
