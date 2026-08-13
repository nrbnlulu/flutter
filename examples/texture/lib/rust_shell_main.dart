// Copyright 2014 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

// The Rust-shell integration fixture creates the first engine texture before
// this entry point's first frame. Its engine-generated registry ID is verified
// independently by the native test harness before screenshots are compared.
const int _rustPluginTextureId = 1;

void main() {
  runApp(const RustShellTextureFixture());
}

class RustShellTextureFixture extends StatefulWidget {
  const RustShellTextureFixture({super.key});

  @override
  State<RustShellTextureFixture> createState() =>
      _RustShellTextureFixtureState();
}

class _RustShellTextureFixtureState extends State<RustShellTextureFixture> {
  static const String channel = 'flutter/rust_texture_fixture';
  int textureId = _rustPluginTextureId;

  @override
  void initState() {
    super.initState();
    final BinaryMessenger messenger =
        ServicesBinding.instance.defaultBinaryMessenger;
    messenger.setMessageHandler(channel, (ByteData? data) async {
      if (data == null) {
        return null;
      }
      final String message = utf8.decode(data.buffer.asUint8List(
        data.offsetInBytes,
        data.lengthInBytes,
      ));
      final int nextId = int.parse(message.split(' ').first);
      if (mounted && nextId != textureId) {
        setState(() => textureId = nextId);
        await WidgetsBinding.instance.endOfFrame;
      }
      await messenger.send(channel, _bytes('ack $nextId'));
      return null;
    });
    messenger.send(channel, _bytes('ready'));
  }

  static ByteData _bytes(String value) {
    final Uint8List bytes = utf8.encode(value);
    return ByteData.sublistView(bytes);
  }

  @override
  void dispose() {
    ServicesBinding.instance.defaultBinaryMessenger
        .setMessageHandler(channel, null);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      home: ColoredBox(
        color: Colors.black,
        child: Center(
          child: SizedBox.square(
            dimension: 256,
            child: Texture(
              key: ValueKey<int>(textureId),
              textureId: textureId,
            ),
          ),
        ),
      ),
    );
  }
}
