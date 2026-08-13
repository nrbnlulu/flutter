// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart' show ExternalLibrary;

import 'src/rust/api.dart';
import 'src/rust/frb_generated.dart';

const String _backgroundIsolateName = 'flutter-rust-frb-background';

Future<Map<String, Object?>> _runBackgroundProbe() async {
  await RustLib.init(externalLibrary: ExternalLibrary.process(iKnowHowToUseIt: true));
  final DispatchProbe probe = await backgroundDispatchProbe();
  final TextureProbe textures = textureProbe();
  return <String, Object?>{
    'isolateName': Isolate.current.debugName,
    'callerWasMainThread': probe.callerWasMainThread,
    'callbackWasMainThread': probe.callbackWasMainThread,
    'synchronousCallbackWasDeferred': probe.synchronousCallbackWasDeferred,
    'gpuTextureId': textures.gpuTextureId,
    'pixelTextureId': textures.pixelTextureId,
  };
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final String? statusPath = Platform.environment['FLUTTER_RUST_FRB_STATUS'];
  if (statusPath == null) {
    throw StateError('FLUTTER_RUST_FRB_STATUS must be set');
  }

  Map<String, Object?> status;
  TextureProbe? textures;
  try {
    await RustLib.init(externalLibrary: ExternalLibrary.process(iKnowHowToUseIt: true));
    final bool synchronousCallerWasMainThread = synchronousProbe();
    final Map<String, Object?> background = await Isolate.run(
      _runBackgroundProbe,
      debugName: _backgroundIsolateName,
    );
    textures = textureProbe();
    status = <String, Object?>{
      'synchronousCallerWasMainThread': synchronousCallerWasMainThread,
      'background': background,
      'gpuTextureId': textures.gpuTextureId,
      'pixelTextureId': textures.pixelTextureId,
    };
  } catch (error, stackTrace) {
    status = <String, Object?>{'error': error.toString(), 'stackTrace': stackTrace.toString()};
  }

  final File statusFile = File(statusPath);
  statusFile.writeAsStringSync(jsonEncode(status));
  runApp(FrbFixtureApp(status: status, textures: textures, statusFile: statusFile));
}

class FrbFixtureApp extends StatefulWidget {
  const FrbFixtureApp({
    required this.status,
    required this.textures,
    required this.statusFile,
    super.key,
  });

  final Map<String, Object?> status;
  final TextureProbe? textures;
  final File statusFile;

  @override
  State<FrbFixtureApp> createState() => _FrbFixtureAppState();
}

class _FrbFixtureAppState extends State<FrbFixtureApp> {
  Timer? _statusTimer;

  @override
  void initState() {
    super.initState();
    if (widget.textures != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        startTextureProducer();
      });
      _statusTimer = Timer.periodic(const Duration(milliseconds: 100), (_) {
        final TextureProbe current = textureProbe();
        final Map<String, Object?> currentStatus = <String, Object?>{
          ...widget.status,
          'gpuFrames': current.gpuFrames.toInt(),
          'pixelFrames': current.pixelFrames.toInt(),
        };
        widget.statusFile.writeAsStringSync(jsonEncode(currentStatus));
      });
    }
  }

  @override
  void dispose() {
    _statusTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final bool passed = !widget.status.containsKey('error') && widget.textures != null;
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      home: passed
          ? Row(
              children: <Widget>[
                Expanded(child: Texture(textureId: widget.textures!.gpuTextureId)),
                Expanded(child: Texture(textureId: widget.textures!.pixelTextureId)),
              ],
            )
          : const ColoredBox(color: Colors.red),
    );
  }
}
