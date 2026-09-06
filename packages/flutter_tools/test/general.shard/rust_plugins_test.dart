// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'package:flutter_tools/src/rust/rust_plugins.dart';

import '../src/common.dart';

void main() {
  testWithoutContext('replaces only the generated Cargo dependency section', () {
    const manifest =
        '''
[dependencies]
app_dependency = "1"
$rustPluginDependenciesBegin
old_plugin = { path = "old" }
$rustPluginDependenciesEnd

[dev-dependencies]
test_dependency = "1"
''';

    expect(
      replaceGeneratedRustDependencies(manifest, <String>[
        'plugin_a = { path = "generated/a" }',
        'plugin_b = { path = "generated/b" }',
      ]),
      '''
[dependencies]
app_dependency = "1"
$rustPluginDependenciesBegin
plugin_a = { path = "generated/a" }
plugin_b = { path = "generated/b" }
$rustPluginDependenciesEnd

[dev-dependencies]
test_dependency = "1"
''',
    );
  });

  testWithoutContext('rejects missing generated Cargo dependency markers', () {
    expect(
      () => replaceGeneratedRustDependencies('[dependencies]\n', const <String>[]),
      throwsToolExit(),
    );
  });
}
