// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdio>

#include "flutter/shell/platform/rust/cpp/rust_runner.h"

// Standalone native runner for the optional, in-tree Rust shell. It is not
// part of Flutter's public Embedder API and is not selected by the existing
// GTK, Win32, Android, or Darwin shell targets.
int main(int argc, char** argv) {
  if (argc != 3 && argc != 4) {
    std::fprintf(stderr,
                 "usage: %s <assets_path> <icu_data_path> [aot_library_path]\n",
                 argv[0]);
    return 1;
  }
  const char* aot_library_path = argc == 4 ? argv[3] : nullptr;
  return FlutterRustShellRun(argv[1], argv[2], aot_library_path) ? 0 : 1;
}
