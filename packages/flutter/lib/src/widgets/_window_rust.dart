// Copyright 2014 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:convert';
import 'dart:ffi' as ffi;
import 'dart:io';
import 'dart:ui' show Display, FlutterView, Offset, Rect, Size;

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show BoxConstraints;

import '_window.dart';
import '_window_positioner.dart';
import 'binding.dart';

const String _createRegularSymbol = 'FlutterRustShellWindowCreateRegular';
const String _createDialogSymbol = 'FlutterRustShellWindowCreateDialog';

/// Whether this process is hosted by Flutter's Rust shell.
@internal
bool get isRustShellWindowingAvailable {
  return Platform.isLinux && ffi.DynamicLibrary.process().providesSymbol(_createRegularSymbol);
}

@internal
class WindowingOwnerRust extends WindowingOwner {
  @internal
  WindowingOwnerRust() {
    if (!isRustShellWindowingAvailable) {
      throw UnsupportedError('The Flutter Rust windowing backend is unavailable.');
    }
    // Winit events are delivered outside a Dart invocation even though the
    // host and isolate share a thread. A listener callable safely schedules
    // the event into the owning isolate instead of re-entering it directly.
    _eventCallback = ffi.NativeCallable<_WindowEventNative>.listener(_handleWindowEvent);
    _RustWindowing.setEventCallback(_engineId, _eventCallback.nativeFunction);
  }

  final Map<int, _RustLifecycleController> _controllers = <int, _RustLifecycleController>{};
  late final ffi.NativeCallable<_WindowEventNative> _eventCallback;

  int get _engineId => WidgetsBinding.instance.platformDispatcher.engineId!;

  @override
  WindowController createWindowController({
    required WindowControllerDelegate delegate,
    Size? size,
    BoxConstraints? constraints,
    required bool resizable,
    String? title,
  }) {
    final controller = WindowControllerRust(
      owner: this,
      delegate: delegate,
      size: size,
      constraints: constraints,
      resizable: resizable,
      title: title,
    );
    _controllers[controller.rootView.viewId] = controller;
    return controller;
  }

  void _handleWindowEvent(int viewId, int event) {
    final _RustLifecycleController? controller = _controllers[viewId];
    if (controller == null) {
      return;
    }
    switch (_WindowEvent.fromNative(event)) {
      case _WindowEvent.stateChanged:
        controller._stateChanged();
        return;
      case _WindowEvent.closeRequested:
        controller._closeRequested();
        return;
      case _WindowEvent.destroyed:
        _controllers.remove(viewId);
        controller._windowDestroyed();
        return;
    }
  }

  @override
  DialogWindowController createDialogWindowController({
    required DialogWindowControllerDelegate delegate,
    Size? size,
    BoxConstraints? constraints,
    required bool resizable,
    BaseWindowController? parent,
    String? title,
  }) {
    if (parent != null &&
        parent is! WindowControllerRust &&
        parent is! DialogWindowControllerRust) {
      throw ArgumentError.value(parent, 'parent', 'must be owned by the Rust shell');
    }
    final controller = DialogWindowControllerRust(
      owner: this,
      delegate: delegate,
      size: size,
      constraints: constraints,
      resizable: resizable,
      parent: parent,
      title: title,
    );
    _controllers[controller.rootView.viewId] = controller;
    return controller;
  }

  @override
  TooltipWindowController createTooltipWindowController({
    required TooltipWindowControllerDelegate delegate,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
    required BaseWindowController parent,
  }) {
    final controller = TooltipWindowControllerRust(
      owner: this,
      delegate: delegate,
      constraints: constraints,
      anchorRect: anchorRect,
      positioner: positioner,
      parent: parent,
    );
    _controllers[controller.rootView.viewId] = controller;
    return controller;
  }

  @override
  PopupWindowController createPopupWindowController({
    required PopupWindowControllerDelegate delegate,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
    required BaseWindowController parent,
  }) {
    final controller = PopupWindowControllerRust(
      owner: this,
      delegate: delegate,
      constraints: constraints,
      anchorRect: anchorRect,
      positioner: positioner,
      parent: parent,
    );
    _controllers[controller.rootView.viewId] = controller;
    return controller;
  }

  @override
  SatelliteWindowController createSatelliteWindowController({
    required SatelliteWindowControllerDelegate delegate,
    required BaseWindowController parent,
    required WindowPositioner initialPositioner,
    Rect? initialAnchorRect,
    Size? size,
    BoxConstraints? constraints,
    required bool resizable,
    String? title,
  }) {
    final controller = SatelliteWindowControllerRust(
      owner: this,
      delegate: delegate,
      parent: parent,
      initialPositioner: initialPositioner,
      initialAnchorRect: initialAnchorRect,
      size: size,
      constraints: constraints,
      resizable: resizable,
      title: title,
    );
    _controllers[controller.rootView.viewId] = controller;
    return controller;
  }
}

abstract interface class _RustLifecycleController {
  void _stateChanged();
  void _closeRequested();
  void _windowDestroyed();
}

