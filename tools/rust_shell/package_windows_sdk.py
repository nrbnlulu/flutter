# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Package physical Windows engine/SDK artifacts without checkout symlinks."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import tarfile
import subprocess

ENGINE_FILES = [
    'flutter_rust_engine.dll',
    'flutter_rust_engine.dll.lib',
    'icudtl.dat',
    'gen_snapshot.exe',
    'font-subset.exe',
    'impellerc.exe',
    'libtessellator.dll',
    'flutter_patched_sdk',
    'dart-sdk',
    'shader_lib',
    'gen/dart-pkg/sky_engine',
    'gen/flutter/lib/snapshot',
]


def copy(source, destination):
  if source.is_dir():
    shutil.copytree(source, destination, dirs_exist_ok=True)
  else:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def engine(source, destination):
  for entry in ENGINE_FILES:
    path = source / entry
    if not path.exists():
      # VM isolate snapshots are unnecessary for the precompiled runtime.
      if entry == 'gen/flutter/lib/snapshot' and source.name.endswith(('release', 'profile')):
        continue
      raise RuntimeError(f'Missing Windows SDK artifact: {path}')
    copy(path, destination / entry)
  for entry in ['frontend_server_aot.dart.snapshot']:
    if (source / entry).exists():
      copy(source / entry, destination / entry)


def sources(checkout, destination):
  shell = checkout / 'engine/src/flutter/shell/platform/rust'
  spec = importlib.util.spec_from_file_location(
      'package_sdk_sources', shell / 'tools/package_sdk_sources.py'
  )
  package = importlib.util.module_from_spec(spec)
  spec.loader.exec_module(package)
  package.stage(shell.resolve(), destination.resolve())


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  sub = parser.add_subparsers(dest='command', required=True)
  item = sub.add_parser('engine')
  item.add_argument('source', type=Path)
  item.add_argument('destination', type=Path)
  item = sub.add_parser('bundle')
  item.add_argument('checkout', type=Path)
  item.add_argument('engines', type=Path, help='Directory containing debug/ and release/')
  item.add_argument('destination', type=Path)
  item.add_argument('archive', type=Path)
  item = sub.add_parser('flutter')
  item.add_argument('checkout', type=Path)
  item.add_argument('destination', type=Path)
  item.add_argument('archive', type=Path)
  item.add_argument(
      '--version-tag', required=True, help='Resolved Flutter tag imported by the release workflow'
  )
  args = parser.parse_args()
  if args.command == 'engine':
    engine(args.source, args.destination)
  elif args.command == 'flutter':
    subprocess.run([
        'git', 'clone', '--no-local', '--single-branch', '--config', 'core.longpaths=true',
        str(args.checkout.resolve()),
        str(args.destination.resolve())
    ],
                   check=True)
    subprocess.run([
        'git', '-C',
        str(args.destination), 'remote', 'set-url', 'origin',
        'https://github.com/nrbnlulu/flutter.git'
    ],
                   check=True)
    if args.version_tag:
      subprocess.run([
          'git', '-C',
          str(args.destination), 'fetch',
          str(args.checkout.resolve()), f'refs/tags/{args.version_tag}:refs/tags/{args.version_tag}'
      ],
                     check=True)
    # This is a new, tool-owned clone. Keep git metadata for Flutter version checks.
    stage = args.destination.resolve()
    engine_dir = stage / 'engine'
    if engine_dir.parent != stage or stage == args.checkout.resolve():
      raise RuntimeError('Refusing to remove engine sources outside the packaging clone')
    if engine_dir.exists():
      shutil.rmtree(engine_dir)
    copy(args.checkout / 'bin/cache', stage / 'bin/cache')
    (stage / 'bin/cache/flutter.version.json').unlink(missing_ok=True)
    for entry in ['bin/flutter.bat', 'bin/cache/dart-sdk/bin/dart.exe',
                  'bin/cache/flutter_tools.snapshot', '.git/HEAD']:
      if not (stage / entry).is_file():
        raise RuntimeError(f'Missing standalone Flutter artifact: {entry}')
    subprocess.run(['git', '-C', str(stage), 'merge-base', 'HEAD', args.version_tag],
                   stdout=subprocess.DEVNULL,
                   check=True)
    # Warm up the Windows wrapper: its first pub bootstrap can write progress to stdout.
    subprocess.run([str(stage / 'bin/flutter.bat'), '--version'], check=True)
    version = subprocess.run([str(stage / 'bin/flutter.bat'), '--version', '--machine'],
                             check=True,
                             stdout=subprocess.PIPE,
                             text=True,
                             encoding='utf-8')
    metadata = json.loads(version.stdout)
    if metadata.get('frameworkVersion') in (None, '', '0.0.0-unknown'):
      raise RuntimeError('Standalone Flutter SDK has no usable numeric version')
    args.archive.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(args.archive, 'w:gz', dereference=True, compresslevel=3) as archive:
      for entry in stage.iterdir():
        archive.add(entry, arcname=entry.name)
    print(args.archive)
  else:
    sources(args.checkout, args.destination)
    for mode in ['debug', 'release']:
      engine(args.engines / mode, args.destination / 'lib' / mode)
    args.archive.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(args.archive, 'w:gz', dereference=True, compresslevel=3) as archive:
      for entry in args.destination.iterdir():
        archive.add(entry, arcname=entry.name)
    print(args.archive)


if __name__ == '__main__':
  main()
