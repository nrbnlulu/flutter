#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Exercise native agent transport, editing, multi-view input and fresh capture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import struct
import subprocess
import tempfile
import time


class Client:

  def __init__(self, stream):
    self.stream = stream.makefile('rwb', buffering=0)
    self.sequence = 0
    self.events = []

  def send(self, method, **params):
    self.sequence += 1
    self.stream.write(
        (json.dumps({'id': self.sequence, 'method': method, 'params': params}) + '\n').encode()
    )
    return self.sequence

  def read(self):
    line = self.stream.readline()
    assert line, 'control channel closed'
    return json.loads(line)

  def response(self, request):
    while True:
      value = self.read()
      if 'event' in value:
        self.events.append(value)
      else:
        assert value['id'] == request, value
        return value

  def rpc(self, method, **params):
    value = self.response(self.send(method, **params))
    assert 'error' not in value, value
    return value['result']

  def error(self, code, method, **params):
    value = self.response(self.send(method, **params))
    assert value['error']['code'] == code, value

  def until(self, predicate, seconds=8):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
      last = predicate()
      if last:
        return last
      time.sleep(.025)
    raise AssertionError(f'predicate timed out; last state: {last!r}')

  def node(self, view, identifier):
    nodes = self.rpc('semantics.snapshot', viewId=view)['nodes']
    return next((n for n in nodes if n['identifier'] == identifier), None)


def dimensions(path):
  data = Path(path).read_bytes()
  assert data[:8] == b'\x89PNG\r\n\x1a\n'
  return struct.unpack('>II', data[16:24])