@internal
class WindowControllerRust extends WindowController implements _RustLifecycleController {
  WindowControllerRust({
    required WindowingOwnerRust owner,
    required WindowControllerDelegate delegate,
    required bool resizable,
    Size? size,
    BoxConstraints? constraints,
    String? title,
  }) : _owner = owner,
       _delegate = delegate,
       _title = title ?? 'Flutter',
       super.empty() {
    final int viewId = _RustWindowing.createRegularWindow(
      engineId: _owner._engineId,
      size: size,
      constraints: constraints,
      title: _title,
      resizable: resizable,
    );
    if (viewId < 0) {
      throw StateError('The Rust shell failed to create a regular window.');
    }
    rootView = WidgetsBinding.instance.platformDispatcher.views.firstWhere(
      (FlutterView view) => view.viewId == viewId,
    );
  }

  final WindowingOwnerRust _owner;
  final WindowControllerDelegate _delegate;
  String _title;
  bool _destroyRequested = false;
  bool _destroyed = false;

  void _ensureNotDestroyed() {
    if (_destroyed) {
      throw StateError('Window has been destroyed.');
    }
  }

  _WindowStateValue get _state {
    _ensureNotDestroyed();
    return _RustWindowing.getWindowState(_owner._engineId, rootView.viewId);
  }

  @override
  Size get contentSize {
    final _WindowStateValue state = _state;
    return Size(state.width, state.height);
  }

  @override
  bool get isDestroyed => _destroyed;

  @override
  String get title {
    _ensureNotDestroyed();
    return _title;
  }

  @override
  bool get isActivated => _state.focused != 0;

  @override
  bool get isMaximized => _state.maximized != 0;

  @override
  bool get isMinimized => _state.minimized != 0;

  @override
  bool get isFullscreen => _state.fullscreen != 0;

  @override
  void destroy() {
    if (_destroyed || _destroyRequested) {
      return;
    }
    _destroyRequested = true;
    _RustWindowing.destroyWindow(_owner._engineId, rootView.viewId);
  }

  @override
  void setSize(Size size) {
    _ensureNotDestroyed();
    _RustWindowing.setSize(_owner._engineId, rootView.viewId, size.width, size.height);
  }

  @override
  void setConstraints(BoxConstraints constraints) {
    _ensureNotDestroyed();
    _RustWindowing.setConstraints(_owner._engineId, rootView.viewId, constraints);
  }

  @override
  void setTitle(String title) {
    _ensureNotDestroyed();
    _RustWindowing.setTitle(_owner._engineId, rootView.viewId, title);
    _title = title;
    notifyListeners();
  }

  @override
  void activate() {
    _ensureNotDestroyed();
    _RustWindowing.activate(_owner._engineId, rootView.viewId);
  }

  @override
  void setMaximized(bool maximized) {
    _ensureNotDestroyed();
    _RustWindowing.setMaximized(_owner._engineId, rootView.viewId, maximized);
  }

  @override
  void setMinimized(bool minimized) {
    _ensureNotDestroyed();
    _RustWindowing.setMinimized(_owner._engineId, rootView.viewId, minimized);
  }

  @override
  void setFullscreen(bool fullscreen, {Display? display}) {
    _ensureNotDestroyed();
    _RustWindowing.setFullscreen(_owner._engineId, rootView.viewId, fullscreen);
  }

  @override
  void _stateChanged() {
    if (!_destroyed) {
      notifyListeners();
    }
  }

  @override
  void _closeRequested() {
    if (!_destroyed && !_destroyRequested) {
      _delegate.onWindowCloseRequested(this);
    }
  }

  @override
  void _windowDestroyed() {
    if (_destroyed) {
      return;
    }
    _destroyed = true;
    notifyListeners();
    _delegate.onWindowDestroyed();
  }
}

