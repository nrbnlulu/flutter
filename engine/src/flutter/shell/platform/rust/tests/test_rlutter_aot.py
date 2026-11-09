# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Build and relocate an AOT app; verify rendering, input and native close."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import struct
import zlib

from native_desktop import desktop
from test_texture import DIAGNOSTIC, wait_until

SOURCE = """import 'package:flutter/widgets.dart';
void main() => runApp(const Probe());
class Probe extends StatefulWidget {
  const Probe({super.key});
  @override
  State<Probe> createState() => _ProbeState();
}
class _ProbeState extends State<Probe> {
  bool changed = false;
  @override
  Widget build(BuildContext context) => Directionality(
    textDirection: TextDirection.ltr,
    child: GestureDetector(behavior: HitTestBehavior.opaque,
      onTap: () => setState(() => changed = !changed),
      child: ColoredBox(color: changed ? const Color(0xff804020) : const Color(0xff204060),
        child: const Center(child: Text('AOT relocation probe',
          style: TextStyle(fontSize: 28, color: Color(0xffffffff)))))));
}
"""


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--cli', type=Path, required=True)
  parser.add_argument('--flutter-root', type=Path, required=True)
  parser.add_argument(
      '--project',
      type=Path,
      required=True,
      help='Disposable generated project; probe source will be replaced'
  )
  parser.add_argument('--mode', choices=['profile', 'release'], default='release')
  args = parser.parse_args()
  evidence = Path(tempfile.mkdtemp(prefix=f'flutter-rust-{args.mode}-'))
  print(f'AOT evidence: {evidence}', flush=True)
  project = args.project.resolve()
  (project / 'lib/aot_probe.dart').write_text(SOURCE, encoding='utf-8')
  platform = 'windows' if sys.platform == 'win32' else 'linux'
  with (evidence / 'build.log').open('w', encoding='utf-8') as log:
    subprocess.run([
        str(args.cli.resolve()), 'build', platform, f'--{args.mode}', '-t', 'lib/aot_probe.dart',
        f'--flutter-root={args.flutter_root.resolve()}'
    ],
                   cwd=project,
                   stdout=log,
                   stderr=subprocess.STDOUT,
                   check=True)
  name = re.search(r'^name:\s*(\w+)', (project / 'pubspec.yaml').read_text(), re.MULTILINE)[1]
  executable = name + ('.exe' if platform == 'windows' else '')
  built = project / 'runner-rs/target' / (
      'release' if args.mode == 'release' else 'flutter-profile'
  )
  relocated = evidence / 'relocated'
  relocated.mkdir()
  shutil.copy2(built / executable, relocated / executable)
  shutil.copytree(built / 'data', relocated / 'data')
  assets = relocated / 'data/flutter_assets'
  if not (assets / 'app.so').is_file() or (assets / 'kernel_blob.bin').exists():
    raise RuntimeError('AOT bundle must contain app.so and no debug kernel')
  with (assets / 'app.so').open('rb') as aot:
    expected = b'MZ' if platform == 'windows' else b'\x7fELF'
    if not aot.read(4).startswith(expected):
      raise RuntimeError('AOT app.so has the wrong native library format')
  if platform == 'windows':
    for library in built.glob('*.dll'):
      shutil.copy2(library, relocated / library.name)
  else:
    shutil.copytree(built / 'libs', relocated / 'libs')
  api = desktop()
  runtime_log = evidence / 'runtime.log'
  with runtime_log.open('w', encoding='utf-8') as log:
    app = subprocess.Popen([str(relocated / executable)],
                           cwd=evidence,
                           stdout=log,
                           stderr=subprocess.STDOUT,
                           env=os.environ | {
                               'VK_INSTANCE_LAYERS': 'VK_LAYER_KHRONOS_validation',
                               'WGPU_VALIDATION': '1', 'WGPU_DEBUG': '1'
                           })
    try:
      client = wait_until(
          lambda: next((c for c in api.clients() if c['pid'] == app.pid), None), app, 30
      )
      if not client:
        raise RuntimeError('Relocated AOT app did not map a window')
      address = client['address']
      if platform != 'windows':
        api.action('float', address)
      api.resize_client(address, 800, 600)
      if platform == 'windows':
        api.move(address, 10, 10)
      api.action('focus', address)

      def frame_color():
        api.capture_window(address, evidence / 'pixel.png', (150, 150, 1, 1))
        png = (evidence / 'pixel.png').read_bytes()
        offset, data = 8, b''
        while offset < len(png):
          length = struct.unpack('!I', png[offset:offset + 4])[0]
          kind = png[offset + 4:offset + 8]
          if kind == b'IHDR' and (png[offset + 16] != 8 or png[offset + 17] not in (2, 6)):
            raise RuntimeError('Expected an 8-bit RGB/RGBA desktop capture')
          if kind == b'IDAT':
            data += png[offset + 8:offset + 8 + length]
          offset += length + 12
        # On the first pixel of a one-row image all PNG filter predictors are zero.
        return tuple(zlib.decompress(data)[1:4])

      if not wait_until(lambda: frame_color() == (32, 64, 96), app):
        raise RuntimeError('AOT app did not render its initial fixture color')
      api.capture_window(address, evidence / 'before.png')
      client = next(c for c in api.clients() if c['pid'] == app.pid)
      api.click(address, client['at'][0] + 150, client['at'][1] + 150)
      if not wait_until(lambda: frame_color() == (128, 64, 32), app):
        raise RuntimeError('AOT native click did not change the rendered fixture color')
      api.capture_window(address, evidence / 'after.png')
      api.action('close', address)
      if app.wait(timeout=15) != 0:
        raise RuntimeError('AOT app did not close with status zero')
    finally:
      if app.poll() is None:
        app.kill()
        app.wait()
  if DIAGNOSTIC.search(runtime_log.read_text(encoding='utf-8', errors='replace')):
    raise RuntimeError('GPU/runtime diagnostics in relocated AOT app')
  (evidence / 'result.json').write_text(
      json.dumps(
          dict(mode=args.mode, aot=True, relocated=True, native_click=True, clean_close=True)
      )
  )
  print(f'{args.mode} AOT relocation, native input/rendering and clean close passed.')


if __name__ == '__main__':
  main()
