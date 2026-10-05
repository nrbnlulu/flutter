#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs the native multi-view lifecycle fixture with Vulkan validation."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile


def main():
  parser = argparse.ArgumentParser()
  parser.add_argument("--runner", required=True, type=Path)
  parser.add_argument("--assets", required=True, type=Path)
  parser.add_argument("--icu", required=True, type=Path)
  args = parser.parse_args()
  with tempfile.TemporaryDirectory(prefix="flutter-rust-windowing-") as directory:
    stats = Path(directory) / "presentations"
    status = Path(directory) / "status"
    environment = os.environ.copy()
    environment.update({
        "VK_INSTANCE_LAYERS": "VK_LAYER_KHRONOS_validation",
        "WGPU_VALIDATION": "1",
        "WGPU_DEBUG": "1",
        "FLUTTER_RUST_PRESENTATION_STATS": str(stats),
        "RUST_BACKTRACE": "1",
        "FLUTTER_RUST_WINDOWING_STATUS": str(status),
    })
    result = subprocess.run(
        [str(args.runner.resolve()),
         str(args.assets.resolve()),
         str(args.icu.resolve())],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=90,
    )
    print(result.stdout)
    if result.returncode != 0:
      raise RuntimeError(f"runner exited with {result.returncode}")
    if not status.exists() or status.read_text() != "complete released=60":
      raise RuntimeError("fixture did not complete all native removals")
    if re.search(r"VUID-|VALIDATION \[|Validation Error|panicked|\[ERROR\]|EXCEPTION CAUGHT",
                 result.stdout, re.IGNORECASE):
      raise RuntimeError("GPU or framework diagnostics during multi-view lifecycle")
    frames = stats.read_text().splitlines()
    if len(frames) < 60:
      raise RuntimeError(f"primary view only presented {len(frames)} frames")
    print(f"Multi-view lifecycle passed: 60 removals, {len(frames)} primary presentations.")


if __name__ == "__main__":
  main()
