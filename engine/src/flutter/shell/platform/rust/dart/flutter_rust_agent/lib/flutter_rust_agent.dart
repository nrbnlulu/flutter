// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:async';
import 'dart:ui';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// Named, app-owned observations. Only JSON values may cross the bridge.
typedef AgentPredicate = Object? Function(Map<String, Object?> arguments);

/// Optional observations on the real UI isolate. No fake clock or VM evaluation.
/// Calls are inert unless built with --dart-define=FLUTTER_RUST_AGENT=true.
class FlutterRustAgent {
  FlutterRustAgent._();

  /// Whether this build explicitly opted into verification observations.
  static const bool enabled = bool.fromEnvironment('FLUTTER_RUST_AGENT');
  static const BasicMessageChannel<Object?> _events = BasicMessageChannel<Object?>(
    'flutter.rs.agent/events',
    JSONMessageCodec(),
  );
  static const BasicMessageChannel<Object?> _queries = BasicMessageChannel<Object?>(
    'flutter.rs.agent/query',
    JSONMessageCodec(),
  );
  static final Map<String, AgentPredicate> _predicates = <String, AgentPredicate>{};
  static bool _installed = false;
  static int _errorCount = 0;
  static FlutterExceptionHandler? _frameworkHandler;
  static ErrorCallback? _platformHandler;

  /// Registers a named JSON observation owned by the application.
  static void registerPredicate(String name, AgentPredicate predicate) {
    if (enabled) {
      _predicates[name] = predicate;
    }
  }

  /// Installs query handling and chains existing framework/platform error hooks.
  static void install() {
    if (!enabled || _installed) {
      return;
    }
    WidgetsFlutterBinding.ensureInitialized();
    _installed = true;
    final FlutterExceptionHandler? previousFramework = FlutterError.onError;
    final ErrorCallback? previousPlatform = PlatformDispatcher.instance.onError;
    _frameworkHandler = (FlutterErrorDetails details) {
      _error('framework', details.exceptionAsString(), details.stack);
      previousFramework?.call(details);
    };
    FlutterError.onError = _frameworkHandler;
    _platformHandler = (Object error, StackTrace stack) {
      _error('unhandled', error.toString(), stack);
      return previousPlatform?.call(error, stack) ?? false;
    };
    PlatformDispatcher.instance.onError = _platformHandler;
    _queries.setMessageHandler((Object? message) async {
      final request = Map<String, Object?>.from(message! as Map);
      try {
        final Object? result = _query(request);
        await _events.send(<String, Object?>{
          'type': 'response',
          'id': request['id'],
          'result': result,
        });
      } catch (error, stack) {
        await _events.send(<String, Object?>{
          'type': 'response',
          'id': request['id'],
          'error': <String, Object?>{'message': error.toString(), 'stack': stack.toString()},
        });
      }
      return null;
    });
    unawaited(
      _events.send(<String, Object?>{
        'type': 'ready',
        'version': 1,
        'capabilities': <String>[
          'valueKey',
          'widgetCount',
          'publicProperties',
          'predicates',
          'frameworkErrors',
        ],
      }),
    );
  }

  static void _error(String kind, String message, StackTrace? stack) {
    _errorCount += 1;
    unawaited(
      _events.send(<String, Object?>{
        'type': 'error',
        'kind': kind,
        'message': message,
        'stack': stack?.toString(),
      }),
    );
  }

  static Object? _query(Map<String, Object?> request) {
    if (request['kind'] == 'diagnostics') {
      return <String, Object?>{
        'frameworkHookInstalled': FlutterError.onError == _frameworkHandler,
        'platformHookInstalled': PlatformDispatcher.instance.onError == _platformHandler,
        'errorCount': _errorCount,
      };
    }
    final query = Map<String, Object?>.from(request['query']! as Map);
    if (request['kind'] == 'app') {
      if (query.keys.any((String key) => key != 'name' && key != 'arguments')) {
        throw ArgumentError('Unknown predicate parameter');
      }
      final AgentPredicate? predicate = _predicates[query['name']];
      if (predicate == null) {
        throw ArgumentError('Unknown predicate ${query['name']}');
      }
      return predicate(
        Map<String, Object?>.from(query['arguments'] as Map? ?? const <String, Object?>{}),
      );
    }
    if (query.keys.any((String key) => !<String>['key', 'type', 'property'].contains(key))) {
      throw ArgumentError('Unknown widget query parameter');
    }
    for (final field in <String>['key', 'type']) {
      if (query[field] != null && query[field] is! String) {
        throw ArgumentError('$field must be a string');
      }
    }
    final matches = <Element>[];
    void visit(Element element) {
      final Widget widget = element.widget;
      final Key? key = widget.key;
      final bool keyMatches =
          query['key'] == null || (key is ValueKey<String> && key.value == query['key']);
      final bool typeMatches =
          query['type'] == null || widget.runtimeType.toString() == query['type'];
      if (keyMatches && typeMatches && View.maybeOf(element)?.viewId == request['viewId']) {
        matches.add(element);
      }
      element.visitChildren(visit);
    }

    final Element? root = WidgetsBinding.instance.rootElement;
    if (root != null) {
      visit(root);
    }
    switch (query['property'] ?? 'count') {
      case 'count':
        return matches.length;
      case 'text':
        if (matches.length != 1) {
          throw StateError('Text property requires one widget');
        }
        final Widget widget = matches.single.widget;
        if (widget is Text) {
          return widget.data ?? widget.textSpan?.toPlainText();
        }
        throw ArgumentError('text is only supported on Text');
      case 'enabled':
        if (matches.length != 1) {
          throw StateError('Enabled property requires one widget');
        }
        final Widget widget = matches.single.widget;
        if (widget is EditableText) {
          return !widget.readOnly;
        }
        throw ArgumentError('enabled is only supported on EditableText');
      default:
        throw ArgumentError('Unsupported public property');
    }
  }
}
