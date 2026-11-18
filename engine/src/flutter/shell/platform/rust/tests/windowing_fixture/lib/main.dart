// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ignore_for_file: implementation_imports, invalid_use_of_internal_member

import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
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
  final List<WindowController> windows = <WindowController>[];
  int destroyed = 0;

  @override
  void initState() {
    super.initState();
    animation.addListener(() => setState(() {}));
    unawaited(exerciseWindows());
  }

  Future<void> exerciseWindows() async {
    // Allow the primary view to start submitting before new surfaces appear.
    await Future<void>.delayed(const Duration(milliseconds: 500));
    for (var cycle = 0; cycle < 20; cycle += 1) {
      for (var index = 0; index < 3; index += 1) {
        final controller = WindowController(
          title: 'Rust GPU lifecycle $cycle/$index',
          size: const Size(240, 160),
          delegate: _FixtureDelegate(() => destroyed += 1),
        );
        setState(() => windows.add(controller));
        await Future<void>.delayed(const Duration(milliseconds: 40));
      }
      for (var resize = 0; resize < 4; resize += 1) {
        for (final WindowController controller in windows) {
          controller.setSize(Size(240 + resize * 17, 160 + resize * 13));
        }
        await Future<void>.delayed(const Duration(milliseconds: 40));
      }
      final retiring = List<WindowController>.of(windows);
      // Unmount each View before detaching its engine view.
      setState(windows.clear);
      await WidgetsBinding.instance.endOfFrame;
      for (final WindowController controller in retiring.reversed) {
        controller.destroy();
      }
      final int expected = (cycle + 1) * 3;
      final DateTime deadline = DateTime.now().add(const Duration(seconds: 5));
      while (destroyed != expected && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      if (destroyed != expected) {
        throw StateError('Only $destroyed of $expected native windows were released');
      }
      for (final controller in retiring) {
        controller.dispose();
      }
      File(
        Platform.environment['FLUTTER_RUST_WINDOWING_STATUS']!,
      ).writeAsStringSync('cycle=${cycle + 1} released=$destroyed');
    }
    // Keep additional rendered views alive through whole-shell teardown.
    for (var index = 0; index < 3; index += 1) {
      setState(
        () => windows.add(
          WindowController(
            title: 'Rust teardown $index',
            size: const Size(240, 160),
            delegate: _FixtureDelegate(() {}),
          ),
        ),
      );
    }
    await Future<void>.delayed(const Duration(milliseconds: 300));
    File(
      Platform.environment['FLUTTER_RUST_WINDOWING_STATUS']!,
    ).writeAsStringSync('complete released=$destroyed');
    await ServicesBinding.instance.exitApplication(ui.AppExitType.required);
  }

  @override
  Widget build(BuildContext context) => ViewCollection(
    views: <Widget>[
      View(view: ui.PlatformDispatcher.instance.implicitView!, child: content()),
      for (final WindowController controller in windows)
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
