// Copyright 2014 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ignore_for_file: invalid_use_of_internal_member

import 'dart:ui' show Size;

import 'package:flutter/rendering.dart' show BoxConstraints;
import 'package:flutter/src/widgets/_window.dart' show WindowController, WindowControllerDelegate;
import 'package:flutter/src/widgets/_window_rust.dart'
    show RustRegularWindowBindings, RustWindowState, WindowingOwnerRust;
import 'package:flutter_test/flutter_test.dart';

import 'multi_view_testing.dart';

class _FakeRegularWindowBindings implements RustRegularWindowBindings {
  int createdEngineId = -1;
  Size? createdSize;
  BoxConstraints? createdConstraints;
  String? createdTitle;
  bool? createdResizable;
  final List<(int, int)> destroyed = <(int, int)>[];
  final List<String> operations = <String>[];

  RustWindowState state = (
    width: 640,
    height: 480,
    focused: 1,
    maximized: 0,
    minimized: 0,
    fullscreen: 0,
  );

  @override
  int createRegularWindow({
    required int engineId,
    required Size? size,
    required BoxConstraints? constraints,
    required String title,
    required bool resizable,
  }) {
    createdEngineId = engineId;
    createdSize = size;
    createdConstraints = constraints;
    createdTitle = title;
    createdResizable = resizable;
    return 41;
  }

  @override
  RustWindowState getWindowState(int engineId, int viewId) => state;

  @override
  void destroyWindow(int engineId, int viewId) {
    destroyed.add((engineId, viewId));
  }

  @override
  void activate(int engineId, int viewId) {
    operations.add('activate:$engineId:$viewId');
  }

  @override
  void setConstraints(int engineId, int viewId, BoxConstraints constraints) {
    operations.add('constraints:$engineId:$viewId:$constraints');
  }

  @override
  void setFullscreen(int engineId, int viewId, bool fullscreen) {
    operations.add('fullscreen:$engineId:$viewId:$fullscreen');
  }

  @override
  void setMaximized(int engineId, int viewId, bool maximized) {
    operations.add('maximized:$engineId:$viewId:$maximized');
  }

  @override
  void setMinimized(int engineId, int viewId, bool minimized) {
    operations.add('minimized:$engineId:$viewId:$minimized');
  }

  @override
  void setSize(int engineId, int viewId, double width, double height) {
    operations.add('size:$engineId:$viewId:$width:$height');
  }

  @override
  void setTitle(int engineId, int viewId, String title) {
    operations.add('title:$engineId:$viewId:$title');
  }
}

class _RecordingDelegate extends WindowControllerDelegate {
  int closeRequests = 0;
  int destructions = 0;
  bool acceptClose = false;

  @override
  void onWindowCloseRequested(WindowController controller) {
    closeRequests += 1;
    if (acceptClose) {
      controller.destroy();
    }
  }

  @override
  void onWindowDestroyed() {
    destructions += 1;
  }
}

void main() {
  testWidgets('regular window forwards creation, state, and mutations to the Rust host', (
    WidgetTester tester,
  ) async {
    final bindings = _FakeRegularWindowBindings();
    final owner = WindowingOwnerRust.test(
      regularWindowing: bindings,
      engineId: 7,
      viewForId: (int viewId) => FakeView(tester.view, viewId: viewId),
    );
    const size = Size(320, 240);
    const constraints = BoxConstraints(minWidth: 100, maxWidth: 800);

    final WindowController controller = owner.createWindowController(
      delegate: _RecordingDelegate(),
      size: size,
      constraints: constraints,
      resizable: true,
      title: 'Typed regular',
    );

    expect(bindings.createdEngineId, 7);
    expect(bindings.createdSize, size);
    expect(bindings.createdConstraints, constraints);
    expect(bindings.createdTitle, 'Typed regular');
    expect(bindings.createdResizable, isTrue);
    expect(controller.rootView.viewId, 41);
    expect(controller.contentSize, const Size(640, 480));
    expect(controller.isActivated, isTrue);
    expect(controller.isMaximized, isFalse);
    expect(controller.isMinimized, isFalse);
    expect(controller.isFullscreen, isFalse);

    controller.setSize(const Size(500, 300));
    controller.setConstraints(const BoxConstraints.tightFor(width: 500, height: 300));
    controller.setTitle('Renamed');
    controller.activate();
    controller.setMaximized(true);
    controller.setMinimized(true);
    controller.setFullscreen(true);

    expect(bindings.operations, <String>[
      'size:7:41:500.0:300.0',
      'constraints:7:41:BoxConstraints(w=500.0, h=300.0)',
      'title:7:41:Renamed',
      'activate:7:41',
      'maximized:7:41:true',
      'minimized:7:41:true',
      'fullscreen:7:41:true',
    ]);
  });

  testWidgets('delegated close waits for asynchronous host destruction', (
    WidgetTester tester,
  ) async {
    final bindings = _FakeRegularWindowBindings();
    final delegate = _RecordingDelegate();
    final owner = WindowingOwnerRust.test(
      regularWindowing: bindings,
      engineId: 7,
      viewForId: (int viewId) => FakeView(tester.view, viewId: viewId),
    );
    final WindowController controller = owner.createWindowController(
      delegate: delegate,
      resizable: true,
    );
    var notifications = 0;
    controller.addListener(() => notifications += 1);

    owner.handleWindowEventForTesting(41, 0);
    expect(notifications, 1);

    // A delegate may veto a close request by leaving the controller alive.
    owner.handleWindowEventForTesting(41, 1);
    expect(delegate.closeRequests, 1);
    expect(bindings.destroyed, isEmpty);
    expect(controller.isDestroyed, isFalse);

    delegate.acceptClose = true;
    owner.handleWindowEventForTesting(41, 1);
    expect(delegate.closeRequests, 2);
    expect(bindings.destroyed, <(int, int)>[(7, 41)]);
    expect(controller.isDestroyed, isFalse);

    // Once removal is pending, duplicate native close requests are coalesced.
    owner.handleWindowEventForTesting(41, 1);
    expect(delegate.closeRequests, 2);
    expect(bindings.destroyed, hasLength(1));

    owner.handleWindowEventForTesting(41, 2);
    expect(controller.isDestroyed, isTrue);
    expect(delegate.destructions, 1);
    expect(notifications, 2);

    // The owner removes the controller after completion, so duplicate host
    // notifications have no effect.
    owner.handleWindowEventForTesting(41, 2);
    expect(delegate.destructions, 1);
    expect(notifications, 2);
    expect(() => controller.contentSize, throwsStateError);
  });
}
