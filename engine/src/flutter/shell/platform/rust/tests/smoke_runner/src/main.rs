use std::ffi::CString;

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: flutter-rust-shell-smoke ASSETS_DIRECTORY ICU_DATA_FILE");
        return std::process::ExitCode::FAILURE;
    }
    let paths: Result<Vec<_>, _> = args
        .iter()
        .map(|path| CString::new(path.as_str()))
        .collect();
    let Ok(paths) = paths else {
        eprintln!("paths must not contain NUL characters");
        return std::process::ExitCode::FAILURE;
    };
    if std::env::var_os("FLUTTER_RUST_HEADLESS_PLUGIN_PROBE").is_some() {
        return plugin_probe(&args);
    }
    // Use the same entry point as the GN runner so presentation statistics
    // and the existing texture/windowing fixture environment switches work.
    // SAFETY: both strings remain owned here until the event loop returns;
    // the null AOT path selects the debug/JIT kernel bundle.
    let result = unsafe {
        flutter_shell_winit::FlutterRustShellRun(
            paths[0].as_ptr(),
            paths[1].as_ptr(),
            std::ptr::null(),
        )
    };
    if result != 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn plugin_probe(args: &[String]) -> std::process::ExitCode {
    use flutter_plugin_sdk::{TextureDescriptor, TextureFormat};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let registrations = count.clone();
    let texture = Arc::new(Mutex::new(None));
    let retained = texture.clone();
    let result = flutter_shell_winit::run_application(
        flutter_shell_winit::ShellConfig {
            assets_path: args[0].clone(),
            icu_data_path: args[1].clone(),
            ..Default::default()
        },
        move |registrar| {
            assert_eq!(registrations.fetch_add(1, Ordering::SeqCst), 0);
            let pixels = registrar
                .gpu()?
                .create_pixel_buffer_texture(TextureDescriptor {
                    width: 80,
                    height: 80,
                    format: TextureFormat::Rgba8Unorm,
                })?;
            assert_eq!(pixels.texture_id(), 1);
            let mut frame = pixels.try_next_frame()?;
            frame.write_pixels(|pixels, _| {
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[255, 255, 0, 255]);
                }
            })?;
            frame.present()?;
            *retained.lock().unwrap() = Some(pixels);
            Ok(())
        },
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    if let Err(error) = result {
        eprintln!("{error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
