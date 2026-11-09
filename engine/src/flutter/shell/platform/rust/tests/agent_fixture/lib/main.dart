// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
// ignore_for_file: implementation_imports, invalid_use_of_internal_member
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_rust_agent/flutter_rust_agent.dart';
import 'package:flutter/src/widgets/_window.dart';

void main() {
  stdout.writeln('fixture stdout is not a protocol message: {broken json');
  WidgetsFlutterBinding.ensureInitialized();
  print('fixture Dart bridge enabled: ${FlutterRustAgent.enabled}');
  FlutterRustAgent.install();
  runWidget(const AgentFixture());
}

class AgentFixture extends StatefulWidget {
  const AgentFixture({super.key});
  @override
  State<AgentFixture> createState() => _AgentFixtureState();
}

class _AgentFixtureState extends State<AgentFixture> {
  late final WindowController mainWindow = WindowController(
    title: 'Agent form',
    size: const Size(700, 500),
  );
  DialogWindowController? dialog;
  final TextEditingController text = TextEditingController();
  String saved = '';
  int clicks = 0;
  @override
  void initState() {
    super.initState();
    mainWindow;
    FlutterRustAgent.registerPredicate('savedValue', (_) => saved);
    if (const bool.fromEnvironment('AGENT_STARTUP_ERROR')) {
      scheduleMicrotask(
        () => FlutterError.reportError(
          FlutterErrorDetails(exception: StateError('intentional startup error')),
        ),
      );
    }
  }

  void open() {
    if (dialog != null) {
      return;
    }
    setState(
      () => dialog = DialogWindowController(
        parent: mainWindow,
        title: 'Save dialog',
        delegate: _DialogDelegate(() => unawaited(closeDialog(save: false))),
        size: const Size(420, 240),
      ),
    );
  }

  Future<void> confirm() => closeDialog(save: true);
  Future<void> closeDialog({required bool save}) async {
    final retiring = dialog;
    if (retiring == null) {
      return;
    }
    setState(() {
      if (save) {
        saved = const bool.fromEnvironment('AGENT_REGRESSION')
            ? 'intentional regression'
            : text.text;
      }
      dialog = null;
    });
    await WidgetsBinding.instance.endOfFrame;
    retiring.destroy();
    final deadline = DateTime.now().add(const Duration(seconds: 5));
    while (!retiring.isDestroyed && DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    if (!retiring.isDestroyed) {
      throw StateError('dialog removal did not complete');
    }
    retiring.dispose();
    mainWindow.activate();
  }

  Widget button(String id, String label, VoidCallback onPressed) => Semantics(
    identifier: id,
    child: ElevatedButton(onPressed: onPressed, child: Text(label)),
  );
  @override
  Widget build(BuildContext context) => ViewCollection(
    views: [
      View(
        view: ui.PlatformDispatcher.instance.implicitView!,
        child: const ColoredBox(color: Color(0xff102030)),
      ),
      View(
        view: mainWindow.rootView,
        child: MaterialApp(
          home: Scaffold(
            backgroundColor: clicks.isEven ? const Color(0xffd0e0f0) : const Color(0xfff0d0e0),
            body: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('Agent verification fixture'),
                  Semantics(
                    identifier: 'name',
                    child: TextField(
                      controller: text,
                      decoration: const InputDecoration(labelText: 'Name'),
                    ),
                  ),
                  button('open_dialog', 'Open save dialog', open),
                  Semantics(
                    identifier: 'saved_value',
                    label: 'Saved value',
                    value: saved,
                    child: ExcludeSemantics(
                      child: Text('Saved: $saved', key: const ValueKey<String>('saved_text')),
                    ),
                  ),
                  button('increment', 'Increment', () => setState(() => clicks++)),
                  Semantics(
                    identifier: 'click_count',
                    label: 'Click count',
                    value: '$clicks',
                    child: ExcludeSemantics(child: Text('Clicks: $clicks')),
                  ),
                  Row(
                    children: [
                      button('duplicate_a', 'Duplicate', () {}),
                      button('duplicate_b', 'Duplicate', () {}),
                    ],
                  ),
                  button('throw_error', 'Throw error', () {
                    throw StateError('intentional agent fixture exception');
                  }),
                ],
              ),
            ),
          ),
        ),
      ),
      if (dialog case final DialogWindowController window)
        View(
          view: window.rootView,
          child: MaterialApp(
            home: Scaffold(
              body: Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  children: [
                    Semantics(
                      identifier: 'dialog_value',
                      label: 'Dialog value',
                      value: text.text,
                      child: ExcludeSemantics(child: Text(text.text)),
                    ),
                    button('confirm', 'Confirm', () => unawaited(confirm())),
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
    text.dispose();
    mainWindow.dispose();
    super.dispose();
  }
}

class _DialogDelegate with DialogWindowControllerDelegate {
  _DialogDelegate(this.close);
  final VoidCallback close;
  @override
  void onWindowCloseRequested(DialogWindowController controller) => close();
}
