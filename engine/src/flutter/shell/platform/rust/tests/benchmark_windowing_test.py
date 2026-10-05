#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Checks that benchmark accounting cannot silently produce misleading results."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import benchmark_windowing as benchmark
import compare_benchmarks as comparator


class AccountingTest(unittest.TestCase):

  def test_process_name_with_spaces_and_parentheses(self):
    fields = ["S"] + ["0"] * 10 + ["150", "50"] + ["0"] * 10
    stat = "123 (name with ) parentheses) " + " ".join(fields)
    self.assertEqual(benchmark.cpu_seconds(stat), 200 / os.sysconf("SC_CLK_TCK"))

  def test_partial_presentation_write_is_ignored(self):
    with tempfile.TemporaryDirectory() as directory:
      path = Path(directory) / "frames"
      path.write_text("1 320 240\n2 320 240\n3 32")
      self.assertEqual(benchmark.presentation_count(path), 2)

  def test_percentiles_and_empty_samples(self):
    self.assertEqual(benchmark.distribution([]), {"samples": 0})
    self.assertEqual(benchmark.distribution([10, 30, 20])["p50"], 20)
    self.assertEqual(benchmark.distribution([10])["p99"], 10)

  def test_native_timings_exclude_warmup_drain_and_partial_rows(self):
    with tempfile.TemporaryDirectory() as directory:
      native = Path(directory) / "frames"
      native.write_text(
          "1 320 240 9000 8000 7000 6000\n"
          "2 320 240 2000 1000 3000 500\n"
          "3 320 240 9000 8000 7000 6000\n4 320"
      )
      timings = benchmark.native_timings(native, 1, 2)
      self.assertEqual(timings["acquire_ms"]["mean"], 2)
      self.assertEqual(timings["swapchain_ms"]["mean"], 1)
      self.assertEqual(timings["handoff_ms"]["mean"], 3)
      self.assertEqual(timings["present_ms"]["mean"], .5)
      self.assertEqual(benchmark.native_timings(native, 3, 3)["acquire_ms"], {"samples": 0})

  def test_timings_units_and_budget(self):
    with tempfile.TemporaryDirectory() as directory:
      path = Path(directory) / "timings.json"
      path.write_text(
          json.dumps({
              "frames": [{
                  "build_start_us": timestamp, "build_us": build, "raster_us": 1000,
                  "raster_queue_us": 300, "vsync_overhead_us": 200, "total_us": build + 1500
              } for timestamp, build in [(100000, 2000), (120000, 18000)]]
          })
      )
      summary = benchmark.summarize_timings(path, 16.67)
      self.assertEqual(summary["engine_frame_interval_ms"]["mean"], 20)
      self.assertEqual(summary["build_over_budget_fraction"], .5)
      self.assertEqual(summary["raster_ms"]["mean"], 1)
      path.write_text('{"frames": []}')
      with self.assertRaises(RuntimeError):
        benchmark.summarize_timings(path, 16.67)

  def test_all_submission_and_resource_counters(self):
    with tempfile.TemporaryDirectory() as directory:
      path = Path(directory) / "frames"
      path.write_text(
          "1 320 240 10 20 30 40 submits=6 rust_submits=2 cpp_submits=4 "
          "image_views=1 transients=1\n"
          "2 320 240 10 20 30 40 submits=7 rust_submits=2 cpp_submits=5 "
          "image_views=1 transients=0\n"
      )
      result = benchmark.native_timings(path, 0, 2)
      self.assertEqual(result["submits_per_frame"]["mean"], 6.5)
      self.assertEqual(result["cpp_submits_per_frame"]["mean"], 4.5)
      self.assertEqual(result["transients_per_frame"]["mean"], .5)

  def test_thread_names_use_linux_truncation(self):
    stage = {
        "threads": [
            {"name": "FlutterRust.ras", "cpu_percent": 60},
            {"name": "IplrVkFenceWait", "cpu_percent": 30},
            {"name": "unrelated", "cpu_percent": 90},
        ]
    }
    self.assertEqual(comparator.get_by_path(stage, "raster_thread_cpu_percent"), 60)
    self.assertEqual(comparator.get_by_path(stage, "fence_waiter_cpu_percent"), 30)

  def test_partial_evidence_cannot_pass(self):
    with tempfile.TemporaryDirectory() as directory:
      path = Path(directory)
      (path / "partial-summary.json").write_text("[]")
      with self.assertRaises(FileNotFoundError):
        comparator.load_summary(path)

  def test_zero_baseline_cannot_hide_a_regression(self):
    result = comparator.compare_metric([0, 0, 0], [1, 1, 1], True, "CPU", 12)
    self.assertTrue(result["regression"])

  def test_missing_metric_has_consistent_result(self):
    result = comparator.compare_metric([], [1], True, "CPU", 12)
    self.assertEqual(result["verdict"], "MISSING")
    self.assertIsNone(result["base"]["median"])

  def test_locked_session_is_rejected(self):
    outputs = [
        {"stdout": json.dumps([{"uid": os.getuid(), "session": "1"}])},
        {"stdout": "Type=wayland\nActive=yes\nLockedHint=yes\n"},
    ]
    with patch.object(benchmark, "command_output", side_effect=outputs):
      with self.assertRaisesRegex(RuntimeError, "locked"):
        benchmark.unlocked_session()

  def test_comparator_rejects_missing_guard_even_with_primary_win(self):
    with tempfile.TemporaryDirectory() as directory:
      base, candidate = Path(directory) / "base", Path(directory) / "candidate"
      for path, raster in ((base, 20), (candidate, 10)):
        path.mkdir()
        (path / "summary.json").write_text(
            json.dumps([{
                "children": n, "scene": list(range(n + 1)),
                "timings": {"raster_ms": {"p50": raster}}
            } for n in benchmark.STAGES])
        )
      result = subprocess.run(
          [sys.executable, comparator.__file__,
           str(base), "--", str(candidate)],
          capture_output=True,
          text=True,
      )
      self.assertEqual(result.returncode, 1)
      self.assertIn("required metrics are missing", result.stdout)
      self.assertNotIn("KEEP:", result.stdout)

  def test_identical_complete_runs_revert(self):
    with tempfile.TemporaryDirectory() as directory:
      path = Path(directory)
      native = {name: {"p95": 1} for name in ("acquire_ms", "handoff_ms", "present_ms")}
      (path / "summary.json").write_text(
          json.dumps([{
              "children": n,
              "scene": list(range(n + 1)),
              "cpu_percent": 100,
              "threads": [{"name": name, "cpu_percent": 20}
                          for name in ("FlutterRust.ras", "IplrVkFenceWait")],
              "timings": {
                  "build_ms": {"p95": 1},
                  "raster_ms": {"p50": 10, "p95": 12},
                  "raster_queue_ms": {"p95": 1},
              },
              "implicit_native_timings": native,
              "native_timings": {str(view): native
                                 for view in range(n)},
              "presentation_fps": {str(view): 75
                                   for view in range(n)},
          }
                      for n in benchmark.STAGES])
      )
      result = subprocess.run(
          [sys.executable, comparator.__file__,
           str(path), "--", str(path)],
          capture_output=True,
          text=True,
      )
      self.assertEqual(result.returncode, 1)
      self.assertIn("no primary metric improvement", result.stdout)
      self.assertNotIn("MISSING", result.stdout)

  def test_mixed_monitor_or_wrong_size_is_rejected(self):
    monitor = {"id": 1, "name": "DP-1", "refreshRate": 75, "scale": 1, "x": 0, "y": 0}
    snapshot = {
        "monitors": {"stdout": json.dumps([monitor])}, "clients": [
            {
                "title": "Flutter Rust Shell", "monitor": 1, "size": [640, 480], "at": [1000, 36],
                "floating": True, "hidden": False
            },
            {
                "title": "Rust window benchmark 1/0", "monitor": 0, "size": [320, 240],
                "at": [10, 36], "floating": True, "hidden": False
            },
        ]
    }
    with self.assertRaisesRegex(RuntimeError, "geometry/visibility"):
      benchmark.verify_windows(snapshot, 1, monitor)
    snapshot["clients"][1]["monitor"] = 1
    self.assertEqual(len(benchmark.verify_windows(snapshot, 1, monitor)), 2)
    snapshot["clients"][1]["size"] = [1, -6]
    with self.assertRaisesRegex(RuntimeError, "geometry/visibility"):
      benchmark.verify_windows(snapshot, 1, monitor)


if __name__ == "__main__":
  unittest.main()
