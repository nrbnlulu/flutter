// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdio>

#include "flutter/shell/platform/rust/cpp/rust_runner.h"

// Standalone native runner for the optional, in-tree Rust shell. It is not
// part of Flutter's public Embedder API and is not selected by the existing
// GTK, Win32, Android, or Darwin shell targets.
int main(int argc, char** argv) {
  if (argc != 3) {
    std::fprintf(stderr, "usage: %s <assets_path> <icu_data_path>\n", argv[0]);
    return 1;
  }
  return FlutterRustShellRun(argv[1], argv[2]) ? 0 : 1;
}
