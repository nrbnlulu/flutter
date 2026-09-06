#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""End-to-end FRB isolate dispatch and texture test for the Rust shell."""

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


def wait_until(predicate, process: subprocess.Popen[bytes], timeout: float = 15.0):
  deadline = time.monotonic() + timeout
  while time.monotonic() < deadline:
    value = predicate()
    if value:
      return value
    if process.poll() is not None:
      return None
    time.sleep(0.05)
  return None


def read_status(path: Path) -> dict[str, object] | None:
  try:
    return json.loads(path.read_text())
  except (FileNotFoundError, json.JSONDecodeError):
    return None


def main() -> None:
  parser = argparse.ArgumentParser()
  parser.add_argument("--runner", required=True, type=Path)
  parser.add_argument("--assets", required=True, type=Path)
  parser.add_argument("--icu", required=True, type=Path)
  args = parser.parse_args()

  with tempfile.TemporaryDirectory(prefix="flutter-rust-frb-") as directory:
    work = Path(directory)
    status_path = work / "status.json"
    log_path = work / "runner.log"
    gpu_first = work / "gpu-first.png"
    gpu_second = work / "gpu-second.png"
    pixels_first = work / "pixels-first.png"
    pixels_second = work / "pixels-second.png"
    environment = os.environ.copy()
    environment.update({
        "FLUTTER_RUST_ASSETS_PATH": str(args.assets),
        "FLUTTER_RUST_ICU_DATA": str(args.icu),
        "FLUTTER_RUST_FRB_STATUS": str(status_path),
        "RUST_LOG": "wgpu_core=warn,wgpu_hal=warn",
    })
    if shutil.which("vulkaninfo"):
      vulkan = subprocess.run(["vulkaninfo"], capture_output=True, text=True)
      if "VK_LAYER_KHRONOS_validation" in vulkan.stdout + vulkan.stderr:
        environment["VK_INSTANCE_LAYERS"] = "VK_LAYER_KHRONOS_validation"

    address = None
    with log_path.open("wb") as log:
      process = subprocess.Popen(
          [str(args.runner)],
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
          status = read_status(status_path)
          return address if address and status else None

        if not wait_until(initialized, process):
          raise RuntimeError("FRB fixture did not initialize")

        status = read_status(status_path)
        if status is None:
          raise RuntimeError("FRB fixture status was not readable")
        if "error" in status:
          raise RuntimeError(
              f"Dart fixture failed: {status['error']}\n{status.get('stackTrace', '')}"
          )
        background = status["background"]
        expected = {
            "synchronousCallerWasMainThread": True,
            "isolateName": "flutter-rust-frb-background",
            "callerWasMainThread": False,
            "callbackWasMainThread": True,
            "synchronousCallbackWasDeferred": True,
        }
        actual = {
            "synchronousCallerWasMainThread": status["synchronousCallerWasMainThread"],
            "isolateName": background["isolateName"],
            "callerWasMainThread": background["callerWasMainThread"],
            "callbackWasMainThread": background["callbackWasMainThread"],
            "synchronousCallbackWasDeferred": background["synchronousCallbackWasDeferred"],
        }
        if actual != expected:
          raise RuntimeError(f"unexpected FRB dispatch result: {actual!r}")

        gpu_id = status["gpuTextureId"]
        pixel_id = status["pixelTextureId"]
        if not isinstance(gpu_id, int) or not isinstance(pixel_id, int):
          raise RuntimeError(f"invalid texture IDs: {gpu_id!r}, {pixel_id!r}")
        if gpu_id <= 0 or pixel_id <= 0 or gpu_id == pixel_id:
          raise RuntimeError(f"unexpected texture IDs: {gpu_id}, {pixel_id}")
        if background["gpuTextureId"] != gpu_id or background["pixelTextureId"] != pixel_id:
          raise RuntimeError("background isolate observed different texture IDs")

        def frames_ready(minimum_gpu: int, minimum_pixels: int):
          current = read_status(status_path)
          if current is None:
            return None
          return (
              current if int(current.get("gpuFrames", 0)) >= minimum_gpu and
              int(current.get("pixelFrames", 0)) >= minimum_pixels else None
          )

        baseline = wait_until(lambda: frames_ready(2, 2), process)
        if baseline is None:
          raise RuntimeError("both FRB texture producers did not start")

        client = next(item for item in clients() if str(item.get("address")) == address)
        x, y = client["at"]
        width, height = client["size"]
        left_width = width // 2
        right_width = width - left_width
        subprocess.run(
            ["grim", "-g", f"{x},{y} {left_width}x{height}",
             str(gpu_first)],
            check=True,
        )
        subprocess.run(
            [
                "grim",
                "-g",
                f"{x + left_width},{y} {right_width}x{height}",
                str(pixels_first),
            ],
            check=True,
        )

        gpu_baseline = int(baseline["gpuFrames"])
        pixel_baseline = int(baseline["pixelFrames"])
        if not wait_until(
            lambda: frames_ready(gpu_baseline + 10, pixel_baseline + 10),
            process,
        ):
          raise RuntimeError(
              "both FRB texture producers did not keep presenting: "
              f"baseline=({gpu_baseline}, {pixel_baseline}), "
              f"latest={read_status(status_path)!r}"
          )

        subprocess.run(
            ["grim", "-g", f"{x},{y} {left_width}x{height}",
             str(gpu_second)],
            check=True,
        )
        subprocess.run(
            [
                "grim",
                "-g",
                f"{x + left_width},{y} {right_width}x{height}",
                str(pixels_second),
            ],
            check=True,
        )
        if gpu_first.read_bytes() == gpu_second.read_bytes():
          raise RuntimeError("wgpu texture frames advanced but its pixels did not change")
        if pixels_first.read_bytes() == pixels_second.read_bytes():
          raise RuntimeError("pixel-buffer frames advanced but its pixels did not change")

        subprocess.run(
            ["hyprctl", "dispatch", "closewindow", f"address:{address}"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        if process.wait(timeout=10) != 0:
          raise RuntimeError("FRB fixture exited unsuccessfully")
        log.flush()
        if DIAGNOSTIC.search(log_path.read_text(errors="replace")):
          raise RuntimeError("Vulkan synchronization diagnostics were reported")
      except Exception:
        if process.poll() is None:
          process.terminate()
          try:
            process.wait(timeout=5)
          except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        log.flush()
        print(log_path.read_text(errors="replace"))
        raise

    print("Rust-shell FRB dispatch, wgpu texture, and pixel-buffer fixture passed.")


if __name__ == "__main__":
  main()
