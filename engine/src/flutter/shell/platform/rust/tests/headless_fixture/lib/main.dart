// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:ui' as ui;
import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

import 'package:flutter/widgets.dart';
import 'package:flutter/scheduler.dart';

void main() {
  print("headless fixture: entering main");
  runApp(const Directionality(textDirection: TextDirection.ltr, child: _AnimatedProbe()));
  print("headless fixture: root attached");
  final exitMs = int.tryParse(Platform.environment['FLUTTER_RUST_HEADLESS_FIXTURE_EXIT_MS'] ?? '');
  if (exitMs != null) {
    unawaited(
      Future<void>.delayed(Duration(milliseconds: exitMs), () async {
        await ServicesBinding.instance.exitApplication(ui.AppExitType.required);
      }),
    );
  }
}

class _AnimatedProbe extends StatefulWidget {
  const _AnimatedProbe();
  @override
  State<_AnimatedProbe> createState() => _AnimatedProbeState();
}

class _AnimatedProbeState extends State<_AnimatedProbe> with SingleTickerProviderStateMixin {
  late final Ticker ticker = createTicker((_) => setState(() => tick++))..start();
  int tick = 0;
  @override
  void initState() {
    super.initState();
    ticker;
  }

  @override
  void dispose() {
    ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Stack(
    fit: StackFit.expand,
    children: [
      CustomPaint(painter: _Probe(tick)),
      const Positioned(left: 320, top: 20, width: 80, height: 80, child: Texture(textureId: 1)),
    ],
  );
}

class _Probe extends CustomPainter {
  const _Probe(this.tick);
  final int tick;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawColor(const Color(0xff112233), BlendMode.src);
    canvas.drawRect(
      Rect.fromLTWH(size.width - 20, 0, 20, 20),
      Paint()..color = tick.isEven ? const Color(0xffff00ff) : const Color(0xff00ffff),
    );
    canvas.drawRect(
      const Rect.fromLTWH(420, 20, 80, 80),
      Paint()
        ..color = const Color(0x80ffffff)
        ..blendMode = BlendMode.src,
    );
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
              ..addText('Headless Vulkan proof: ABCDEFG 0123456789'))
            .build()
          ..layout(const ui.ParagraphConstraints(width: 750));
    canvas.drawParagraph(paragraph, const Offset(20, 420));
  }

  @override
  bool shouldRepaint(_Probe oldDelegate) => oldDelegate.tick != tick;
}
