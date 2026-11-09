#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Exercise native text/clipboard shortcuts and compare Flutter/native metrics."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from native_desktop import desktop
from test_texture import DIAGNOSTIC, wait_until


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for name in ('runner', 'assets', 'icu'):
    parser.add_argument('--' + name, required=True, type=Path)
  args = parser.parse_args()
  api = desktop()
  work = Path(tempfile.mkdtemp(prefix='flutter-rust-input-'))
  state_path = work / 'state.json'
  log_path = work / 'runner.log'
  environment = os.environ | {
      'FLUTTER_RUST_INPUT_STATUS': str(state_path),
      'VK_INSTANCE_LAYERS': 'VK_LAYER_KHRONOS_validation',
      'WGPU_VALIDATION': '1',
      'WGPU_DEBUG': '1',
  }
  address = None
  succeeded = False
  with log_path.open('wb') as log:
    process = subprocess.Popen([str(args.runner), str(args.assets),
                                str(args.icu)],
                               env=environment,
                               stdout=log,
                               stderr=subprocess.STDOUT)

    def state():
      try:
        return json.loads(state_path.read_text(encoding='utf-8'))
      except (OSError, ValueError):
        return {}

    def require(predicate, message):
      result = wait_until(lambda: state() if predicate(state()) else None, process, 15)
      if not result:
        raise RuntimeError(message)
      return result

    def click(control):
      current = require(lambda s: control in s.get('controls', {}), 'No control geometry')
      client = next(c for c in api.clients() if str(c['address']) == address)
      # Native adapters use their compositor's coordinates. Derive the mapping
      # from the actual client area instead of assuming 100% display scaling.
      x, y = current['controls'][control]
      sx = client['size'][0] / current['logicalSize'][0]
      sy = client['size'][1] / current['logicalSize'][1]
      api.click(address, client['at'][0] + x * sx, client['at'][1] + y * sy)
      time.sleep(.15)

    def check_metrics():
      native = api.metrics(address)
      size_key = 'physicalSize' if native['coordinate_space'] == 'physical' else 'logicalSize'
      value = require(
          lambda s: s.get(size_key) == native['size'],
          'Flutter size differs from native client size'
      )
      if native['scale'] is not None and abs(value['dpr'] - native['scale']) > .001:
        raise RuntimeError(f"Flutter/native DPI mismatch: {value['dpr']} vs {native['scale']}")
      return dict(native=native, flutter=value)

    try:
      client = wait_until(
          lambda: next((c for c in api.clients() if c['pid'] == process.pid), None), process, 20
      )
      if not client:
        raise RuntimeError('Input fixture did not map a native window')
      address = str(client['address'])
      require(
          lambda s: s.get('ready') and len(s.get('controls', {})) == 4,
          'Input fixture did not become ready'
      )
      initial = check_metrics()
      api.capture_window(address, work / 'initial.png')
      click('field')
      api.capture_window(address, work / 'focused.png')
      require(lambda s: s.get('focused'), 'Native pointer click did not focus the text field')
      for character in 'hello123':
        api.chord(address, character)
        time.sleep(.03)
      require(lambda s: s.get('text') == 'hello123', 'Native typing did not reach Flutter')
      api.chord(address, 'CTRL', 'A')
      api.chord(address, 'CTRL', 'C')
      click('read')
      require(
          lambda s: s.get('clipboard') == 'hello123' and s.get('hasStrings'),
          'Native copy did not round-trip through Flutter Clipboard.getData'
      )
      click('copy')
      sample = 'A\U0001f600\u05e9\u05dc\u05d5\u05dd'
      require(
          lambda s: s.get('clipboard') == sample and s.get('hasStrings'),
          'Unicode Clipboard.setData/getData/hasStrings failed'
      )
      click('field')
      api.chord(address, 'CTRL', 'A')
      api.chord(address, 'CTRL', 'V')
      require(
          lambda s: s.get('text') == sample and s.get('selection') == [7, 7] and s.get('composing')
          == [-1, -1], 'Unicode paste or UTF-16 selection was incorrect'
      )
      api.capture_window(address, work / 'unicode-paste.png')
      api.action('hide', address)
      require(
          lambda s: s.get('lifecycle') in ('hidden', 'paused', 'inactive'),
          'Minimizing did not report a lifecycle transition'
      )
      api.action('restore', address)
      api.action('focus', address)
      require(lambda s: s.get('lifecycle') == 'resumed', 'Restore did not resume the app')
      api.action('resize', address, width=911, height=677)
      wait_until(lambda: api.metrics(address)['size'] != initial['native']['size'], process)
      resized = check_metrics()
      if resized['native']['size'] == initial['native']['size']:
        raise RuntimeError('Native resize did not change client metrics')
      layouts = api.ime_layouts()
      report = dict(
          initial=initial,
          resized=resized,
          ime_layouts=layouts,
          ime_composition='not exercised; requires an interactive IME',
          scale_transition='not exercised; requires a second display/scaling setting'
      )
      (work / 'coverage.json').write_text(json.dumps(report, ensure_ascii=True, indent=2))
      succeeded = True
    finally:
      try:
        if address and process.poll() is None:
          click('restore')
          require(lambda s: s.get('restored'), 'Could not restore previous clipboard text')
          api.action('close', address)
          if process.wait(timeout=10) != 0:
            raise RuntimeError(f'Input fixture exited with {process.returncode}')
      finally:
        if process.poll() is None:
          process.kill()
          process.wait()
        print(f'Input validation evidence: {work}', flush=True)
  if DIAGNOSTIC.search(log_path.read_text(errors='replace')):
    raise RuntimeError('GPU/runtime diagnostics were reported')
  if succeeded:
    print('Native typing, copy/paste, UTF-16 selection, lifecycle and current-scale DPI passed.')
    print(f'Installed IMM32 IME layouts: {layouts}; composition and DPI transitions not validated.')


if __name__ == '__main__':
  main()
