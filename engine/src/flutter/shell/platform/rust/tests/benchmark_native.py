# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Platform selection for benchmark CPU, display and window measurements."""
import ctypes
from ctypes import wintypes as W
import json
import re
import sys
import time

from native_desktop import Windows


class MonitorInfo(ctypes.Structure):
  _fields_ = [('size', W.DWORD), ('monitor', W.RECT), ('work', W.RECT), ('flags', W.DWORD),
              ('device', W.WCHAR * 32)]


class DevMode(ctypes.Structure):
  _fields_ = [('device', W.WCHAR * 32), ('spec', W.WORD), ('driver', W.WORD), ('size', W.WORD),
              ('extra', W.WORD), ('fields', W.DWORD), ('position', W.POINT),
              ('orientation', W.DWORD), ('fixed', W.DWORD), ('color', W.SHORT), ('duplex', W.SHORT),
              ('yresolution', W.SHORT), ('ttoption', W.SHORT), ('collate', W.SHORT),
              ('form', W.WCHAR * 32), ('logpixels', W.WORD), ('bits', W.DWORD), ('width', W.DWORD),
              ('height', W.DWORD), ('displayflags', W.DWORD), ('frequency', W.DWORD),
              ('icmmethod', W.DWORD), ('icmintent', W.DWORD), ('mediatype', W.DWORD),
              ('dithertype', W.DWORD), ('reserved1', W.DWORD), ('reserved2', W.DWORD),
              ('panningwidth', W.DWORD), ('panningheight', W.DWORD)]


class ThreadEntry(ctypes.Structure):
  _fields_ = [('size', W.DWORD), ('usage', W.DWORD), ('tid', W.DWORD), ('pid', W.DWORD),
              ('priority', W.LONG), ('delta', W.LONG), ('flags', W.DWORD)]


