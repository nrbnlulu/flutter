#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Checks that benchmark accounting cannot silently produce misleading results."""

import json
import os
from pathlib import Path
import tempfile
import unittest

import benchmark_windowing as benchmark


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


if __name__ == "__main__":
  unittest.main()
