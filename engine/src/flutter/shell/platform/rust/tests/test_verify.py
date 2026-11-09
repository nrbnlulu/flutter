#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Run a generated agent fixture with passing, regression and error controls."""
import argparse
import json
import os
from pathlib import Path
import subprocess


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  for flag in ('rlutter', 'flutter-root', 'project', 'artifacts'):
    parser.add_argument('--' + flag, type=Path, required=True)
  parser.add_argument('--local-engine-src', type=Path)
  args = parser.parse_args()
  output = args.artifacts.resolve()
  output.mkdir(parents=True, exist_ok=False)
  shell = Path(__file__).resolve().parents[1]
  scenario = shell / 'tests/agent_fixture/test/agent/save_dialog.json'
  error_scenario = output / 'caught-error.json'
  error_scenario.write_text(
      json.dumps({
          'version':
              1, 'name':
                  'Caught framework error must fail', 'steps': [
                      {
                          'name': 'Wait for form', 'method': 'verify.wait',
                          'params': {'viewId': 1, 'selector': {'identifier': 'throw_error'}}
                      },
                      {
                          'name': 'Trigger framework-caught error', 'method': 'input.tap',
                          'params': {'viewId': 1, 'selector': {'identifier': 'throw_error'}}
                      },
                      {
                          'name': 'Error cannot be swallowed', 'method': 'capture.screenshot',
                          'params': {'viewId': 1}
                      },
                  ]
      })
  )
  base = [str(args.rlutter.resolve()), '--flutter-root', str(args.flutter_root.resolve())]
  if args.local_engine_src:
    base += ['--local-engine-src', str(args.local_engine_src.resolve())]
  env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
  env['RUSTUP_TOOLCHAIN'] = '1.93.1'
  results = {}
  for name, defines, selected in [('positive', [], scenario),
                                  ('regression', ['--dart-define=AGENT_REGRESSION=true'], scenario),
                                  ('startup-error', ['--dart-define=AGENT_STARTUP_ERROR=true'],
                                   scenario), ('caught-error', [], error_scenario),
                                     ('replaced-hook', ['--dart-define=AGENT_REPLACE_ERROR_HOOK=true'], scenario)]:
    report_path = output / name / 'result.json'
    with (output / f'{name}.log').open('w') as log:
      process = subprocess.run(
          base + [
              'verify',
              str(selected), '--headless', '--pixel-ratio=1.25', '--report',
              str(report_path)
          ] + defines,
          cwd=args.project.resolve(),
          env=env,
          stdout=log,
          stderr=log,
          timeout=300
      )
    report = json.loads(report_path.read_text())
    assert (process.returncode == 0) == (name == 'positive'), (name, report)
    assert (report['status'] == 'passed') == (name == 'positive'), (name, report)
    if name == 'positive':
      assert report['session']['capabilities']['frameworkErrors']
      assert all(step['passed'] for step in report['steps'])
    if name == 'regression':
      assert report['status'] == 'assertion_failed'
      assert report['steps'][-1]['name'] == 'Observe persisted value'
      assert list(report_path.with_suffix('.artifacts').glob('*.png'))
      assert report['build']['staged']['sha256'] != results['positive']['build']['staged']['sha256']
    if name.endswith('error') or name == 'replaced-hook':
      assert 'app_exception' in json.dumps(report
                                          ) or 'control channel' in json.dumps(report), report
    results[name] = report
  unavailable = output / 'adapter-unavailable' / 'result.json'
  with (output / 'adapter-unavailable.log').open('w') as log:
    bad_driver = str(output / 'missing-vulkan-driver.json')
    process = subprocess.run(
        base + ['verify', str(scenario), '--headless', '--report', str(unavailable)],
        cwd=args.project.resolve(),
        env=env | {'VK_DRIVER_FILES': bad_driver, 'VK_ICD_FILENAMES': bad_driver},
        stdout=log, stderr=log, timeout=300)
  report = json.loads(unavailable.read_text())
  assert process.returncode != 0 and report['status'] == 'infrastructure_failed', report
  assert 'Vulkan' in (output / 'adapter-unavailable.log').read_text()
  results['adapter-unavailable'] = report
  (output / 'report.json').write_text(
      json.dumps({name: value['status'] for name, value in results.items()}, indent=2) + '\n'
  )
  print(f"PASS: {output / 'report.json'}")


if __name__ == '__main__':
  main()