@internal
class DialogWindowControllerRust extends DialogWindowController
    implements _RustLifecycleController {
  DialogWindowControllerRust({
    required WindowingOwnerRust owner,
    required DialogWindowControllerDelegate delegate,
    required bool resizable,
    required this.parent,
    Size? size,
    BoxConstraints? constraints,
    String? title,
  }) : _owner = owner,
       _delegate = delegate,
       _title = title ?? 'Flutter',
       super.empty() {
    final int viewId = _RustWindowing.createDialogWindow(
      engineId: _owner._engineId,
      size: size,
      constraints: constraints,
      title: _title,
      resizable: resizable,
      parentViewId: parent?.rootView.viewId,
    );
    if (viewId < 0) {
      throw StateError('The Rust shell failed to create a dialog window.');
    }
    rootView = WidgetsBinding.instance.platformDispatcher.views.firstWhere(
      (FlutterView view) => view.viewId == viewId,
    );
  }

  final WindowingOwnerRust _owner;
  final DialogWindowControllerDelegate _delegate;
  @override
  final BaseWindowController? parent;
  String _title;
  bool _destroyRequested = false;
  bool _destroyed = false;

  void _ensureNotDestroyed() {
    if (_destroyed) {
      throw StateError('Window has been destroyed.');
    }
  }

  _WindowStateValue get _state {
    _ensureNotDestroyed();
    return _RustWindowing.getWindowState(_owner._engineId, rootView.viewId);
  }

  @override
  Size get contentSize => Size(_state.width, _state.height);

  @override
  bool get isDestroyed => _destroyed;

  @override
  String get title {
    _ensureNotDestroyed();
    return _title;
  }

  @override
  bool get isActivated => _state.focused != 0;

  @override
  bool get isMinimized => _state.minimized != 0;

  @override
  void destroy() {
    if (_destroyed || _destroyRequested) {
      return;
    }
    _destroyRequested = true;
    _RustWindowing.destroyWindow(_owner._engineId, rootView.viewId);
  }

  @override
  void setSize(Size size) {
    _ensureNotDestroyed();
    _RustWindowing.setSize(_owner._engineId, rootView.viewId, size.width, size.height);
  }

  @override
  void setConstraints(BoxConstraints constraints) {
    _ensureNotDestroyed();
    _RustWindowing.setConstraints(_owner._engineId, rootView.viewId, constraints);
  }

  @override
  void setTitle(String title) {
    _ensureNotDestroyed();
    _RustWindowing.setTitle(_owner._engineId, rootView.viewId, title);
    _title = title;
    notifyListeners();
  }

  @override
  void activate() {
    _ensureNotDestroyed();
    _RustWindowing.activate(_owner._engineId, rootView.viewId);
  }

  @override
  void setMinimized(bool minimized) {
    _ensureNotDestroyed();
    _RustWindowing.setMinimized(_owner._engineId, rootView.viewId, minimized);
  }

  @override
  void _stateChanged() {
    if (!_destroyed) {
      notifyListeners();
    }
  }

  @override
  void _closeRequested() {
    if (!_destroyed && !_destroyRequested) {
      _delegate.onWindowCloseRequested(this);
    }
  }

  @override
  void _windowDestroyed() {
    if (_destroyed) {
      return;
    }
    _destroyed = true;
    notifyListeners();
    _delegate.onWindowDestroyed();
  }
}

@internal
class TooltipWindowControllerRust extends TooltipWindowController
    implements _RustLifecycleController {
  TooltipWindowControllerRust({
    required WindowingOwnerRust owner,
    required TooltipWindowControllerDelegate delegate,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
    required this.parent,
  }) : _owner = owner,
       _delegate = delegate,
       _constraints = constraints,
       _anchorRect = anchorRect,
       _positioner = positioner,
       super.empty() {
    _create(_PopupWindowKind.tooltip);
  }

  final WindowingOwnerRust _owner;
  final TooltipWindowControllerDelegate _delegate;
  @override
  final BaseWindowController parent;
  BoxConstraints _constraints;
  Rect _anchorRect;
  WindowPositioner _positioner;
  bool _destroyRequested = false;
  bool _destroyed = false;

  void _create(_PopupWindowKind kind) {
    if (parent is! _RustLifecycleController) {
      throw ArgumentError.value(parent, 'parent', 'must be owned by the Rust shell');
    }
    final int viewId = _RustWindowing.createPopupWindow(
      engineId: _owner._engineId,
      kind: kind,
      parentViewId: parent.rootView.viewId,
      constraints: _constraints,
      anchorRect: _anchorRect,
      positioner: _positioner,
    );
    if (viewId < 0) {
      throw StateError('The Rust shell failed to create a tooltip window.');
    }
    rootView = WidgetsBinding.instance.platformDispatcher.views.firstWhere(
      (FlutterView view) => view.viewId == viewId,
    );
  }

  @override
  Size get contentSize {
    final _WindowStateValue state = _RustWindowing.getWindowState(
      _owner._engineId,
      rootView.viewId,
    );
    return Size(state.width, state.height);
  }

  @override
  bool get isDestroyed => _destroyed;

  @override
  void destroy() {
    if (_destroyed || _destroyRequested) {
      return;
    }
    _destroyRequested = true;
    _RustWindowing.destroyWindow(_owner._engineId, rootView.viewId);
  }

  @override
  void setConstraints(BoxConstraints constraints) {
    _constraints = constraints;
    _RustWindowing.setConstraints(_owner._engineId, rootView.viewId, constraints);
  }

  @override
  void updatePosition({Rect? anchorRect, WindowPositioner? positioner}) {
    _anchorRect = anchorRect ?? _anchorRect;
    _positioner = positioner ?? _positioner;
    // The Linux windowing contract permits position updates to be ignored.
  }

  @override
  void _stateChanged() => notifyListeners();

  @override
  void _closeRequested() => destroy();

  @override
  void _windowDestroyed() {
    if (_destroyed) {
      return;
    }
    _destroyed = true;
    notifyListeners();
    _delegate.onWindowDestroyed();
  }
}

