// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ignore_for_file: implementation_imports, invalid_use_of_internal_member

import 'dart:async';
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
    await Future<void>.delayed(const Duration(milliseconds: 500));
    for (var cycle = 0; cycle < 15; cycle++) {
      final parent = WindowController(
        title: 'Owned parent $cycle',
        size: const Size(260, 180),
        delegate: _FixtureDelegate(() => destroyed++),
      );
      final other = WindowController(
        title: 'Reparent target $cycle',
        size: const Size(260, 180),
        delegate: _FixtureDelegate(() => destroyed++),
      );
      final dialog = DialogWindowController(
        parent: parent,
        title: 'Owned dialog $cycle',
        size: const Size(240, 160),
        delegate: _DialogDelegate(() => destroyed++),
      );
      final satellite = SatelliteWindowController(
        parent: parent,
        title: 'Owned satellite $cycle',
        size: const Size(220, 140),
        initialPositioner: const WindowPositioner(offset: Offset(30, 40)),
        delegate: _SatelliteDelegate(() => destroyed++),
      );
      setState(() => windows.addAll([parent, other, dialog, satellite]));
      await Future<void>.delayed(const Duration(milliseconds: 200));
      dialog.setSize(const Size(280, 190));
      satellite.setSize(const Size(250, 170));
      satellite.setParent(other);
      if (satellite.parent != other) {
        throw StateError('Satellite parent was not updated');
      }
      await Future<void>.delayed(const Duration(milliseconds: 200));
      satellite.setParent(dialog);
      await Future<void>.delayed(const Duration(milliseconds: 100));
      final retiring = List<BaseWindowController>.of(windows);
      setState(windows.clear);
      await WidgetsBinding.instance.endOfFrame;
      // Destroy only the owners. The dialog and its reparented satellite must
      // be removed by the host's descendant teardown path.
      parent.destroy();
      other.destroy();
      final int expected = (cycle + 1) * 4;
      final DateTime deadline = DateTime.now().add(const Duration(seconds: 5));
      while (destroyed != expected && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      if (destroyed != expected) {
        throw StateError('Only $destroyed of $expected owned views released');
      }
      for (final controller in retiring) {
        controller.dispose();
      }
    }
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

class _DialogDelegate with DialogWindowControllerDelegate {
  _DialogDelegate(this.released);
  final VoidCallback released;
  @override
  void onWindowDestroyed() => released();
}

class _SatelliteDelegate with SatelliteWindowControllerDelegate {
  _SatelliteDelegate(this.released);
  final VoidCallback released;
  @override
  void onWindowDestroyed() => released();
}
