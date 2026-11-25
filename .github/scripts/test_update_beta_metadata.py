#!/usr/bin/env python3
"""Tests for the beta release metadata command."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name('update-beta-metadata.py')


class UpdateBetaMetadataTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.current = self.root / 'current'
        self.current.mkdir()
        self.previous = self.root / 'previous'
        self.previous.mkdir()

    def run_command(self, *arguments):
        return subprocess.run(
            [sys.executable, str(SCRIPT), str(self.current), 'new-sha', *arguments],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_fresh_release(self):
        assets = {
            'flutter-rust-linux-x64.tar.gz': b'linux',
            'flutter-rust-android-arm64.tar.gz': b'android',
            'rlutter-flutter-windows-x64.tar.gz': b'windows sdk',
            'rlutter.exe': b'cli',
            'flutter-tools.snapshot': b'tools',
        }
        for name, content in assets.items():
            (self.current / name).write_bytes(content)
        (self.current / 'unrelated.tar.gz').write_bytes(b'ignore')
        (self.current / 'flutter-rust-directory.tar.gz').mkdir()
        (self.current / 'SHA256SUMS').write_text('stale output')

        result = self.run_command('--platform', 'windows', '--platform', 'linux')
        self.assertEqual(result.returncode, 0, result.stderr)
        expected = ''.join(
            f'{hashlib.sha256(assets[name]).hexdigest()}  {name}\n'
            for name in sorted(assets)
        )
        self.assertEqual((self.current / 'SHA256SUMS').read_text(), expected)
        self.assertEqual(
            (self.current / 'platform-revisions.json').read_text(),
            json.dumps({'linux': 'new-sha', 'windows': 'new-sha'}, indent=2) + '\n',
        )
        result = self.run_command('--platform', 'linux', '--platform', 'windows')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.current / 'SHA256SUMS').read_text(), expected)

    def test_partial_update_preserves_other_platforms_and_replaces_current_digest(self):
        old_checksums = {
            'flutter-rust-linux-x64.tar.gz': 'a' * 64,
            'flutter-rust-android-arm64.tar.gz': 'b' * 64,
            'flutter-rust-darwin-arm64.tar.gz': 'c' * 64,
            'rlutter.exe': 'd' * 64,
            'flutter-tools.snapshot': 'e' * 64,
        }
        (self.previous / 'SHA256SUMS').write_text(
            ''.join(f'{digest}  {name}\n' for name, digest in old_checksums.items())
        )
        old_revisions = {
            'linux': 'old-linux',
            'android': 'old-android',
            'macos': 'old-macos',
            'windows': 'old-windows',
        }
        (self.previous / 'platform-revisions.json').write_text(json.dumps(old_revisions))
        (self.current / 'flutter-rust-linux-x64.tar.gz').write_bytes(b'new linux')
        (self.current / 'flutter-tools.snapshot').write_bytes(b'new tools')

        result = self.run_command('--platform', 'linux', '--previous', str(self.previous))
        self.assertEqual(result.returncode, 0, result.stderr)
        old_checksums['flutter-rust-linux-x64.tar.gz'] = hashlib.sha256(b'new linux').hexdigest()
        old_checksums['flutter-tools.snapshot'] = hashlib.sha256(b'new tools').hexdigest()
        self.assertEqual(
            (self.current / 'SHA256SUMS').read_text(),
            ''.join(f'{old_checksums[name]}  {name}\n' for name in sorted(old_checksums)),
        )
        old_revisions['linux'] = 'new-sha'
        self.assertEqual(
            (self.current / 'platform-revisions.json').read_text(),
            json.dumps(old_revisions, indent=2, sort_keys=True) + '\n',
        )

    def test_legacy_manifest_merge_normalizes_paths_and_preserves_previous_metadata(self):
        old_manifest = (
            f'{"a" * 64}  build/flutter-rust-linux-x64.tar.gz\n'
            f'{"b" * 64}  build/flutter-rust-darwin-arm64.tar.gz\n'
            f'{"c" * 64}  rlutter.exe\n'
        )
        old_revisions = json.dumps({'linux': 'old-linux', 'macos': 'old-macos'})
        (self.previous / 'SHA256SUMS').write_text(old_manifest)
        (self.previous / 'platform-revisions.json').write_text(old_revisions)
        (self.current / 'flutter-rust-linux-x64.tar.gz').write_bytes(b'new linux')

        result = self.run_command('--platform', 'linux', '--previous', str(self.previous))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            (self.current / 'SHA256SUMS').read_text(),
            f'{"b" * 64}  flutter-rust-darwin-arm64.tar.gz\n'
            f'{hashlib.sha256(b"new linux").hexdigest()}  flutter-rust-linux-x64.tar.gz\n'
            f'{"c" * 64}  rlutter.exe\n',
        )
        self.assertEqual(
            json.loads((self.current / 'platform-revisions.json').read_text()),
            {'linux': 'new-sha', 'macos': 'old-macos'},
        )
        self.assertEqual((self.previous / 'SHA256SUMS').read_text(), old_manifest)
        self.assertEqual((self.previous / 'platform-revisions.json').read_text(), old_revisions)

    def test_missing_previous_metadata(self):
        result = self.run_command('--platform', 'android', '--previous', str(self.previous))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.current / 'SHA256SUMS').read_text(), '')
        self.assertEqual(
            json.loads((self.current / 'platform-revisions.json').read_text()),
            {'android': 'new-sha'},
        )

    def test_accepts_binary_checksum_marker_and_uppercase_digest(self):
        (self.previous / 'SHA256SUMS').write_text(f'{"A" * 64} *rlutter.exe\n')
        result = self.run_command('--platform', 'macos', '--previous', str(self.previous))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            (self.current / 'SHA256SUMS').read_text(), f'{"a" * 64}  rlutter.exe\n'
        )

    def test_rejects_bad_checksum_input(self):
        digest = 'a' * 64
        bad_lines = [
            f'{digest}  ../escape',
            f'{digest}  /absolute',
            f'{digest}  nested/asset',
            f'{digest}  build/../escape',
            f'{digest}  build/nested/asset',
            f'{digest}  build/build/asset',
            f'{digest}  build//asset',
            f'{digest}  ./build/asset',
            f'{digest}  /build/asset',
            f'{digest}  build/..',
            f'{digest}  build/',
            f'{digest}  nested\\asset',
            f'{digest}  C:asset',
            f'{digest}  .',
            f'{digest}  ..',
            f'{digest}  asset with spaces',
            f'{"g" * 64}  rlutter.exe',
            f'{"a" * 63}  rlutter.exe',
            f'{"a" * 65}  rlutter.exe',
            f'{digest} rlutter.exe',
            f'{digest}  rlutter.exe trailing',
            'not a checksum',
            '',
            f'{digest}  rlutter.exe\n{digest}  rlutter.exe',
            f'{digest}  build/rlutter.exe\n{digest}  rlutter.exe',
            f'{digest}  rlutter.exe\n{digest}  build/rlutter.exe',
        ]
        for line in bad_lines:
            with self.subTest(line=line):
                (self.previous / 'SHA256SUMS').write_text(line + '\n')
                result = self.run_command('--platform', 'linux', '--previous', str(self.previous))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('checksum', result.stderr)
                self.assertFalse((self.current / 'SHA256SUMS').exists())
                self.assertFalse((self.current / 'platform-revisions.json').exists())

    def test_requires_supported_platform(self):
        for arguments in ((), ('--platform', 'ios')):
            with self.subTest(arguments=arguments):
                result = self.run_command(*arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('--platform', result.stderr)


if __name__ == '__main__':
    unittest.main()
