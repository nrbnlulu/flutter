// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Run with Flutter tools' package configuration, which supplies vm_service.
// ignore_for_file: depend_on_referenced_packages

import 'dart:convert';
import 'dart:io';

import 'package:vm_service/vm_service.dart';
import 'package:vm_service/vm_service_io.dart';

// HTTP on DDS proxies directly to the VM. Real debugger expression evaluation
// must use DDS's WebSocket, where Flutter's compileExpression service lives.
Future<void> main(List<String> args) async {
  final Uri httpUri = Uri.parse(args[0]);
  final Uri websocketUri = httpUri.replace(
    scheme: httpUri.scheme == 'https' ? 'wss' : 'ws',
    path: '${httpUri.path}ws',
  );
  final status = File(args[1]);
  final int line = int.parse(args[2]);
  final VmService service = await vmServiceConnectUri(websocketUri.toString());
  String? isolateId;
  Breakpoint? breakpoint;
  try {
    final VM vm = await service.getVM();
    isolateId = vm.isolates!.singleWhere((IsolateRef i) => i.name == 'main').id!;
    final Isolate isolate = await service.getIsolate(isolateId);
    final LibraryRef library = isolate.rootLib!;
    Future<String?> evaluate(String expression) async {
      final Response result = await service.evaluate(isolateId!, library.id!, expression);
      if (result is! InstanceRef) {
        throw StateError('Unexpected evaluation result: ${result.json}');
      }
      return result.valueAsString;
    }

    if (await evaluate('probeState + 41') != '41' ||
        await evaluate('probeLabel') != 'rlutter-before') {
      throw StateError('DDS expression evaluation returned incorrect values');
    }
    await service.streamListen(EventStreams.kDebug);
    final Future<Event> pause = service.onDebugEvent
        .firstWhere(
          (Event event) =>
              event.kind == EventKind.kPauseBreakpoint && event.isolate?.id == isolateId,
        )
        .timeout(const Duration(seconds: 30));
    breakpoint = await service.addBreakpointWithScriptUri(isolateId, library.uri!, line);
    status.writeAsStringSync(jsonEncode(<String, Object>{'phase': 'breakpoint-ready'}));
    final Event event = await pause;
    if (!event.pauseBreakpoints!.any((Breakpoint b) => b.id == breakpoint!.id)) {
      throw StateError('Stopped at an unexpected breakpoint');
    }
    final Stack stack = await service.getStack(isolateId);
    if (stack.frames!.first.function?.name != 'incrementProbe') {
      throw StateError('Breakpoint did not stop in the fixture function');
    }
    final Response local = await service.evaluateInFrame(isolateId, 0, 'next');
    if (local is! InstanceRef || local.valueAsString != '1') {
      throw StateError('Paused-frame local evaluation returned ${local.json}');
    }
    await service.removeBreakpoint(isolateId, breakpoint.id!);
    breakpoint = null;
    await service.resume(isolateId);
    status.writeAsStringSync(
      jsonEncode(<String, Object>{
        'phase': 'complete',
        'expression': 41,
        'label': 'rlutter-before',
        'paused_function': 'incrementProbe',
        'local_next': 1,
      }),
    );
  } finally {
    if (isolateId != null) {
      if (breakpoint != null) {
        await service.removeBreakpoint(isolateId, breakpoint.id!);
      }
      final Isolate isolate = await service.getIsolate(isolateId);
      if (isolate.pauseEvent?.kind?.startsWith('Pause') ?? false) {
        await service.resume(isolateId);
      }
    }
    await service.dispose();
  }
}
