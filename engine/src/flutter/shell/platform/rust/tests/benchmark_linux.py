# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Linux benchmark measurements and controlled Hyprland placement."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


def cpu_seconds(stat: str, ticks=None) -> float:
  # comm is parenthesized and can contain spaces and parentheses.
  fields = stat.rsplit(")", 1)[1].split()
  return (int(fields[11]) + int(fields[12])) / (
      os.sysconf("SC_CLK_TCK") if ticks is None else ticks
  )


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
          "loginctl", "show-session", session["session"], "-p", "Type", "-p", "Active", "-p",
          "LockedHint"
      ])
      properties = dict(line.split("=", 1) for line in result.get("stdout", "").splitlines())
      if properties.get("Type") in ("wayland", "x11") and properties.get("Active") == "yes":
        if properties.get("LockedHint") != "no":
          raise RuntimeError("Refusing benchmark in a locked session")
        return {"id": session["session"], **properties}
  except (ValueError, KeyError) as error:
    raise RuntimeError(f"Cannot verify session lock state: {sessions}") from error
  raise RuntimeError("Cannot find an active, unlocked graphical session")


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
  selected = [
      monitor for monitor in monitors
      if (monitor.get("focused") if name == "focused" else monitor["name"] == name)
  ]
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


class LinuxBenchmark:
  cpu_seconds = staticmethod(cpu_seconds)
  process_sample = staticmethod(process_sample)
  command_output = staticmethod(command_output)
  unlocked_session = staticmethod(unlocked_session)
  windows = staticmethod(windows)
  monitor_snapshot = staticmethod(lambda: command_output(["hyprctl", "monitors", "-j"]))
  benchmark_monitor = staticmethod(benchmark_monitor)
  arrange_windows = staticmethod(arrange_windows)
