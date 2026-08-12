// Copyright 2014 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'package:flutter/material.dart';

// The Rust-shell integration fixture creates the first engine texture before
// this entry point's first frame. Its engine-generated registry ID is verified
// independently by the native test harness before screenshots are compared.
const int _rustPluginTextureId = 1;

void main() {
  runApp(const RustShellTextureFixture());
}

class RustShellTextureFixture extends StatelessWidget {
  const RustShellTextureFixture({super.key});

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      debugShowCheckedModeBanner: false,
      home: ColoredBox(
        color: Colors.black,
        child: Center(
          child: SizedBox.square(
            dimension: 256,
            child: Texture(textureId: _rustPluginTextureId),
          ),
        ),
      ),
    );
  }
}