@internal
class PopupWindowControllerRust extends PopupWindowController implements _RustLifecycleController {
  PopupWindowControllerRust({
    required WindowingOwnerRust owner,
    required PopupWindowControllerDelegate delegate,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
    required this.parent,
  }) : _owner = owner,
       _delegate = delegate,
       _constraints = constraints,
       _anchorRect = anchorRect,
       _positioner = positioner,
       super.empty() {
    if (parent is! _RustLifecycleController) {
      throw ArgumentError.value(parent, 'parent', 'must be owned by the Rust shell');
    }
    final int viewId = _RustWindowing.createPopupWindow(
      engineId: _owner._engineId,
      kind: _PopupWindowKind.popup,
      parentViewId: parent.rootView.viewId,
      constraints: _constraints,
      anchorRect: _anchorRect,
      positioner: _positioner,
    );
    if (viewId < 0) {
      throw StateError('The Rust shell failed to create a popup window.');
    }
    rootView = WidgetsBinding.instance.platformDispatcher.views.firstWhere(
      (FlutterView view) => view.viewId == viewId,
    );
  }

  final WindowingOwnerRust _owner;
  final PopupWindowControllerDelegate _delegate;
  @override
  final BaseWindowController parent;
  BoxConstraints _constraints;
  Rect _anchorRect;
  WindowPositioner _positioner;
  bool _destroyRequested = false;
  bool _destroyed = false;

  @override
  Size get contentSize {
    final _WindowStateValue state = _RustWindowing.getWindowState(
      _owner._engineId,
      rootView.viewId,
    );
    return Size(state.width, state.height);
  }

  @override
  bool get isDestroyed => _destroyed;

  @override
  void destroy() {
    if (_destroyed || _destroyRequested) {
      return;
    }
    _destroyRequested = true;
    _RustWindowing.destroyWindow(_owner._engineId, rootView.viewId);
  }

  @override
  void setConstraints(BoxConstraints constraints) {
    _constraints = constraints;
    _RustWindowing.setConstraints(_owner._engineId, rootView.viewId, constraints);
  }

  @override
  void updatePosition({Rect? anchorRect, WindowPositioner? positioner}) {
    _anchorRect = anchorRect ?? _anchorRect;
    _positioner = positioner ?? _positioner;
  }

  @override
  Offset get offsetFromParent {
    final Offset anchor = switch (_positioner.parentAnchor) {
      WindowPositionerAnchor.center => _anchorRect.center,
      WindowPositionerAnchor.top => _anchorRect.topCenter,
      WindowPositionerAnchor.bottom => _anchorRect.bottomCenter,
      WindowPositionerAnchor.left => _anchorRect.centerLeft,
      WindowPositionerAnchor.right => _anchorRect.centerRight,
      WindowPositionerAnchor.topLeft => _anchorRect.topLeft,
      WindowPositionerAnchor.bottomLeft => _anchorRect.bottomLeft,
      WindowPositionerAnchor.topRight => _anchorRect.topRight,
      WindowPositionerAnchor.bottomRight => _anchorRect.bottomRight,
    };
    final Size size = contentSize;
    final Offset childOffset = switch (_positioner.childAnchor) {
      WindowPositionerAnchor.center => Offset(-size.width / 2, -size.height / 2),
      WindowPositionerAnchor.top => Offset(-size.width / 2, 0),
      WindowPositionerAnchor.bottom => Offset(-size.width / 2, -size.height),
      WindowPositionerAnchor.left => Offset(0, -size.height / 2),
      WindowPositionerAnchor.right => Offset(-size.width, -size.height / 2),
      WindowPositionerAnchor.topLeft => Offset.zero,
      WindowPositionerAnchor.bottomLeft => Offset(0, -size.height),
      WindowPositionerAnchor.topRight => Offset(-size.width, 0),
      WindowPositionerAnchor.bottomRight => Offset(-size.width, -size.height),
    };
    return anchor + _positioner.offset + childOffset;
  }

  @override
  void _stateChanged() => notifyListeners();

  @override
  void _closeRequested() => destroy();

  @override
  void _windowDestroyed() {
    if (_destroyed) {
      return;
    }
    _destroyed = true;
    notifyListeners();
    _delegate.onWindowDestroyed();
  }
}

