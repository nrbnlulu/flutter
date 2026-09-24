// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:convert';

import '../base/common.dart';
import '../base/error_handling_io.dart';
import '../base/file_system.dart';
import '../base/logger.dart';
import '../base/process.dart';
import '../project.dart';

/// A Pub package's Rust `cdylib` that the Rust shell builds and loads with
/// `dlopen`, instead of linking it into the runner.
class RustNativeLibrary {
  const RustNativeLibrary({
    required this.dartPackageName,
    required this.manifestPath,
    required this.libraryName,
    this.rustflags = const <String>[],
  });

  factory RustNativeLibrary.fromJson(Map<String, Object?> json) => RustNativeLibrary(
    dartPackageName: json['package']! as String,
    manifestPath: json['manifest']! as String,
    libraryName: json['library']! as String,
    rustflags: (json['rustflags']! as List<Object?>).cast<String>(),
  );

  final String dartPackageName;
  final String manifestPath;

  /// The Cargo library target name, so the file is `lib<libraryName>.so`.
  final String libraryName;

  /// Extra rustflags the package needs (`[package.metadata.flutter] rustflags`),
  /// for what its own build tooling would otherwise inject.
  final List<String> rustflags;

  String get fileName => 'lib$libraryName.so';

  Map<String, Object?> toJson() => <String, Object?>{
    'package': dartPackageName,
    'manifest': manifestPath,
    'library': libraryName,
    'rustflags': rustflags,
  };
}

Directory _flutterRsDirectory(FlutterProject project) =>
    project.directory.childDirectory('.dart_tool').childDirectory('flutter_rs');

/// Where built native libraries are staged for [releaseMode]; this directory
/// is added to the runner's `LD_LIBRARY_PATH` when launched by the tool.
Directory nativeLibraryDirectory(FlutterProject project, {required bool releaseMode}) =>
    _flutterRsDirectory(project)
        .childDirectory('native_libs')
        .childDirectory(releaseMode ? 'release' : 'debug');

File _nativeLibrariesFile(FlutterProject project) =>
    _flutterRsDirectory(project).childFile('native_libs.json');

void writeNativeLibraries(FlutterProject project, List<RustNativeLibrary> libraries) {
  final File file = _nativeLibrariesFile(project);
  if (libraries.isEmpty) {
    ErrorHandlingFileSystem.deleteIfExists(file);
    return;
  }
  libraries.sort(
    (RustNativeLibrary a, RustNativeLibrary b) => a.dartPackageName.compareTo(b.dartPackageName),
  );
  file.createSync(recursive: true);
  file.writeAsStringSync(
    const JsonEncoder.withIndent('  ').convert(<Object?>[for (final l in libraries) l.toJson()]),
  );
}

List<RustNativeLibrary> readNativeLibraries(FlutterProject project) {
  final File file = _nativeLibrariesFile(project);
  if (!file.existsSync()) {
    return const <RustNativeLibrary>[];
  }
  return <RustNativeLibrary>[
    for (final Object? entry in json.decode(file.readAsStringSync()) as List<Object?>)
      RustNativeLibrary.fromJson(entry! as Map<String, Object?>),
  ];
}

/// Builds every native library found by plugin discovery with the same
/// profile as the runner and stages the results in [nativeLibraryDirectory].
Future<void> buildNativeLibraries(
  FlutterProject project, {
  required bool releaseMode,
  required ProcessUtils processUtils,
  required Logger logger,
  required FileSystem fileSystem,
}) async {
  final List<RustNativeLibrary> libraries = readNativeLibraries(project);
  if (libraries.isEmpty) {
    return;
  }
  final Directory targetDir = _flutterRsDirectory(project)
      .childDirectory('native_libs')
      .childDirectory('target');
  final Directory staging = nativeLibraryDirectory(project, releaseMode: releaseMode);
  staging.createSync(recursive: true);
  for (final library in libraries) {
    logger.printStatus(
      'Building native library ${library.fileName} (${library.dartPackageName})...',
    );
    final int result = await processUtils.stream(
      <String>[
        'cargo',
        'build',
        if (releaseMode) '--release',
        '--manifest-path',
        library.manifestPath,
        '--target-dir',
        targetDir.path,
      ],
      workingDirectory: fileSystem.file(library.manifestPath).parent.path,
      environment: <String, String>{
        if (library.rustflags.isNotEmpty) 'CARGO_ENCODED_RUSTFLAGS': library.rustflags.join('\x1f'),
      },
    );
    if (result != 0) {
      throwToolExit(
        'Unable to build native library ${library.fileName} for ${library.dartPackageName}.',
      );
    }
    final File built = targetDir
        .childDirectory(releaseMode ? 'release' : 'debug')
        .childFile(library.fileName);
    if (!built.existsSync()) {
      throwToolExit('Cargo did not produce ${built.path} for ${library.dartPackageName}.');
    }
    built.copySync(staging.childFile(library.fileName).path);
  }
}
