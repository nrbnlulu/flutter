#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs the native multi-view lifecycle fixture with Vulkan validation."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from native_desktop import desktop
from test_texture import wait_until


def main():
  parser = argparse.ArgumentParser()
  parser.add_argument("--runner", required=True, type=Path)
  parser.add_argument("--assets", required=True, type=Path)
  parser.add_argument("--icu", required=True, type=Path)
  parser.add_argument("--timeout", type=float, default=90)
  parser.add_argument("--tooltip-motion", action="store_true")
  args = parser.parse_args()
  directory = tempfile.mkdtemp(prefix="flutter-rust-windowing-")
  print(f"Windowing validation evidence: {directory}", flush=True)
  stats = Path(directory) / "presentations"
  status = Path(directory) / "status"
  environment = os.environ.copy()
  environment.update({
      "VK_INSTANCE_LAYERS": "VK_LAYER_KHRONOS_validation",
      "WGPU_VALIDATION": "1",
      "WGPU_DEBUG": "1",
      "FLUTTER_RUST_PRESENTATION_STATS": str(stats),
      "RUST_BACKTRACE": "1",
      "FLUTTER_RUST_WINDOWING_STATUS": str(status),
  })
  motion = Path(directory) / "motion"
  if args.tooltip_motion:
    environment['FLUTTER_RUST_TOOLTIP_MOTION'] = str(motion)
  log_path = Path(directory) / "runner.log"
  with log_path.open("w", encoding="utf-8") as log:
    process = subprocess.Popen([
        str(args.runner.resolve()),
        str(args.assets.resolve()),
        str(args.icu.resolve())
    ],
                               env=environment,
                               stdout=log,
                               stderr=subprocess.STDOUT)
    try:
      if args.tooltip_motion:
        api = desktop()

        def find_parent():
          return next((
              c for c in api.clients() if c['pid'] == process.pid and c['title'] == 'Owned parent 0'
          ), None)

        parent = wait_until(find_parent, process)
        if not parent:
          raise RuntimeError('Motion fixture parent did not appear')
        address = parent['address']
        scale = api.metrics(address)['scale'] or 1
        initial_size = [round(160 * scale), round(100 * scale)]
        resized_size = [round(200 * scale), round(120 * scale)]

        def find_tooltip():
          return next((
              c for c in api.owned_windows(address)
              if c['pid'] == process.pid and c['size'] == initial_size
          ), None)

        tooltip = wait_until(find_tooltip, process)
        if not tooltip:
          raise RuntimeError('Owned tooltip did not appear')

        def current_tooltip():
          return next((
              c for c in api.clients()
              if c['pid'] == process.pid and c['address'] == tooltip['address']
          ), None)

        api.move(address, 200, 200)
        time.sleep(.2)
        before = current_tooltip()
        parent_before = find_parent()
        api.move(address, 260, 240)

        def moved():
          child, owner = current_tooltip(), find_parent()
          return child and owner and all(
              child['at'][axis] - before['at'][axis] == owner['at'][axis] -
              parent_before['at'][axis] for axis in (0, 1)
          ) and owner['at'] != parent_before['at']

        if not wait_until(moved, process):
          raise RuntimeError('Tooltip did not follow its native parent movement')
        (Path(directory) / 'motion-before.txt').write_text(str(before))
        (Path(directory) / 'motion-after.txt').write_text(str(current_tooltip()))
        before_resize = current_tooltip()
        motion.write_text('move complete')
        if not wait_until(lambda: current_tooltip() and current_tooltip()['size'] == resized_size,
                          process):
          raise RuntimeError('Tooltip did not apply changed content constraints')
        after_resize = current_tooltip()
        expected = [
            before_resize['at'][axis] - (resized_size[axis] - initial_size[axis]) / 2
            for axis in (0, 1)
        ]
        if any(abs(after_resize['at'][axis] - expected[axis]) > 1 for axis in (0, 1)):
          raise RuntimeError('Tooltip size change did not preserve its centered anchor')
        (Path(directory) / 'motion-resized.txt').write_text(str(after_resize))
        # Wait for Dart's acknowledgement before overwriting the shared signal.
        if not wait_until(lambda: motion.read_text() == 'resized', process):
          raise RuntimeError('Dart did not acknowledge tooltip constraint change')
        motion.write_text('anchor')

        def offset_matches(before, dx, dy):
          child = current_tooltip()
          return child and all(
              abs(child['at'][axis] - before['at'][axis] - delta * scale) <= 1
              for axis, delta in enumerate((dx, dy))
          )

        if not wait_until(lambda: offset_matches(after_resize, 27, 24), process):
          raise RuntimeError('Explicit anchor/positioner update did not move the same tooltip')
        after_anchor = current_tooltip()
        (Path(directory) / 'anchor-after.txt').write_text(str(after_anchor))
        if not wait_until(lambda: motion.read_text() == 'anchored', process):
          raise RuntimeError('Dart did not acknowledge anchor update')
        motion.write_text('partial')
        if not wait_until(lambda: offset_matches(after_anchor, -10, -7), process):
          raise RuntimeError('Positioner-only update did not preserve the previous anchor')
        if not wait_until(lambda: motion.read_text() == 'partial complete', process):
          raise RuntimeError('Dart did not acknowledge partial position update')
        motion.write_text('complete')
        print(
            'Tooltip parent movement, anchored resizing and explicit/partial position updates passed.'
        )
      returncode = process.wait(timeout=args.timeout)
    finally:
      if process.poll() is None:
        process.kill()
        process.wait()
  output = log_path.read_text(encoding="utf-8", errors="replace")
  print(output)
  if returncode != 0:
    raise RuntimeError(f"runner exited with {returncode}")
  if not status.exists() or status.read_text() != "complete released=60":
    raise RuntimeError("fixture did not complete all native removals")
  if re.search(r"VUID-|VALIDATION \[|Validation Error|panicked|\[ERROR\]|EXCEPTION CAUGHT", output,
               re.IGNORECASE):
    raise RuntimeError("GPU or framework diagnostics during multi-view lifecycle")
  for view_id in range(1, 61):
    view_stats = Path(str(stats) + f".view-{view_id}")
    if not view_stats.exists() or not view_stats.read_text().strip():
      raise RuntimeError(f"Created view {view_id} did not present a frame")
  frames = stats.read_text().splitlines()
  if len(frames) < 60:
    raise RuntimeError(f"primary view only presented {len(frames)} frames")
  print(f"Multi-view lifecycle passed: 60 removals, {len(frames)} primary presentations.")


if __name__ == "__main__":
  main()
