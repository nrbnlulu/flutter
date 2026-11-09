# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Portable backing commands for Taskfile.yml (no shell snippets required)."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
SHELL = ROOT / 'engine/src/flutter/shell/platform/rust'
TESTS = SHELL / 'tests'
sys.path.insert(0, str(TESTS))
from native_desktop import desktop
from test_texture import DIAGNOSTIC, wait_until


class Host:

  def __init__(self, args):
    self.args = args
    self.windows = sys.platform == 'win32'
    self.src = ROOT / 'engine/src'
    self.out = self.src / 'out' / args.engine
    self.env = os.environ.copy()
    paths = [self.src / 'flutter/third_party/depot_tools']
    # depot_tools and engine build helpers still require modules removed from
    # newer system Pythons (for example, pipes in Python 3.13). Use the
    # checkout's DEPS-pinned interpreter for their python3 subprocesses.
    python_bin = self.src / 'flutter/third_party/cpython3/bin'
    if not self.windows and (python_bin / 'python3').is_file():
      paths.insert(0, python_bin)
    for key in ('DEPOT_PY_BIN', 'DEPOT_TOOLS'):
      if self.env.get(key):
        paths.insert(0, Path(self.env[key]))
    self.env['PATH'] = os.pathsep.join(map(str, paths)) + os.pathsep + self.env['PATH']
    if self.windows:
      self.env.setdefault('DEPOT_TOOLS_WIN_TOOLCHAIN', '0')
      self.env.setdefault('GYP_MSVS_VERSION', '2022')
      build_tools = Path(
          os.environ.get('ProgramFiles(x86)', 'C:/Program Files (x86)')
      ) / 'Microsoft Visual Studio/2022/BuildTools'
      if build_tools.exists():
        self.env.setdefault('GYP_MSVS_OVERRIDE_PATH', str(build_tools))
    if self.windows:
      # Dart's SDK-generation scripts invoke python3 via cmd.exe. Avoid the
      # Microsoft Store alias without modifying the user's system PATH.
      shims = self.src / 'out/rust-host-tools'
      shims.mkdir(parents=True, exist_ok=True)
      shim = shims / 'python3.cmd'
      content = f'@echo off\n"{sys.executable}" %*\n'
      if not shim.exists() or shim.read_text() != content:
        shim.write_text(content)
      self.env['PATH'] = str(shims) + os.pathsep + self.env['PATH']
    self.exe = '.exe' if self.windows else ''

  def run(self, command, cwd=ROOT, env=None):
    command = list(map(str, command))
    print(subprocess.list2cmdline(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env or self.env, check=True)

  def flutter(self, *args, cwd=ROOT):
    self.run([ROOT / ('bin/flutter.bat' if self.windows else 'bin/flutter'), *args], cwd)

  def configure(self):
    args = self.out / 'args.gn'
    if not args.exists():
      command = [
          sys.executable, self.src / 'flutter/tools/gn', '--runtime-mode', self.args.mode,
          '--no-goma'
      ]
      command += ['--no-prebuilt-dart-sdk'] if self.windows else ['--enable-fontconfig']
      self.run(command)
    elif sys.platform.startswith('linux'):
      contents = args.read_text()
      updated = contents.replace('flutter_use_fontconfig = false',
                                 'flutter_use_fontconfig = true').replace(
                                     'skia_use_fontconfig = false', 'skia_use_fontconfig = true'
                                 )
      if updated != contents:
        args.write_text(updated)
        self.run([self.src / 'flutter/third_party/gn/gn', 'gen', self.out, f'--root={self.src}'])

  def build(self):
    self.configure()
    targets = [
        'flutter_rust_engine.dll', 'gen_snapshot.exe', 'font-subset.exe', 'impellerc.exe',
        'libtessellator.dll', 'flutter_patched_sdk', 'flutter/sky/packages/sky_engine:sky_engine'
    ] if self.windows else ['flutter_rust_shell_runner', 'libflutter_rust_engine.so']
    self.run(['ninja', '-C', self.out, *targets, 'dart_sdk', 'frontend_server_aot'])

  def smoke(self):
    fixture = TESTS / 'smoke_runner'
    target = SHELL / 'target'
    triple = 'x86_64-pc-windows-msvc' if self.windows else None
    command = [
        'cargo', '+1.93.1', 'build', '--locked', '--manifest-path', fixture / 'Cargo.toml',
        '--target-dir', target
    ]
    profile = 'release' if self.args.mode in ('release', 'profile') else 'debug'
    if profile == 'release':
      command += ['--release']
    if triple:
      command += ['--target', triple]
    self.run(command, fixture, self.env | {'FLUTTER_RUST_ENGINE_DIR': str(self.out)})
    source = target / triple / profile / 'flutter-rust-shell-smoke.exe' if self.windows else target / profile / 'flutter-rust-shell-smoke'
    stage = self.out / 'rust-smoke'
    stage.mkdir(exist_ok=True)
    shutil.copy2(source, stage)
    if self.windows:
      shutil.copy2(self.out / 'flutter_rust_engine.dll', stage)
    shutil.copy2(self.out / 'icudtl.dat', stage)

  def tests(self):
    command = [
        'cargo', '+1.93.1', 'test', '--locked', '-p', 'flutter-shell-core', '-p',
        'flutter-shell-wgpu', '-p', 'flutter-gpu-queue'
    ]
    if self.windows:
      command += ['--target', 'x86_64-pc-windows-msvc']
    self.run(command, SHELL)
    if self.windows:
      command = [
          'cargo', '+1.93.1', 'rustc', '--locked', '--target', 'x86_64-pc-windows-msvc', '-p',
          'flutter-shell-winit', '--tests', '--message-format=json', '--', '-L',
          f'native={self.out}', '-l', 'dylib=flutter_rust_engine.dll'
      ]
      result = subprocess.run(
          command, cwd=SHELL, env=self.env, check=True, stdout=subprocess.PIPE, text=True
      )
      executables = [
          Path(item['executable'])
          for line in result.stdout.splitlines()
          if (item := json.loads(line)).get('reason') == 'compiler-artifact' and
          item.get('profile', {}).get('test') and item.get('executable')
      ]
      if not executables:
        raise RuntimeError('Cargo produced no host test executable')
      for executable in executables:
        self.run([executable], SHELL,
                 self.env | {'PATH': str(self.out) + os.pathsep + self.env['PATH']})
    else:
      self.run(['cargo', '+1.93.1', 'test', '--locked', '-p', 'flutter-shell-winit'], SHELL)
    self.run(['ninja', '-C', self.out, 'flutter_rust_shell_unittests'])
    self.run([self.out / ('flutter_rust_shell_unittests' + self.exe)])
    self.run([sys.executable, SHELL / 'build_rust_test.py'])
    for pattern in ('benchmark_windowing_test.py', 'native_desktop_test.py'):
      self.run([sys.executable, '-m', 'unittest', 'discover', '-s', TESTS, '-p', pattern])

  def runner(self):
    if self.windows:
      runner = self.out / 'rust-smoke/flutter-rust-shell-smoke.exe'
    else:
      runner = self.out / 'flutter_rust_shell_runner'
    if not runner.is_file():
      raise RuntimeError(f'Native runner is missing: {runner}. Build the smoke runner first.')
    return runner

  def framework_tests(self):
    # A Rust-shell build does not refresh flutter_tester. Build the test host
    # and compiler together to avoid an Invalid SDK hash from a stale tester.
    self.run(['ninja', '-C', self.out, 'flutter_tester', 'dart_sdk', 'frontend_server_aot'])
    self.flutter(
        'test',
        f'--local-engine={self.args.engine}',
        f'--local-engine-host={self.args.engine}',
        f'--local-engine-src-path={self.src}',
        'test/foundation',
        'test/widgets/debug_view_metrics_override_test.dart',
        'test/widgets/windowing_test.dart',
        'test/rendering/binding_test.dart',
        'test/rendering/multi_view_binding_test.dart',
        'test/rendering/texture_test.dart',
        'test/widgets/texture_test.dart',
        cwd=ROOT / 'packages/flutter'
    )

  def bundle(self, project, target='lib/main.dart', windowing=False):
    if windowing:
      self.flutter('config', '--enable-windowing')
    self.flutter('pub', 'get', cwd=project)
    self.flutter(
        'build',
        'bundle',
        '--debug',
        f'--target={target}',
        f'--local-engine={self.args.engine}',
        f'--local-engine-host={self.args.engine}',
        f'--local-engine-src-path={self.src}',
        cwd=project
    )
    return project / 'build/flutter_assets'

  def fixture(self, script, assets, *extra, runner=None):
    self.run([
        sys.executable, TESTS / script, f'--runner={runner or self.runner()}', f'--assets={assets}',
        f'--icu={self.out / "icudtl.dat"}', *extra
    ])


def runtime(host, stress):
  """Require input and resized presentations, then clean compositor close."""
  api = desktop()
  assets = host.bundle(host.args.sample)
  runner = host.runner()
  iterations = host.args.resize_iterations if stress else host.args.lifecycle_iterations
  directory = Path(
      tempfile.mkdtemp(prefix='flutter-rust-stress-' if stress else 'flutter-rust-lifecycle-')
  )
  success = False
  try:
    for iteration in range(1 if stress else iterations):
      stats = directory / f'presentations-{iteration}'
      log_path = directory / f'runner-{iteration}.log'
      env = host.env | {
          'FLUTTER_RUST_PRESENTATION_STATS': str(stats), 'WGPU_VALIDATION': '1', 'WGPU_DEBUG': '1',
          'RUST_LOG': 'wgpu_core=warn,wgpu_hal=warn'
      }
      # Explicitly report absent driver validation rather than claiming coverage.
      if shutil.which('vulkaninfo'):
        probe = subprocess.run(['vulkaninfo', '--summary'],
                               capture_output=True,
                               text=True,
                               timeout=20)
        if 'VK_LAYER_KHRONOS_validation' in probe.stdout + probe.stderr:
          env['VK_INSTANCE_LAYERS'] = 'VK_LAYER_KHRONOS_validation'
      print(
          f'Driver validation: {env.get("VK_INSTANCE_LAYERS", "unavailable; wgpu validation only")}',
          flush=True
      )
      with log_path.open('wb') as log:
        process = subprocess.Popen([str(runner),
                                    str(assets),
                                    str(host.out / 'icudtl.dat')],
                                   env=env,
                                   stdout=log,
                                   stderr=subprocess.STDOUT)
        try:
          client = wait_until(
              lambda: next((
                  c for c in api.clients()
                  if c['pid'] == process.pid and c['title'] == 'Flutter Rust Shell'
              ), None), process, 20
          )
          if not client:
            raise RuntimeError('Runner did not map its window')
          address = str(client['address'])

          def frame():
            try:
              rows = [
                  line.split()
                  for line in stats.read_text().splitlines(keepends=True)
                  if line.endswith('\n')
              ]
              return tuple(map(int, rows[-1][:3])) if rows else None
            except (FileNotFoundError, ValueError):
              return None

          initial = wait_until(frame, process)
          if not initial:
            raise RuntimeError('Runner did not present its initial frame')
          if iteration == 0:
            api.capture_window(address, directory / 'first-frame.png')
          if stress:
            api.action('focus', address)
            time.sleep(0.75)
            baseline = frame()[0]
            api.action('tab', address)
            if not wait_until(lambda: frame() and frame()[0] > baseline, process):
              raise RuntimeError('Keyboard input produced no new presented frame')
            api.action('float', address)
            for i in range(iterations):
              api.action(
                  'resize', address, width=320 + (i * 73) % 1250, height=240 + (i * 47) % 720
              )
              if i % 30 == 0:
                api.action('hide', address)
                api.action('restore', address)
              if i % 50 == 0:
                api.action('fullscreen', address)
                api.action('fullscreen', address)
            baseline = frame()[0]
            api.action('resize', address, width=911, height=677)
            if not wait_until(
                lambda: frame() and frame()[0] > baseline and frame()[1:] != initial[1:], process):
              raise RuntimeError('Resize produced no newly sized presented frame')
          if stress:
            baseline = frame()[0]
            advanced = api.live_resize(address, lambda: frame()[0] if frame() else None)
            if advanced is False:
              raise RuntimeError('Win32 modal live resize starved Flutter presentations')
          api.action('close', address)
          if process.wait(timeout=10) != 0:
            raise RuntimeError(f'Runner exited with {process.returncode}')
          if DIAGNOSTIC.search(log_path.read_text(errors='replace')):
            raise RuntimeError('GPU/runtime diagnostics were reported')
        finally:
          if process.poll() is None:
            process.kill()
            process.wait()
    print(
        f'Runtime validation passed: {iterations} {"resizes" if stress else "startup/shutdown iterations"}; evidence: {directory}'
    )
    success = True
  finally:
    if not success:
      print(f'Validation evidence retained: {directory}', file=sys.stderr)


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('task')
  parser.add_argument('--engine', default=os.environ.get('LOCAL_ENGINE', 'host_debug'))
  parser.add_argument('--mode', default='debug', choices=['debug', 'profile', 'release'])
  parser.add_argument(
      '--sample',
      type=Path,
      default=Path(os.environ.get('RUST_SHELL_SAMPLE', str(TESTS / 'smoke_fixture')))
  )
  parser.add_argument(
      '--windowing-example',
      type=Path,
      default=Path(
          os.environ.get('RUST_WINDOWING_EXAMPLE', str(ROOT / 'examples/multiple_windows'))
      )
  )
  parser.add_argument(
      '--texture-example',
      type=Path,
      default=Path(os.environ.get('RUST_TEXTURE_EXAMPLE', str(ROOT / 'examples/texture')))
  )
  parser.add_argument(
      '--resize-iterations', type=int, default=int(os.environ.get('RESIZE_ITERATIONS', '500'))
  )
  parser.add_argument(
      '--lifecycle-iterations', type=int, default=int(os.environ.get('LIFECYCLE_ITERATIONS', '20'))
  )
  parser.add_argument(
      '--texture-lifecycle-iterations',
      type=int,
      default=int(os.environ.get('TEXTURE_LIFECYCLE_ITERATIONS', '40'))
  )
  parser.add_argument('--target', default='lib/main.dart')
  parser.add_argument(
      '--android-engine', default=os.environ.get('ANDROID_ENGINE', 'android_debug_unopt_arm64')
  )
  parser.add_argument(
      '--android-gradle-task', default=os.environ.get('ANDROID_GRADLE_TASK', 'assembleDebug')
  )
  args, extra = parser.parse_known_args()
  if extra and args.task != 'benchmark':
    parser.error(f'Unrecognized arguments: {extra}')
  if min(args.resize_iterations, args.lifecycle_iterations, args.texture_lifecycle_iterations) < 1:
    parser.error('Iteration counts must be positive')
  host = Host(args)
  task = args.task
  if task == 'build':
    host.build()
    if host.windows:
      host.smoke()
  elif task == 'smoke':
    host.smoke()
  elif task == 'test':
    host.tests()
  elif task == 'framework-tests':
    host.framework_tests()
  elif task == 'gpu-tests':
    env = host.env | {
        'WGPU_VALIDATION': '1', 'WGPU_DEBUG': '1', 'VK_INSTANCE_LAYERS':
            'VK_LAYER_KHRONOS_validation'
    }
    for name in ('direct_pixel_upload_preserves_padded_rows_and_can_mix_with_staged_writes',
                 'pixel_upload_reuses_aligned_and_unaligned_slots',
                 'slow_pixel_writer_allows_other_slots_and_shutdown',
                 'empty_handoff_pass_preserves_fragment_uniform_colors'):
      command = ['cargo', '+1.93.1', 'test', '--locked', '-p', 'flutter-shell-wgpu', name]
      if host.windows:
        command += ['--target', 'x86_64-pc-windows-msvc']
      host.run(command + ['--', '--ignored', '--nocapture'], SHELL, env)
  elif task == 'configure':
    host.configure()
  elif task in ('fmt-rust', 'fix-rust-lints'):
    command = ['cargo', 'fmt', '--all'] if task == 'fmt-rust' else [
        'cargo', 'clippy', '--workspace', '--all-targets', '--fix', '--allow-dirty',
        '--allow-staged'
    ]
    host.run(command, SHELL)
    if task == 'fmt-rust':
      host.run(['cargo', 'fmt', '--manifest-path', SHELL / 'rlutter/Cargo.toml'], SHELL)
      for manifest in (TESTS / 'smoke_runner/Cargo.toml', TESTS / 'frb_fixture/rust/Cargo.toml'):
        host.run(['cargo', '+1.93.1', 'fmt', '--manifest-path', manifest], SHELL)
  elif task == 'fmt-engine':
    sdk = host.src / 'flutter/third_party/dart/tools/sdks/dart-sdk/bin' / (
        'dart.exe' if host.windows else 'dart'
    )
    if not sdk.exists():
      sdk = host.src / 'third_party/dart/tools/sdks/dart-sdk/bin' / (
          'dart.exe' if host.windows else 'dart'
      )
    host.run([sdk, host.src / 'flutter/ci/bin/format.dart', '--fix'], host.src / 'flutter/ci')
  elif task in ('build-rlutter', 'install-rlutter', 'test-rlutter'):
    commands = {
        'build-rlutter': [['cargo', 'build', '--release']],
        'install-rlutter': [['cargo', 'install', '--path', '.', '--locked']],
        'test-rlutter': [['cargo', 'test'],
                         ['cargo', 'clippy', '--all-targets', '--', '-D', 'warnings']]
    }
    for command in commands[task]:
      host.run(command, SHELL / 'rlutter')
  elif task == 'push':
    host.run(['git', 'push'])
  elif task == 'build-android':
    targets = [
        'flutter/shell/platform/android:android_jar',
        'flutter/shell/platform/android:pom_embedding',
        'flutter/shell/platform/android:pom_libflutter', 'flutter/lib/snapshot:strong_platform'
    ]
    if args.android_engine.startswith(('android_release', 'android_profile')):
      targets += ['clang_x64/gen_snapshot']
    host.run(['ninja', '-C', host.src / 'out' / args.android_engine, *targets])
  elif task == 'android-apk':
    host.run([
        SHELL / 'android_shell_app' /
        ('gradlew.bat' if host.windows else 'gradlew'), args.android_gradle_task,
        f'-PflutterShellJniLibsDir={host.src / "out" / args.android_engine / "android_shell/jniLibs"}'
    ], SHELL / 'android_shell_app')
  elif task == 'run':
    assets = host.bundle(args.windowing_example, args.target, True)
    host.run([host.runner(), assets, host.out / 'icudtl.dat'])
  elif task == 'popup-windowing':
    assets = host.bundle(TESTS / 'windowing_fixture', 'lib/popups.dart', True)
    host.fixture('test_windowing.py', assets, '--popup-dismissal')
  elif task == 'tooltip-windowing':
    assets = host.bundle(TESTS / 'windowing_fixture', 'lib/tooltips.dart', True)
    host.fixture('test_windowing.py', assets,
                 '--tooltip-motion' if host.windows else '--wait-parent-mapped')
  elif task == 'owned-windowing':
    assets = host.bundle(TESTS / 'windowing_fixture', 'lib/owned.dart', True)
    host.fixture('test_windowing.py', assets)
  elif task in ('windowing', 'benchmark'):
    assets = host.bundle(
        TESTS / 'windowing_fixture',
        'lib/benchmark.dart' if task == 'benchmark' else 'lib/main.dart', True
    )
    host.fixture(
        'benchmark_windowing.py' if task == 'benchmark' else 'test_windowing.py', assets, *extra
    )
  elif task in ('texture', 'pixel-buffer', 'texture-lifecycle'):
    target = args.texture_example / 'lib/rust_shell_main.dart'
    # This entry point only uses Flutter. Avoid resolving every example in
    # the root pub workspace for the default integration fixture.
    project = TESTS / 'smoke_fixture' if args.texture_example.resolve(
    ) == (ROOT / 'examples/texture').resolve() else args.texture_example
    assets = host.bundle(project, str(target.resolve()))
    flags = ['--pixel-buffer'] if task == 'pixel-buffer' else []
    if task == 'texture-lifecycle':
      flags += [f'--lifecycle-iterations={args.texture_lifecycle_iterations}']
    host.fixture('test_texture.py', assets, *flags)
  elif task == 'text-input':
    assets = host.bundle(TESTS / 'smoke_fixture', 'lib/input_probe.dart')
    host.fixture('test_text_input.py', assets)
  elif task == 'frb':
    fixture = TESTS / 'frb_fixture'
    assets = host.bundle(fixture)
    host.run([
        'cargo', '+1.93.1', 'build', '--locked', '--features', 'runner', '--bin',
        'flutter-rust-shell-frb-fixture'
    ], fixture / 'rust', host.env | {'FLUTTER_RUST_ENGINE_DIR': str(host.out)})
    runner = fixture / 'rust/target/debug' / ('flutter-rust-shell-frb-fixture' + host.exe)
    if host.windows:
      shutil.copy2(host.out / 'flutter_rust_engine.dll', runner.parent)
    host.fixture('test_frb_dispatch.py', assets, runner=runner)
  elif task in ('lifecycle', 'stress'):
    runtime(host, task == 'stress')
  elif task == 'rlutter-aot':
    aot_mode = 'release' if args.mode == 'debug' else args.mode
    cli = SHELL / 'rlutter/target/release' / ('rlutter' + host.exe)
    with tempfile.TemporaryDirectory(prefix='rlutter-aot-project-') as work:
      project = Path(work) / 'aot_probe'
      host.run([cli, 'create', project, f'--flutter-root={ROOT}'])
      host.run([sys.executable, TESTS / 'test_rlutter_aot.py', f'--cli={cli}',
                f'--flutter-root={ROOT}', f'--project={project}', f'--mode={aot_mode}'])
  elif task == 'rlutter-runtime':
    cli = SHELL / 'rlutter/target/release' / ('rlutter' + host.exe)
    host.run([sys.executable, TESTS / 'test_rlutter.py', f'--cli={cli}',
              f'--flutter-root={ROOT}'])
  elif task == 'rlutter-e2e':
    with tempfile.TemporaryDirectory(prefix='rlutter-e2e-') as work:
      cli = SHELL / 'rlutter/target/release' / ('rlutter' + host.exe)
      host.run([cli, 'create', 'smoke_app', f'--flutter-root={ROOT}'], work)
      project = Path(work) / 'smoke_app'
      host.run([cli, 'build', 'windows' if host.windows else 'linux', f'--flutter-root={ROOT}'],
               project)
      bundle = project / 'runner-rs/target/debug'
      for path in ['smoke_app' + host.exe, 'data/flutter_assets/kernel_blob.bin', 'data/icudtl.dat',
                   'flutter_rust_engine.dll' if host.windows else 'libs/libflutter_rust_engine.so']:
        if not (bundle / path).is_file():
          raise RuntimeError(f'Missing bundled artifact: {bundle / path}')
  else:
    parser.error(f'Unknown task: {task}')


if __name__ == '__main__':
  try:
    main()
  except (RuntimeError, subprocess.CalledProcessError) as error:
    raise SystemExit(str(error)) from error