@internal
class SatelliteWindowControllerRust extends SatelliteWindowController
    implements _RustLifecycleController {
  SatelliteWindowControllerRust({
    required WindowingOwnerRust owner,
    required SatelliteWindowControllerDelegate delegate,
    required BaseWindowController parent,
    required WindowPositioner initialPositioner,
    required Rect? initialAnchorRect,
    required Size? size,
    required BoxConstraints? constraints,
    required bool resizable,
    required String? title,
  }) : _owner = owner,
       _delegate = delegate,
       _parent = parent,
       _title = title ?? 'Flutter',
       super.empty() {
    _validateParent(parent);
    final int viewId = _RustWindowing.createSatelliteWindow(
      engineId: _owner._engineId,
      parentViewId: parent.rootView.viewId,
      initialPositioner: initialPositioner,
      initialAnchorRect: initialAnchorRect,
      size: size,
      constraints: constraints,
      resizable: resizable,
      title: _title,
    );
    if (viewId < 0) {
      throw StateError('The Rust shell failed to create a satellite window.');
    }
    rootView = WidgetsBinding.instance.platformDispatcher.views.firstWhere(
      (FlutterView view) => view.viewId == viewId,
    );
  }

  final WindowingOwnerRust _owner;
  final SatelliteWindowControllerDelegate _delegate;
  BaseWindowController _parent;
  String _title;
  bool _destroyRequested = false;
  bool _destroyed = false;

  void _validateParent(BaseWindowController value) {
    if (value is! WindowControllerRust && value is! DialogWindowControllerRust) {
      throw ArgumentError.value(
        value,
        'parent',
        'must be a regular or dialog window owned by the Rust shell',
      );
    }
  }

  void _ensureNotDestroyed() {
    if (_destroyed) {
      throw StateError('Window has been destroyed.');
    }
  }

  _WindowStateValue get _state {
    _ensureNotDestroyed();
    return _RustWindowing.getWindowState(_owner._engineId, rootView.viewId);
  }

  @override
  BaseWindowController get parent => _parent;

  @override
  Size get contentSize => Size(_state.width, _state.height);

  @override
  bool get isDestroyed => _destroyed;

  @override
  String get title {
    _ensureNotDestroyed();
    return _title;
  }

  @override
  bool get isActivated => _state.focused != 0;

  @override
  void destroy() {
    if (_destroyed || _destroyRequested) {
      return;
    }
    _destroyRequested = true;
    _RustWindowing.destroyWindow(_owner._engineId, rootView.viewId);
  }

  @override
  void setParent(BaseWindowController parent) {
    _ensureNotDestroyed();
    _validateParent(parent);
    if (!_RustWindowing.setParent(_owner._engineId, rootView.viewId, parent.rootView.viewId)) {
      throw StateError('The Rust shell rejected the satellite parent.');
    }
    _parent = parent;
    notifyListeners();
  }

  @override
  void setSize(Size size) {
    _ensureNotDestroyed();
    _RustWindowing.setSize(_owner._engineId, rootView.viewId, size.width, size.height);
  }

  @override
  void setConstraints(BoxConstraints constraints) {
    _ensureNotDestroyed();
    _RustWindowing.setConstraints(_owner._engineId, rootView.viewId, constraints);
  }

  @override
  void setTitle(String title) {
    _ensureNotDestroyed();
    _RustWindowing.setTitle(_owner._engineId, rootView.viewId, title);
    _title = title;
    notifyListeners();
  }

  @override
  void activate() {
    _ensureNotDestroyed();
    _RustWindowing.activate(_owner._engineId, rootView.viewId);
  }

  @override
  void _stateChanged() {
    if (!_destroyed) {
      notifyListeners();
    }
  }

  @override
  void _closeRequested() {
    if (!_destroyed && !_destroyRequested) {
      _delegate.onWindowCloseRequested(this);
    }
  }

  @override
  void _windowDestroyed() {
    if (_destroyed) {
      return;
    }
    _destroyed = true;
    notifyListeners();
    _delegate.onWindowDestroyed();
  }
}

enum _WindowEvent {
  stateChanged(0),
  closeRequested(1),
  destroyed(2);

  const _WindowEvent(this.nativeValue);
  final int nativeValue;

  static _WindowEvent fromNative(int value) {
    return values.firstWhere(
      (_WindowEvent event) => event.nativeValue == value,
      orElse: () => throw StateError('Unknown Rust window event: $value'),
    );
  }
}

final class _RegularWindowRequest extends ffi.Struct {
  @ffi.Int32()
  external int hasSize;

  @ffi.Double()
  external double width;

  @ffi.Double()
  external double height;

  external ffi.Pointer<ffi.Uint8> title;

  @ffi.Uint64()
  external int titleLength;

  @ffi.Int32()
  external int resizable;

  @ffi.Int32()
  external int hasConstraints;

  @ffi.Double()
  external double minWidth;

  @ffi.Double()
  external double minHeight;

  @ffi.Double()
  external double maxWidth;

  @ffi.Double()
  external double maxHeight;
}

final class _DialogWindowRequest extends ffi.Struct {
  external _RegularWindowRequest window;

  @ffi.Int32()
  external int hasParent;

  @ffi.Int64()
  external int parentViewId;
}

enum _PopupWindowKind {
  tooltip(0),
  popup(1);

  const _PopupWindowKind(this.nativeValue);
  final int nativeValue;
}

final class _PopupWindowRequest extends ffi.Struct {
  @ffi.Int32()
  external int kind;

  @ffi.Int64()
  external int parentViewId;

  @ffi.Double()
  external double minWidth;
  @ffi.Double()
  external double minHeight;
  @ffi.Double()
  external double maxWidth;
  @ffi.Double()
  external double maxHeight;
  @ffi.Double()
  external double anchorX;
  @ffi.Double()
  external double anchorY;
  @ffi.Double()
  external double anchorWidth;
  @ffi.Double()
  external double anchorHeight;

  @ffi.Int32()
  external int parentAnchor;
  @ffi.Int32()
  external int childAnchor;

  @ffi.Double()
  external double offsetX;
  @ffi.Double()
  external double offsetY;

  @ffi.Uint32()
  external int constraintAdjustment;
}

final class _SatelliteWindowRequest extends ffi.Struct {
  external _RegularWindowRequest window;

  @ffi.Int64()
  external int parentViewId;

