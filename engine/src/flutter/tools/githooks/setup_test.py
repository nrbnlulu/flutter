# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Exercise hook installation with native Git, without a depot_tools git.bat."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import setup


class SetupTest(unittest.TestCase):

  @unittest.skipUnless(os.name == 'nt', 'Windows native Git regression')
  def test_install_and_unset_with_git_exe_only_on_path(self):
    git = shutil.which('git.exe')
    self.assertIsNotNone(git)
    git_directory = Path(git).parent
    self.assertFalse((git_directory / 'git.bat').exists())
    with tempfile.TemporaryDirectory(prefix='flutter hooks ') as directory:
      repo = Path(directory)
      subprocess.run([git, 'init', str(repo)], check=True, capture_output=True)
      flutter = repo / 'engine/src/flutter'
      (flutter / 'tools/githooks').mkdir(parents=True)
      with patch.object(setup, 'FLUTTER_DIR', str(flutter)), \
           patch.dict(os.environ, {'PATH': str(git_directory)}):
        self.assertEqual(setup.Main(['setup.py']), 0)
        hooks = subprocess.check_output([git, 'config', '--local', '--get', 'core.hooksPath'],
                                        cwd=repo,
                                        text=True).strip()
        self.assertEqual(hooks, 'engine/src/flutter/tools/githooks')
        self.assertEqual(setup.Main(['setup.py', '--unset']), 0)
        result = subprocess.run([git, 'config', '--local', '--get', 'core.hooksPath'],
                                cwd=repo,
                                capture_output=True)
        self.assertEqual(result.returncode, 1)


if __name__ == '__main__':
  unittest.main()
