// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

void main() => runApp(const MaterialApp(home: _InputProbe()));

class _InputProbe extends StatefulWidget {
  const _InputProbe();
  @override
  State<_InputProbe> createState() => _InputProbeState();
}

class _InputProbeState extends State<_InputProbe> with WidgetsBindingObserver {
  final controller = TextEditingController();
  final focus = FocusNode();
  final controls = <String, GlobalKey>{
    for (final name in ['field', 'copy', 'read', 'restore']) name: GlobalKey(),
  };
  String? clipboard;
  String? originalClipboard;
  bool hasStrings = false;
  bool ready = false;
  bool restored = false;
  String lifecycle = 'resumed';
  static const sample = 'A\u{1f600}\u05e9\u05dc\u05d5\u05dd';

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    controller.addListener(report);
    focus.addListener(report);
    initialize();
  }

  Future<void> initialize() async {
    originalClipboard = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    ready = true;
    WidgetsBinding.instance.addPostFrameCallback((_) => report());
    if (mounted) {
      setState(() {});
    }
  }

  void report() {
    if (!mounted || !ready) {
      return;
    }
    final String? path = Platform.environment['FLUTTER_RUST_INPUT_STATUS'];
    if (path == null) {
      return;
    }
    final ui.FlutterView view = View.of(context);
    final TextEditingValue value = controller.value;
    final positions = <String, List<double>>{};
    for (final MapEntry<String, GlobalKey> entry in controls.entries) {
      final RenderObject? box = entry.value.currentContext?.findRenderObject();
      if (box is RenderBox && box.hasSize) {
        final Offset center = box.localToGlobal(box.size.center(Offset.zero));
        positions[entry.key] = [center.dx, center.dy];
      }
    }
    File(path).writeAsStringSync(
      jsonEncode({
        'ready': ready,
        'text': value.text,
        'selection': [value.selection.baseOffset, value.selection.extentOffset],
        'composing': [value.composing.start, value.composing.end],
        'focused': focus.hasFocus,
        'lifecycle': lifecycle,
        'clipboard': clipboard,
        'hasStrings': hasStrings,
        'restored': restored,
        'dpr': view.devicePixelRatio,
        'physicalSize': [view.physicalSize.width, view.physicalSize.height],
        'logicalSize': [
          view.physicalSize.width / view.devicePixelRatio,
          view.physicalSize.height / view.devicePixelRatio,
        ],
        'controls': positions,
      }),
    );
  }

  @override
  void didChangeMetrics() {
    WidgetsBinding.instance.addPostFrameCallback((_) => report());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    lifecycle = state.name;
    report();
  }

  Future<void> readClipboard() async {
    clipboard = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    hasStrings = await Clipboard.hasStrings();
    report();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    controller.dispose();
    focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    body: Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('Windows text, clipboard and viewport probe'),
          const SizedBox(height: 16),
          TextField(
            key: controls['field'],
            controller: controller,
            focusNode: focus,
            decoration: const InputDecoration(labelText: 'Text input'),
          ),
          const SizedBox(height: 16),
          Wrap(
            spacing: 12,
            children: [
              ElevatedButton(
                key: controls['copy'],
                onPressed: () async {
                  await Clipboard.setData(const ClipboardData(text: sample));
                  await readClipboard();
                },
                child: const Text('Copy sample'),
              ),
              ElevatedButton(
                key: controls['read'],
                onPressed: readClipboard,
                child: const Text('Read clipboard'),
              ),
              ElevatedButton(
                key: controls['restore'],
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: originalClipboard ?? ''));
                  restored = true;
                  report();
                },
                child: const Text('Restore clipboard text'),
              ),
            ],
          ),
        ],
      ),
    ),
  );
}