  @ffi.Int32()
  external int hasAnchorRect;

  @ffi.Double()
  external double anchorX;
  @ffi.Double()
  external double anchorY;
  @ffi.Double()
  external double anchorWidth;
  @ffi.Double()
  external double anchorHeight;

  @ffi.Int32()
  external int parentAnchor;
  @ffi.Int32()
  external int childAnchor;

  @ffi.Double()
  external double offsetX;
  @ffi.Double()
  external double offsetY;

  @ffi.Uint32()
  external int constraintAdjustment;
}

final class _WindowState extends ffi.Struct {
  @ffi.Double()
  external double width;

  @ffi.Double()
  external double height;

  @ffi.Int32()
  external int focused;

  @ffi.Int32()
  external int maximized;

  @ffi.Int32()
  external int minimized;

  @ffi.Int32()
  external int fullscreen;
}

typedef _WindowEventNative = ffi.Void Function(ffi.Int64 viewId, ffi.Int32 event);
typedef _WindowStateValue = ({
  double width,
  double height,
  int focused,
  int maximized,
  int minimized,
  int fullscreen,
});

final class _RustWindowing {
  static int createRegularWindow({
    required int engineId,
    required Size? size,
    required BoxConstraints? constraints,
    required String title,
    required bool resizable,
  }) {
    final List<int> titleBytes = utf8.encode(title);
    final ffi.Pointer<_RegularWindowRequest> request = _malloc(
      ffi.sizeOf<_RegularWindowRequest>(),
    ).cast<_RegularWindowRequest>();
    if (request == ffi.nullptr) {
      throw StateError('Native allocation failed.');
    }
    final ffi.Pointer<ffi.Uint8> titlePointer = _allocateBytes(titleBytes.length);
    if (titleBytes.isNotEmpty) {
      titlePointer.asTypedList(titleBytes.length).setAll(0, titleBytes);
    }
    request.ref
      ..hasSize = size == null ? 0 : 1
      ..width = size?.width ?? 0
      ..height = size?.height ?? 0
      ..title = titlePointer
      ..titleLength = titleBytes.length
      ..resizable = resizable ? 1 : 0
      ..hasConstraints = constraints == null ? 0 : 1
      ..minWidth = constraints?.minWidth ?? 0
      ..minHeight = constraints?.minHeight ?? 0
      ..maxWidth = constraints?.maxWidth ?? 0
      ..maxHeight = constraints?.maxHeight ?? 0;
    try {
      return _createRegular(engineId, request);
    } finally {
      if (titlePointer != ffi.nullptr) {
        _free(titlePointer.cast());
      }
      _free(request.cast());
    }
  }

  static int createDialogWindow({
    required int engineId,
    required Size? size,
    required BoxConstraints? constraints,
    required String title,
    required bool resizable,
    required int? parentViewId,
  }) {
    final List<int> titleBytes = utf8.encode(title);
    final ffi.Pointer<_DialogWindowRequest> request = _malloc(
      ffi.sizeOf<_DialogWindowRequest>(),
    ).cast<_DialogWindowRequest>();
    if (request == ffi.nullptr) {
      throw StateError('Native allocation failed.');
    }
    final ffi.Pointer<ffi.Uint8> titlePointer = _allocateBytes(titleBytes.length);
    if (titleBytes.isNotEmpty) {
      titlePointer.asTypedList(titleBytes.length).setAll(0, titleBytes);
    }
    request.ref.window
      ..hasSize = size == null ? 0 : 1
      ..width = size?.width ?? 0
      ..height = size?.height ?? 0
      ..title = titlePointer
      ..titleLength = titleBytes.length
      ..resizable = resizable ? 1 : 0
      ..hasConstraints = constraints == null ? 0 : 1
      ..minWidth = constraints?.minWidth ?? 0
      ..minHeight = constraints?.minHeight ?? 0
      ..maxWidth = constraints?.maxWidth ?? 0
      ..maxHeight = constraints?.maxHeight ?? 0;
    request.ref
      ..hasParent = parentViewId == null ? 0 : 1
      ..parentViewId = parentViewId ?? 0;
    try {
      return _createDialog(engineId, request);
    } finally {
      if (titlePointer != ffi.nullptr) {
        _free(titlePointer.cast());
      }
      _free(request.cast());
    }
  }

  static int createPopupWindow({
    required int engineId,
    required _PopupWindowKind kind,
    required int parentViewId,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
  }) {
    final ffi.Pointer<_PopupWindowRequest> request = _malloc(
      ffi.sizeOf<_PopupWindowRequest>(),
    ).cast<_PopupWindowRequest>();
    if (request == ffi.nullptr) {
      throw StateError('Native allocation failed.');
    }
    final WindowPositionerConstraintAdjustment adjustment = positioner.constraintAdjustment;
    request.ref
      ..kind = kind.nativeValue
      ..parentViewId = parentViewId
      ..minWidth = constraints.minWidth
      ..minHeight = constraints.minHeight
      ..maxWidth = constraints.maxWidth
      ..maxHeight = constraints.maxHeight
      ..anchorX = anchorRect.left
      ..anchorY = anchorRect.top
      ..anchorWidth = anchorRect.width
      ..anchorHeight = anchorRect.height
      ..parentAnchor = positioner.parentAnchor.index
      ..childAnchor = positioner.childAnchor.index
      ..offsetX = positioner.offset.dx
      ..offsetY = positioner.offset.dy
      ..constraintAdjustment =
          (adjustment.slideX ? 1 : 0) |
          (adjustment.slideY ? 2 : 0) |
          (adjustment.flipX ? 4 : 0) |
          (adjustment.flipY ? 8 : 0) |
          (adjustment.resizeX ? 16 : 0) |
          (adjustment.resizeY ? 32 : 0);
    try {
      return _createPopup(engineId, request);
    } finally {
      _free(request.cast());
    }
  }

