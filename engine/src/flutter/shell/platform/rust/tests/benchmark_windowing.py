#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Preserve per-stage multi-view performance evidence on Linux.

Presentation counters count submissions, not displayed frames. CPU percentages
use one logical core as 100%. GPU utilization is device-wide, including other
applications. Only this fixture's windows are positioned on the selected monitor
before warm-up; no compositor configuration is changed.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

DIAGNOSTIC = re.compile(
    r"VUID-|VALIDATION \[|Validation Error|panicked|\[ERROR\]|EXCEPTION CAUGHT",
    re.IGNORECASE,
)
STAGES = [1, 4, 5, 8, 12]


def cpu_seconds(stat: str) -> float:
  # comm is parenthesized and can contain spaces and parentheses.
  fields = stat.rsplit(")", 1)[1].split()
  return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def process_sample(pid: int) -> dict:
  tasks = {}
  for task in Path(f"/proc/{pid}/task").iterdir():
    try:
      tasks[task.name] = {
          "name": (task / "comm").read_text().strip(),
          "cpu_s": cpu_seconds((task / "stat").read_text()),
      }
    except FileNotFoundError:
      pass  # A worker may exit while /proc is being sampled.
  return {
      "monotonic_s": time.monotonic(),
      "cpu_s": cpu_seconds(Path(f"/proc/{pid}/stat").read_text()),
      "threads": tasks,
  }


def presentation_count(path: Path) -> int:
  try:
    # Ignore a partially written last line.
    lines = path.read_text().splitlines(keepends=True)
    complete = [line for line in lines if line.endswith("\n")]
    return int(complete[-1].split()[0]) if complete else 0
  except FileNotFoundError:
    return 0


def native_timings(path: Path, first: int, last: int) -> dict:
  rows = [
      line.split() for line in path.read_text().splitlines(keepends=True) if line.endswith("\n")
  ]
  rows = [row for row in rows if len(row) >= 7 and first < int(row[0]) <= last]
  result = {
      name: distribution([int(row[index]) / 1000 for row in rows])
      for index, name in enumerate(("acquire_ms", "swapchain_ms", "handoff_ms", "present_ms"),
                                   start=3)
  }
  for counter in ("submits", "rust_submits", "cpp_submits", "image_views", "transients"):
    values = []
    for row in rows:
      counters = dict(token.split("=", 1) for token in row[7:] if "=" in token)
      if counter in counters:
        values.append(float(counters[counter]))
    if values:
      result[f"{counter}_per_frame"] = distribution(values)
  return result


