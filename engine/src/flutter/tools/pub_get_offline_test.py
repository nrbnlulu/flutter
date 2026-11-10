# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Keep Flutter-dependent Rust packages outside the engine's offline pub audit."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import pub_get_offline


class PackageAuditTest(unittest.TestCase):

  def test_flutter_rust_packages_are_excluded_but_unknown_packages_are_rejected(self):
    with tempfile.TemporaryDirectory() as directory:
      engine = Path(directory)
      original = Path(pub_get_offline.ENGINE_DIR)
      excluded = [
          str(engine / Path(path).relative_to(original)) for path in pub_get_offline.EXCLUDED_DIRS
      ]
      for name in [
          'agent_fixture', 'frb_fixture', 'headless_fixture', 'smoke_fixture',
          'windowing_fixture', 'unlisted_package'
      ]:
        package = engine / 'shell/platform/rust/tests' / name
        package.mkdir(parents=True)
        (package / 'pubspec.yaml').write_text('name: test_package\n', encoding='utf-8')
      for name in ['flutter_rust_agent', 'unlisted_package']:
        package = engine / 'shell/platform/rust/dart' / name
        package.mkdir(parents=True)
        (package / 'pubspec.yaml').write_text('name: test_package\n', encoding='utf-8')
      (engine / 'pubspec.yaml').write_text('name: engine_root\n', encoding='utf-8')
      with patch.object(pub_get_offline, 'ENGINE_DIR', str(engine)), \
           patch.object(pub_get_offline, 'ALL_PACKAGES', [str(engine)]), \
           patch.object(pub_get_offline, 'EXCLUDED_DIRS', excluded):
        self.assertEqual(
            set(pub_get_offline.find_unlisted_packages()),
            {
                str(engine / 'shell/platform/rust/tests/unlisted_package'),
                str(engine / 'shell/platform/rust/dart/unlisted_package'),
            }
        )

  def test_tracked_packages_have_no_unlisted_packages(self):
    original = Path(pub_get_offline.ENGINE_DIR)
    manifests = subprocess.check_output(
        ['git', 'ls-files', '-z', '--', 'pubspec.yaml', '**/pubspec.yaml'],
        cwd=original,
    ).decode('utf-8').split('\0')
    self.assertIn('shell/platform/rust/dart/flutter_rust_agent/pubspec.yaml', manifests)
    # Reconstruct a clean checkout so generated SDKs under target/ cannot
    # contaminate this regression when it runs in a developer's working tree.
    with tempfile.TemporaryDirectory() as directory:
      engine = Path(directory)
      for manifest in filter(None, manifests):
        destination = engine / manifest
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.touch()
      packages = [str(engine / Path(path).relative_to(original))
                  for path in pub_get_offline.ALL_PACKAGES]
      excluded = [str(engine / Path(path).relative_to(original))
                  for path in pub_get_offline.EXCLUDED_DIRS]
      with patch.object(pub_get_offline, 'ENGINE_DIR', str(engine)), \
           patch.object(pub_get_offline, 'ALL_PACKAGES', packages), \
           patch.object(pub_get_offline, 'EXCLUDED_DIRS', excluded):
        self.assertEqual(pub_get_offline.find_unlisted_packages(), [])


if __name__ == '__main__':
  unittest.main()
