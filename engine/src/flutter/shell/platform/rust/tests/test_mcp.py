#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Exercise MCP discovery, real app input, negative assertions, images and EOF.

Requires a generated project using tests/agent_fixture/lib/main.dart and its
flutter_rust_agent dependency, as used by test_verify.py.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import selectors
import subprocess


def exercise(args, output, shutdown):
  artifacts = output / ('shutdown' if shutdown else 'eof')
  command = [str(args.rlutter.resolve()), '--flutter-root', str(args.flutter_root.resolve())]
  if args.local_engine_src:
    command += ['--local-engine-src', str(args.local_engine_src.resolve())]
  command += ['mcp', '--headless', '--stdio', '--artifacts', str(artifacts)]
  env = dict(
      os.environ, RUSTUP_TOOLCHAIN='1.93.1', VK_INSTANCE_LAYERS='VK_LAYER_KHRONOS_validation'
  )
  with (output / f'{artifacts.name}.log').open('w') as log:
    process = subprocess.Popen(
        command,
        cwd=args.project.resolve(),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=log
    )
    sequence = 0
    transcript = []
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)

    def rpc(method, params):
      nonlocal sequence
      sequence += 1
      request = {'jsonrpc': '2.0', 'id': sequence, 'method': method, 'params': params}
      process.stdin.write((json.dumps(request) + '\n').encode())
      process.stdin.flush()
      assert selector.select(300), f'MCP timeout: {method}'
      response = json.loads(process.stdout.readline())
      assert response['jsonrpc'] == '2.0' and response['id'] == sequence, response
      transcript.append(response)
      assert 'error' not in response, response
      return response['result']

    def tool(name, expected_error=False, **params):
      result = rpc('tools/call', {'name': 'rlutter_' + name, 'arguments': params})
      assert bool(result.get('isError')) == expected_error, result
      return result

    try:
      rpc(
          'initialize', {
              'protocolVersion': '2025-11-25', 'capabilities': {},
              'clientInfo': {'name': 'rlutter-test', 'version': '1'}
          }
      )
      process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
      process.stdin.flush()
      tools = rpc('tools/list', {})['tools']
      assert any(t['name'] == 'rlutter_capture_screenshot' for t in tools)
      assert not artifacts.exists(), 'discovery must not build or launch'
      tool('verify_wait', kind='windows', where={'title': 'Agent form', 'ready': True}, count=1)
      windows = json.loads(tool('windows_list')['content'][0]['text'])['views']
      view = next(window['viewId'] for window in windows if window['title'] == 'Agent form')
      tool('verify_wait', viewId=view, selector={'identifier': 'increment'}, count=1)
      tool('input_tap', viewId=view, selector={'identifier': 'increment'})
      tool('verify_wait', viewId=view, selector={'identifier': 'click_count'}, value='1')
      tool(
          'verify_assert',
          expected_error=True,
          viewId=view,
          selector={'identifier': 'click_count'},
          value='impossible'
      )
      capture = tool('capture_screenshot', viewId=view)
      image = next(c for c in capture['content'] if c['type'] == 'image')
      assert image['mimeType'] == 'image/png'
      assert base64.b64decode(image['data'], validate=True).startswith(b'\x89PNG\r\n\x1a\n')
      metadata = json.loads(capture['content'][0]['text'])
      assert metadata['uiFrameNumber'] > metadata['uiFrameBoundary']
      tool('session_health')
      if shutdown:
        tool('session_shutdown')
        tool('windows_list', expected_error=True)
      process.stdin.close()
      assert process.wait(timeout=30) == 0
      assert not process.stdout.read(), 'unexpected protocol output after EOF'
      # Keep image bytes in the artifact PNG; avoid duplicating them in the trace.
      image['data'] = '<retained in artifact PNG>'
      (output / f'{artifacts.name}.json').write_text(json.dumps(transcript, indent=2) + '\n')
    finally:
      selector.close()
      if process.poll() is None:
        process.kill()
        process.wait()


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for flag in ('rlutter', 'flutter-root', 'project', 'artifacts'):
    parser.add_argument('--' + flag, type=Path, required=True)
  parser.add_argument('--local-engine-src', type=Path)
  args = parser.parse_args()
  output = args.artifacts.resolve()
  output.mkdir(parents=True, exist_ok=False)
  for shutdown in (True, False):
    exercise(args, output, shutdown)
  print(f'PASS: {output}')


if __name__ == '__main__':
  main()
