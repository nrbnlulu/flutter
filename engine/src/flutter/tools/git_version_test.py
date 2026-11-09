# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Verify engine revision discovery with native Git on PATH."""
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch


def load(name, path):
  loader = importlib.machinery.SourceFileLoader(name, str(path))
  spec = importlib.util.spec_from_loader(name, loader)
  module = importlib.util.module_from_spec(spec)
  loader.exec_module(module)
  return module


class GitVersionTest(unittest.TestCase):

  def test_gn_and_build_helper_resolve_the_native_git_revision(self):
    git = shutil.which('git.exe' if os.name == 'nt' else 'git')
    self.assertIsNotNone(git)
    engine = Path(__file__).resolve().parents[1]
    modules = [
        load('flutter_gn', engine / 'tools/gn'),
        load('flutter_git_revision', engine / 'build/git_revision.py')
    ]
    with tempfile.TemporaryDirectory(prefix='flutter revision ') as directory:
      subprocess.run([git, 'init', directory], check=True, capture_output=True)
      subprocess.run([
          git, '-c', 'user.name=Revision Test', '-c', 'user.email=revision@example.invalid',
          'commit', '--allow-empty', '-m', 'Seed'
      ],
                     cwd=directory,
                     check=True,
                     capture_output=True)
      expected = subprocess.check_output([git, '-C', directory, 'rev-parse', 'HEAD'],
                                         text=True).strip()
      with patch.dict(os.environ, {'PATH': str(Path(git).parent)}):
        for module in modules:
          with self.subTest(helper=module.__name__):
            self.assertEqual(module.get_repository_version(directory), expected)


if __name__ == '__main__':
  unittest.main()
