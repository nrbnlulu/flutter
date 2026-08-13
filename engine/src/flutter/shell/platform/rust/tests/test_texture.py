#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""End-to-end animated wgpu texture test for the Flutter Rust shell."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

DIAGNOSTIC = re.compile(
    r"VUID-|validation error|failed waiting on fences|semaphore.*error|"
    r"invalid (?:Vk)?image|device lost",
    re.IGNORECASE,
)


def clients() -> list[dict[str, object]]:
  result = subprocess.run(
      ["hyprctl", "clients", "-j"],
      check=True,
      capture_output=True,
      text=True,
  )
  return json.loads(result.stdout)


def presentation_count(path: Path) -> int:
  try:
    last = path.read_text().splitlines()[-1]
    return int(last.split()[0])
  except (FileNotFoundError, IndexError, ValueError):
    return 0


def lifecycle_count(path: Path) -> int:
  try:
    return int(path.read_text().split()[0])
  except (FileNotFoundError, IndexError, ValueError):
    return 0


def wait_until(predicate, process: subprocess.Popen[bytes], timeout: float = 10.0):
  deadline = time.monotonic() + timeout
  while time.monotonic() < deadline:
    value = predicate()
    if value:
      return value
    if process.poll() is not None:
      return None
    time.sleep(0.05)
  return None