  static int createSatelliteWindow({
    required int engineId,
    required int parentViewId,
    required WindowPositioner initialPositioner,
    required Rect? initialAnchorRect,
    required Size? size,
    required BoxConstraints? constraints,
    required bool resizable,
    required String title,
  }) {
    final List<int> titleBytes = utf8.encode(title);
    final ffi.Pointer<_SatelliteWindowRequest> request = _malloc(
      ffi.sizeOf<_SatelliteWindowRequest>(),
    ).cast<_SatelliteWindowRequest>();
    if (request == ffi.nullptr) {
      throw StateError('Native allocation failed.');
    }
    final ffi.Pointer<ffi.Uint8> titlePointer = _allocateBytes(titleBytes.length);
    if (titleBytes.isNotEmpty) {
      titlePointer.asTypedList(titleBytes.length).setAll(0, titleBytes);
    }
    final WindowPositionerConstraintAdjustment adjustment = initialPositioner.constraintAdjustment;
    request.ref.window
      ..hasSize = size == null ? 0 : 1
      ..width = size?.width ?? 0
      ..height = size?.height ?? 0
      ..title = titlePointer
      ..titleLength = titleBytes.length
      ..resizable = resizable ? 1 : 0
      ..hasConstraints = constraints == null ? 0 : 1
      ..minWidth = constraints?.minWidth ?? 0
      ..minHeight = constraints?.minHeight ?? 0
      ..maxWidth = constraints?.maxWidth ?? 0
      ..maxHeight = constraints?.maxHeight ?? 0;
    request.ref
      ..parentViewId = parentViewId
      ..hasAnchorRect = initialAnchorRect == null ? 0 : 1
      ..anchorX = initialAnchorRect?.left ?? 0
      ..anchorY = initialAnchorRect?.top ?? 0
      ..anchorWidth = initialAnchorRect?.width ?? 0
      ..anchorHeight = initialAnchorRect?.height ?? 0
      ..parentAnchor = initialPositioner.parentAnchor.index
      ..childAnchor = initialPositioner.childAnchor.index
      ..offsetX = initialPositioner.offset.dx
      ..offsetY = initialPositioner.offset.dy
      ..constraintAdjustment =
          (adjustment.slideX ? 1 : 0) |
          (adjustment.slideY ? 2 : 0) |
          (adjustment.flipX ? 4 : 0) |
          (adjustment.flipY ? 8 : 0) |
          (adjustment.resizeX ? 16 : 0) |
          (adjustment.resizeY ? 32 : 0);
    try {
      return _createSatellite(engineId, request);
    } finally {
      if (titlePointer != ffi.nullptr) {
        _free(titlePointer.cast());
      }
      _free(request.cast());
    }
  }

  static _WindowStateValue getWindowState(int engineId, int viewId) {
    final ffi.Pointer<_WindowState> state = _malloc(
      ffi.sizeOf<_WindowState>(),
    ).cast<_WindowState>();
    if (state == ffi.nullptr) {
      throw StateError('Native allocation failed.');
    }
    if (_getState(engineId, viewId, state) == 0) {
      _free(state.cast());
      throw StateError('The Rust shell no longer has window $viewId.');
    }
    final _WindowStateValue result = (
      width: state.ref.width,
      height: state.ref.height,
      focused: state.ref.focused,
      maximized: state.ref.maximized,
      minimized: state.ref.minimized,
      fullscreen: state.ref.fullscreen,
    );
    _free(state.cast());
    return result;
  }

  static void setConstraints(int engineId, int viewId, BoxConstraints constraints) {
    _setConstraints(
      engineId,
      viewId,
      1,
      constraints.minWidth,
      constraints.minHeight,
      constraints.maxWidth,
      constraints.maxHeight,
    );
  }

  static void setTitle(int engineId, int viewId, String title) {
    final List<int> bytes = utf8.encode(title);
    final ffi.Pointer<ffi.Uint8> pointer = _allocateBytes(bytes.length);
    if (bytes.isNotEmpty) {
      pointer.asTypedList(bytes.length).setAll(0, bytes);
    }
    try {
      _setTitle(engineId, viewId, pointer, bytes.length);
    } finally {
      if (pointer != ffi.nullptr) {
        _free(pointer.cast());
      }
    }
  }

