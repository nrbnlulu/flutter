// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'package:file/memory.dart';
import 'package:flutter_tools/src/base/file_system.dart';
import 'package:flutter_tools/src/base/logger.dart';
import 'package:flutter_tools/src/base/os.dart';
import 'package:flutter_tools/src/base/platform.dart';
import 'package:flutter_tools/src/build_info.dart';
import 'package:flutter_tools/src/device.dart';
import 'package:flutter_tools/src/project.dart';
import 'package:flutter_tools/src/rust/rust_device.dart';
import 'package:test/fake.dart';

import '../src/common.dart';
import '../src/fake_process_manager.dart';

void main() {
  testWithoutContext('discovers rust as a well-known device on Linux x64', () async {
    final discovery = RustShellDevices(
      platform: FakePlatform(),
      operatingSystemUtils: FakeOperatingSystemUtils(),
      processManager: FakeProcessManager.any(),
      logger: BufferLogger.test(),
      fileSystem: MemoryFileSystem.test(),
    );

    expect(discovery.wellKnownIds, <String>['rust']);
    expect(discovery.canListAnything, true);
    expect(await discovery.devices(), hasLength(1));
    expect((await discovery.devices()).single.id, 'rust');
    expect(
      await discovery.devices(
        filter: DeviceDiscoveryFilter(
          deviceConnectionInterface: DeviceConnectionInterface.wireless,
        ),
      ),
      isEmpty,
    );
  });

  testWithoutContext('does not discover rust on unsupported hosts', () async {
    final discovery = RustShellDevices(
      platform: FakePlatform(operatingSystem: 'windows'),
      operatingSystemUtils: FakeOperatingSystemUtils(hostPlatform: HostPlatform.windows_x64),
      processManager: FakeProcessManager.any(),
      logger: BufferLogger.test(),
      fileSystem: MemoryFileSystem.test(),
    );

    expect(discovery.canListAnything, false);
    expect(await discovery.devices(), isEmpty);
  });

  testWithoutContext('supports generated Rust-shell applications in debug mode', () async {
    final FileSystem fileSystem = MemoryFileSystem.test();
    fileSystem.file('pubspec.yaml')
      ..createSync()
      ..writeAsStringSync('''
name: rust_app
environment:
  sdk: ^3.9.0
flutter:
  shell: rust
''');
    fileSystem.file('runner-rs/Cargo.toml').createSync(recursive: true);
    final FlutterProject project = FlutterProjectFactory(
      fileSystem: fileSystem,
      logger: BufferLogger.test(),
    ).fromDirectory(fileSystem.currentDirectory);
    final device = RustShellDevice(
      processManager: FakeProcessManager.any(),
      logger: BufferLogger.test(),
      fileSystem: fileSystem,
      operatingSystemUtils: FakeOperatingSystemUtils(),
    );

    expect(device.isSupportedForProject(project), true);
    expect(device.supportsRuntimeMode(BuildMode.debug), true);
    expect(device.supportsRuntimeMode(BuildMode.profile), false);
    expect(device.supportsRuntimeMode(BuildMode.release), false);
  });
}

class FakeOperatingSystemUtils extends Fake implements OperatingSystemUtils {
  FakeOperatingSystemUtils({this.hostPlatform = HostPlatform.linux_x64});

  @override
  final HostPlatform hostPlatform;

  @override
  String get name => 'Linux';
}
