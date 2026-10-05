// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// ignore_for_file: implementation_imports, invalid_use_of_internal_member

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:flutter/widgets.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runWidget(const _WindowingBenchmark());
}

class _WindowingBenchmark extends StatefulWidget {
  const _WindowingBenchmark();

  @override
  State<_WindowingBenchmark> createState() => _WindowingBenchmarkState();
}

class _WindowingBenchmarkState extends State<_WindowingBenchmark>
    with SingleTickerProviderStateMixin {
  static const List<int> stageSizes = <int>[1, 4, 5, 8, 12];
  static const Duration stageDuration = Duration(seconds: 6);

  late final AnimationController animation = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 2),
  )..repeat();
  final List<WindowController> windows = <WindowController>[];
  final Stopwatch runtime = Stopwatch()..start();
  late final File status = File(Platform.environment['FLUTTER_RUST_WINDOWING_BENCHMARK_STATUS']!);
  late final bool animateEveryView =
      Platform.environment['FLUTTER_RUST_WINDOWING_BENCHMARK_WORKLOAD'] != 'implicit-only';
  int destroyed = 0;
  final List<ui.FrameTiming> timings = <ui.FrameTiming>[];

  void onTimings(List<ui.FrameTiming> batch) => timings.addAll(batch);

  @override
  void initState() {
    super.initState();
    animation.addListener(() => setState(() {}));
    WidgetsBinding.instance.addTimingsCallback(onTimings);
    unawaited(runStages());
  }

  void report(String event, int count, List<int> viewIds, int elapsedMicros) {
    status.writeAsStringSync(
      '$event count=$count views=${viewIds.join(',')} '
      'elapsed_us=$elapsedMicros runtime_us=${runtime.elapsedMicroseconds}\n',
      mode: FileMode.append,
      flush: true,
    );
  }

  void writeTimings(int count, int start, int end) {
    final List<ui.FrameTiming> measured = timings.where((ui.FrameTiming timing) {
      final int timestamp = timing.timestampInMicroseconds(ui.FramePhase.buildStart);
      return timestamp >= start && timestamp < end;
    }).toList();
    File('${status.path}.timings-$count.json').writeAsStringSync(
      jsonEncode({
        'children': count,
        'total_views': count + 1,
        'start_us': start,
        'end_us': end,
        'frames': measured
            .map(
              (ui.FrameTiming timing) => {
                'build_start_us': timing.timestampInMicroseconds(ui.FramePhase.buildStart),
                'build_us': timing.buildDuration.inMicroseconds,
                'raster_us': timing.rasterDuration.inMicroseconds,
                'raster_queue_us':
                    timing.timestampInMicroseconds(ui.FramePhase.rasterStart) -
                    timing.timestampInMicroseconds(ui.FramePhase.buildFinish),
                'vsync_overhead_us': timing.vsyncOverhead.inMicroseconds,
                'total_us': timing.totalSpan.inMicroseconds,
              },
            )
            .toList(),
      }),
    );
  }

  Future<void> runStages() async {
    await Future<void>.delayed(const Duration(milliseconds: 500));
    for (final int count in stageSizes) {
      final int destroyedBefore = destroyed;
      final stage = List<WindowController>.generate(
        count,
        (int index) => WindowController(
          title: 'Rust window benchmark $count/$index',
          size: const Size(320, 240),
          delegate: _BenchmarkDelegate(() => destroyed += 1),
        ),
      );
      setState(() => windows.addAll(stage));
      await WidgetsBinding.instance.endOfFrame.timeout(const Duration(seconds: 10));
      final List<int> viewIds = stage.map((controller) => controller.rootView.viewId).toList();
      if (Platform.environment['FLUTTER_RUST_WINDOWING_BENCHMARK_CONTROL_GEOMETRY'] == '1') {
        report('prepare', count, viewIds, 0);
        final File ready = File('${status.path}.ready-$count');
        final DateTime deadline = DateTime.now().add(const Duration(seconds: 10));
        while (!ready.existsSync()) {
          if (DateTime.now().isAfter(deadline)) {
            throw StateError('Benchmark window placement timed out');
          }
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
      }
      await Future<void>.delayed(const Duration(seconds: 2));
      timings.clear();
      final int timingStart = developer.Timeline.now;
      final stageClock = Stopwatch()..start();
      report('start', count, viewIds, 0);
      await Future<void>.delayed(stageDuration);
      stageClock.stop();
      final int timingEnd = developer.Timeline.now;
      report('end', count, viewIds, stageClock.elapsedMicroseconds);
      // Frame timings are batched by the engine. Drain delivery before filtering
      // by build timestamps, excluding warm-up and this drain from the sample.
      await Future<void>.delayed(const Duration(seconds: 1));
      writeTimings(count, timingStart, timingEnd);

      setState(() => windows.clear());
      await WidgetsBinding.instance.endOfFrame.timeout(const Duration(seconds: 10));
      for (final WindowController controller in stage.reversed) {
        controller.destroy();
      }
      final int expected = destroyedBefore + count;
      final DateTime deadline = DateTime.now().add(const Duration(seconds: 5));
      while (destroyed != expected && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      if (destroyed != expected) {
        throw StateError('Only $destroyed of $expected windows were released');
      }
      for (final controller in stage) {
        controller.dispose();
      }
      await Future<void>.delayed(const Duration(milliseconds: 300));
    }
    status.writeAsStringSync('complete destroyed=$destroyed\n', mode: FileMode.append, flush: true);
    await ServicesBinding.instance.exitApplication(ui.AppExitType.required);
  }

  Widget content({required bool animated}) => ColoredBox(
    color: animated
        ? Color.lerp(const Color(0xff203050), const Color(0xff70b0e0), animation.value)!
        : const Color(0xff203050),
    child: Directionality(
      textDirection: TextDirection.ltr,
      child: Center(
        child: Transform.rotate(
          angle: animated ? animation.value * 6.283185307179586 : 0,
          child: const SizedBox(
            width: 120,
            height: 120,
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: Color(0xffe08040),
                borderRadius: BorderRadius.all(Radius.circular(18)),
              ),
            ),
          ),
        ),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) => ViewCollection(
    views: <Widget>[
      View(view: ui.PlatformDispatcher.instance.implicitView!, child: content(animated: true)),
      for (final WindowController controller in windows)
        View(
          key: ValueKey<int>(controller.rootView.viewId),
          view: controller.rootView,
          child: content(animated: animateEveryView),
        ),
    ],
  );

  @override
  void dispose() {
    animation.dispose();
    WidgetsBinding.instance.removeTimingsCallback(onTimings);
    super.dispose();
  }
}

class _BenchmarkDelegate with WindowControllerDelegate {
  _BenchmarkDelegate(this.released);

  final VoidCallback released;

  @override
  void onWindowDestroyed() => released();
}
