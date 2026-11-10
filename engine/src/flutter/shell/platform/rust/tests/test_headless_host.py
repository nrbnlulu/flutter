#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Verify display-free host lifecycle fixtures and source-linked SDK textures."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

from test_headless_probe import save_png


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for option in ('runner', 'assets', 'icu', 'strace', 'artifacts'):
    parser.add_argument('--' + option, type=Path, required=True)
  parser.add_argument('--expected-status', default='complete released=60')
  parser.add_argument('--pixel-ratio', type=float, default=1.25)
  parser.add_argument('--plugin', action='store_true')
  parser.add_argument('--idle', action='store_true')
  parser.add_argument('--tooltip-motion', action='store_true')
  parser.add_argument('--icd', type=Path)
  parser.add_argument('--driver-library-dir', type=Path)
  args = parser.parse_args()
  output = args.artifacts.resolve()
  output.mkdir(parents=True, exist_ok=False)
  env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
  env.update(
      FLUTTER_RUST_HEADLESS='1',
      WGPU_VALIDATION='1',
      WGPU_DEBUG='1',
      FLUTTER_RUST_HEADLESS_PIXEL_RATIO=str(args.pixel_ratio),
      FLUTTER_RUST_WINDOWING_STATUS=str(output / 'status')
  )
  if args.icd:
    env['VK_DRIVER_FILES'] = str(args.icd.resolve())
  if args.driver_library_dir:
    env['LD_LIBRARY_PATH'] = str(args.driver_library_dir.resolve())
  if args.plugin:
    env.update(
        FLUTTER_RUST_HEADLESS_PLUGIN_PROBE='1',
        FLUTTER_RUST_HEADLESS_FIXTURE_EXIT_MS='800',
        FLUTTER_RUST_HEADLESS_ARTIFACTS=str(output / 'frames')
    )
  if args.tooltip_motion:
    env.update(
        FLUTTER_RUST_TOOLTIP_MOTION=str(output / 'motion'),
        FLUTTER_RUST_HEADLESS_STATE=str(output / 'windows.json')
    )
  report = {'status': 'failed'}
  command = [
      str(args.strace.resolve()), '-f', '-e', 'trace=connect', '-o',
      str(output / 'connect.trace'),
      str(args.runner.resolve()),
      str(args.assets.resolve()),
      str(args.icu.resolve())
  ]
  try:
    with (output / 'log').open('w') as log:
      process = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=True)
      try:
        if args.tooltip_motion:

          def until(predicate):
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline and process.poll() is None:
              try:
                value = predicate()
                if value:
                  return value
              except (FileNotFoundError, json.JSONDecodeError):
                pass
              time.sleep(.02)
            raise AssertionError('tooltip motion predicate timed out')

          def tooltip():
            return next((
                v for v in json.loads((output / 'windows.json').read_text())['views']
                if v['kind'] == 'tooltip' and v['ready'] and not v['removing']
            ), None)

          motion = output / 'motion'
          until(tooltip)
          motion.write_text('resize')
          until(lambda: motion.read_text() == 'resized')
          before = until(tooltip)['bounds']
          assert before[2:] == [200, 120], before
          motion.write_text('anchor')
          until(lambda: motion.read_text() == 'anchored')
          anchored = until(tooltip)['bounds']
          assert anchored[:2] == [before[0] + 27, before[1] + 24], (before, anchored)
          motion.write_text('partial')
          until(lambda: motion.read_text() == 'partial complete')
          partial = until(tooltip)['bounds']
          assert partial[:2] == [anchored[0] - 10, anchored[1] - 7], (anchored, partial)
          motion.write_text('complete')
          report['tooltipBounds'] = [before, anchored, partial]
        if args.idle:
          # Locate the runner below strace, then measure its owning UI
          # thread after startup. No capture or animation is enabled.
          child_file = Path(f'/proc/{process.pid}/task/{process.pid}/children')
          limit = time.monotonic() + 10
          while not child_file.read_text().strip():
            assert time.monotonic() < limit
            time.sleep(.05)
          runner_pid = child_file.read_text().split()[0]
          time.sleep(2)

          def ticks():
            fields = Path(f'/proc/{runner_pid}/task/{runner_pid}/stat'
                         ).read_text().rsplit(')', 1)[1].split()
            return int(fields[11]) + int(fields[12])

          first = ticks()
          time.sleep(1)
          consumed = ticks() - first
          report['idleUiCpuSeconds'] = consumed / os.sysconf('SC_CLK_TCK')
          assert consumed <= os.sysconf('SC_CLK_TCK') * .05, report
          # Static fixture intentionally stays alive; terminate only
          # this measurement process. Other runs check graceful exit.
          os.kill(int(runner_pid), 15)
        code = process.wait(timeout=90)
        report['exitCode'] = code
        assert args.idle or code == 0
      finally:
        if process.poll() is None:
          os.killpg(process.pid, 9)
          process.wait()
    logs = (output / 'log').read_text()
    assert not re.search(
        r'VUID-|Validation Error|UNASSIGNED-|panicked|\[ERROR\]|Unhandled Exception', logs
    ), logs[-10000:]
    trace = (output / 'connect.trace').read_text()
    assert not re.search(r'\.X11-unix|wayland-[0-9]+', trace), trace
    if args.plugin:
      frames = sorted((output / 'frames').glob('view-0-*.rgba'))
      assert len(frames) >= 2, frames
      for raw in (frames[0], frames[-1]):
        width, height = map(int, re.search(r'-(\d+)x(\d+)\.rgba$', raw.name).groups())
        rgba = raw.read_bytes()
        x, y = int(340 * args.pixel_ratio), int(40 * args.pixel_ratio)
        assert rgba[(y * width + x) * 4:][:4] == bytes([255, 255, 0, 255])
        save_png(raw.with_suffix('.png'), width, height, rgba)
      report['pluginTextureCaptures'] = len(frames)
    elif not args.idle:
      status = (output / 'status').read_text()
      assert status == args.expected_status, status
      report['fixtureStatus'] = status
    report.update(status='passed', displaySocketConnections=0)
  finally:
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
  print(f"PASS: {output / 'report.json'}")


if __name__ == '__main__':
  main()
