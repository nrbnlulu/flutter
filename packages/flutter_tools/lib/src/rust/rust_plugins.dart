// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'package:package_config/package_config.dart';

import '../base/common.dart';
import '../base/error_handling_io.dart';
import '../base/file_system.dart';
import '../base/io.dart';
import '../base/net.dart';
import '../cache.dart';
import '../convert.dart';
import '../dart/package_map.dart';
import '../globals.dart' as globals;
import '../package_graph.dart';
import '../project.dart';

const rustPluginDependenciesBegin = '# === BEGIN FLUTTER GENERATED RUST PLUGINS ===';
const rustPluginDependenciesEnd = '# === END FLUTTER GENERATED RUST PLUGINS ===';
const _rustShellBundleUrl =
    'https://github.com/nrbnlulu/flutter/releases/download/BETA/flutter-rust-linux-x64.tar.gz';

final RegExp _rustPath = RegExp(
  r'^(?:r#)?[A-Za-z_][A-Za-z0-9_]*(?:::(?:r#)?[A-Za-z_][A-Za-z0-9_]*)*$',
);

class RustPlugin {
  const RustPlugin({
    required this.dartPackageName,
    required this.cargoPackageName,
    required this.registrar,
    required this.rustDirectory,
  });

  final String dartPackageName;
  final String cargoPackageName;
  final String registrar;
  final Directory rustDirectory;

  String get dependencyAlias => 'flutter_rs_plugin_$dartPackageName';
}

String replaceGeneratedRustDependencies(String manifest, List<String> dependencies) {
  final int begin = manifest.indexOf(rustPluginDependenciesBegin);
  final int end = manifest.indexOf(rustPluginDependenciesEnd);
  if (begin < 0 || end < 0 || begin >= end) {
    throwToolExit(
      'runner-rs/Cargo.toml must contain one valid Flutter generated Rust plugins section.',
    );
  }
  if (manifest.indexOf(rustPluginDependenciesBegin, begin + 1) >= 0 ||
      manifest.indexOf(rustPluginDependenciesEnd, end + 1) >= 0) {
    throwToolExit(
      'runner-rs/Cargo.toml contains more than one Flutter generated Rust plugins section.',
    );
  }
  final int contentStart = begin + rustPluginDependenciesBegin.length;
  final generated = dependencies.isEmpty ? '\n' : '\n${dependencies.join('\n')}\n';
  return manifest.replaceRange(contentStart, end, generated);
}

