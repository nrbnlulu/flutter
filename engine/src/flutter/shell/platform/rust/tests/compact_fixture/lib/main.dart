// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
// ignore_for_file: implementation_imports, invalid_use_of_internal_member
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:flutter_rust_agent/flutter_rust_agent.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  FlutterRustAgent.install();
  runWidget(const CompactFixture());
}

class CompactFixture extends StatefulWidget {
  const CompactFixture({super.key});

  @override
  State<CompactFixture> createState() => _CompactFixtureState();
}

class _CompactFixtureState extends State<CompactFixture> {
  late final WindowController listWindow = WindowController(
    title: 'Compact list',
    size: const Size(700, 500),
  );
  late final WindowController paintWindow = WindowController(
    title: 'Compact paint',
    size: const Size(440, 360),
  );
  int scrolls = 0;
  bool orange = false;

  @override
  void initState() {
    super.initState();
    listWindow;
    paintWindow;
  }

  @override
  Widget build(BuildContext context) => ViewCollection(
    views: [
      View(
        view: ui.PlatformDispatcher.instance.implicitView!,
        child: const ColoredBox(color: Color(0xff102030)),
      ),
      View(
        view: listWindow.rootView,
        child: MaterialApp(
          home: Scaffold(
            body: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                children: [
                  const Text('Long list: 160 lazily built rows'),
                  Semantics(
                    identifier: 'scroll_count',
                    value: '$scrolls',
                    child: const ExcludeSemantics(child: Text('Completed scroll actions')),
                  ),
                  Expanded(
                    child: NotificationListener<ScrollEndNotification>(
                      onNotification: (_) {
                        setState(() => scrolls++);
                        return false;
                      },
                      child: Semantics(
                        identifier: 'long_list',
                        child: ListView.builder(
                          // Semantics names content movement: reverse makes
                          // scrollDown the supported forward action at offset 0.
                          reverse: true,
                          cacheExtent: 0,
                          itemExtent: 48,
                          itemCount: 160,
                          itemBuilder: (context, index) => Semantics(
                            identifier: 'item_$index',
                            child: Text('List row $index — שלום 東京'),
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
      View(
        view: paintWindow.rootView,
        child: MaterialApp(
          home: Scaffold(
            body: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                children: [
                  Semantics(
                    identifier: 'change_paint',
                    child: ElevatedButton(
                      onPressed: () => setState(() => orange = !orange),
                      child: const Text('Change paint color'),
                    ),
                  ),
                  Semantics(
                    identifier: 'paint_color',
                    value: orange ? 'orange' : 'blue',
                    child: const ExcludeSemantics(child: Text('Paint state')),
                  ),
                  Expanded(
                    child: CustomPaint(
                      painter: _CanvasPainter(orange),
                      child: const SizedBox.expand(),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    ],
  );

  @override
  void dispose() {
    listWindow.dispose();
    paintWindow.dispose();
    super.dispose();
  }
}

class _CanvasPainter extends CustomPainter {
  _CanvasPainter(this.orange);
  final bool orange;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(
      Offset.zero & size,
      Paint()..color = orange ? const Color(0xffff8800) : const Color(0xff0055cc),
    );
    canvas.drawCircle(size.center(Offset.zero), 40, Paint()..color = Colors.white);
    final label = TextPainter(
      text: const TextSpan(
        text: 'PAINT_ONLY_GLYPH',
        style: TextStyle(color: Colors.black, fontSize: 18),
      ),
      textDirection: TextDirection.ltr,
    )..layout(maxWidth: size.width);
    label.paint(canvas, const Offset(8, 8));
  }

  @override
  bool shouldRepaint(_CanvasPainter oldDelegate) => oldDelegate.orange != orange;
}
