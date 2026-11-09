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
