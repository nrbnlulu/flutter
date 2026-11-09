#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for GN's Cargo artifact staging, without an engine build."""

import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import build_rust


class BuildRustTest(unittest.TestCase):

  def test_target_artifact_names(self):
    cases = [
        ("x86_64-pc-windows-msvc", "staticlib", "shell_core.lib"),
        ("aarch64-pc-windows-msvc", "cdylib", "shell_core.dll"),
        ("x86_64-pc-windows-gnu", "staticlib", "libshell_core.a"),
        ("aarch64-linux-android", "cdylib", "libshell_core.so"),
        ("x86_64-unknown-linux-gnu", "staticlib", "libshell_core.a"),
        ("aarch64-apple-darwin", "cdylib", "libshell_core.dylib"),
    ]
    for target, crate_type, expected in cases:
      with self.subTest(target=target, crate_type=crate_type):
        self.assertEqual(build_rust.artifact_name("shell-core", crate_type, target), expected)

  def test_stages_target_artifact_for_each_runtime_mode(self):
    for mode, profile in [("debug", "debug"), ("profile", "release"), ("release", "release")]:
      with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        output = root / "staged" / "shell_core.lib"
        target_dir = root / "cargo"
        target = "x86_64-pc-windows-msvc"
        artifact = target_dir / target / profile / "shell_core.lib"
        artifact.parent.mkdir(parents=True)
        artifact.write_bytes(b"target archive")
        argv = [
            "build_rust.py",
            "--manifest-path",
            str(root / "Cargo.toml"),
            "--package",
            "shell-core",
            "--output",
            str(output),
            "--target",
            target,
            "--cargo-target-dir",
            str(target_dir),
        ]
        with mock.patch("sys.argv", argv), mock.patch.dict(
            os.environ,
            {"FLUTTER_RUNTIME_MODE": mode}), mock.patch("build_rust.subprocess.run") as run:
          build_rust.main()
        self.assertEqual(output.read_bytes(), b"target archive")
        command = run.call_args.args[0]
        self.assertEqual(command[-2:], ["--target", target])
        self.assertEqual("--release" in command, profile == "release")
        self.assertTrue(run.call_args.kwargs["check"])
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_TARGET_DIR"], str(target_dir))


if __name__ == "__main__":
  unittest.main()
