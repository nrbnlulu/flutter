# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Check that Flutter app fixtures do not enter the engine's offline pub audit."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import pub_get_offline


class PackageAuditTest(unittest.TestCase):

  def test_rust_app_fixtures_are_excluded_but_unknown_packages_are_rejected(self):
    with tempfile.TemporaryDirectory() as directory:
      engine = Path(directory)
      original = Path(pub_get_offline.ENGINE_DIR)
      excluded = [
          str(engine / Path(path).relative_to(original)) for path in pub_get_offline.EXCLUDED_DIRS
      ]
      for name in ['frb_fixture', 'smoke_fixture', 'windowing_fixture', 'unlisted_package']:
        package = engine / 'shell/platform/rust/tests' / name
        package.mkdir(parents=True)
        (package / 'pubspec.yaml').write_text('name: test_package\n', encoding='utf-8')
      (engine / 'pubspec.yaml').write_text('name: engine_root\n', encoding='utf-8')
      with patch.object(pub_get_offline, 'ENGINE_DIR', str(engine)), \
           patch.object(pub_get_offline, 'ALL_PACKAGES', [str(engine)]), \
           patch.object(pub_get_offline, 'EXCLUDED_DIRS', excluded):
        self.assertEqual(
            pub_get_offline.find_unlisted_packages(),
            [str(engine / 'shell/platform/rust/tests/unlisted_package')]
        )


if __name__ == '__main__':
  unittest.main()
