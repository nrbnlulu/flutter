// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';

void main() => runApp(
  const Directionality(
    textDirection: TextDirection.ltr,
    child: CustomPaint(painter: _Probe()),
  ),
);

class _Probe extends CustomPainter {
  const _Probe();

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawColor(const Color(0xff112233), BlendMode.src);
    const colors = <Color>[
      Color(0xffff0000),
      Color(0xff00ff00),
      Color(0xff0000ff),
      Color(0xffffffff),
      Color(0xff00ffff),
      Color(0xffff00ff),
    ];
    for (var i = 0; i < colors.length; i++) {
      canvas.drawRect(Rect.fromLTWH(20, 20 + i * 60, 240, 40), Paint()..color = colors[i]);
    }
    final paragraph =
        (ui.ParagraphBuilder(ui.ParagraphStyle(fontSize: 24))
              ..pushStyle(ui.TextStyle(color: const Color(0xffffffff)))
              ..addText('Flutter Rust Windows: ABCDEFG 0123456789'))
            .build()
          ..layout(const ui.ParagraphConstraints(width: 750));
    canvas.drawParagraph(paragraph, const Offset(20, 420));
  }

  @override
  bool shouldRepaint(_Probe oldDelegate) => false;
}