Future<void> refreshRustPlugins(
  FlutterProject project, {
  required bool releaseMode,
  PackageGraph? packageGraph,
  PackageConfig? packageConfig,
}) async {
  if (!project.manifest.usesRustShell) {
    return;
  }
  final Directory runner = project.directory.childDirectory('runner-rs');
  final File cargoManifest = runner.childFile('Cargo.toml');
  if (!cargoManifest.existsSync()) {
    throwToolExit('Rust-shell project is missing ${cargoManifest.path}.');
  }

  await _ensureRustShellSdk(project);

  final bool useSharedResources =
      packageGraph != null && packageGraph.dependencies.containsKey(project.manifest.appName);
  final File packageConfigFile = findPackageConfigFileOrDefault(project.directory);
  final PackageConfig resolvedPackageConfig =
      (useSharedResources ? packageConfig : null) ??
      await loadPackageConfigWithLogging(packageConfigFile, logger: globals.logger);
  final List<Dependency> dependencies = computeTransitiveDependencies(
    project,
    resolvedPackageConfig,
    packageGraph: useSharedResources ? packageGraph : null,
  );
  final plugins = <RustPlugin>[];
  for (final dependency in dependencies) {
    if (dependency.name == project.manifest.appName ||
        (releaseMode && dependency.isExclusiveDevDependency)) {
      continue;
    }
    final Directory rustDirectory = globals.fs.directory(dependency.rootUri.resolve('rust/'));
    final File manifest = rustDirectory.childFile('Cargo.toml');
    if (!manifest.existsSync()) {
      continue;
    }
    final RustPlugin? plugin = await _readRustPlugin(dependency.name, rustDirectory, manifest);
    if (plugin != null) {
      plugins.add(plugin);
    }
  }
  plugins.sort(
    (RustPlugin left, RustPlugin right) => left.dartPackageName.compareTo(right.dartPackageName),
  );

  final Directory links = project.directory
      .childDirectory('.dart_tool')
      .childDirectory('flutter_rs')
      .childDirectory('plugins');
  links.createSync(recursive: true);
  final keep = <String>{};
  for (final plugin in plugins) {
    keep.add(plugin.dartPackageName);
    _replaceLink(links.childLink(plugin.dartPackageName), plugin.rustDirectory.path);
  }
  for (final FileSystemEntity entity in links.listSync(followLinks: false)) {
    if (!keep.contains(entity.basename)) {
      ErrorHandlingFileSystem.deleteIfExists(entity, recursive: true);
    }
  }

  final dependencyLines = <String>[
    for (final RustPlugin plugin in plugins)
      '${plugin.dependencyAlias} = { package = "${plugin.cargoPackageName}", path = "../.dart_tool/flutter_rs/plugins/${plugin.dartPackageName}" }',
  ];
  final String oldManifest = cargoManifest.readAsStringSync();
  final String newManifest = replaceGeneratedRustDependencies(oldManifest, dependencyLines);
  if (newManifest != oldManifest) {
    cargoManifest.writeAsStringSync(newManifest);
  }

  final File registrant = runner.childDirectory('src').childFile('flutter_plugins.rs');
  final buffer = StringBuffer()
    ..writeln('// Generated by Flutter. Do not edit.')
    ..writeln()
    ..writeln('use flutter_plugin_sdk::{PluginRegistrar, Result};')
    ..writeln()
    ..writeln('pub(crate) fn register_plugins(registrar: &mut PluginRegistrar) -> Result<()> {');
  if (plugins.isEmpty) {
    buffer.writeln('    let _ = registrar;');
  } else {
    for (final plugin in plugins) {
      buffer.writeln('    ${plugin.dependencyAlias}::${plugin.registrar}(registrar)?;');
    }
  }
  buffer
    ..writeln('    Ok(())')
    ..writeln('}');
  final generatedRegistrant = buffer.toString();
  if (!registrant.existsSync() || registrant.readAsStringSync() != generatedRegistrant) {
    registrant.createSync(recursive: true);
    registrant.writeAsStringSync(generatedRegistrant);
  }
  if (plugins.isNotEmpty) {
    await _validatePluginSdkResolution(cargoManifest, runner);
  }
}

Future<void> _validatePluginSdkResolution(File manifest, Directory runner) async {
  final ProcessResult result = await globals.processManager.run(<String>[
    'cargo',
    'metadata',
    '--format-version',
    '1',
    '--manifest-path',
    manifest.path,
  ], workingDirectory: runner.path);
  if (result.exitCode != 0) {
    throwToolExit('Unable to resolve Rust-shell dependencies:\n${result.stderr}');
  }
  final metadata = json.decode(result.stdout as String) as Map<String, Object?>;
  final List<Map<String, Object?>> packages = (metadata['packages']! as List<Object?>)
      .cast<Map<String, Object?>>();
  final Iterable<Map<String, Object?>> sdkPackages = packages.where(
    (Map<String, Object?> package) => package['name'] == 'flutter-plugin-sdk',
  );
  if (sdkPackages.length != 1) {
    throwToolExit(
      'Rust-shell applications must resolve exactly one flutter-plugin-sdk package; '
      'Cargo resolved ${sdkPackages.length}.',
    );
  }
}

Future<RustPlugin?> _readRustPlugin(
  String dartPackageName,
  Directory rustDirectory,
  File manifest,
) async {
  if (!globals.processManager.canRun('cargo')) {
    throwToolExit('Cargo is required to resolve Rust-shell plugins. Install Rust and try again.');
  }
  final ProcessResult result = await globals.processManager.run(<String>[
    'cargo',
    'metadata',
    '--no-deps',
    '--format-version',
    '1',
    '--manifest-path',
    manifest.path,
  ]);
  if (result.exitCode != 0) {
    throwToolExit('Unable to read ${manifest.path}:\n${result.stderr}');
  }
  final metadata = json.decode(result.stdout as String) as Map<String, Object?>;
  final List<Map<String, Object?>> packages = (metadata['packages']! as List<Object?>)
      .cast<Map<String, Object?>>();
  final String manifestPath = globals.fs.path.canonicalize(manifest.path);
  final Map<String, Object?> package = packages.firstWhere(
    (Map<String, Object?> value) =>
        globals.fs.path.canonicalize(value['manifest_path']! as String) == manifestPath,
    orElse: () => throwToolExit('Cargo metadata did not describe ${manifest.path}.'),
  );
  final flutterMetadata =
      (package['metadata'] as Map<String, Object?>?)?['flutter'] as Map<String, Object?>?;
  if (flutterMetadata?['plugin'] != true) {
    return null;
  }
  final Object? registrarValue = flutterMetadata?['registrar'];
  if (registrarValue is! String || !_rustPath.hasMatch(registrarValue)) {
    throwToolExit(
      'Rust plugin $dartPackageName must provide a valid '
      '[package.metadata.flutter] registrar.',
    );
  }
  return RustPlugin(
    dartPackageName: dartPackageName,
    cargoPackageName: package['name']! as String,
    registrar: registrarValue,
    rustDirectory: rustDirectory,
  );
}