def exercise(client, output):
  c = client
  c.error('version_mismatch', 'session.handshake', protocolVersion=99)
  handshake = c.rpc('session.handshake', protocolVersion=1)
  assert handshake['shellAbi'] == 16
  c.rpc(
      'session.subscribe',
      events=['windows.changed', 'semantics.updated', 'action.dispatched', 'capture.completed']
  )
  c.error('invalid_request', 'windows.list', typo=True)
  c.error('unsupported_capability', 'made.up')
  c.error('invalid_request', 'verify.assert', kind='windows', where={'focused': 'true'}, count=0)
  c.error('invalid_request', 'verify.assert', kind=123, viewId=1, selector={'identifier': 'name'}, count=0)
  c.until(
      lambda: c.node(1, 'name')
      if any(v['viewId'] == 1 and v['ready'] for v in c.rpc('windows.list')['views']) else None
  )
  c.error('ambiguous_selector', 'semantics.find', viewId=1, selector={'label': 'Duplicate'})
  handle = c.rpc('semantics.find', viewId=1, selector={'identifier': 'name'})['handle']
  c.rpc('input.tap', viewId=1, selector={'identifier': 'name'})
  text = 'שלום 🌍 café'
  c.rpc('input.replaceText', viewId=1, text=text)
  c.until(lambda: (n := c.node(1, 'name')) and n['value'] == text)
  c.error('stale_handle', 'input.tap', viewId=1, handle=handle)
  c.error('invalid_request', 'capture.screenshot', viewId=1, name='build-manifest.png')
  before = c.rpc('capture.screenshot', viewId=1)
  assert before['uiFrameNumber'] > before['uiFrameBoundary']
  assert dimensions(before['path']) == (875, 625), before
  c.rpc('input.tap', viewId=1, selector={'identifier': 'increment'})
  c.until(lambda: c.node(1, 'click_count')['value'] == '1')
  after = c.rpc('capture.screenshot', viewId=1)
  assert after['uiFrameNumber'] > before['uiFrameNumber']
  assert hashlib.sha256(Path(before['path']).read_bytes()
                       ).digest() != hashlib.sha256(Path(after['path']).read_bytes()).digest()
  c.rpc('windows.resize', viewId=1, width=641, height=481)
  resized = c.rpc('capture.screenshot', viewId=1)
  assert dimensions(resized['path']) == (801, 601), resized
  assert resized['sizeGeneration'] > after['sizeGeneration']
  c.error(
      'stale_handle',
      'capture.screenshot',
      viewId=1,
      mode='afterFrame',
      afterFrame=after['frameId'],
      sizeGeneration=after['sizeGeneration']
  )
  c.rpc('input.tap', viewId=1, selector={'identifier': 'open_dialog'})
  c.until(
      lambda: c.node(2, 'dialog_value')
      if any(v['viewId'] == 2 and v['ready'] for v in c.rpc('windows.list')['views']) else None
  )
  assert c.node(2, 'dialog_value')['value'] == text
  dialog = c.rpc('windows.inspect', viewId=2)
  assert dialog['focused'], dialog
  capture = c.rpc('capture.screenshot', viewId=2)
  assert dimensions(capture['path']) == (525, 300)
  # A long frame constraint must still allow cancellation and UI commands.
  blocked = c.send(
      'capture.screenshot',
      viewId=2,
      mode='afterFrame',
      afterFrame=2**63,
      sizeGeneration=capture['sizeGeneration'],
      timeoutMs=5000
  )
  cancellation = c.send('request.cancel', id=blocked)
  replies = {}
  while len(replies) < 2:
    message = c.read()
    if 'event' in message:
      c.events.append(message)
    else:
      replies[message['id']] = message
  assert replies[blocked]['error']['code'] == 'cancelled', replies
  assert replies[cancellation]['result']['cancelled'] == blocked
  blocked = c.send(
      'capture.screenshot',
      viewId=2,
      mode='afterFrame',
      afterFrame=2**63,
      sizeGeneration=capture['sizeGeneration'],
      timeoutMs=5000
  )
  close = c.send('input.tap', viewId=2, selector={'identifier': 'confirm'})
  replies = {}
  while len(replies) < 2:
    message = c.read()
    if 'event' in message:
      c.events.append(message)
    else:
      replies[message['id']] = message
  assert 'result' in replies[close], replies
  assert replies[blocked]['error']['code'] == 'view_closed', replies
  c.until(lambda: c.node(1, 'saved_value')['value'] == text)
  c.until(lambda: all(v['viewId'] != 2 for v in c.rpc('windows.list')['views']))
  c.rpc('capture.screenshot', viewId=1)
  # Held pointer/key cleanup is also exercised on graceful session shutdown.
  c.rpc('input.pointer', viewId=1, phase='down', x=5, y=5)
  c.rpc('input.key', viewId=1, physical=0x700e1, logical=0x200000102, down=True)
  c.rpc('session.shutdown')
  return {
      'status': 'passed', 'handshake': handshake, 'captures': [before, after, resized, capture],
      'events': len(c.events)
  }


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for option in ('runner', 'assets', 'icu', 'artifacts'):
    parser.add_argument('--' + option, type=Path, required=True)
  parser.add_argument('--icd', type=Path)
  args = parser.parse_args()
  output = args.artifacts.resolve()
  output.mkdir(parents=True, exist_ok=False)
  report = {'status': 'failed'}
  with tempfile.TemporaryDirectory(prefix='frsa-') as temporary:
    endpoint = Path(temporary) / 'control.sock'
    env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
    env.update(
        FLUTTER_RUST_HEADLESS='1',
        FLUTTER_RUST_HEADLESS_PIXEL_RATIO='1.25',
        FLUTTER_RUST_AGENT_SOCKET=str(endpoint),
        FLUTTER_RUST_AGENT_ARTIFACTS=str(output),
        WGPU_VALIDATION='1',
        WGPU_DEBUG='1'
    )
    if args.icd:
      env['VK_DRIVER_FILES'] = str(args.icd.resolve())
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
        stream.settimeout(10)
        deadline = time.monotonic() + 30
        while True:
          try:
            stream.connect(str(endpoint))
            break
          except (FileNotFoundError, ConnectionRefusedError):
            assert process.poll() is None and time.monotonic() < deadline, (output /
                                                                            'app.log').read_text()
            time.sleep(.025)
        report = exercise(Client(stream), output)
        stream.close()
        assert process.wait(timeout=10) == 0
        logs = (output / 'app.log').read_text()
        assert not re.search(r'VUID-|Validation Error|UNASSIGNED-|panicked|\[ERROR\]',
                             logs), logs[-10000:]
        assert 'fixture stdout is not a protocol message' in logs
      finally:
        if process.poll() is None:
          os.killpg(process.pid, 9)
          process.wait()
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
  print(f"PASS: {output / 'report.json'}")


if __name__ == '__main__':
  main()
