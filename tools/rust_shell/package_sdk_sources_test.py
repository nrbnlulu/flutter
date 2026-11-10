# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Source SDK completeness and exact engine/runner ABI packaging checks."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[2]
SOURCE=ROOT/'engine/src/flutter/shell/platform/rust'
spec=importlib.util.spec_from_file_location('package_sdk_sources',SOURCE/'tools/package_sdk_sources.py')
package=importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)
windows_spec=importlib.util.spec_from_file_location('package_windows_sdk',ROOT/'tools/rust_shell/package_windows_sdk.py')
windows_package=importlib.util.module_from_spec(windows_spec)
windows_spec.loader.exec_module(windows_package)


class PackagingTest(unittest.TestCase):
    def test_windows_package_uses_the_complete_shared_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            destination=Path(directory)/'sdk'
            windows_package.sources(ROOT,destination)
            self.assertTrue((destination/'dart/flutter_rust_agent/lib/flutter_rust_agent.dart').is_file())
            self.assertTrue((destination/'third_party/wgpu-hal/Cargo.toml').is_file())
            self.assertTrue((destination/'skills/application/SKILL.md').is_file())
            self.assertTrue((destination/'rust-toolchain.toml').is_file())

    def test_every_headless_source_and_both_patches_are_self_contained(self):
        with tempfile.TemporaryDirectory() as directory:
            destination=Path(directory)/'sdk'
            result=package.stage(SOURCE,destination)
            self.assertEqual(result['shellAbi'],16)
            self.assertTrue((destination/'rust-toolchain.toml').is_file())
            self.assertTrue((destination/'tests/agent_fixture/test/agent/save_dialog.json').is_file())
            self.assertTrue((destination/'skills/application/SKILL.md').is_file())
            self.assertTrue((destination/'skills/application/../../docs/agent-protocol.md').is_file())
            self.assertFalse(any(p.is_symlink() or p.name in ('target','.dart_tool') for p in destination.rglob('*')))
            obsolete=destination/'crates/obsolete.rs';obsolete.write_text('stale')
            package.stage(SOURCE,destination)
            self.assertFalse(obsolete.exists())

    def test_mismatched_mode_or_engine_abi_rejects_distribution(self):
        with tempfile.TemporaryDirectory() as directory:
            destination=Path(directory)/'sdk'
            for mode in ('debug','profile','release'):
                engine=destination/f'lib/{mode}';engine.mkdir(parents=True)
                (engine/'engine-manifest.json').write_text(json.dumps({'shellAbi':16,'buildMode':mode}))
            package.stage(SOURCE,destination,True)
            (destination/'lib/release/engine-manifest.json').write_text(json.dumps({'shellAbi':15,'buildMode':'release'}))
            with self.assertRaisesRegex(RuntimeError,'does not match'):
                package.stage(SOURCE,destination,True)


if __name__=='__main__':unittest.main()