  static ffi.Pointer<ffi.Uint8> _allocateBytes(int length) {
    if (length == 0) {
      return ffi.nullptr;
    }
    final ffi.Pointer<ffi.Uint8> result = _malloc(length).cast<ffi.Uint8>();
    if (result == ffi.nullptr) {
      throw StateError('Native allocation failed.');
    }
    return result;
  }

  @ffi.Native<ffi.Pointer<ffi.Void> Function(ffi.IntPtr)>(symbol: 'malloc')
  external static ffi.Pointer<ffi.Void> _malloc(int size);

  @ffi.Native<ffi.Void Function(ffi.Pointer<ffi.Void>)>(symbol: 'free')
  external static void _free(ffi.Pointer<ffi.Void> pointer);

  @ffi.Native<ffi.Int64 Function(ffi.Int64, ffi.Pointer<_RegularWindowRequest>)>(
    symbol: _createRegularSymbol,
  )
  external static int _createRegular(int engineId, ffi.Pointer<_RegularWindowRequest> request);

  @ffi.Native<ffi.Int64 Function(ffi.Int64, ffi.Pointer<_DialogWindowRequest>)>(
    symbol: _createDialogSymbol,
  )
  external static int _createDialog(int engineId, ffi.Pointer<_DialogWindowRequest> request);

  @ffi.Native<ffi.Int64 Function(ffi.Int64, ffi.Pointer<_PopupWindowRequest>)>(
    symbol: 'FlutterRustShellWindowCreatePopup',
  )
  external static int _createPopup(int engineId, ffi.Pointer<_PopupWindowRequest> request);

  @ffi.Native<ffi.Int64 Function(ffi.Int64, ffi.Pointer<_SatelliteWindowRequest>)>(
    symbol: 'FlutterRustShellWindowCreateSatellite',
  )
  external static int _createSatellite(int engineId, ffi.Pointer<_SatelliteWindowRequest> request);

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64)>(symbol: 'FlutterRustShellWindowDestroy')
  external static void destroyWindow(int engineId, int viewId);

  @ffi.Native<ffi.Int32 Function(ffi.Int64, ffi.Int64, ffi.Pointer<_WindowState>)>(
    symbol: 'FlutterRustShellWindowGetState',
  )
  external static int _getState(int engineId, int viewId, ffi.Pointer<_WindowState> state);

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64, ffi.Double, ffi.Double)>(
    symbol: 'FlutterRustShellWindowSetSize',
  )
  external static void setSize(int engineId, int viewId, double width, double height);

  @ffi.Native<
    ffi.Void Function(
      ffi.Int64,
      ffi.Int64,
      ffi.Int32,
      ffi.Double,
      ffi.Double,
      ffi.Double,
      ffi.Double,
    )
  >(symbol: 'FlutterRustShellWindowSetConstraints')
  external static void _setConstraints(
    int engineId,
    int viewId,
    int hasConstraints,
    double minWidth,
    double minHeight,
    double maxWidth,
    double maxHeight,
  );

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64, ffi.Pointer<ffi.Uint8>, ffi.Uint64)>(
    symbol: 'FlutterRustShellWindowSetTitle',
  )
  external static void _setTitle(
    int engineId,
    int viewId,
    ffi.Pointer<ffi.Uint8> title,
    int titleLength,
  );

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64)>(symbol: 'FlutterRustShellWindowActivate')
  external static void activate(int engineId, int viewId);

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64, ffi.Int32)>(
    symbol: 'FlutterRustShellWindowSetMaximized',
  )
  external static void _setMaximized(int engineId, int viewId, int maximized);

  static void setMaximized(int engineId, int viewId, bool maximized) {
    _setMaximized(engineId, viewId, maximized ? 1 : 0);
  }

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64, ffi.Int32)>(
    symbol: 'FlutterRustShellWindowSetMinimized',
  )
  external static void _setMinimized(int engineId, int viewId, int minimized);

  static void setMinimized(int engineId, int viewId, bool minimized) {
    _setMinimized(engineId, viewId, minimized ? 1 : 0);
  }

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Int64, ffi.Int32)>(
    symbol: 'FlutterRustShellWindowSetFullscreen',
  )
  external static void _setFullscreen(int engineId, int viewId, int fullscreen);

  static void setFullscreen(int engineId, int viewId, bool fullscreen) {
    _setFullscreen(engineId, viewId, fullscreen ? 1 : 0);
  }

  @ffi.Native<ffi.Void Function(ffi.Int64, ffi.Pointer<ffi.NativeFunction<_WindowEventNative>>)>(
    symbol: 'FlutterRustShellWindowSetEventCallback',
  )
  external static void setEventCallback(
    int engineId,
    ffi.Pointer<ffi.NativeFunction<_WindowEventNative>> callback,
  );

  @ffi.Native<ffi.Int32 Function(ffi.Int64, ffi.Int64, ffi.Int64)>(
    symbol: 'FlutterRustShellWindowSetParent',
  )
  external static int _setParent(int engineId, int viewId, int parentViewId);

  static bool setParent(int engineId, int viewId, int parentViewId) {
    return _setParent(engineId, viewId, parentViewId) != 0;
  }
}
