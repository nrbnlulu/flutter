#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Kill the owned runner and verify CLI supervision retains partial recording."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time
from test_agent import Client


class Pipe:

  def __init__(self, process):
    self.process = process

  def makefile(self, *args, **kwargs):
    return self

  def write(self, value):
    self.process.stdin.write(value)
    self.process.stdin.flush()

  def readline(self):
    return self.process.stdout.readline()


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for flag in ('rlutter', 'flutter-root', 'project', 'artifacts'):
    parser.add_argument('--' + flag, type=Path, required=True)
  parser.add_argument('--local-engine-src', type=Path)
  args = parser.parse_args()
  output = args.artifacts.resolve()
  assert not output.exists()
  command = [str(args.rlutter.resolve()), '--flutter-root', str(args.flutter_root.resolve())]
  if args.local_engine_src:
    command += ['--local-engine-src', str(args.local_engine_src.resolve())]
  command += ['agent', '--headless', '--stdio', '--artifacts', str(output)]
  env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
  env['RUSTUP_TOOLCHAIN'] = '1.93.1'
  output.parent.mkdir(parents=True, exist_ok=True)
  with output.with_suffix('.log').open('w') as log:
    process = subprocess.Popen(
        command,
        cwd=args.project.resolve(),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=log
    )
    try:
      c = Client(Pipe(process))
      handshake = c.rpc('session.handshake', protocolVersion=1)
      c.until(
          lambda: c.node(1, 'increment')
          if any(v['viewId'] == 1 for v in c.rpc('windows.list')['views']) else None
      )
      c.rpc('recording.start', viewId=1, name='crash', rate=15)
      c.until(lambda: (output / 'recording-crash/manifest.json').exists())
      children = Path(f'/proc/{process.pid}/task/{process.pid}/children').read_text().split()
      assert len(children) == 1, children
      os.kill(int(children[0]), signal.SIGKILL)
      event = json.loads(process.stdout.readline())
      assert event['event'] == 'session.exited' and not event['success'], event
      process.stdin.close()
      assert process.wait(timeout=15) != 0
      manifest = json.loads((output / 'recording-crash/manifest.json').read_text())
      assert manifest['status'] == 'failed' and manifest['partial'], manifest
      assert manifest['reason'] == 'process_exit', manifest
      assert list((output / 'recording-crash').glob('*.mkv'))
      print(f'PASS: {output}')
    finally:
      if process.poll() is None:
        process.kill()
        process.wait()


if __name__ == '__main__':
  main()
