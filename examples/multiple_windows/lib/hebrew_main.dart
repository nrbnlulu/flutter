// Copyright 2014 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

void main() => runApp(const HebrewEditingApp());

class HebrewEditingApp extends StatefulWidget {
  const HebrewEditingApp({super.key});

  @override
  State<HebrewEditingApp> createState() => _HebrewEditingAppState();
}

class _HebrewEditingAppState extends State<HebrewEditingApp> {
  static const String sample =
      'שלום עולם! זהו שדה לעריכת טקסט בעברית.\n'
      'טקסט מעורב: Flutter 123 — בדיקת בחירה, העתקה והדבקה.\n'
      'אימוג׳י: 😀 👍🏽 👨‍👩‍👧‍👦 🇮🇱 ❤️\n'
      'נסו להזיז את הסמן ולמחוק ליד אימוג׳י ובין עברית ל-English.';
  final TextEditingController controller = TextEditingController(text: sample);
  final UndoHistoryController undoController = UndoHistoryController();
  bool hebrew = true;

  @override
  void dispose() {
    controller.dispose();
    undoController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      locale: Locale(hebrew ? 'he' : 'en'),
      supportedLocales: const <Locale>[Locale('he'), Locale('en')],
      localizationsDelegates: GlobalMaterialLocalizations.delegates,
      home: Scaffold(
        appBar: AppBar(title: const Text('בדיקת עריכת טקסט / Text editing')),
        body: ListView(
          padding: const EdgeInsets.all(24),
          children: <Widget>[
            SwitchListTile(
              title: const Text('עברית (RTL) / English (LTR)'),
              value: hebrew,
              onChanged: (bool value) => setState(() => hebrew = value),
            ),
            const Text(
              'Select text, then right-click for the standard editing menu.\n'
              'Try Ctrl+A/C/X/V, Ctrl+Z, Ctrl+Shift+Z, Shift+arrows, '
              'Ctrl+arrows, Home/End, Backspace and Delete.\n'
              'Switch your keyboard layout to type Hebrew; paste emoji into either field.',
              textDirection: TextDirection.ltr,
            ),
            const SizedBox(height: 24),
            TextField(
              controller: controller,
              undoController: undoController,
              autofocus: true,
              minLines: 6,
              maxLines: 10,
              decoration: const InputDecoration(
                border: OutlineInputBorder(),
                labelText: 'טקסט מעורב / Mixed text',
              ),
            ),
            ValueListenableBuilder<UndoHistoryValue>(
              valueListenable: undoController,
              builder: (BuildContext context, UndoHistoryValue value, Widget? child) {
                return Row(
                  children: <Widget>[
                    TextButton(
                      onPressed: value.canUndo ? undoController.undo : null,
                      child: const Text('Undo button'),
                    ),
                    TextButton(
                      onPressed: value.canRedo ? undoController.redo : null,
                      child: const Text('Redo button'),
                    ),
                  ],
                );
              },
            ),
            const SizedBox(height: 24),
            const TextField(
              minLines: 2,
              maxLines: 5,
              decoration: InputDecoration(
                border: OutlineInputBorder(),
                labelText: 'הדביקו כאן / Paste here',
              ),
            ),
            const SizedBox(height: 16),
            Align(
              alignment: AlignmentDirectional.centerStart,
              child: TextButton(
                onPressed: () => controller.text = sample,
                child: const Text('איפוס טקסט / Reset sample'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
