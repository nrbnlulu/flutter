#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Decode per-view recordings and verify timing, actions, resize and failures."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from test_agent import Client


def probe(path):
  result = subprocess.run([
      'ffprobe', '-v', 'error', '-show_streams', '-show_format', '-show_frames', '-of', 'json',
      str(path)
  ],
                          check=True,
                          capture_output=True)
  value = json.loads(result.stdout)
  frames = value['frames']
  stamps = [float(f['best_effort_timestamp_time']) for f in frames]
  assert stamps == sorted(stamps) and len(stamps) == len(set(stamps)), stamps
  return value


def check_recording(status):
  assert status['status'] == 'complete', status
  for segment in status['segments']:
    video = probe(segment['path'])
    stream = video['streams'][0]
    assert (stream['width'], stream['height']) == (segment['width'], segment['height'])
    assert len(video['frames']) == segment['outputFrames'] == segment['targetFrames'], segment
    seconds = float(video['format']['duration'])
    actual = (segment['endSessionMicros'] - segment['startSessionMicros']) / 1e6
    assert abs(seconds - actual) < .25, (seconds, actual, segment)
  timeline = [
      json.loads(line)
      for line in (Path(status['directory']) / 'frames.jsonl').read_text().splitlines()
  ]
  stamps = [p['sessionMicros'] for p in timeline]
  assert stamps == sorted(stamps)
  assert all(p['sourceCompletedSessionMicros'] <= p['sessionMicros'] for p in timeline)
  return timeline


def exercise(c, output):
  c.rpc('session.handshake', protocolVersion=1)
  c.until(
      lambda: c.node(1, 'increment')
      if any(v['viewId'] == 1 and v['ready'] for v in c.rpc('windows.list')['views']) else None
  )
  c.rpc('recording.start', viewId=1, name='main', rate=15)
  c.until(lambda: c.rpc('recording.status', name='main')['status'] == 'recording')
  idle_start = time.monotonic()
  time.sleep(1.2)
  c.rpc('input.tap', viewId=1, selector={'identifier': 'increment'})
  c.until(lambda: c.node(1, 'click_count')['value'] == '1')
  c.rpc('capture.screenshot', viewId=1)
  time.sleep(.25)
  c.rpc('windows.resize', viewId=1, width=641, height=481)
  c.until(
      lambda: c.rpc('recording.status', name='main').get('activeSegment', {}).get('width') == 801
  )
  c.rpc('input.tap', viewId=1, selector={'identifier': 'open_dialog'})
  c.until(
      lambda: c.node(2, 'confirm')
      if any(v['viewId'] == 2 and v['ready'] for v in c.rpc('windows.list')['views']) else None
  )
  c.rpc('recording.start', viewId=2, name='dialog', rate=15)
  c.until(lambda: c.rpc('recording.status', name='dialog')['status'] == 'recording')
  time.sleep(.5)
  c.rpc('input.tap', viewId=2, selector={'identifier': 'confirm'})
  c.until(lambda: c.rpc('recording.status', name='dialog')['status'] == 'complete')
  dialog = c.rpc('recording.stop', name='dialog')
  main = c.rpc('recording.stop', name='main')
  points = check_recording(main)
  dialog_points = check_recording(dialog)
  assert len(main['segments']) == 2, main
  assert [(s['width'], s['height']) for s in main['segments']] == [(875, 625), (801, 601)]
  assert [(s['width'], s['height']) for s in dialog['segments']] == [(525, 300)]
  # Idle output repeats the same completed source frame for elapsed real time.
  from collections import Counter
  assert max(Counter(p['sourceFrameId'] for p in points).values()) >= 12
  assert points[0]['sessionMicros'] < dialog_points[0]['sessionMicros'] < points[-1]['sessionMicros']
  # Decode a corner pixel from every video frame: the pointer action changes
  # the controlled Scaffold background, proving visible action evidence.
  pixels = subprocess.run([
      'ffmpeg', '-v', 'error', '-i', main['segments'][0]['path'], '-vf', 'crop=1:1:0:0,format=rgba',
      '-f', 'rawvideo', 'pipe:1'
  ],
                          check=True,
                          capture_output=True).stdout
  assert len({pixels[i:i + 4] for i in range(0, len(pixels), 4)}) >= 2
  stats = c.rpc('windows.inspect', viewId=1)['captureStatistics']
  c.rpc('session.shutdown')
  return {
      'status': 'passed', 'main': main, 'dialog': dialog, 'captureStatistics': stats,
      'elapsed': time.monotonic() - idle_start
  }


