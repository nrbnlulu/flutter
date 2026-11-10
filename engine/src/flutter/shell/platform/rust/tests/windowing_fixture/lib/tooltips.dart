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
    for (var cycle = 0; cycle < 12; cycle++) {
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
      // Wayland configures a popup only after its parent has been mapped.
      // Let Flutter render the parent before synchronously creating the popup.
      setState(() => windows.addAll([parent, other, dialog, satellite]));
      await WidgetsBinding.instance.endOfFrame;
      final String? readyPath = Platform.environment['FLUTTER_RUST_WINDOWING_PARENT_READY'];
      if (readyPath != null) {
        final ready = File(readyPath);
        final DateTime deadline = DateTime.now().add(const Duration(seconds: 10));
        while (!ready.existsSync() || ready.readAsStringSync() != '$cycle') {
          if (DateTime.now().isAfter(deadline)) {
            throw StateError('Tooltip parent $cycle did not map');
          }
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
      }
      final tooltip = TooltipWindowController(
        parent: parent,
        anchorRect: const Rect.fromLTWH(30, 30, 20, 20),
        positioner: const WindowPositioner(),
        constraints: const BoxConstraints.tightFor(width: 160, height: 100),
        delegate: _TooltipDelegate(() => destroyed++),
      );
      setState(() => windows.add(tooltip));
      final String? motionPath = Platform.environment['FLUTTER_RUST_TOOLTIP_MOTION'];
      if (cycle == 0 && motionPath != null) {
        final signal = File(motionPath);
        final DateTime deadline = DateTime.now().add(const Duration(seconds: 15));
        while (!signal.existsSync() && DateTime.now().isBefore(deadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        if (!signal.existsSync()) {
          throw StateError('Tooltip motion harness timed out');
        }
        tooltip.setConstraints(const BoxConstraints.tightFor(width: 200, height: 120));
        signal.writeAsStringSync('resized');
        while (signal.readAsStringSync() != 'anchor' && DateTime.now().isBefore(deadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        tooltip.updatePosition(
          anchorRect: const Rect.fromLTWH(50, 45, 20, 20),
          positioner: const WindowPositioner(offset: Offset(7, 9)),
        );
        signal.writeAsStringSync('anchored');
        while (signal.readAsStringSync() != 'partial' && DateTime.now().isBefore(deadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        tooltip.updatePosition(positioner: const WindowPositioner(offset: Offset(-3, 2)));
        signal.writeAsStringSync('partial complete');
        while (signal.readAsStringSync() != 'complete' && DateTime.now().isBefore(deadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        if (signal.readAsStringSync() != 'complete') {
          throw StateError('Tooltip size check timed out');
        }
      }
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
      final int expected = (cycle + 1) * 5;
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
    File(
      Platform.environment['FLUTTER_RUST_WINDOWING_STATUS']!,
    ).writeAsStringSync('complete released=$destroyed');
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

class _TooltipDelegate with TooltipWindowControllerDelegate {
  _TooltipDelegate(this.released);
  final VoidCallback released;
  @override
  void onWindowDestroyed() => released();
}
