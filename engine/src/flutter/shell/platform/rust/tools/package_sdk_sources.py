#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Stage the complete Rust-shell source SDK and optional engine ABI manifest."""
import argparse
import json
from pathlib import Path
import re
import shutil


def stage(source, destination, validate_engines=False):
  if source == destination or source.is_relative_to(destination):
    raise ValueError('SDK destination must not be the source directory or its parent')
  destination.mkdir(parents=True, exist_ok=True)
  for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'):
    shutil.copy2(source / name, destination / name)
  ignore = shutil.ignore_patterns('target', '.dart_tool', 'build', '__pycache__', '.git')
  for name in ('crates', 'third_party', 'dart', 'docs'):
    if (destination / name).exists():
      shutil.rmtree(destination / name)
    shutil.copytree(source / name, destination / name, ignore=ignore, dirs_exist_ok=True)
  if (destination / 'tests/agent_fixture').exists():
    shutil.rmtree(destination / 'tests/agent_fixture')
  shutil.copytree(
      source / 'tests/agent_fixture',
      destination / 'tests/agent_fixture',
      ignore=ignore,
      dirs_exist_ok=True
  )
  required = (
      'crates/flutter-shell-agent/src/lib.rs', 'crates/flutter-shell-winit/src/headless/agent.rs',
      'crates/flutter-shell-winit/src/headless/recording.rs', 'third_party/wgpu-hal/Cargo.toml',
      'third_party/winit-wayland/Cargo.toml', 'dart/flutter_rust_agent/lib/flutter_rust_agent.dart',
      'tests/agent_fixture/test/agent/save_dialog.json'
  )
  for name in required:
    if not (destination / name).is_file():
      raise RuntimeError(f'SDK source package is missing {name}')
  abi = int(
      re.search(
          r'pub const SHELL_ABI_VERSION: u32 = (\d+);',
          (source / 'crates/flutter-shell-core/src/lib.rs').read_text()
      ).group(1)
  )
  engines = {}
  if validate_engines:
    for mode in ('debug', 'profile', 'release'):
      metadata = json.loads((destination / f'lib/{mode}/engine-manifest.json').read_text())
      if metadata['shellAbi'] != abi or metadata['buildMode'] != mode:
        raise RuntimeError(f'{mode} engine does not match packaged Rust ABI {abi}: {metadata}')
      engines[mode] = metadata
  manifest = {
      'formatVersion': 1, 'shellAbi': abi, 'headlessPlatforms': ['linux-x64'], 'protocolVersion': 1,
      'engines': engines, 'requiredSources': required
  }
  (destination / 'sdk-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
  return manifest


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('destination', type=Path)
  parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[1])
  parser.add_argument('--validate-engines', action='store_true')
  args = parser.parse_args()
  stage(args.source.resolve(), args.destination.resolve(), args.validate_engines)


if __name__ == '__main__':
  main()
