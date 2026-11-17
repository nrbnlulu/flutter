#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compare full and compact MCP observations against the real agent fixture.

Example (the supplied generated project is copied, never edited in place):
  python3 tests/test_compact_mcp.py --rlutter rlutter/target/debug/rlutter \
    --flutter-root /path/to/flutter-sdk --project /path/to/generated/app \
    --artifacts /path/to/new/evidence-directory

The form flow uses --project. A second isolated copy uses compact_fixture for
long-list and custom-paint coverage. Optionally supply --coverage-project as its
generated-app template; neither original generated app is modified.

Both modes use the same current server and advertised schema, not a historical
baseline binary. Wire counts include JSON-RPC envelopes, escaping and newlines,
initialization, tool discovery, negative controls, pagination and cleanup. PNG
base64 is counted on the wire but replaced with a hash/path in transcripts.
No model token counts or token savings are inferred from character counts.
"""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import struct
import subprocess
import time
import traceback
import zlib


def require(condition, message):
  if not condition:
    raise AssertionError(message)


def encoded(value):
  return (json.dumps(value, ensure_ascii=False, separators=(',', ':')) + '\n').encode('utf-8')


def text(result):
  return '\n'.join(item['text'] for item in result['content'] if item['type'] == 'text')


def payload(result):
  return json.loads(text(result))


class Client:

  def __init__(self, args, directory):
    self.directory = directory
    self.sequence = 0
    self.session = None
    self.buffer = b''
    self.events = []
    self.category = 'discovery'
    self.log = (directory / 'server.log').open('w')
    command = [str(args.rlutter.resolve()), '--flutter-root', str(args.flutter_root.resolve())]
    if args.local_engine_src:
      command += ['--local-engine-src', str(args.local_engine_src.resolve())]
    command += ['mcp', '--stdio']
    self.command = command
    self.timeout = args.timeout
    self.process = subprocess.Popen(
        command,
        cwd=directory,
        env=dict(os.environ, RUSTUP_TOOLCHAIN='1.93.1'),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=self.log,
        start_new_session=True,
        bufsize=0,
    )
    self.selector = selectors.DefaultSelector()
    self.selector.register(self.process.stdout, selectors.EVENT_READ)

  def retain(self, message):
    # Work on a deep copy so image validation still sees the original bytes.
    copy = json.loads(json.dumps(message))
    for index, item in enumerate(copy.get('result', {}).get('content', [])):
      if item.get('type') == 'image':
        data = base64.b64decode(item['data'], validate=True)
        path = self.directory / f'image-{self.sequence}-{index}.png'
        path.write_bytes(data)
        item['data'] = {
            'omitted_base64': True,
            'base64_chars': len(item['data']),
            'decoded_bytes': len(data),
            'sha256': hashlib.sha256(data).hexdigest(),
            'path': str(path),
        }
    return copy

  def record(self, direction, raw, message, elapsed_ms=None):
    event = {
        'direction': direction,
        'category': self.category,
        'wire_bytes': len(raw),
        'wire_chars': len(raw.decode('utf-8')),
        'message': self.retain(message),
    }
    if elapsed_ms is not None:
      event['latency_ms'] = elapsed_ms
    if direction == 'response':
      event['image_count'] = sum(
          c.get('type') == 'image' for c in message.get('result', {}).get('content', [])
      )
    self.events.append(event)
    # Persist each exchange, including failures, before any assertion can fail.
    with (self.directory / 'transcript.jsonl').open('a') as stream:
      stream.write(json.dumps(event, ensure_ascii=False) + '\n')

  def send(self, message):
    raw = encoded(message)
    self.process.stdin.write(raw)
    self.process.stdin.flush()
    self.record('request', raw, message)

  def line(self, deadline):
    while b'\n' not in self.buffer:
      remaining = deadline - time.monotonic()
      if remaining <= 0 or not self.selector.select(remaining):
        raise TimeoutError(
            f'MCP response deadline ({self.timeout}s), see {self.directory}/server.log'
        )
      chunk = os.read(self.process.stdout.fileno(), 65536)
      if not chunk:
        raise RuntimeError(f'MCP EOF, exit={self.process.poll()}, see {self.directory}/server.log')
      self.buffer += chunk
    line, self.buffer = self.buffer.split(b'\n', 1)
    return line + b'\n'

  def rpc(self, method, params):
    self.sequence += 1
    request = {'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': params}
    start = time.monotonic()
    self.send(request)
    deadline = start + self.timeout
    while True:
      raw = self.line(deadline)
      response = json.loads(raw)
      elapsed = (time.monotonic() - start) * 1000
      self.record('response', raw, response, elapsed if 'id' in response else None)
      require(response.get('jsonrpc') == '2.0', response)
      if 'id' not in response:
        continue
      require(response['id'] == self.sequence, response)
      require('error' not in response, response)
      return response['result']

  def tool(self, name, expected_error=False, **params):
    if name != 'session_start':
      params['session_id'] = self.session
    params.setdefault('timeoutMs', 120000 if name == 'session_start' else 10000)
    result = self.rpc('tools/call', {'name': 'rlutter_' + name, 'arguments': params})
    require(bool(result.get('isError')) == expected_error, result)
    return result

  def close(self):
    try:
      if self.process.poll() is None:
        self.process.stdin.close()
        try:
          code = self.process.wait(timeout=30)
          require(code == 0, f'MCP exit: {code}')
        except subprocess.TimeoutExpired:
          raise TimeoutError('MCP did not exit within 30s of EOF')
    finally:
      # Include descendants if startup/build stalls; never leave a runner behind.
      try:
        os.killpg(self.process.pid, signal.SIGTERM)
      except ProcessLookupError:
        pass
      if self.process.poll() is None:
        try:
          self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
          os.killpg(self.process.pid, signal.SIGKILL)
          self.process.wait(timeout=5)
      self.selector.close()
      self.process.stdout.close()
      self.log.close()

  def metrics(self):

    def summarize(events):
      return {
          'request_bytes': sum(e['wire_bytes'] for e in events if e['direction'] == 'request'),
          'request_chars': sum(e['wire_chars'] for e in events if e['direction'] == 'request'),
          'response_bytes': sum(e['wire_bytes'] for e in events if e['direction'] == 'response'),
          'response_chars': sum(e['wire_chars'] for e in events if e['direction'] == 'response'),
          'requests': sum(e['direction'] == 'request' for e in events),
          'responses': sum(e['direction'] == 'response' for e in events),
          'latency_ms': sum(e.get('latency_ms', 0) for e in events),
          'images': sum(e.get('image_count', 0) for e in events),
      }

    return {
        'total': summarize(self.events),
        'by_category': {
            category: summarize([e for e in self.events if e['category'] == category
                                ]) for category in sorted({e['category'] for e in self.events})
        },
    }


def png_pixels(png):
  """Decode the non-interlaced 8-bit RGB(A) PNGs emitted by the native capture.

  Compare actual pixel samples, not compressed bytes, ancillary chunks or frame
  metadata. Keep this dependency-free; reject formats we cannot verify honestly.
  """
  require(png.startswith(b'\x89PNG\r\n\x1a\n'), 'Invalid PNG')
  position = 8
  compressed = bytearray()
  header = None
  while position < len(png):
    length = struct.unpack_from('>I', png, position)[0]
    kind = png[position + 4:position + 8]
    data = png[position + 8:position + 8 + length]
    crc = struct.unpack_from('>I', png, position + 8 + length)[0]
    require(zlib.crc32(kind + data) == crc, 'PNG chunk CRC mismatch')
    if kind == b'IHDR':
      header = struct.unpack('>IIBBBBB', data)
    elif kind == b'IDAT':
      compressed.extend(data)
    position += length + 12
    if kind == b'IEND':
      break
  require(header is not None, 'PNG has no IHDR')
  width, height, depth, color, compression, filtering, interlace = header
  require(
      depth == 8 and color in (2, 6) and compression == filtering == interlace == 0,
      f'Unsupported PNG format: {header}'
  )
  channels = 4 if color == 6 else 3
  stride = width * channels
  raw = zlib.decompress(compressed)
  require(len(raw) == height * (stride + 1), 'PNG scanline length mismatch')
  previous = bytearray(stride)
  pixels = bytearray()
  for y in range(height):
    offset = y * (stride + 1)
    mode = raw[offset]
    row = bytearray(raw[offset + 1:offset + 1 + stride])
    require(mode <= 4, f'Unknown PNG filter: {mode}')
    for x in range(stride):
      left = row[x - channels] if x >= channels else 0
      above = previous[x]
      upper_left = previous[x - channels] if x >= channels else 0
      if mode == 1:
        predictor = left
      elif mode == 2:
        predictor = above
      elif mode == 3:
        predictor = (left + above) // 2
      elif mode == 4:
        estimate = left + above - upper_left
        distances = [abs(estimate - n) for n in (left, above, upper_left)]
        predictor = (left, above, upper_left)[distances.index(min(distances))]
      else:
        predictor = 0
      row[x] = (row[x] + predictor) & 255
    pixels.extend(row)
    previous = row
  return width, height, channels, bytes(pixels)


def screenshot(client, view, compact):
  capture = client.tool('ui_screenshot' if compact else 'capture_screenshot', viewId=view)
  require([c['type'] for c in capture['content']] == ['text', 'image'], capture.keys())
  image = capture['content'][1]
  require(image['mimeType'] == 'image/png', image['mimeType'])
  png = base64.b64decode(image['data'], validate=True)
  require(png.startswith(b'\x89PNG\r\n\x1a\n'), 'Invalid PNG')
  metadata = json.loads(capture['content'][0]['text'])
  require(metadata['viewId'] == view, metadata)
  require(metadata['uiFrameNumber'] > metadata['uiFrameBoundary'], metadata)
  return metadata, png


def coverage_flow(client, result, compact, window, wait, observe, target):
  view = window('Compact list')
  wait(view, 'scroll_count', value='0')
  nodes = observe(view)
  require(
      not any(n.get('identifier') == 'item_30' for n in nodes),
      'Distant row must not initially be in the observed semantics'
  )
  client.tool('verify_assert', viewId=view, selector={'identifier': 'item_30'}, count=0)
  for step in range(1, 13):
    scroll_target = target(nodes, 'long_list', 'scrollDown')
    client.tool('semantics_perform', viewId=view, action='scrollDown', **scroll_target)
    wait(view, 'scroll_count', value=str(step))
    nodes = observe(view)
    if any(n.get('identifier') == 'item_30' for n in nodes):
      break
  else:
    raise AssertionError('Offscreen item_30 never appeared after 12 semantic scrolls')
  wait(view, 'item_30', count=1)
  # A scroll action must remain discoverable after viewport/revision changes.
  target(nodes, 'long_list', 'scrollDown')
  result['scroll_actions'] = step
  result['checks'].append(
      'scrollDown remains actionable; initially absent row 30 becomes observable'
  )

  if compact:
    client.category = 'pagination'
    observe(view, max_chars=650, category='pagination')
    require(len(result['pagination'][-1]) > 1, 'Long-list pagination was not exercised')
    result['checks'].append(
        'long-list pagination: same epoch/revision/generation; no gaps or duplicates'
    )
  client.category = 'task'
  paint = window('Compact paint')
  wait(paint, 'paint_color', value='blue')
  nodes = observe(paint)
  require('PAINT_ONLY_GLYPH' not in json.dumps(nodes), 'Custom paint leaked into semantics')
  client.tool('verify_assert', viewId=paint, selector={'label': 'PAINT_ONLY_GLYPH'}, count=0)
  client.category = 'screenshots'
  before_meta, before_png = screenshot(client, paint, compact)
  # Negative visual control: a fresh capture without a mutation has equal pixels.
  unchanged_meta, unchanged_png = screenshot(client, paint, compact)
  before = png_pixels(before_png)
  require(before == png_pixels(unchanged_png), 'Unchanged canvas unexpectedly changed pixels')
  client.category = 'task'
  client.tool(
      'semantics_perform', viewId=paint, action='tap', **target(nodes, 'change_paint', 'tap')
  )
  wait(paint, 'paint_color', value='orange')
  nodes = observe(paint)
  require('PAINT_ONLY_GLYPH' not in json.dumps(nodes), 'Paint became semantically discoverable')
  client.category = 'screenshots'
  after_meta, after_png = screenshot(client, paint, compact)
  after = png_pixels(after_png)
  require(before[:3] == after[:3], 'Paint capture dimensions changed')
  width, height, channels, before_pixels = before
  after_pixels = after[3]
  # The lower half is canvas, not the semantic status/button above it.
  start = width * (height // 2) * channels
  changed = sum(
      before_pixels[i:i + channels] != after_pixels[i:i + channels]
      for i in range(start, len(before_pixels), channels)
  )
  require(changed > 1000, f'Expected canvas pixels to change, got {changed}')
  result['paint'] = {
      'before_metadata': before_meta,
      'unchanged_metadata': unchanged_meta,
      'after_metadata': after_meta,
      'width': width,
      'height': height,
      'lower_half_changed_pixels': changed,
      'before_pixel_sha256': hashlib.sha256(before_pixels).hexdigest(),
      'after_pixel_sha256': hashlib.sha256(after_pixels).hexdigest(),
      'unchanged_control_equal_pixels': True,
      'decoder': 'stdlib PNG CRC/zlib/filter reconstruction, 8-bit RGB(A)',
  }
  result['checks'].append(
      'paint-only glyph absent from semantics; explicit PNGs show changed canvas pixels'
  )
  client.category = 'task'


def exercise(args, project, directory, compact, coverage=False):
  client = Client(args, directory)
  result = {
      'status': 'failed',
      'mode': 'compact' if compact else 'baseline',
      'scope': 'list_and_paint' if coverage else 'form',
      'checks': [],
      'pagination': [],
  }
  start = time.monotonic()
  try:
    client.rpc(
        'initialize', {
            'protocolVersion': '2025-11-25',
            'capabilities': {},
            'clientInfo': {'name': 'compact-mcp-integration', 'version': '1'},
        }
    )
    client.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
    tools = client.rpc('tools/list', {})['tools']
    require({'rlutter_ui_snapshot', 'rlutter_ui_screenshot'} <= {t['name'] for t in tools}, tools)
    result['schema'] = {
        'tool_count': len(tools),
        'canonical_tools_json_bytes': len(encoded(tools)) - 1,
        'canonical_tools_json_chars': len(encoded(tools).decode('utf-8')) - 1,
        'note': 'Canonical schema size is descriptive; full tools/list wire cost is in discovery.',
    }
    client.category = 'startup'
    client.session = payload(client.tool('session_start', project_dir=str(project)))['session_id']
    client.category = 'task'

    def wait(view, identifier, **conditions):
      return client.tool(
          'verify_wait', viewId=view, selector={'identifier': identifier}, **conditions
      )

    def window(title):
      client.tool('verify_wait', kind='windows', where={'title': title, 'ready': True}, count=1)
      views = payload(client.tool('windows_list'))['views']
      return next(v['viewId'] for v in views if v['title'] == title)

    def observe(view, max_chars=4000, category='observations'):
      previous_category = client.category
      client.category = category
      try:
        if not compact:
          response = client.tool('semantics_snapshot', viewId=view)
          require(
              all(c['type'] == 'text' for c in response['content']), 'Implicit observation image'
          )
          return payload(response)['nodes']
        offset = 0
        identity = None
        nodes = []
        refs = set()
        pages = []
        for _ in range(100):
          params = {'viewId': view, 'max_chars': max_chars, 'offset': offset}
          if identity is not None:
            params['generation'] = identity[2]
          response = client.tool('ui_snapshot', **params)
          require(
              all(c['type'] == 'text' for c in response['content']), 'Implicit observation image'
          )
          value = text(response)
          require(0 < len(value) <= max_chars, 'Compact character budget exceeded')
          records = [json.loads(line) for line in value.splitlines()]
          meta = records[-1]['meta']
          current = (meta['epoch'], meta['revision'], meta['generation'], meta['total'])
          require(identity is None or identity == current, 'Pagination identity changed')
          identity = current
          require(meta['offset'] == offset, meta)
          page_refs = {n['ref'] for n in records[:-1]}
          require(len(page_refs) == meta['returned'] and not refs.intersection(page_refs), meta)
          refs.update(page_refs)
          nodes.extend(records[:-1])
          pages.append(meta)
          if meta['nextOffset'] is None:
            require(len(refs) == meta['total'], 'Pagination lost records')
            result['pagination'].append(pages)
            return nodes
          require(
              meta['nextOffset'] == offset + meta['returned'] > offset, 'Pagination gap or stall'
          )
          offset = meta['nextOffset']
        raise AssertionError('Pagination exceeded 100 bounded pages')
      finally:
        client.category = previous_category

    def target(nodes, identifier, action=None):
      matches = [n for n in nodes if n.get('identifier') == identifier]
      require(len(matches) == 1, f'Expected unique {identifier}: {matches}')
      node = matches[0]
      if action is not None:
        # Identifier wrappers have pointer bounds but the button child owns tap.
        if compact:
          descendants = [node]
          for child in nodes[nodes.index(node) + 1:]:
            if child['depth'] <= node['depth']:
              break
            descendants.append(child)
          candidates = [n for n in descendants if action in n.get('actions', [])]
        else:
          by_id = {n['id']: n for n in nodes}
          pending = [node]
          candidates = []
          while pending:
            child = pending.pop()
            if child.get('actions', 0) & {'tap': 1, 'scrollDown': 32}[action]:
              candidates.append(child)
            pending.extend(by_id[i] for i in child.get('children', []) if i in by_id)
        require(
            len(candidates) == 1,
            f'Expected one actionable descendant of {identifier}: {candidates}'
        )
        node = candidates[0]
      field = 'ref' if compact else 'handle'
      require(field in node, node)
      return {field: node[field]}

    def act(view, nodes, identifier, method='input_tap', **extra):
      return client.tool(
          method, viewId=view, **target(nodes, identifier, extra.get('action')), **extra
      )

    if coverage:
      coverage_flow(client, result, compact, window, wait, observe, target)
    else:
      view = window('Agent form')
      wait(view, 'increment', count=1)
      nodes = observe(view)
      old_increment = target(nodes, 'increment', 'tap')
      act(view, nodes, 'increment', method='semantics_perform', action='tap')
      wait(view, 'click_count', value='1')
      client.category = 'negative_controls'
      stale = client.tool(
          'semantics_perform', expected_error=True, viewId=view, action='tap', **old_increment
      )
      require('stale' in text(stale).lower(), f'Expected stale action error: {stale}')
      wait(view, 'click_count', value='1', stableMs=150)
      result['checks'].append('stale action rejected after increment; counter remains 1')
      client.category = 'task'
      nodes = observe(view)
      act(view, nodes, 'name')
      wait(view, 'name', focused=True)
      unicode_text = 'Zoë שלום 東京 🦄'
      client.tool('input_replaceText', viewId=view, text=unicode_text)
      wait(view, 'name', value=unicode_text)
      nodes = observe(view)
      act(view, nodes, 'open_dialog')
      dialog = window('Save dialog')
      require(dialog != view, 'Dialog must have a different real view ID')
      wait(dialog, 'dialog_value', value=unicode_text)
      nodes = observe(dialog)
      act(dialog, nodes, 'confirm', method='semantics_perform', action='tap')
      wait(view, 'saved_value', value=unicode_text)
      client.tool('verify_wait', kind='windows', where={'title': 'Save dialog'}, count=0)
      result['checks'].append('Unicode form -> separate dialog view -> confirm -> saved value')
      observe(view)

      if compact:
        observe(view, max_chars=650, category='pagination')
        require(len(result['pagination'][-1]) > 1, 'Pagination was not exercised')
        result['checks'].append('bounded pagination covers every visible record exactly once')

      client.category = 'screenshots'
      metadata, _ = screenshot(client, view, compact)
      result['screenshot_metadata'] = metadata
      result['checks'].append('explicit screenshot: metadata before valid PNG; fresh UI frame')
    client.category = 'health'
    client.tool('session_health')
    client.category = 'shutdown'
    client.tool('session_shutdown')
    client.session = None
    result['status'] = 'passed'
  except Exception:
    result['failure'] = traceback.format_exc()
  finally:
    try:
      client.close()
    except Exception:
      result['status'] = 'failed'
      result['cleanup_failure'] = traceback.format_exc()
    result['wall_ms'] = (time.monotonic() - start) * 1000
    result['metrics'] = client.metrics()
    result['command'] = client.command
    result['automatic_mutation_retries'] = 0
    result['transcript'] = str(directory / 'transcript.jsonl')
    (directory / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
  return result


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for flag in ('rlutter', 'flutter-root', 'project', 'artifacts'):
    parser.add_argument('--' + flag, type=Path, required=True)
  parser.add_argument(
      '--coverage-project',
      type=Path,
      help='Optional generated app template for the isolated list/paint fixture'
  )
  parser.add_argument('--local-engine-src', type=Path)
  parser.add_argument('--timeout', type=float, default=300, help='Per-RPC deadline in seconds')
  args = parser.parse_args()
  require(args.timeout > 0, '--timeout must be positive')
  output = args.artifacts.resolve()
  output.mkdir(parents=True, exist_ok=False)
  report = {
      'status': 'failed',
      'runs': [],
      'methodology': {
          'baseline':
              'Full semantics_snapshot with native handles, current server',
          'compact':
              'ui_snapshot with generation-scoped refs, current server',
          'wire_units':
              'Actual UTF-8 bytes and Unicode characters, including JSON-RPC and LF',
          'schema': (
              'Same-server comparison, NOT original-server baseline: complete current tools/list '
              'charged to every run. No legacy subtraction: exact historical descriptions and '
              'ref-related schema changes were not reconstructed. A historical baseline would '
              'have smaller discovery cost, so these savings must not be attributed to that baseline.'
          ),
          'tokens':
              'Unavailable: no model tokenizer used; bytes/chars are not model tokens',
          'latency':
              'Serial wall-clock RPC latency; startup includes build, not model inference',
          'ordering':
              'Baseline then compact on the same isolated copy; cache warming can bias latency',
          'retries':
              'No automatic retries; every request including negative controls is charged',
          'coverage':
              'Separate form and 160-row lazy list/custom-paint app flows in both modes',
          'coverage_limits':
              'One deterministic fixture per scope, not arbitrary app coverage',
          'scope_comparison': (
              'Matched task flows per scope; compact additionally runs a 650-character pagination '
              'check. Totals charge that extra work and all schema/image traffic. Observation totals '
              'include every continuation needed to obtain a complete compact observation.'
          ),
      },
  }
  try:
    for scope in ('form', 'list_and_paint'):
      # Reflink where supported; never hardlink writable generated app contents.
      template = args.project if scope == 'form' else (args.coverage_project or args.project)
      project = output / (scope + '-project')
      subprocess.run(['cp', '-a', '--reflink=auto',
                      str(template.resolve()),
                      str(project)],
                     check=True,
                     timeout=120)
      if scope == 'list_and_paint':
        fixture = Path(__file__).resolve().parent / 'compact_fixture/lib/main.dart'
        (project / 'lib/main.dart').write_bytes(fixture.read_bytes())
        report['coverage_fixture_sha256'] = hashlib.sha256(fixture.read_bytes()).hexdigest()
      for mode in ('baseline', 'compact'):
        directory = output / (scope + '-' + mode)
        directory.mkdir()
        report['runs'].append(
            exercise(
                args, project, directory, mode == 'compact', coverage=scope == 'list_and_paint'
            )
        )
    if all(run['status'] == 'passed' for run in report['runs']):
      report['status'] = 'passed'
      report['comparison'] = {}
      for scope in ('form', 'list_and_paint', 'combined'):
        report['comparison'][scope] = {}
        for category in ('total', 'observations'):
          totals = {}
          for mode in ('baseline', 'compact'):
            runs = [
                run for run in report['runs']
                if run['mode'] == mode and (scope == 'combined' or run['scope'] == scope)
            ]
            measurements = [
                run['metrics']['total']
                if category == 'total' else run['metrics']['by_category'][category] for run in runs
            ]
            totals[mode] = {key: sum(m[key] for m in measurements) for key in measurements[0]}
          before, after = totals['baseline'], totals['compact']
          report['comparison'][scope][category] = {
              'baseline':
                  before,
              'compact':
                  after,
              'response_byte_reduction_percent':
                  100 * (1 - after['response_bytes'] / before['response_bytes']),
              'baseline_request_plus_response_bytes':
                  before['request_bytes'] + before['response_bytes'],
              'compact_request_plus_response_bytes':
                  after['request_bytes'] + after['response_bytes'],
          }
  except Exception:
    report['failure'] = traceback.format_exc()
  finally:
    destination = output / 'report.json'
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(f'{report["status"].upper()}: {destination}')
    for run in report['runs']:
      if run['status'] != 'passed':
        print(run.get('failure', run.get('cleanup_failure', 'Unknown failure')))
    if 'failure' in report:
      print(report['failure'])
  return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
  raise SystemExit(main())