Future<void> _ensureRustShellSdk(FlutterProject project) async {
  final Directory sdk = project.directory
      .childDirectory('.dart_tool')
      .childDirectory('flutter_rs')
      .childDirectory('sdk');
  final File sdkManifest = sdk
      .childDirectory('crates')
      .childDirectory('flutter-plugin-sdk')
      .childFile('Cargo.toml');
  final File sdkWorkspaceManifest = sdk.childFile('Cargo.toml');
  final File engineLibrary = sdk.childDirectory('lib').childFile('libflutter_rust_engine.so');
  if (sdkWorkspaceManifest.existsSync() && sdkManifest.existsSync() && engineLibrary.existsSync()) {
    return;
  }

  final Directory localShell = globals.fs.directory(
    globals.fs.path.join(
      Cache.flutterRoot!,
      'engine',
      'src',
      'flutter',
      'shell',
      'platform',
      'rust',
    ),
  );
  if (localShell.childDirectory('crates').existsSync()) {
    sdk.createSync(recursive: true);
    localShell.childFile('Cargo.toml').copySync(sdkWorkspaceManifest.path);
    localShell.childFile('Cargo.lock').copySync(sdk.childFile('Cargo.lock').path);
    _replaceLink(sdk.childLink('crates'), localShell.childDirectory('crates').path);
    _replaceLink(sdk.childLink('third_party'), localShell.childDirectory('third_party').path);
    final Directory localEngine = globals.fs.directory(
      globals.fs.path.join(Cache.flutterRoot!, 'engine', 'src', 'out', 'host_debug'),
    );
    if (localEngine.childFile('libflutter_rust_engine.so').existsSync()) {
      sdk.childDirectory('lib').createSync(recursive: true);
      _replaceLink(
        sdk.childDirectory('lib').childLink('libflutter_rust_engine.so'),
        localEngine.childFile('libflutter_rust_engine.so').path,
      );
      _replaceLink(
        sdk.childDirectory('lib').childLink('icudtl.dat'),
        localEngine.childFile('icudtl.dat').path,
      );
      return;
    }
  }

  if (!globals.platform.isLinux) {
    throwToolExit('The BETA Rust-shell SDK currently provides Linux x64 artifacts only.');
  }
  globals.logger.printStatus('Downloading the Flutter Rust shell BETA SDK...');
  final File archive = project.directory
      .childDirectory('.dart_tool')
      .childDirectory('flutter_rs')
      .childFile('flutter-rust-sdk.tar.gz');
  archive.parent.createSync(recursive: true);
  final net = Net(
    httpClientFactory: globals.httpClientFactory,
    logger: globals.logger,
    platform: globals.platform,
  );
  final List<int>? result = await net.fetchUrl(
    Uri.parse(_rustShellBundleUrl),
    maxAttempts: 3,
    destFile: archive,
  );
  if (result == null) {
    throwToolExit('Unable to download the Flutter Rust shell BETA SDK.');
  }
  if (sdk.existsSync()) {
    ErrorHandlingFileSystem.deleteIfExists(sdk, recursive: true);
  }
  sdk.createSync(recursive: true);
  globals.os.unpack(archive, sdk);
  ErrorHandlingFileSystem.deleteIfExists(archive);
  if (!sdkWorkspaceManifest.existsSync() ||
      !sdkManifest.existsSync() ||
      !engineLibrary.existsSync()) {
    throwToolExit('The Flutter Rust shell BETA SDK archive is incomplete.');
  }
}

void _replaceLink(Link link, String target) {
  final FileSystemEntityType type = link.fileSystem.typeSync(link.path, followLinks: false);
  if (type == FileSystemEntityType.link) {
    try {
      if (link.targetSync() == target && link.existsSync()) {
        return;
      }
    } on FileSystemException {
      // Replace broken links below.
    }
  }
  if (type != FileSystemEntityType.notFound) {
    ErrorHandlingFileSystem.deleteIfExists(link, recursive: true);
  }
  link.createSync(target, recursive: true);
}