def command_output(command: list[str]) -> dict:
  try:
    result = subprocess.run(command, capture_output=True, text=True, timeout=3)
    return {"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
  except (OSError, subprocess.TimeoutExpired) as error:
    return {"error": str(error)}


def unlocked_session() -> dict:
  sessions = command_output(["loginctl", "list-sessions", "--json=short"])
  try:
    for session in json.loads(sessions.get("stdout", "")):
      if session["uid"] != os.getuid():
        continue
      result = command_output([
          "loginctl", "show-session", session["session"],
          "-p", "Type", "-p", "Active", "-p", "LockedHint"
      ])
      properties = dict(line.split("=", 1) for line in result.get("stdout", "").splitlines())
      if properties.get("Type") in ("wayland", "x11") and properties.get("Active") == "yes":
        if properties.get("LockedHint") != "no":
          raise RuntimeError("Refusing benchmark in a locked session")
        return {"id": session["session"], **properties}
  except (ValueError, KeyError) as error:
    raise RuntimeError(f"Cannot verify session lock state: {sessions}") from error
  raise RuntimeError("Cannot find an active, unlocked graphical session")


def file_hash(path: Path) -> str:
  digest = hashlib.sha256()
  with path.open("rb") as source:
    for chunk in iter(lambda: source.read(1024 * 1024), b""):
      digest.update(chunk)
  return digest.hexdigest()


def windows(pid: int) -> dict:
  if not shutil.which("hyprctl"):
    return {"unavailable": "hyprctl not installed"}
  result = command_output(["hyprctl", "clients", "-j"])
  try:
    clients = json.loads(result.get("stdout", ""))
    return {
        "clients": [client for client in clients if client.get("pid") == pid],
        "monitors": command_output(["hyprctl", "monitors", "-j"])
    }
  except (ValueError, AttributeError):
    return {"unavailable": result}


def benchmark_monitor(name: str) -> dict:
  result = command_output(["hyprctl", "monitors", "-j"])
  monitors = json.loads(result.get("stdout", ""))
  selected = [monitor for monitor in monitors
              if (monitor.get("focused") if name == "focused" else monitor["name"] == name)]
  if len(selected) != 1 or not selected[0].get("dpmsStatus"):
    raise RuntimeError(f"Cannot select an active benchmark monitor: {name}")
  monitor = selected[0]
  if monitor.get("transform") != 0:
    raise RuntimeError("Controlled benchmark placement requires an unrotated monitor")
  width, height = monitor["width"] / monitor["scale"], monitor["height"] / monitor["scale"]
  if width < 1660 or height - monitor["reserved"][1] < 1010:
    raise RuntimeError("Monitor is too small for thirteen non-overlapping benchmark windows")
  return monitor


def arrange_windows(pid: int, children: int, monitor: dict) -> None:
  deadline = time.monotonic() + 5
  while True:
    clients = windows(pid).get("clients", [])
    if len(clients) == children + 1:
      break
    if time.monotonic() >= deadline:
      raise RuntimeError("Missing native windows at benchmark preparation")
    time.sleep(.05)
  probe = command_output(["hyprctl", "eval", 'assert(type(hl.dsp.window.move) == "function")'])
  lua = probe.get("returncode") == 0
  for client in clients:
    title = client["title"]
    if title == "Flutter Rust Shell":
      width, height, column, row = 640, 480, 3, 0
    else:
      match = re.fullmatch(rf"Rust window benchmark {children}/(\d+)", title)
      if match is None:
        raise RuntimeError(f"Unexpected fixture window: {title}")
      index = int(match[1])
      width, height, column, row = 320, 240, index % 3, index // 3
    x, y = monitor["x"] + 10 + column * 330, monitor["y"] + monitor["reserved"][1] + 10 + row * 250
    address = "address:" + client["address"]
    workspace = str(monitor["activeWorkspace"]["id"])
    if lua:
      selector = f"window = {json.dumps(address)}"
      dispatches = [
          f'hl.dsp.window.float({{{selector}, action = "enable"}})',
          f'hl.dsp.window.move({{{selector}, workspace = {json.dumps(workspace)}, follow = false}})',
          f'hl.dsp.window.resize({{{selector}, x = {width}, y = {height}, relative = false}})',
          f'hl.dsp.window.move({{{selector}, x = {x}, y = {y}, relative = false}})',
      ]
      commands = [["hyprctl", "dispatch", dispatch] for dispatch in dispatches]
    else:
      commands = [
          ["hyprctl", "dispatch", "setfloating", address],
          ["hyprctl", "dispatch", "movetoworkspacesilent", f"{workspace},{address}"],
          ["hyprctl", "dispatch", "resizewindowpixel", f"exact {width} {height},{address}"],
          ["hyprctl", "dispatch", "movewindowpixel", f"exact {x} {y},{address}"],
      ]
    for command in commands:
      result = subprocess.run(command, capture_output=True, text=True)
      if result.returncode != 0:
        raise RuntimeError(f"Window placement failed: {command}: {result.stdout} {result.stderr}")


def verify_windows(snapshot: dict, children: int, monitor: dict) -> list:
  clients = snapshot.get("clients", [])
  if len(clients) != children + 1:
    raise RuntimeError("Incomplete native window geometry evidence")
  expected_titles = {"Flutter Rust Shell"} | {f"Rust window benchmark {children}/{i}" for i in range(children)}
  if {client["title"] for client in clients} != expected_titles:
    raise RuntimeError("Unexpected native benchmark windows")
  monitors = json.loads(snapshot.get("monitors", {}).get("stdout", ""))
  actual = next((m for m in monitors if m["name"] == monitor["name"]), None)
  if actual is None or any(actual[key] != monitor[key] for key in ("id", "refreshRate", "scale", "x", "y")):
    raise RuntimeError("Benchmark monitor configuration changed during measurement")
  scene = []
  for client in clients:
    expected_size = [640, 480] if client["title"] == "Flutter Rust Shell" else [320, 240]
    if (client.get("monitor") != monitor["id"] or client.get("size") != expected_size
        or client.get("hidden") or not client.get("floating")):
      raise RuntimeError(f"Unexpected benchmark geometry/visibility: {client}")
    scene.append([client["title"], client["size"], client["at"], monitor["name"],
                  actual["refreshRate"], actual["scale"]])
  return sorted(scene)


def distribution(values: list[float]) -> dict:
  if not values:
    return {"samples": 0}
  ordered = sorted(values)

  def percentile(fraction):
    position = (len(ordered) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)

  return {
      "samples": len(values),
      "mean": statistics.mean(values),
      "p50": percentile(.5),
      "p95": percentile(.95),
      "p99": percentile(.99),
      "max": max(values),
  }


def parse_event(line: str):
  match = re.fullmatch(
      r"(prepare|start|end) count=(\d+) views=([0-9,]+) elapsed_us=(\d+) runtime_us=\d+", line
  )
  if match is None:
    raise ValueError(f"Malformed stage event: {line!r}")
  return match[1], int(match[2]), [int(value) for value in match[3].split(",")], int(match[4])


def summarize_timings(path: Path, budget_ms: float) -> dict:
  timings = json.loads(path.read_text())
  frames = timings["frames"]
  if not frames:
    raise RuntimeError(f"No Flutter frame timings in {path}")
  starts = sorted(set(frame["build_start_us"] for frame in frames))
  metrics = {
      field.replace("_us", "_ms"): distribution([frame[field] / 1000 for frame in frames])
      for field in ("build_us", "raster_us", "raster_queue_us", "vsync_overhead_us", "total_us")
  }
  # A timing record is an engine frame, not an individual view or display flip.
  metrics["engine_frame_interval_ms"] = distribution([
      (b - a) / 1000 for a, b in zip(starts, starts[1:])
  ])
  metrics["build_over_budget_fraction"] = sum(
      frame["build_us"] > budget_ms * 1000 for frame in frames
  ) / len(frames)
  metrics["raster_over_budget_fraction"] = sum(
      frame["raster_us"] > budget_ms * 1000 for frame in frames
  ) / len(frames)
  return metrics


def main() -> None:
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument("--runner", required=True, type=Path)
  parser.add_argument("--assets", required=True, type=Path)
  parser.add_argument("--icu", required=True, type=Path)
  parser.add_argument(
      "--validation",
      action="store_true",
      help="Measure with Vulkan validation; by default it is disabled for performance",
  )
  parser.add_argument("--output-dir", type=Path, help="New directory for retained evidence")
  parser.add_argument("--label", default="unspecified", help="Build/experiment description")
  parser.add_argument(
      "--workload",
      choices=("all", "implicit-only"),
      default="all",
      help="Animate every view or only the implicit view",
  )
  parser.add_argument("--frame-budget-ms", type=float, default=1000 / 60)
  parser.add_argument("--timeout", type=float, default=120)
  parser.add_argument("--repeat", type=int, default=3, help="Independent runs (default: 3)")
  parser.add_argument("--monitor", default="focused", help="Hyprland output name (default: focused)")
  parser.add_argument(
      "--allow-stack-sampling", action="store_true",
      help="Allow eu-stack attachment; use separate profiling runs"
  )
  args = parser.parse_args()
  session = unlocked_session()
  monitor = benchmark_monitor(args.monitor)
  args.monitor = monitor["name"]
  if args.frame_budget_ms <= 0 or args.timeout <= 0 or args.repeat < 1:
    parser.error("frame budget, timeout and repeat must be positive")
  if args.output_dir:
    work = args.output_dir.resolve()
    work.mkdir(parents=True, exist_ok=False)
  else:
    work = Path(tempfile.mkdtemp(prefix="flutter-rust-window-benchmark-"))
  print(f"Evidence: {work}", flush=True)
  if args.repeat > 1:
    for index in range(args.repeat):
      command = [sys.executable, str(Path(__file__).resolve()), "--repeat=1"]
      for name in ("runner", "assets", "icu", "label", "workload", "frame_budget_ms", "timeout", "monitor"):
        command.append(f"--{name.replace('_', '-')}={getattr(args, name)}")
      command.append(f"--output-dir={work / f'run-{index + 1}'}")
      if args.validation:
        command.append("--validation")
      if args.allow_stack_sampling:
        command.append("--allow-stack-sampling")
      subprocess.run(command, check=True)
    return
  stats, status = work / "presentations", work / "status"
  status.touch()
  environment = os.environ.copy()
  environment.update({
      "FLUTTER_RUST_PRESENTATION_STATS": str(stats),
      "FLUTTER_RUST_WINDOWING_BENCHMARK_STATUS": str(status),
      "FLUTTER_RUST_WINDOWING_BENCHMARK_WORKLOAD": args.workload,
      "FLUTTER_RUST_WINDOWING_BENCHMARK_CONTROL_GEOMETRY": "1",
      "RUST_LOG": "wgpu_core=warn,wgpu_hal=warn",
  })
  # Debug builds enable wgpu validation on the Vulkan instance Impeller shares,
  # which dominates multi-view raster time. Measure without it unless asked.
  environment["WGPU_VALIDATION"] = "1" if args.validation else "0"
  environment["WGPU_DEBUG"] = "1" if args.validation else "0"
  if args.validation:
    environment["VK_INSTANCE_LAYERS"] = "VK_LAYER_KHRONOS_validation"
  metadata = {
      "label":
          args.label,
      "workload":
          args.workload,
      "platform":
          platform.platform(),
      "runner":
          str(args.runner.resolve()),
      "runner_mtime_ns":
          args.runner.stat().st_mtime_ns,
      "runner_sha256": file_hash(args.runner),
      "session": session,
      "benchmark_monitor": monitor,
      "allow_stack_sampling": args.allow_stack_sampling,
      "assets":
          str(args.assets.resolve()),
      "kernel_sha256": file_hash(args.assets / "kernel_blob.bin"),
      "frame_budget_ms":
          args.frame_budget_ms,
      "git":
          command_output(["git", "rev-parse", "HEAD"]),
      "git_status":
          command_output(["git", "status", "--short"]),
      "validation_requested":
          args.validation,
      "environment": {k: v for k, v in environment.items() if k.startswith(("VK_", "WGPU_"))},
      "monitors":
          command_output(["hyprctl", "monitors", "-j"]),
      "gpu":
          command_output(["nvidia-smi", "--query-gpu=name,driver_version", "--format=csv"]),
      "notes": [
          "Presentation FPS counts submissions, not display flips.",
          (
              "Every view is animated." if args.workload == "all" else
              "Only the implicit view is animated; child views are static."
          ), "Window sizes and visibility are compositor-controlled; inspect snapshots.",
          "GPU utilization includes other applications.",
          (
              "Vulkan validation forced on." if args.validation else
              "wgpu validation/debug disabled; explicit VK_* layers are still inherited."
          )
      ],
  }
  (work / "metadata.json").write_text(json.dumps(metadata, indent=2))
  results = []
  active = None
  seen_lines = 0
  process = None
  next_gpu_sample = 0.0
  try:
    with (work / "runner.log").open("wb") as log, (work / "samples.jsonl").open("w") as samples:
      command = [str(args.runner.resolve()), str(args.assets.resolve()), str(args.icu.resolve())]
      if args.allow_stack_sampling:
        wrapper = Path(__file__).resolve().parent.parent / "tools" / "exec_ptracer.py"
        command = [sys.executable, str(wrapper), *command]
      process = subprocess.Popen(
          command,
          stdout=log,
          stderr=subprocess.STDOUT,
          env=environment,
      )
      deadline = time.monotonic() + args.timeout
      while True:
        running = process.poll() is None
        lines = status.read_text().splitlines(keepends=True)
        lines = [line.strip() for line in lines if line.endswith("\n")]
        for line in lines[seen_lines:]:
          if line.startswith("complete "):
            continue
          kind, count, view_ids, elapsed_us = parse_event(line)
          unlocked_session()
          if kind == "prepare":
            arrange_windows(process.pid, count, monitor)
            Path(f"{status}.ready-{count}").touch()
            continue
          counts = {
              str(view): presentation_count(Path(f"{stats}.view-{view}")) for view in view_ids
          }
          sample = process_sample(process.pid) if running else None
          if kind == "start":
            if active is not None or count != STAGES[len(results)]:
              raise RuntimeError("Unexpected benchmark stage order")
            snapshot = windows(process.pid)
            verify_windows(snapshot, count, monitor)
            active = {
                "children": count, "start": sample, "counts": counts,
                "implicit_count": presentation_count(stats), "windows_start": snapshot
            }
          else:
            if active is None or active["children"] != count or sample is None:
              raise RuntimeError("Missing stage boundary CPU sample")
            wall = sample["monotonic_s"] - active["start"]["monotonic_s"]
            threads = []
            for tid, end in sample["threads"].items():
              start_cpu = active["start"]["threads"].get(tid, {}).get("cpu_s", 0)
              threads.append({
                  "tid": tid, "name": end["name"],
                  "cpu_percent": max(0, end["cpu_s"] - start_cpu) / wall * 100
              })
            result = {
                "children":
                    count,
                "total_views":
                    count + 1,
                "observed_seconds":
                    wall,
                "dart_stage_seconds":
                    elapsed_us / 1e6,
                "cpu_percent": (sample["cpu_s"] - active["start"]["cpu_s"]) / wall * 100,
                "threads":
                    sorted(threads, key=lambda item: item["cpu_percent"], reverse=True),
                "presentation_fps": {
                    view: (value - active["counts"][view]) / wall for view, value in counts.items()
                },
                "implicit_presentation_fps":
                    (presentation_count(stats) - active["implicit_count"]) / wall,
                "native_timings": {
                    view:
                    native_timings(Path(f"{stats}.view-{view}"), active["counts"][view], value)
                    for view, value in counts.items()
                },
                "implicit_native_timings":
                    native_timings(stats, active["implicit_count"], presentation_count(stats)),
                "windows_start":
                    active["windows_start"],
                "windows_end":
                    windows(process.pid),
            }
            initial = result["windows_start"].get("clients", [])
            final = result["windows_end"].get("clients", [])
            result["scene"] = verify_windows(result["windows_start"], count, monitor)
            if result["scene"] != verify_windows(result["windows_end"], count, monitor):
              raise RuntimeError("Native benchmark geometry changed during measurement")

            def geometry(clients):
              return sorted((
                  client["address"], client.get("size"), client.get("at"), client.get("hidden"),
                  client.get("workspace"), client.get("monitor")
              ) for client in clients)

            result["geometry_changed"] = geometry(initial) != geometry(final)
            result["geometry_observed"] = bool(initial) and bool(final)
            results.append(result)
            active = None
            if result["implicit_presentation_fps"] <= 0:
              raise RuntimeError(f"The animated implicit view stopped in stage {count}")
            if args.workload == "all" and any(
                rate <= 0 for rate in result["presentation_fps"].values()):
              raise RuntimeError(f"A view stopped presenting in stage {count}")
            print(
                f"Measured {count} children: "
                f"{statistics.mean(result['presentation_fps'].values()):.1f} submissions/s/view, "
                f"CPU {result['cpu_percent']:.1f}%",
                flush=True
            )
            if result["geometry_changed"]:
              print(
                  "  Window geometry/visibility changed during measurement; inspect snapshots.",
                  flush=True
              )
        seen_lines = len(lines)
        if not running:
          break
        if time.monotonic() >= deadline:
          (work / "windows-at-timeout.json").write_text(json.dumps(windows(process.pid), indent=2))
          raise TimeoutError("Benchmark timed out")
        try:
          sample = process_sample(process.pid)
          sample["children"] = active["children"] if active else None
          if shutil.which("nvidia-smi") and time.monotonic() >= next_gpu_sample:
            sample["gpu"] = command_output([
                "nvidia-smi",
                "--query-gpu=utilization.gpu,utilization.memory,memory.used,power.draw",
                "--format=csv,noheader,nounits",
            ])
            next_gpu_sample = time.monotonic() + 1
          samples.write(json.dumps(sample) + "\n")
        except FileNotFoundError:
          pass
        time.sleep(.1)
      if process.returncode != 0:
        raise RuntimeError(f"Runner exited with {process.returncode}; see {work / 'runner.log'}")
    output = (work / "runner.log").read_text(errors="replace")
    if DIAGNOSTIC.search(output):
      raise RuntimeError(f"GPU/framework diagnostics; see {work / 'runner.log'}")
    if "complete destroyed=30" not in status.read_text() or len(results) != len(STAGES):
      raise RuntimeError("Incomplete benchmark stages or native removals")
    for result in results:
      result["timings"] = summarize_timings(
          Path(f"{status}.timings-{result['children']}.json"), args.frame_budget_ms
      )
      timings = result["timings"]
      print(
          f"{result['children']:2d} children: "
          f"build p95={timings['build_ms']['p95']:.2f}ms, "
          f"raster p95={timings['raster_ms']['p95']:.2f}ms, "
          f"raster queue p95={timings['raster_queue_ms']['p95']:.2f}ms, "
          f"frame interval p95={timings['engine_frame_interval_ms'].get('p95', 0):.2f}ms"
      )
    (work / "summary.json").write_text(json.dumps(results, indent=2))
  except BaseException as error:
    (work / "failure.txt").write_text(str(error))
    (work / "partial-summary.json").write_text(json.dumps(results, indent=2))
    if process is not None and process.poll() is None:
      (work / "windows-at-failure.json").write_text(json.dumps(windows(process.pid), indent=2))
    raise
  finally:
    if process is not None and process.poll() is None:
      process.terminate()
      try:
        process.wait(timeout=5)
      except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


if __name__ == "__main__":
  main()
