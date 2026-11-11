// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_rust_agent/flutter_rust_agent.dart';

void main() {
  FlutterRustAgent.install();
  runApp(const MaterialApp(title: 'Linux release monitor proof', home: Demo()));
}

class Demo extends StatefulWidget {
  const Demo({super.key});
  @override
  State<Demo> createState() => _DemoState();
}

class _DemoState extends State<Demo> {
  int ticks = 0;
  Timer? timer;
  @override
  void initState() {
    super.initState();
    timer = Timer.periodic(const Duration(seconds: 1), (t) {
      setState(() => ticks++);
      if (ticks == 12) {
        FlutterError.reportError(
          FlutterErrorDetails(
            exception: StateError('intentional-monitor-proof'),
            library: 'monitor fixture',
          ),
        );
      }
    });
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    backgroundColor: const Color(0xff101827),
    body: Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Text(
            'LINUX · RELEASE / AOT',
            style: TextStyle(color: Colors.cyanAccent, fontSize: 24),
          ),
          const SizedBox(height: 24),
          const Text(
            'Persistent MCP monitoring',
            style: TextStyle(color: Colors.white, fontSize: 38),
          ),
          const SizedBox(height: 24),
          Semantics(
            identifier: 'uptime',
            child: Text(
              'App running: ${ticks}s',
              style: const TextStyle(color: Colors.white, fontSize: 30),
            ),
          ),
          const SizedBox(height: 24),
          Text(
            ticks < 12
                ? 'Error scheduled at 12 seconds'
                : 'Known error captured — app keeps running',
            style: TextStyle(color: ticks < 12 ? Colors.amber : Colors.greenAccent, fontSize: 22),
          ),
        ],
      ),
    ),
  );
}