def main() -> None:
  parser = argparse.ArgumentParser()
  parser.add_argument("--runner", required=True, type=Path)
  parser.add_argument("--assets", required=True, type=Path)
  parser.add_argument("--icu", required=True, type=Path)
  parser.add_argument("--pixel-buffer", action="store_true")
  parser.add_argument("--lifecycle-iterations", type=int, default=0)
  args = parser.parse_args()

  with tempfile.TemporaryDirectory(prefix="flutter-rust-texture-") as directory:
    work = Path(directory)
    texture_id = work / "texture-id"
    presentations = work / "presentations"
    lifecycle_status = work / "lifecycle-status"
    log_path = work / "runner.log"
    first_capture = work / "first.png"
    second_capture = work / "second.png"
    presentations.touch()

    environment = os.environ.copy()
    environment.update({
        "FLUTTER_RUST_TEXTURE_DEMO": "1",
        "FLUTTER_RUST_TEXTURE_ID_FILE": str(texture_id),
        "FLUTTER_RUST_PRESENTATION_STATS": str(presentations),
        "RUST_LOG": "wgpu_core=warn,wgpu_hal=warn",
    })
    if args.pixel_buffer:
      environment["FLUTTER_RUST_PIXEL_BUFFER_TEXTURE_DEMO"] = "1"
    if args.lifecycle_iterations:
      environment.update({
          "FLUTTER_RUST_TEXTURE_LIFECYCLE_ITERATIONS": str(args.lifecycle_iterations),
          "FLUTTER_RUST_TEXTURE_LIFECYCLE_STATUS": str(lifecycle_status),
      })
    if shutil.which("vulkaninfo"):
      vulkan = subprocess.run(["vulkaninfo"], capture_output=True, text=True)
      if "VK_LAYER_KHRONOS_validation" in vulkan.stdout + vulkan.stderr:
        environment["VK_INSTANCE_LAYERS"] = "VK_LAYER_KHRONOS_validation"

    address = None
    with log_path.open("wb") as log:
      process = subprocess.Popen(
          [str(args.runner), str(args.assets), str(args.icu)],
          stdout=log,
          stderr=subprocess.STDOUT,
          env=environment,
      )
      try:

        def initialized():
          nonlocal address
          for client in clients():
            if client.get("pid") == process.pid and client.get("title") == "Flutter Rust Shell":
              address = str(client["address"])
              break
          return address if address and texture_id.exists() and texture_id.stat().st_size else None

        if not wait_until(initialized, process):
          raise RuntimeError("fixture did not initialize")
        if texture_id.read_text().strip() != "1":
          raise RuntimeError(f"Dart expects texture ID 1, got {texture_id.read_text().strip()!r}")

        baseline = presentation_count(presentations)

        if args.lifecycle_iterations:
          client = next(item for item in clients() if str(item.get("address")) == address)
          width, height = client["size"]
          subprocess.run(
              ["hyprctl", "dispatch", "togglefloating", f"address:{address}"],
              check=True,
              stdout=subprocess.DEVNULL,
          )
          resized_generation = 0

          def lifecycle_completed():
            nonlocal resized_generation
            count = lifecycle_count(lifecycle_status)
            milestone = count - count % 5
            if milestone > resized_generation:
              resized_generation = milestone
              subprocess.run(
                  [
                      "hyprctl",
                      "dispatch",
                      "resizewindowpixel",
                      f"exact {640 + milestone * 3} {480 + milestone * 2},address:{address}",
                  ],
                  check=True,
                  stdout=subprocess.DEVNULL,
              )
            return count >= args.lifecycle_iterations

          if not wait_until(
              lifecycle_completed,
              process,
              timeout=max(10.0, args.lifecycle_iterations * 0.5),
          ):
            raise RuntimeError(
                f"completed only {lifecycle_count(lifecycle_status)} of "
                f"{args.lifecycle_iterations} texture lifecycle iterations"
            )
          expected_reclaims = args.lifecycle_iterations

          def reclaimed():
            log.flush()
            return log_path.read_text(errors="replace"
                                     ).count("Flutter Rust external texture") >= expected_reclaims

          if not wait_until(reclaimed, process):
            raise RuntimeError(
                f"fewer than {expected_reclaims} retired texture rings were reclaimed"
            )
          log.flush()
          diagnostics = log_path.read_text(errors="replace")
          if diagnostics.count("Flutter Rust texture context recreation completed") != 1:
            raise RuntimeError("texture context recreation did not complete exactly once")
          if presentation_count(presentations) < baseline + args.lifecycle_iterations:
            raise RuntimeError("live texture replacement produced too few presentations")
        else:
          client = next(item for item in clients() if str(item.get("address")) == address)
          x, y = client["at"]
          width, height = client["size"]
          geometry = f"{x},{y} {width}x{height}"
          subprocess.run(["grim", "-g", geometry, str(first_capture)], check=True)
          if not wait_until(
              lambda: presentation_count(presentations) >= baseline + 10,
              process,
          ):
            raise RuntimeError("animated texture stopped presenting")
          subprocess.run(["grim", "-g", geometry, str(second_capture)], check=True)
          if first_capture.read_bytes() == second_capture.read_bytes():
            raise RuntimeError("presentations advanced but captured pixels did not change")

        subprocess.run(["hyprctl", "dispatch", "closewindow", f"address:{address}"],
                       check=True,
                       stdout=subprocess.DEVNULL)
        process.wait(timeout=5)
        if process.returncode != 0:
          raise RuntimeError(f"runner exited with status {process.returncode}")
        diagnostics = log_path.read_text(errors="replace")
        if DIAGNOSTIC.search(diagnostics):
          raise RuntimeError("Vulkan synchronization diagnostics were reported")
        producer = "pixel-buffer" if args.pixel_buffer else "wgpu"
        lifecycle = (
            f", {args.lifecycle_iterations} lifecycle rotations"
            if args.lifecycle_iterations else ""
        )
        print(
            f"Rust-shell {producer} texture fixture passed "
            f"({presentation_count(presentations)} presentations{lifecycle})."
        )
      except Exception as error:
        log.flush()
        print(log_path.read_text(errors="replace"), end="")
        raise SystemExit(f"Texture fixture failed: {error}; log: {log_path}") from error
      finally:
        if process.poll() is None:
          if address:
            subprocess.run(
                ["hyprctl", "dispatch", "closewindow", f"address:{address}"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
          try:
            process.wait(timeout=5)
          except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


if __name__ == "__main__":
  main()