class WindowsBenchmark:

  def __init__(self):
    self.desktop = Windows()
    self.user = self.desktop.user
    self.kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    self.monitor_callback = ctypes.WINFUNCTYPE(
        W.BOOL, W.HANDLE, W.HDC, ctypes.POINTER(W.RECT), W.LPARAM
    )
    for name, args, result in [
        ('EnumDisplayMonitors', [W.HDC, ctypes.c_void_p, self.monitor_callback, W.LPARAM], W.BOOL),
        ('GetMonitorInfoW', [W.HANDLE, ctypes.POINTER(MonitorInfo)], W.BOOL),
        ('EnumDisplaySettingsW', [W.LPCWSTR, W.DWORD, ctypes.POINTER(DevMode)], W.BOOL),
        ('OpenInputDesktop', [W.DWORD, W.BOOL, W.DWORD], W.HANDLE),
        ('SwitchDesktop', [W.HANDLE], W.BOOL),
        ('CloseDesktop', [W.HANDLE], W.BOOL),
        ('GetWindowRect', [W.HWND, ctypes.POINTER(W.RECT)], W.BOOL),
    ]:
      function = getattr(self.user, name)
      function.argtypes, function.restype = args, result
    for name, args, result in [
        ('OpenProcess', [W.DWORD, W.BOOL, W.DWORD], W.HANDLE),
        ('GetThreadDescription', [W.HANDLE, ctypes.POINTER(W.LPWSTR)], W.LONG),
        ('LocalFree', [W.HANDLE], W.HANDLE),
        ('OpenThread', [W.DWORD, W.BOOL, W.DWORD], W.HANDLE),
        ('CloseHandle', [W.HANDLE], W.BOOL),
        ('GetProcessTimes', [W.HANDLE] + [ctypes.POINTER(W.FILETIME)] * 4, W.BOOL),
        ('GetThreadTimes', [W.HANDLE] + [ctypes.POINTER(W.FILETIME)] * 4, W.BOOL),
        ('CreateToolhelp32Snapshot', [W.DWORD, W.DWORD], W.HANDLE),
        ('Thread32First', [W.HANDLE, ctypes.POINTER(ThreadEntry)], W.BOOL),
        ('Thread32Next', [W.HANDLE, ctypes.POINTER(ThreadEntry)], W.BOOL),
    ]:
      function = getattr(self.kernel, name)
      function.argtypes, function.restype = args, result

  def process_sample(self, pid):

    def seconds(handle, function):
      values = [W.FILETIME() for _ in range(4)]
      if not function(handle, *(ctypes.byref(value) for value in values)):
        raise ctypes.WinError(ctypes.get_last_error())
      return sum((value.dwHighDateTime << 32) | value.dwLowDateTime for value in values[2:]) / 1e7

    handle = self.kernel.OpenProcess(0x1000, False, pid)
    if not handle:
      raise FileNotFoundError(f'Process {pid} is unavailable')
    try:
      cpu = seconds(handle, self.kernel.GetProcessTimes)
    finally:
      self.kernel.CloseHandle(handle)
    threads = {}
    snapshot = self.kernel.CreateToolhelp32Snapshot(4, 0)
    if snapshot == ctypes.c_void_p(-1).value:
      raise ctypes.WinError(ctypes.get_last_error())
    try:
      entry = ThreadEntry()
      entry.size = ctypes.sizeof(entry)
      more = self.kernel.Thread32First(snapshot, ctypes.byref(entry))
      while more:
        if entry.pid == pid:
          thread = self.kernel.OpenThread(0x0800, False, entry.tid)
          if thread:
            try:
              description = W.LPWSTR()
              name = f'thread-{entry.tid}'
              if self.kernel.GetThreadDescription(thread,
                                                  ctypes.byref(description)) >= 0 and description:
                try:
                  name = description.value or name
                finally:
                  self.kernel.LocalFree(ctypes.cast(description, W.HANDLE))
              threads[str(entry.tid)
                     ] = {'name': name, 'cpu_s': seconds(thread, self.kernel.GetThreadTimes)}
            finally:
              self.kernel.CloseHandle(thread)
        more = self.kernel.Thread32Next(snapshot, ctypes.byref(entry))
    finally:
      self.kernel.CloseHandle(snapshot)
    return {'monotonic_s': time.monotonic(), 'cpu_s': cpu, 'threads': threads}

  def unlocked_session(self):
    handle = self.user.OpenInputDesktop(0, False, 0x100)
    if not handle:
      raise RuntimeError('Cannot inspect the interactive desktop; it may be locked')
    try:
      if not self.user.SwitchDesktop(handle):
        raise RuntimeError('Refusing benchmark in a locked session')
    finally:
      self.user.CloseDesktop(handle)
    return {'Type': 'win32', 'Active': 'yes', 'LockedHint': 'no'}

  def monitors(self):
    monitors = []

    @self.monitor_callback
    def visit(handle, dc, rect, unused):
      info = MonitorInfo()
      info.size = ctypes.sizeof(info)
      mode = DevMode()
      mode.size = ctypes.sizeof(mode)
      if self.user.GetMonitorInfoW(handle, ctypes.byref(info)) and self.user.EnumDisplaySettingsW(
          info.device, 0xffffffff, ctypes.byref(mode)):
        r, w = info.monitor, info.work
        monitors.append(
            dict(
                name=info.device,
                id=int(handle),
                x=r.left,
                y=r.top,
                width=r.right - r.left,
                height=r.bottom - r.top,
                scale=1,
                refreshRate=mode.frequency,
                dpmsStatus=True,
                transform=mode.orientation,
                focused=bool(info.flags & 1),
                reserved=[w.left - r.left, w.top - r.top, r.right - w.right, r.bottom - w.bottom]
            )
        )
      return True

    if not self.user.EnumDisplayMonitors(None, None, visit, 0):
      raise ctypes.WinError(ctypes.get_last_error())
    return monitors

  def monitor_snapshot(self):
    return {'returncode': 0, 'stdout': json.dumps(self.monitors()), 'stderr': ''}

  def benchmark_monitor(self, name):
    monitors = self.monitors()
    selected = [m for m in monitors if m['focused']
               ] if name == 'focused' else [m for m in monitors if m['name'] == name]
    if len(selected) != 1:
      raise RuntimeError(f'Cannot select a benchmark monitor: {name}')
    monitor = selected[0]
    if monitor['width'] < 1660 or monitor['height'] - monitor['reserved'][1] < 1010:
      raise RuntimeError('Monitor is too small for thirteen non-overlapping benchmark windows')
    if monitor['transform'] != 0:
      raise RuntimeError('Controlled benchmark placement requires an unrotated monitor')
    return monitor

  def windows(self, pid):
    monitors = self.monitors()
    clients = [c for c in self.desktop.clients() if c['pid'] == pid]
    for client in clients:
      x, y = client['at']
      monitor = next((
          m for m in monitors
          if m['x'] <= x < m['x'] + m['width'] and m['y'] <= y < m['y'] + m['height']
      ), None)
      client.update(
          monitor=monitor['id'] if monitor else None,
          floating=True,
          hidden=False,
          workspace={'id': 0}
      )
    return {'clients': clients, 'monitors': self.monitor_snapshot()}

  def arrange_windows(self, pid, children, monitor):
    deadline = time.monotonic() + 5
    while True:
      clients = self.windows(pid)['clients']
      if len(clients) == children + 1:
        break
      if time.monotonic() > deadline:
        raise RuntimeError('Missing native windows at benchmark preparation')
      time.sleep(.05)
    for client in clients:
      if client['title'] == 'Flutter Rust Shell':
        width, height, column, row = 640, 480, 3, 0
      else:
        match = re.fullmatch(rf'Rust window benchmark {children}/(\d+)', client['title'])
        if not match:
          raise RuntimeError(f'Unexpected fixture window: {client}')
        index = int(match[1])
        width, height, column, row = 320, 240, index % 3, index // 3
      hwnd = int(client['address'])
      outer, inner, origin = W.RECT(), W.RECT(), W.POINT()
      if not self.user.GetWindowRect(hwnd, ctypes.byref(outer)) or not self.user.GetClientRect(
          hwnd, ctypes.byref(inner)) or not self.user.ClientToScreen(hwnd, ctypes.byref(origin)):
        raise ctypes.WinError(ctypes.get_last_error())
      x = monitor['x'] + 10 + column * 330
      y = monitor['y'] + monitor['reserved'][1] + 10 + row * 250
      if not self.user.SetWindowPos(
          hwnd, None, x - (origin.x - outer.left), y - (origin.y - outer.top), width + outer.right -
          outer.left - inner.right, height + outer.bottom - outer.top - inner.bottom, 0x14):
        raise ctypes.WinError(ctypes.get_last_error())


def benchmark_backend():
  if sys.platform == 'win32':
    return WindowsBenchmark()
  if sys.platform.startswith('linux'):
    from benchmark_linux import LinuxBenchmark
    return LinuxBenchmark()
  raise RuntimeError(f'Native benchmark measurements are unavailable on {sys.platform}')
