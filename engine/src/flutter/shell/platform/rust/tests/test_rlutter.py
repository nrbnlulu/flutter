# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Create/run a local-engine rlutter app and prove reload/restart through its VM."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlencode
from urllib.request import urlopen

from native_desktop import desktop
from test_texture import DIAGNOSTIC, wait_until

SOURCE = """import 'package:flutter/widgets.dart';
int probeState = 0;
String lastRenderedLabel = '';
String get probeLabel => 'rlutter-before';
void main() => runApp(const Probe());
class Probe extends StatefulWidget {
  const Probe({super.key});
  @override
  State<Probe> createState() => _ProbeState();
}
class _ProbeState extends State<Probe> {
  @override
  Widget build(BuildContext context) {
    lastRenderedLabel = probeLabel;
    return Directionality(textDirection: TextDirection.ltr,
      child: GestureDetector(behavior: HitTestBehavior.opaque,
        onTap: () => setState(() => probeState++),
        child: ColoredBox(color: const Color(0xff204060),
          child: Center(child: Text('$probeLabel state=$probeState',
            style: const TextStyle(fontSize: 28, color: Color(0xffffffff)))))));
  }
}
"""


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--cli', type=Path, required=True)
  parser.add_argument('--flutter-root', type=Path, required=True)
  parser.add_argument('--project', type=Path, help='Reuse a disposable generated project')
  args = parser.parse_args()
  evidence = Path(tempfile.mkdtemp(prefix='flutter-rust-rlutter-'))
  print(f'rlutter runtime evidence: {evidence}', flush=True)
  cli = str(args.cli.resolve())
  root_arg = f'--flutter-root={args.flutter_root.resolve()}'
  project = args.project or evidence / 'project'
  if args.project is None:
    subprocess.run([cli, 'create',
                    str(project), '--project-name=rlutter_runtime_smoke', root_arg],
                   check=True)
  project = project.resolve()
  source = project / 'lib/rlutter_probe.dart'
  source.write_text(SOURCE, encoding='utf-8')
  service = evidence / 'service.json'
  api = desktop()
  environment = os.environ | {
      'VK_INSTANCE_LAYERS': 'VK_LAYER_KHRONOS_validation', 'WGPU_VALIDATION': '1', 'WGPU_DEBUG': '1'
  }
  log_path = evidence / 'run.log'
  app_pid = None
  with log_path.open('w', encoding='utf-8') as log:
    process = subprocess.Popen([
        cli, 'run', '-d', 'windows' if sys.platform == 'win32' else 'linux', '-t',
        'lib/rlutter_probe.dart', root_arg, f'--vm-service-info-file={service}'
    ],
                               cwd=project,
                               env=environment,
                               stdin=subprocess.PIPE,
                               stdout=log,
                               stderr=subprocess.STDOUT,
                               text=True,
                               start_new_session=sys.platform != 'win32')
    try:
      if not wait_until(service.is_file, process, 600):
        raise RuntimeError('rlutter attach did not publish its debugger URI')
      uri = json.loads(service.read_text())['uri']

      def rpc(method, **params):
        with urlopen(uri + method + '?' + urlencode(params), timeout=10) as response:
          result = json.load(response)
        if 'error' in result:
          raise RuntimeError(result['error'])
        return result['result']

      def isolate():
        vm = rpc('getVM')
        return next(i['id'] for i in vm['isolates'] if i['name'] == 'main')

      app_pid = rpc('getVM')['pid']

      def read_field(name):
        isolate_id = isolate()
        library_id = rpc('getIsolate', isolateId=isolate_id)['rootLib']['id']
        library = rpc('getObject', isolateId=isolate_id, objectId=library_id)
        field = next(f for f in library['variables'] if f['name'] == name)
        value = rpc('getObject', isolateId=isolate_id, objectId=field['id'])['staticValue']
        return value.get('valueAsString')

      def find_window():
        return next((c for c in api.clients() if c['pid'] == app_pid), None)

      client = wait_until(find_window, process)
      if not client or read_field('lastRenderedLabel') != 'rlutter-before':
        raise RuntimeError('Generated application did not initialize')
      address = client['address']
      api.action('focus', address)
      api.capture_window(address, evidence / 'before.png')
      # Compare the centered widget, excluding desktop corners and client
      # edges that may extend beyond the screen on small desktops.
      width, height = client['size']
      content_region = (width // 4, height // 3, width // 2, height // 3)
      api.capture_window(address, evidence / 'before-content.png', content_region)
      api.click(
          address, client['at'][0] + client['size'][0] // 2,
          client['at'][1] + client['size'][1] // 2
      )
      if not wait_until(lambda: read_field('probeState') == '1', process):
        raise RuntimeError('Native click did not change application state')
      source.write_text(SOURCE.replace('rlutter-before', 'rlutter-after'), encoding='utf-8')
      process.stdin.write('r\n')
      process.stdin.flush()
      if not wait_until(lambda: read_field('lastRenderedLabel') == 'rlutter-after', process, 60):
        raise RuntimeError('Hot reload did not apply the changed getter')
      if read_field('probeState') != '1':
        raise RuntimeError('Hot reload lost application state')
      time.sleep(.3)
      api.capture_window(address, evidence / 'reloaded.png')
      if (evidence / 'before.png').read_bytes() == (evidence / 'reloaded.png').read_bytes():
        raise RuntimeError('Hot reload changed the VM but not the rendered frame')
      process.stdin.write('R\n')
      process.stdin.flush()
      if not wait_until(
          lambda: 'Restarted application' in log_path.read_text(encoding='utf-8', errors='replace'),
          process, 60):
        raise RuntimeError('Flutter attach did not complete hot restart')
      if not wait_until(lambda: read_field('probeState') == '0' and read_field('lastRenderedLabel')
                        == 'rlutter-after', process):
        raise RuntimeError('Hot restart did not reset state using the changed source')
      (evidence / 'observations.json').write_text(
          json.dumps({'reload_state': 1, 'restart_state': 0, 'rendered_label': 'rlutter-after'})
      )
      api.capture_window(address, evidence / 'restarted.png')
      process.stdin.write('q\n')
      process.stdin.flush()
      if process.wait(timeout=30) != 0:
        raise RuntimeError('rlutter run did not exit cleanly after detach')
      if any(c['pid'] == app_pid for c in api.clients()):
        raise RuntimeError('rlutter left its native application running')
    finally:
      if process.poll() is None:
        if app_pid is not None:
          for client in api.clients():
            if client['pid'] == app_pid:
              api.action('close', client['address'])
        if sys.platform == 'win32':
          subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                         check=False,
                         stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
        else:
          os.killpg(process.pid, signal.SIGTERM)
        process.wait()
  output = log_path.read_text(encoding='utf-8', errors='replace')
  if DIAGNOSTIC.search(output):
    raise RuntimeError('GPU/runtime diagnostics during rlutter runtime validation')
  if not re.search(r'Reloaded \d+ of \d+ libraries', output):
    raise RuntimeError('Flutter attach did not confirm hot reload')
  # A standalone bundle must find its data/DLLs from an unrelated directory.
  name = re.search(r'^name:\s*(\w+)', (project / 'pubspec.yaml').read_text(), re.MULTILINE)[1]
  executable = name + ('.exe' if sys.platform == 'win32' else '')
  built = project / 'runner-rs/target/debug'
  relocated = evidence / 'relocated'
  relocated.mkdir()
  shutil.copy2(built / executable, relocated / executable)
  shutil.copytree(built / 'data', relocated / 'data')
  if sys.platform == 'win32':
    for library in built.glob('*.dll'):
      shutil.copy2(library, relocated / library.name)
  else:
    shutil.copytree(built / 'libs', relocated / 'libs')
  standalone_log = evidence / 'standalone.log'
  with standalone_log.open('w', encoding='utf-8') as log:
    app = subprocess.Popen([str(relocated / executable)],
                           cwd=evidence,
                           env=environment,
                           stdout=log,
                           stderr=subprocess.STDOUT)
    try:
      client = wait_until(
          lambda: next((c for c in api.clients() if c['pid'] == app.pid), None), app, 30
      )
      if not client:
        raise RuntimeError('Relocated bundle did not map a window')

      def same_frame():
        api.capture_window(client['address'], evidence / 'standalone.png')
        api.capture_window(client['address'], evidence / 'standalone-content.png', content_region)
        return ((evidence /
                 'standalone-content.png').read_bytes() == (evidence /
                                                            'before-content.png').read_bytes())

      if not wait_until(same_frame, app):
        raise RuntimeError('Relocated bundle did not render the initial application frame')
      api.action('close', client['address'])
      if app.wait(timeout=15) != 0:
        raise RuntimeError('Relocated bundle did not close with status zero')
    finally:
      if app.poll() is None:
        app.kill()
        app.wait()
  if DIAGNOSTIC.search(standalone_log.read_text(encoding='utf-8', errors='replace')):
    raise RuntimeError('GPU/runtime diagnostics in the relocated bundle')
  print('rlutter reload/state preservation, restart, detach and relocated-bundle close passed.')


if __name__ == '__main__':
  main()
