// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_RUNNER_H_
#define FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_RUNNER_H_

#ifdef __cplusplus
extern "C" {
#endif

// Implemented in the flutter-shell-winit Rust crate. Owns the winit main loop
// and native window for the entire process lifetime; returns non-zero once
// the loop exits normally. `assets_path` and `icu_data_path` are borrowed
// only for the duration of this call. `aot_library_path` may be null when
// running from a JIT kernel snapshot; otherwise it is borrowed for the same
// duration and points at the AOT-compiled application library.
int FlutterRustShellRun(const char* assets_path,
                        const char* icu_data_path,
                        const char* aot_library_path);

#ifdef __cplusplus
}
#endif

#endif  // FLUTTER_SHELL_PLATFORM_RUST_CPP_RUST_RUNNER_H_
