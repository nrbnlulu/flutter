#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Builds a Rust static library for consumption by a GN target."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess


def main() -> None:
  parser = argparse.ArgumentParser()
  parser.add_argument("--cargo", default="cargo")
  parser.add_argument("--manifest-path", required=True)
  parser.add_argument("--package", required=True)
  parser.add_argument("--output", required=True)
  args = parser.parse_args()

  manifest_path = Path(args.manifest_path).resolve()
  output_path = Path(args.output).resolve()
  cargo_target_dir = output_path.parent / "cargo-target"
  profile_dir = ("release" if os.environ.get("FLUTTER_RUNTIME_MODE") == "release" else "debug")
  artifact = cargo_target_dir / profile_dir / ("lib" + args.package.replace("-", "_") + ".a")

  environment = os.environ.copy()
  environment["CARGO_TARGET_DIR"] = str(cargo_target_dir)
  subprocess.run(
      [
          args.cargo,
          "build",
          "--locked",
          "--manifest-path",
          str(manifest_path),
          "--package",
          args.package,
      ],
      check=True,
      env=environment,
  )

  output_path.parent.mkdir(parents=True, exist_ok=True)
  shutil.copy2(artifact, output_path)


if __name__ == "__main__":
  main()
