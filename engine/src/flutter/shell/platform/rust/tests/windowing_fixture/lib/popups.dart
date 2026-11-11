// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ignore_for_file: implementation_imports, invalid_use_of_internal_member

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:flutter/src/widgets/_window_positioner.dart';
import 'package:flutter/widgets.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runWidget(const _WindowingFixture());
}

class _WindowingFixture extends StatefulWidget {
  const _WindowingFixture();

  @override
  State<_WindowingFixture> createState() => _WindowingFixtureState();
}

class _WindowingFixtureState extends State<_WindowingFixture> with SingleTickerProviderStateMixin {
  late final AnimationController animation = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 400),
  )..repeat();
  final List<BaseWindowController> windows = <BaseWindowController>[];
  int destroyed = 0;

  @override
  void initState() {
    super.initState();
    animation.addListener(() => setState(() {}));
    unawaited(exerciseWindows());
  }

  Future<void> exerciseWindows() async {
    final status = File(Platform.environment['FLUTTER_RUST_POPUP_STATUS']!);
    var dismissed = 0;
    void report(int cycle, String phase) => status.writeAsStringSync(
      jsonEncode(<String, Object>{'cycle': cycle, 'phase': phase, 'dismissed': dismissed}),
    );
    await Future<void>.delayed(const Duration(milliseconds: 500));
    for (var cycle = 0; cycle < 20; cycle++) {
      final parent = WindowController(
        title: 'Popup parent $cycle',
        size: const Size(360, 240),
        delegate: _FixtureDelegate(() => destroyed++),
      );
      final other = WindowController(
        title: 'Popup other $cycle',
        size: const Size(260, 180),
        delegate: _FixtureDelegate(() => destroyed++),
      );
      setState(() => windows.addAll([parent, other]));
      parent.activate();
      await Future<void>.delayed(const Duration(milliseconds: 100));
      late final PopupWindowController popup;
      popup = PopupWindowController(
        parent: parent,
        anchorRect: const Rect.fromLTWH(30, 30, 20, 20),
        positioner: const WindowPositioner(),
        constraints: const BoxConstraints.tightFor(width: 160, height: 100),
        delegate: _PopupDelegate(() {
          destroyed++;
          dismissed++;
          if (mounted) {
            setState(() => windows.remove(popup));
          }
          report(cycle, 'dismissed');
        }),
      );
      setState(() => windows.add(popup));
      await Future<void>.delayed(const Duration(milliseconds: 200));
      report(cycle, 'ready');
      if (cycle < 3) {
        final DateTime deadline = DateTime.now().add(const Duration(seconds: 15));
        while (!popup.isDestroyed && DateTime.now().isBefore(deadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        if (!popup.isDestroyed) {
          throw StateError('Native popup dismissal timed out');
        }
      }
      final retiring = List<BaseWindowController>.of(windows);
      setState(windows.clear);
      await WidgetsBinding.instance.endOfFrame;
      parent.destroy();
      other.destroy();
      final DateTime deadline = DateTime.now().add(const Duration(seconds: 5));
      while (destroyed != (cycle + 1) * 3 && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      if (destroyed != (cycle + 1) * 3) {
        throw StateError('Popup descendants were not released');
      }
      for (final controller in retiring) {
        controller.dispose();
      }
      if (!retiring.contains(popup)) {
        popup.dispose();
      }
    }
    report(20, 'complete');
    File(Platform.environment['FLUTTER_RUST_WINDOWING_STATUS']!)
        .writeAsStringSync('complete released=$destroyed');
    await ServicesBinding.instance.exitApplication(ui.AppExitType.required);
  }

  @override
  Widget build(BuildContext context) => ViewCollection(
    views: <Widget>[
      View(view: ui.PlatformDispatcher.instance.implicitView!, child: content()),
      for (final BaseWindowController controller in windows)
        View(
          key: ValueKey<int>(controller.rootView.viewId),
          view: controller.rootView,
          child: content(),
        ),
    ],
  );

  Widget content() => ColoredBox(
    color: Color.lerp(const Color(0xff204060), const Color(0xff80c0e0), animation.value)!,
    child: const Directionality(
      textDirection: TextDirection.ltr,
      child: Center(child: Text('Rust multi-view lifecycle', style: TextStyle(fontSize: 16))),
    ),
  );

  @override
  void dispose() {
    animation.dispose();
    super.dispose();
  }
}

class _FixtureDelegate with WindowControllerDelegate {
  _FixtureDelegate(this.released);
  final VoidCallback released;

  @override
  void onWindowDestroyed() => released();
}

class _PopupDelegate with PopupWindowControllerDelegate {
  _PopupDelegate(this.released);
  final VoidCallback released;
  @override
  void onWindowDestroyed() => released();
}