def run(args, output, encoder=None, fail=False):
  output.mkdir(parents=True, exist_ok=False)
  with tempfile.TemporaryDirectory(prefix='frsr-') as temporary:
    endpoint = Path(temporary) / 'control.sock'
    env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
    env.update(
        FLUTTER_RUST_HEADLESS='1',
        FLUTTER_RUST_HEADLESS_PIXEL_RATIO='1.25',
        FLUTTER_RUST_AGENT_SOCKET=str(endpoint),
        FLUTTER_RUST_AGENT_ARTIFACTS=str(output),
        WGPU_VALIDATION='1'
    )
    if encoder:
      env['FLUTTER_RUST_FFMPEG'] = str(encoder)
    with (output / 'app.log').open('w') as log:
      process = subprocess.Popen([
          str(args.runner.resolve()),
          str(args.assets.resolve()),
          str(args.icu.resolve())
      ],
                                 env=env,
                                 stdout=log,
                                 stderr=log,
                                 start_new_session=True)
      try:
        stream = socket.socket(socket.AF_UNIX)
        stream.settimeout(15)
        deadline = time.monotonic() + 30
        while True:
          try:
            stream.connect(str(endpoint))
            break
          except (FileNotFoundError, ConnectionRefusedError):
            assert process.poll() is None and time.monotonic() < deadline, (output /
                                                                            'app.log').read_text()
            time.sleep(.025)
        c = Client(stream)
        if encoder:
          c.rpc('session.handshake', protocolVersion=1)
          c.until(
              lambda: c.node(1, 'increment')
              if any(v['viewId'] == 1 for v in c.rpc('windows.list')['views']) else None
          )
          if fail:
            c.rpc('recording.start', viewId=1, name='failed')
            if fail == 'hang':
              c.until(lambda: c.rpc('recording.status', name='failed')['status'] == 'recording')
              time.sleep(.25)
              c.rpc('windows.list')
            else:
              c.until(lambda: c.rpc('recording.status', name='failed')['status'] == 'failed')
            started = time.monotonic()
            c.error('recording_failed', 'recording.stop', name='failed', timeoutMs=12000)
            assert time.monotonic() - started < 8
            c.error('recording_failed', 'session.health')
          else:
            c.error('unsupported_capability', 'recording.start', viewId=1, name='missing')
            c.rpc('capture.screenshot', viewId=1)
          c.rpc('session.shutdown')
          result = {'status': 'passed'}
        else:
          result = exercise(c, output)
        stream.close()
        code = process.wait(timeout=15)
        assert code == 0 or fail, code
        (output / 'report.json').write_text(json.dumps(result, indent=2) + '\n')
        return result
      finally:
        if process.poll() is None:
          os.killpg(process.pid, 9)
          process.wait()


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for flag in ('runner', 'assets', 'icu', 'artifacts'):
    parser.add_argument('--' + flag, type=Path, required=True)
  args = parser.parse_args()
  output = args.artifacts.resolve()
  output.mkdir(parents=True, exist_ok=False)
  run(args, output / 'normal')
  run(args, output / 'missing', encoder=output / 'missing-ffmpeg')
  encoder = output / 'crashing-ffmpeg'
  encoder.write_text(
      '#!/usr/bin/env python3\nimport sys\nif "-encoders" in sys.argv:\n print(" V..... ffv1 lossless");sys.exit(0)\nsys.exit(23)\n'
  )
  encoder.chmod(0o700)
  run(args, output / 'crash', encoder=encoder, fail=True)
  hung = output / 'blocked-ffmpeg'
  hung.write_text(
      '#!/usr/bin/env python3\nimport sys,time\nif "-encoders" in sys.argv:\n print(" V..... ffv1 lossless");sys.exit(0)\ntime.sleep(60)\n'
  )
  hung.chmod(0o700)
  run(args, output / 'blocked', encoder=hung, fail='hang')
  probe = output / 'blocked-codec-discovery'
  probe.write_text('#!/usr/bin/env python3\nimport time\ntime.sleep(60)\n')
  probe.chmod(0o700)
  run(args, output / 'blocked-discovery', encoder=probe)
  print(f'PASS: {output}')


if __name__ == '__main__':
  main()
