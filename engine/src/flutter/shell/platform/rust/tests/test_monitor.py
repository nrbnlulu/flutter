#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Real visible Linux/AOT monitor integration; requires a Wayland/X11 session.

Build a generated app from monitor_fixture/main.dart with agent-monitor and
FLUTTER_RUST_AGENT=true, then pass its release binary and the rebuilt rlutter.
"""
import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--runner', type=Path, required=True)
  parser.add_argument('--rlutter', type=Path, required=True)
  parser.add_argument('--artifacts', type=Path, required=True)
  args = parser.parse_args()
  args.artifacts.mkdir(parents=True, exist_ok=False)
  transcript = []
  with tempfile.TemporaryDirectory(prefix='rlutter-monitor-') as directory:
    endpoint = Path(directory) / 'control.sock'
    env = dict(os.environ)
    env.pop('FLUTTER_RUST_MONITOR_SOCKET', None)
    with (args.artifacts / 'app.log').open('w') as log:
      ordinary = subprocess.Popen([str(args.runner.resolve())], env=env, stdout=log, stderr=log)
      try:
        time.sleep(2)
        assert ordinary.poll() is None
        assert not endpoint.exists()
        print("PASS: normal release launch exposes no endpoint", flush=True)
      finally:
        ordinary.terminate()
        ordinary.wait(timeout=10)
      env['FLUTTER_RUST_MONITOR_SOCKET'] = str(endpoint)
      app = subprocess.Popen([str(args.runner.resolve())], env=env, stdout=log, stderr=log)
      client = None
      try:
        deadline = time.monotonic() + 20
        while not endpoint.exists() and time.monotonic() < deadline:
          assert app.poll() is None, 'app exited during startup'
          time.sleep(.1)
        assert endpoint.stat().st_mode & 0o777 == 0o600

        def start():
          return subprocess.Popen([str(args.rlutter.resolve()), 'mcp', '--stdio'],
                                  stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE,
                                  stderr=log,
                                  cwd=directory)

        sequence = 0

        def rpc(method, params):
          nonlocal sequence
          sequence += 1
          client.stdin.write((
              json.dumps({'jsonrpc': '2.0', 'id': sequence, 'method': method, 'params': params}) +
              '\n'
          ).encode())
          client.stdin.flush()
          with selectors.DefaultSelector() as ready:
            ready.register(client.stdout, selectors.EVENT_READ)
            assert ready.select(10), f'timeout: {method}'
          result = json.loads(client.stdout.readline())
          assert result['id'] == sequence and 'error' not in result, result
          transcript.append(result)
          return result['result']

        def tool(name, **params):
          result = rpc('tools/call', {'name': 'rlutter_' + name, 'arguments': params})
          assert not result.get('isError'), result
          return json.loads(result['content'][0]['text'])

        def attach():
          rpc(
              'initialize', {
                  'protocolVersion': '2025-11-25', 'capabilities': {},
                  'clientInfo': {'name': 'monitor-proof', 'version': '1'}
              }
          )
          client.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
          client.stdin.flush()
          return tool('session_attach', socket_path=str(endpoint))

        client = start()
        attached = attach()
        sid = attached['session_id']
        caps = tool('session_capabilities', session_id=sid)
        assert caps['monitor'] and caps['readOnly'] and caps['buildMode'] == 'release', caps
        print('PASS: MCP attached to release/AOT app', flush=True)
        epoch = caps['epoch']
        time.sleep(2)
        windows = tool('windows_list', session_id=sid)['views']
        assert windows, windows
        view = next(w['viewId'] for w in windows if w['title'] == 'Linux release monitor proof')
        snapshot = tool('semantics_snapshot', session_id=sid, viewId=view)
        assert any(n.get('identifier') == 'uptime' for n in snapshot['nodes']), snapshot
        health = tool('session_health', session_id=sid)
        assert health['healthy'] and health['frameworkErrors'], health
        print('PASS: live window, semantics and Flutter error bridge', flush=True)
        events = tool('monitor_events', session_id=sid, after=0)
        cursor = events['nextSequence']
        tool('session_detach', session_id=sid)
        assert app.poll() is None
        client.stdin.close()
        assert client.wait(timeout=10) == 0
        print('MCP detached. App runs independently for 13 seconds...', flush=True)
        time.sleep(13)
        assert app.poll() is None, 'disconnect killed the app'
        client = start()
        sid = attach()['session_id']
        assert tool('session_capabilities', session_id=sid)['epoch'] == epoch
        events = tool('monitor_events', session_id=sid, after=cursor)
        assert events['nextSequence'] > cursor
        assert any('intentional-monitor-proof' in json.dumps(e) for e in events['events']), events
        health = tool('session_health', session_id=sid)
        assert not health['healthy'] and health['errorCount'] > 0, health
        print('PASS: reconnected to same app; known error retained', flush=True)
        result = rpc(
            'tools/call', {
                'name': 'rlutter_input_tap',
                'arguments': {'session_id': sid, 'viewId': view, 'x': 1, 'y': 1}
            }
        )
        assert result.get('isError'), result
        client.stdin.close()
        assert client.wait(timeout=10) == 0
        assert app.poll() is None, 'MCP EOF killed attached app'
        print('PASS: MCP EOF leaves app alive', flush=True)
        client = start()
        sid = attach()['session_id']
        tool('session_shutdown', session_id=sid)
        assert app.poll() is None, 'attached shutdown killed the app'
        print('PASS: attached shutdown detaches; app still alive', flush=True)
        client.stdin.close()
        assert client.wait(timeout=10) == 0
        (args.artifacts / 'result.json').write_text(
            json.dumps({'passed': True, 'pid': app.pid, 'epoch': epoch, 'transcript': transcript},
                       indent=2)
        )
      finally:
        if client and client.poll() is None:
          client.kill()
          client.wait()
        app.terminate()
        app.wait(timeout=10)
  print('PASS: release monitor attach, errors, reconnect and independent app lifetime')


if __name__ == '__main__':
  main()
