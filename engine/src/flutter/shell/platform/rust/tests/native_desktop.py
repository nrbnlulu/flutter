# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Native desktop automation used by Rust-shell runtime validation.

Coordinates use the native compositor (physical pixels on Win32, global points on macOS). Windows are selected by PID by
callers; adapters never operate on unrelated application windows.
"""
from __future__ import annotations

from abc import ABC, abstractmethod
import ctypes
from ctypes import wintypes as W
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import time
import zlib


def runner_environment(environment: dict) -> dict:
  """Returns `environment` adjusted for launching the native runner.

  macOS strips DYLD_* from processes started through protected binaries such
  as /usr/bin/python3, and Homebrew's Vulkan validation-layer manifest names
  its library by leaf name, so the (unprotected) runner needs an explicit
  library search path. FLUTTER_RUST_DYLD_LIBRARY_PATH overrides the default.
  """
  if sys.platform == 'darwin' and 'DYLD_FALLBACK_LIBRARY_PATH' not in environment:
    environment['DYLD_FALLBACK_LIBRARY_PATH'] = environment.get(
        'FLUTTER_RUST_DYLD_LIBRARY_PATH', '/opt/homebrew/lib:/usr/local/lib:/usr/lib'
    )
  return environment


class Desktop(ABC):

  def resize_client(self, address, width, height):
    """Resize the viewport in the native units reported by clients()."""
    self.action('resize', address, width=width, height=height)

  def click(self, address, x, y):
    raise NotImplementedError('Pointer injection is unavailable on this desktop')

  def chord(self, address, *keys):
    raise NotImplementedError('Keyboard injection is unavailable on this desktop')

  def metrics(self, address):
    client = next(c for c in self.clients() if str(c['address']) == str(address))
    return dict(size=client['size'], scale=None, coordinate_space='physical')

  def move(self, address, x, y):
    raise NotImplementedError('Native window movement is unavailable')

  def owned_windows(self, address):
    raise NotImplementedError('Native owner lookup is unavailable')

  def ime_layouts(self):
    return None

  def monitor_work_areas(self):
    return []

  def live_resize(self, address, progress):
    # Compositor resize operations are already exercised by the shared burst.
    return None

  def capture_window(self, address, path, region=None):
    client = next(c for c in self.clients() if str(c['address']) == str(address))
    x, y = client['at']
    if region is None:
      region = (0, 0, *client['size'])
    left, top, width, height = region
    self.capture((x + left, y + top, width, height), path)

  @abstractmethod
  def clients(self) -> list[dict]:
    ...

  @abstractmethod
  def action(self, action: str, address: str, *, width=0, height=0):
    ...

  @abstractmethod
  def capture(self, rect: tuple[int, int, int, int], path: Path):
    ...


class Hyprland(Desktop):

  def __init__(self):
    if not shutil.which('hyprctl'):
      raise RuntimeError('Linux desktop validation requires Hyprland and hyprctl')
    probe = subprocess.run(['hyprctl', 'eval', 'assert(type(hl.dsp.window.close) == "function")'],
                           capture_output=True,
                           text=True)
    self.lua = probe.returncode == 0

  def clients(self):
    return json.loads(subprocess.check_output(['hyprctl', 'clients', '-j'], text=True))

  def action(self, action, address, *, width=0, height=0):
    selector = f'address:{address}'
    if action == 'tab':
      self.chord(address, 'Tab')
      return
    if self.lua:
      options = f'window = {json.dumps(selector)}'
      dispatcher = 'hl.dsp.focus' if action == 'focus' else f'hl.dsp.window.{action}'
      if action == 'resize':
        options += f', x = {width}, y = {height}, relative = false'
      elif action == 'float':
        options += ', action = "enable"'
      elif action in ('hide', 'restore'):
        if action == 'hide':
          client = next(c for c in self.clients() if str(c['address']) == address)
          self.workspace = client['workspace']['id']
        workspace = 'special:rust-shell-stress' if action == 'hide' else str(self.workspace)
        options += f', workspace = {json.dumps(workspace)}, follow = false'
        dispatcher = 'hl.dsp.window.move'
      command = ['hyprctl', 'dispatch', f'{dispatcher}({{{options}}})']
    else:
      names = dict(
          close='closewindow',
          float='setfloating',
          resize='resizewindowpixel',
          focus='focuswindow',
          tab='sendshortcut',
          hide='movetoworkspacesilent',
          restore='movetoworkspacesilent',
          fullscreen='fullscreen'
      )
      argument = selector
      if action == 'resize':
        argument = f'exact {width} {height},{selector}'
      elif action == 'tab':
        argument = f',TAB,{selector}'
      elif action in ('hide', 'restore'):
        client = next(c for c in self.clients() if str(c['address']) == address)
        if action == 'hide':
          self.workspace = client['workspace']['id']
        argument = f'{"special:rust-shell-stress" if action == "hide" else self.workspace},{selector}'
      elif action == 'fullscreen':
        argument = f'0,{selector}'
      command = ['hyprctl', 'dispatch', names[action], argument]
    subprocess.run(command, check=True, capture_output=True, text=True)

  def metrics(self, address):
    result = super().metrics(address)
    result['coordinate_space'] = 'logical'
    return result

  def click(self, address, x, y):
    self.action('focus', address)
    command = ['hyprctl', 'dispatch']
    if self.lua:
      command += [f'hl.dsp.cursor.move({{x = {round(x)}, y = {round(y)}}})']
    else:
      command += ['movecursor', str(round(x)), str(round(y))]
    subprocess.run(command, check=True)
    # A window selector makes Hyprland reset the pointer position to (1, 1).
    # The cursor is already on our focused window; preserve its hit-test point.
    command = ['hyprctl', 'dispatch']
    if self.lua:
      command += ['hl.dsp.send_shortcut({mods = "", key = "mouse:272"})']
    else:
      command += ['sendshortcut', ',mouse:272']
    subprocess.run(command, check=True)

  def chord(self, address, *keys):
    self.action('focus', address)
    modifiers = [key.upper() for key in keys[:-1]]
    command = ['hyprctl', 'dispatch']
    if self.lua:
      options = (
          f'mods = {json.dumps(" ".join(modifiers))}, key = {json.dumps(keys[-1])}, '
          f'window = {json.dumps(f"address:{address}")}'
      )
      command += [f'hl.dsp.send_shortcut({{{options}}})']
    else:
      command += ['sendshortcut', ','.join([' '.join(modifiers), keys[-1], f'address:{address}'])]
    subprocess.run(command, check=True)

  def capture(self, rect, path):
    if not shutil.which('grim'):
      raise RuntimeError('Pixel validation on Hyprland requires grim')
    x, y, width, height = rect
    subprocess.run(['grim', '-g', f'{x},{y} {width}x{height}', str(path)], check=True)


def png_rgba(path: Path, width: int, height: int, rows: bytes):
  """Write deterministic PNGs so byte comparisons compare pixels, not metadata."""

  def chunk(kind, data):
    return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))

  stride = width * 4
  filtered = b''.join(b'\0' + rows[y * stride:(y + 1) * stride] for y in range(height))
  path.write_bytes(
      b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!2I5B', width, height, 8, 6, 0, 0, 0)) +
      chunk(b'IDAT', zlib.compress(filtered)) + chunk(b'IEND', b'')
  )


class Windows(Desktop):

  def __init__(self):
    self.user = ctypes.WinDLL('user32', use_last_error=True)
    self.gdi = ctypes.WinDLL('gdi32', use_last_error=True)
    self.callback = ctypes.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
    signatures = {
        'EnumWindows': ([self.callback, W.LPARAM], W.BOOL),
        'IsWindowVisible': ([W.HWND], W.BOOL),
        'GetWindowThreadProcessId': ([W.HWND, ctypes.POINTER(W.DWORD)], W.DWORD),
        'GetWindowTextW': ([W.HWND, W.LPWSTR, ctypes.c_int], ctypes.c_int),
        'GetClientRect': ([W.HWND, ctypes.POINTER(W.RECT)], W.BOOL),
        'ClientToScreen': ([W.HWND, ctypes.POINTER(W.POINT)], W.BOOL),
        'PostMessageW': ([W.HWND, W.UINT, W.WPARAM, W.LPARAM], W.BOOL),
        'SetWindowPos':
            ([W.HWND, W.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
              W.UINT], W.BOOL),
        'ShowWindow': ([W.HWND, ctypes.c_int], W.BOOL),
        'SetForegroundWindow': ([W.HWND], W.BOOL),
        'GetForegroundWindow': ([], W.HWND),
        'WindowFromPoint': ([W.POINT], W.HWND),
        'GetAncestor': ([W.HWND, W.UINT], W.HWND),
        'GetWindowRect': ([W.HWND, ctypes.POINTER(W.RECT)], W.BOOL),
        'AttachThreadInput': ([W.DWORD, W.DWORD, W.BOOL], W.BOOL),
        'BringWindowToTop': ([W.HWND], W.BOOL),
        'SetCursorPos': ([ctypes.c_int, ctypes.c_int], W.BOOL),
        'mouse_event': ([W.DWORD, W.DWORD, W.DWORD, W.DWORD, ctypes.c_size_t], None),
        'GetDC': ([W.HWND], W.HDC),
        'ReleaseDC': ([W.HWND, W.HDC], ctypes.c_int),
        'keybd_event': ([W.BYTE, W.BYTE, W.DWORD, ctypes.c_size_t], None),
    }
    for name, (args, result) in signatures.items():
      fn = getattr(self.user, name)
      fn.argtypes, fn.restype = args, result
    for name, args, result in [
        ('CreateCompatibleDC', [W.HDC], W.HDC),
        ('CreateCompatibleBitmap', [W.HDC, ctypes.c_int, ctypes.c_int], W.HBITMAP),
        ('SelectObject', [W.HDC, W.HANDLE], W.HANDLE),
        ('DeleteObject', [W.HANDLE], W.BOOL),
        ('DeleteDC', [W.HDC], W.BOOL),
        ('BitBlt', [W.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, W.HDC,
                    ctypes.c_int, ctypes.c_int, W.DWORD], W.BOOL),
        ('GetDIBits', [W.HDC, W.HBITMAP, W.UINT, W.UINT, ctypes.c_void_p, ctypes.c_void_p,
                       W.UINT], ctypes.c_int),
    ]:
      fn = getattr(self.gdi, name)
      fn.argtypes, fn.restype = args, result
    # Match the per-monitor-aware runner, including screen captures at 150% DPI.
    self.user.SetThreadDpiAwarenessContext.argtypes = [W.HANDLE]
    self.user.SetThreadDpiAwarenessContext.restype = W.HANDLE
    self.user.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4))

  def clients(self):
    result = []

    @self.callback
    def visit(hwnd, unused):
      if self.user.IsWindowVisible(hwnd):
        pid = W.DWORD()
        self.user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        title = ctypes.create_unicode_buffer(1024)
        self.user.GetWindowTextW(hwnd, title, len(title))
        rect, origin = W.RECT(), W.POINT()
        # Winit's event-loop helper may be marked visible but has no client
        # surface. It must never be counted, resized, captured, or closed.
        if (self.user.GetClientRect(hwnd, ctypes.byref(rect)) and rect.right > 0 and
            rect.bottom > 0 and self.user.ClientToScreen(hwnd, ctypes.byref(origin))):
          result.append(
              dict(
                  address=str(hwnd),
                  pid=pid.value,
                  title=title.value,
                  at=[origin.x, origin.y],
                  size=[rect.right, rect.bottom]
              )
          )
      return True

    if not self.user.EnumWindows(visit, 0):
      raise ctypes.WinError(ctypes.get_last_error())
    return result

  def action(self, action, address, *, width=0, height=0):
    hwnd = int(address)
    if action == 'close':
      if not self.user.PostMessageW(hwnd, 0x10, 0, 0):
        raise ctypes.WinError(ctypes.get_last_error())
    elif action == 'resize':
      # SetWindowPos sizes the outer window; validation reads the resulting
      # client dimensions from presentation statistics rather than assuming them.
      if not self.user.SetWindowPos(hwnd, None, 0, 0, width, height, 0x16):
        raise ctypes.WinError(ctypes.get_last_error())
    elif action == 'focus':
      self.user.ShowWindow(hwnd, 9)
      self.user.SetForegroundWindow(hwnd)
      if self.user.GetForegroundWindow() != hwnd:
        kernel = ctypes.WinDLL('kernel32')
        kernel.GetCurrentThreadId.restype = W.DWORD
        current = kernel.GetCurrentThreadId()
        foreground = self.user.GetWindowThreadProcessId(self.user.GetForegroundWindow(), None)
        attached = foreground != current and self.user.AttachThreadInput(current, foreground, True)
        try:
          self.user.BringWindowToTop(hwnd)
          self.user.SetForegroundWindow(hwnd)
        finally:
          if attached:
            self.user.AttachThreadInput(current, foreground, False)
        if self.user.GetForegroundWindow() != hwnd:
          raise RuntimeError('Cannot focus fixture window for real keyboard input')
    elif action == 'tab':
      self.action('focus', address)
      self.user.keybd_event(9, 0, 0, 0)
      self.user.keybd_event(9, 0, 2, 0)
    elif action in ('hide', 'restore'):
      self.user.ShowWindow(hwnd, 6 if action == 'hide' else 9)
    elif action == 'fullscreen':
      self.user.ShowWindow(hwnd, 3 if not getattr(self, 'maximized', False) else 9)
      self.maximized = not getattr(self, 'maximized', False)
    elif action != 'float':
      raise RuntimeError(f'Unsupported Windows automation action: {action}')

  def resize_client(self, address, width, height):
    hwnd = int(address)
    outer, client = W.RECT(), W.RECT()
    if not self.user.GetWindowRect(hwnd, ctypes.byref(outer)):
      raise ctypes.WinError(ctypes.get_last_error())
    if not self.user.GetClientRect(hwnd, ctypes.byref(client)):
      raise ctypes.WinError(ctypes.get_last_error())
    self.action(
        'resize',
        address,
        width=width + (outer.right - outer.left) - (client.right - client.left),
        height=height + (outer.bottom - outer.top) - (client.bottom - client.top)
    )

  def capture_window(self, address, path, region=None):
    hwnd = int(address)
    # Raise only the fixture without stealing foreground keyboard focus.
    # Otherwise a changing unrelated window could make pixel tests pass.
    if not self.user.SetWindowPos(hwnd, ctypes.c_void_p(-1), 0, 0, 0, 0, 0x13):
      raise ctypes.WinError(ctypes.get_last_error())
    try:
      time.sleep(.2)
      client = next(c for c in self.clients() if str(c['address']) == str(address))
      left, top, width, height = region or (0, 0, *client['size'])
      for dx in (1, width // 2, width - 2):
        for dy in (1, height // 2, height - 2):
          point = W.POINT(client['at'][0] + left + dx, client['at'][1] + top + dy)
          actual = self.user.WindowFromPoint(point)
          if not actual or self.user.GetAncestor(actual, 2) != hwnd:
            raise RuntimeError(
                'Fixture capture is occluded; dismiss overlapping system menus/windows'
            )
      super().capture_window(address, path, region)
    finally:
      self.user.SetWindowPos(hwnd, ctypes.c_void_p(-2), 0, 0, 0, 0, 0x13)

  def click(self, address, x, y):
    self.action('focus', address)
    if not self.user.SetCursorPos(round(x), round(y)):
      raise ctypes.WinError(ctypes.get_last_error())
    time.sleep(.05)
    self.user.mouse_event(2, 0, 0, 0, 0)
    time.sleep(.05)
    self.user.mouse_event(4, 0, 0, 0, 0)

  def chord(self, address, *keys):
    codes = {
        'CTRL': 0x11, 'SHIFT': 0x10, 'ALT': 0x12, 'TAB': 9, 'BACKSPACE': 8, 'ENTER': 13,
        'ESCAPE': 0x1b, 'LEFT': 0x25, 'RIGHT': 0x27
    }
    values = [codes.get(key.upper(), ord(key.upper()) if len(key) == 1 else None) for key in keys]
    if None in values:
      raise ValueError(f'Unsupported native key chord: {keys}')
    self.action('focus', address)
    try:
      for value in values:
        self.user.keybd_event(value, 0, 0, 0)
    finally:
      for value in reversed(values):
        self.user.keybd_event(value, 0, 2, 0)

  def metrics(self, address):
    result = super().metrics(address)
    self.user.GetDpiForWindow.argtypes = [W.HWND]
    self.user.GetDpiForWindow.restype = W.UINT
    dpi = self.user.GetDpiForWindow(int(address))
    if not dpi:
      raise ctypes.WinError(ctypes.get_last_error())
    result['scale'] = dpi / 96
    return result

  def move(self, address, x, y):
    if not self.user.SetWindowPos(int(address), None, round(x), round(y), 0, 0, 0x15):
      raise ctypes.WinError(ctypes.get_last_error())

  def owned_windows(self, address):
    self.user.GetWindow.argtypes = [W.HWND, W.UINT]
    self.user.GetWindow.restype = W.HWND
    return [
        client for client in self.clients()
        if self.user.GetWindow(int(client['address']), 4) == int(address)
    ]

  def monitor_work_areas(self):

    class MonitorInfo(ctypes.Structure):
      _fields_ = [('size', W.DWORD), ('monitor', W.RECT), ('work', W.RECT), ('flags', W.DWORD)]

    callback_type = ctypes.WINFUNCTYPE(W.BOOL, W.HANDLE, W.HDC, ctypes.POINTER(W.RECT), W.LPARAM)
    self.user.GetMonitorInfoW.argtypes = [W.HANDLE, ctypes.POINTER(MonitorInfo)]
    self.user.GetMonitorInfoW.restype = W.BOOL
    areas = []
    failures = []

    @callback_type
    def collect(monitor, dc, rect, data):
      info = MonitorInfo()
      info.size = ctypes.sizeof(info)
      if not self.user.GetMonitorInfoW(monitor, ctypes.byref(info)):
        failures.append(ctypes.get_last_error())
        return False
      areas.append([info.work.left, info.work.top, info.work.right, info.work.bottom])
      return True

    self.user.EnumDisplayMonitors.argtypes = [
        W.HDC, ctypes.POINTER(W.RECT), callback_type, W.LPARAM
    ]
    self.user.EnumDisplayMonitors.restype = W.BOOL
    if not self.user.EnumDisplayMonitors(None, None, collect, 0):
      raise ctypes.WinError(failures[0] if failures else ctypes.get_last_error())
    return areas

  def ime_layouts(self):
    self.user.GetKeyboardLayoutList.argtypes = [ctypes.c_int, ctypes.POINTER(W.HANDLE)]
    self.user.GetKeyboardLayoutList.restype = ctypes.c_int
    count = self.user.GetKeyboardLayoutList(0, None)
    layouts = (W.HANDLE * count)()
    count = self.user.GetKeyboardLayoutList(count, layouts)
    imm = ctypes.WinDLL('imm32')
    imm.ImmIsIME.argtypes = [W.HANDLE]
    imm.ImmIsIME.restype = W.BOOL
    return [hex(layouts[index]) for index in range(count) if imm.ImmIsIME(layouts[index])]

  def live_resize(self, address, progress):
    hwnd = int(address)
    self.action('focus', address)
    # Keep the native sizing grip reachable even on a small RDP display.
    if not self.user.SetWindowPos(hwnd, None, 10, 10, 800, 600, 0x14):
      raise ctypes.WinError(ctypes.get_last_error())
    rect = W.RECT()
    if not self.user.GetWindowRect(hwnd, ctypes.byref(rect)):
      raise ctypes.WinError(ctypes.get_last_error())
    if not self.user.SetCursorPos(rect.right - 2, rect.bottom - 2):
      raise ctypes.WinError(ctypes.get_last_error())
    time.sleep(.3)
    baseline = progress()
    self.user.mouse_event(2, 0, 0, 0, 0)
    advanced = False
    try:
      for step in range(1, 9):
        if not self.user.SetCursorPos(rect.right - 2 - step * 20, rect.bottom - 2 - step * 12):
          raise ctypes.WinError(ctypes.get_last_error())
        time.sleep(.1)
        current = W.RECT()
        resized = self.user.GetWindowRect(hwnd, ctypes.byref(current)) and (
            current.right - current.left != rect.right - rect.left or
            current.bottom - current.top != rect.bottom - rect.top
        )
        advanced = (resized and progress() != baseline) or advanced
    finally:
      self.user.mouse_event(4, 0, 0, 0, 0)
    return advanced

  def capture(self, rect, path):
    x, y, width, height = rect
    if width <= 0 or height <= 0:
      raise RuntimeError(f'Invalid capture rectangle: {rect}')
    screen = self.user.GetDC(None)
    memory = self.gdi.CreateCompatibleDC(screen)
    bitmap = self.gdi.CreateCompatibleBitmap(screen, width, height)
    if not screen or not memory or not bitmap:
      raise ctypes.WinError(ctypes.get_last_error())
    previous = self.gdi.SelectObject(memory, bitmap)
    try:
      if not self.gdi.BitBlt(memory, 0, 0, width, height, screen, x, y, 0x00CC0020):
        raise ctypes.WinError(ctypes.get_last_error())
      self.gdi.SelectObject(memory, previous)
      info = ctypes.create_string_buffer(
          struct.pack('<IiiHHIIiiII', 40, width, -height, 1, 32, 0, 0, 0, 0, 0, 0)
      )
      pixels = ctypes.create_string_buffer(width * height * 4)
      if self.gdi.GetDIBits(screen, bitmap, 0, height, pixels, info, 0) != height:
        raise ctypes.WinError(ctypes.get_last_error())
      rgba = bytearray(pixels.raw)
      rgba[0::4], rgba[2::4] = rgba[2::4], rgba[0::4]
      rgba[3::4] = b'\xff' * (width * height)
      png_rgba(path, width, height, bytes(rgba))
    finally:
      self.gdi.SelectObject(memory, previous)
      self.gdi.DeleteObject(bitmap)
      self.gdi.DeleteDC(memory)
      self.user.ReleaseDC(None, screen)


class _CGPoint(ctypes.Structure):
  _fields_ = [('x', ctypes.c_double), ('y', ctypes.c_double)]


class _CGSize(ctypes.Structure):
  _fields_ = [('width', ctypes.c_double), ('height', ctypes.c_double)]


class _CGRect(ctypes.Structure):
  _fields_ = [('origin', _CGPoint), ('size', _CGSize)]


class MacOS(Desktop):
  """Quartz window list plus Accessibility and CGEvent control.

  Coordinates are global points with a top-left origin, as reported by
  CGWindowList and used by CGEvent and AXPosition, so metrics report a
  logical coordinate space. Like the Windows backend, `at`/`size` are the
  client (content) area: titled windows exclude their title bar. `move` and
  the `resize` action address the whole window frame. Reading the window list needs
  Screen Recording permission for titles; input, focus and movement need
  Accessibility permission for the app that runs Python.
  """

  _KEYS = {
      'ESCAPE': 53, 'TAB': 48, 'ENTER': 36, 'BACKSPACE': 51, 'LEFT': 123, 'RIGHT': 124, 'CTRL': 59,
      'SHIFT': 56, 'ALT': 58, 'CMD': 55, 'A': 0, 'S': 1, 'D': 2, 'F': 3, 'H': 4, 'G': 5, 'Z': 6,
      'X': 7, 'C': 8, 'V': 9, 'B': 11, 'Q': 12, 'W': 13, 'E': 14, 'R': 15, 'Y': 16, 'T': 17,
      'O': 31, 'U': 32, 'I': 34, 'P': 35, 'L': 37, 'J': 38, 'K': 40, 'N': 45, 'M': 46, '1': 18,
      '2': 19, '3': 20, '4': 21, '5': 23, '6': 22, '7': 26, '8': 28, '9': 25, '0': 29, 'SPACE': 49
  }
  _FLAGS = {'CTRL': 1 << 18, 'SHIFT': 1 << 17, 'ALT': 1 << 19, 'CMD': 1 << 20}

  def __init__(self):
    void = ctypes.c_void_p
    self.cf = ctypes.cdll.LoadLibrary(
        '/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation'
    )
    self.cg = ctypes.cdll.LoadLibrary(
        '/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics'
    )
    self.ax = ctypes.cdll.LoadLibrary(
        '/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices'
    )
    for library, name, args, result in [
        (self.cf, 'CFArrayGetCount', [void], ctypes.c_long),
        (self.cf, 'CFArrayGetValueAtIndex', [void, ctypes.c_long], void),
        (self.cf, 'CFDictionaryGetValue', [void, void], void),
        (self.cf, 'CFNumberGetValue', [void, ctypes.c_int, void], ctypes.c_bool),
        (self.cf, 'CFStringCreateWithCString', [void, ctypes.c_char_p, ctypes.c_uint32], void),
        (self.cf, 'CFStringGetCString', [void, ctypes.c_char_p, ctypes.c_long,
                                         ctypes.c_uint32], ctypes.c_bool),
        (self.cf, 'CFRelease', [void], None),
        (self.cf, 'CFRetain', [void], void),
        (self.cg, 'CGWindowListCopyWindowInfo', [ctypes.c_uint32, ctypes.c_uint32], void),
        (self.cg, 'CGRectMakeWithDictionaryRepresentation', [void, ctypes.POINTER(_CGRect)],
         ctypes.c_bool),
        (self.cg, 'CGEventCreateMouseEvent', [void, ctypes.c_uint32, _CGPoint,
                                              ctypes.c_uint32], void),
        (self.cg, 'CGEventCreateKeyboardEvent', [void, ctypes.c_uint16, ctypes.c_bool], void),
        (self.cg, 'CGEventSetFlags', [void, ctypes.c_uint64], None),
        (self.cg, 'CGEventPost', [ctypes.c_uint32, void], None),
        (self.ax, 'AXIsProcessTrusted', [], ctypes.c_bool),
        (self.ax, 'AXUIElementCreateApplication', [ctypes.c_int], void),
        (self.ax, 'AXUIElementCopyAttributeValue', [void, void,
                                                    ctypes.POINTER(void)], ctypes.c_int32),
        (self.ax, 'AXUIElementSetAttributeValue', [void, void, void], ctypes.c_int32),
        (self.ax, 'AXUIElementPerformAction', [void, void], ctypes.c_int32),
        (self.ax, 'AXValueCreate', [ctypes.c_uint32, void], void),
        (self.ax, '_AXUIElementGetWindow', [void, ctypes.POINTER(ctypes.c_uint32)], ctypes.c_int32),
    ]:
      function = getattr(library, name)
      function.argtypes, function.restype = args, result
    self.true = void.in_dll(self.cf, 'kCFBooleanTrue')
    self.false = void.in_dll(self.cf, 'kCFBooleanFalse')
    self.keys = {
        name: void.in_dll(self.cg, name) for name in (
            'kCGWindowNumber', 'kCGWindowOwnerPID', 'kCGWindowName', 'kCGWindowBounds',
            'kCGWindowLayer'
        )
    }

  def titlebar_height(self):
    """Height of a standard titled window's title bar, from AppKit."""
    if not hasattr(self, '_titlebar'):
      objc = ctypes.cdll.LoadLibrary('/usr/lib/libobjc.A.dylib')
      ctypes.cdll.LoadLibrary('/System/Library/Frameworks/AppKit.framework/AppKit')
      objc.objc_getClass.restype = ctypes.c_void_p
      objc.objc_getClass.argtypes = [ctypes.c_char_p]
      objc.sel_registerName.restype = ctypes.c_void_p
      objc.sel_registerName.argtypes = [ctypes.c_char_p]
      send = ctypes.CFUNCTYPE(_CGRect, ctypes.c_void_p, ctypes.c_void_p, _CGRect,
                              ctypes.c_uint64)(('objc_msgSend', objc))
      # Titled | closable | miniaturizable | resizable.
      frame = send(
          objc.objc_getClass(b'NSWindow'),
          objc.sel_registerName(b'frameRectForContentRect:styleMask:'),
          _CGRect(_CGPoint(0, 0), _CGSize(100, 100)), 15
      )
      self._titlebar = round(frame.size.height - 100)
    return self._titlebar

  def _string(self, text):
    return self.cf.CFStringCreateWithCString(None, text.encode(), 0x08000100)

  def _number(self, value):
    result = ctypes.c_int64()
    return result.value if value and self.cf.CFNumberGetValue(
        value, 4, ctypes.byref(result)
    ) else None

  def _text(self, value):
    buffer = ctypes.create_string_buffer(4096)
    if value and self.cf.CFStringGetCString(value, buffer, len(buffer), 0x08000100):
      return buffer.value.decode()
    return ''

  def clients(self):
    # On-screen windows only, excluding desktop elements.
    windows = self.cg.CGWindowListCopyWindowInfo(1 | 16, 0)
    if not windows:
      raise RuntimeError('CGWindowListCopyWindowInfo failed')
    result = []
    try:
      for index in range(self.cf.CFArrayGetCount(windows)):
        info = self.cf.CFArrayGetValueAtIndex(windows, index)
        value = lambda key: self.cf.CFDictionaryGetValue(info, self.keys[key])
        if self._number(value('kCGWindowLayer')) != 0:
          continue
        rect = _CGRect()
        if not self.cg.CGRectMakeWithDictionaryRepresentation(value('kCGWindowBounds'),
                                                              ctypes.byref(rect)):
          continue
        if rect.size.width <= 0 or rect.size.height <= 0:
          continue
        title = self._text(value('kCGWindowName'))
        # Titled windows carry a title bar above their client area; popups
        # and tooltips are untitled and borderless.
        titlebar = self.titlebar_height() if title else 0
        result.append(
            dict(
                address=str(self._number(value('kCGWindowNumber'))),
                pid=self._number(value('kCGWindowOwnerPID')),
                title=title,
                at=[round(rect.origin.x), round(rect.origin.y) + titlebar],
                size=[round(rect.size.width),
                      round(rect.size.height) - titlebar]
            )
        )
    finally:
      self.cf.CFRelease(windows)
    return result

  def _client(self, address):
    return next(c for c in self.clients() if str(c['address']) == str(address))

  def _owner_pid(self, address):
    """The owning process of a window, including minimized or off-screen ones."""
    # Only the full list (option 0) includes minimized windows; the
    # single-window option omits them.
    windows = self.cg.CGWindowListCopyWindowInfo(0, 0)
    if not windows:
      raise RuntimeError('CGWindowListCopyWindowInfo failed')
    try:
      for index in range(self.cf.CFArrayGetCount(windows)):
        info = self.cf.CFArrayGetValueAtIndex(windows, index)
        number = self._number(self.cf.CFDictionaryGetValue(info, self.keys['kCGWindowNumber']))
        if str(number) == str(address):
          return self._number(self.cf.CFDictionaryGetValue(info, self.keys['kCGWindowOwnerPID']))
    finally:
      self.cf.CFRelease(windows)
    raise RuntimeError(f'Window {address} does not exist')

  def _require_accessibility(self):
    if not self.ax.AXIsProcessTrusted():
      raise RuntimeError(
          'macOS input and window control need Accessibility permission: enable the app that '
          'runs this script in System Settings > Privacy & Security > Accessibility'
      )

  def _ax_window(self, address):
    """Returns (application, window) AX elements for a CGWindowID; caller releases both."""
    self._require_accessibility()
    pid = self._owner_pid(address)
    application = self.ax.AXUIElementCreateApplication(pid)
    windows = ctypes.c_void_p()
    attribute = self._string('AXWindows')
    try:
      if self.ax.AXUIElementCopyAttributeValue(application, attribute, ctypes.byref(windows)):
        raise RuntimeError(f'Cannot read Accessibility windows of process {pid}')
    finally:
      self.cf.CFRelease(attribute)
    try:
      for index in range(self.cf.CFArrayGetCount(windows)):
        window = self.cf.CFArrayGetValueAtIndex(windows, index)
        number = ctypes.c_uint32()
        if not self.ax._AXUIElementGetWindow(window, ctypes.byref(number)) and str(number.value
                                                                                  ) == str(address):
          # Retain the element beyond the array's lifetime.
          return application, self.cf.CFRetain(window)
    finally:
      self.cf.CFRelease(windows)
    self.cf.CFRelease(application)
    raise RuntimeError(f'Window {address} has no Accessibility element')

  def _set(self, element, name, value):
    attribute = self._string(name)
    try:
      error = self.ax.AXUIElementSetAttributeValue(element, attribute, value)
    finally:
      self.cf.CFRelease(attribute)
    if error:
      raise RuntimeError(f'Setting {name} failed with AXError {error}')

  def _set_geometry(self, address, name, kind, value):
    application, window = self._ax_window(address)
    ax_value = self.ax.AXValueCreate(kind, ctypes.byref(value))
    try:
      self._set(window, name, ax_value)
    finally:
      self.cf.CFRelease(ax_value)
      self.cf.CFRelease(window)
      self.cf.CFRelease(application)

  def action(self, action, address, *, width=0, height=0):
    if action == 'resize':
      self._set_geometry(address, 'AXSize', 2, _CGSize(width, height))
    elif action == 'tab':
      self.chord(address, 'Tab')
    elif action == 'float':
      pass
    elif action in ('focus', 'close', 'hide', 'restore', 'fullscreen'):
      application, window = self._ax_window(address)
      try:
        if action == 'focus':
          self._set(application, 'AXFrontmost', self.true)
          self._set(window, 'AXMain', self.true)
          raise_action = self._string('AXRaise')
          try:
            self.ax.AXUIElementPerformAction(window, raise_action)
          finally:
            self.cf.CFRelease(raise_action)
        elif action == 'close':
          button = ctypes.c_void_p()
          attribute, press = self._string('AXCloseButton'), self._string('AXPress')
          try:
            if self.ax.AXUIElementCopyAttributeValue(window, attribute, ctypes.byref(button)):
              raise RuntimeError(f'Window {address} has no close button')
            self.ax.AXUIElementPerformAction(button, press)
            self.cf.CFRelease(button)
          finally:
            self.cf.CFRelease(attribute)
            self.cf.CFRelease(press)
        elif action in ('hide', 'restore'):
          self._set(window, 'AXMinimized', self.true if action == 'hide' else self.false)
        else:
          self.fullscreen = not getattr(self, 'fullscreen', False)
          self._set(window, 'AXFullScreen', self.true if self.fullscreen else self.false)
      finally:
        self.cf.CFRelease(window)
        self.cf.CFRelease(application)
    else:
      raise RuntimeError(f'Unsupported macOS automation action: {action}')

  def metrics(self, address):
    result = super().metrics(address)
    result['coordinate_space'] = 'logical'
    return result

  def resize_client(self, address, width, height):
    titlebar = self.titlebar_height() if self._client(address)['title'] else 0
    self.action('resize', address, width=width, height=height + titlebar)

  def move(self, address, x, y):
    self._set_geometry(address, 'AXPosition', 1, _CGPoint(x, y))

  def owned_windows(self, address):
    # The window server does not report AppKit child-window ownership to
    # other processes. Popups and tooltips are the owner's untitled same-PID
    # windows; callers verify real ownership behavior, for example a tooltip
    # following its owner's movement.
    clients = self.clients()
    owner = next(c for c in clients if str(c['address']) == str(address))
    return [
        c for c in clients
        if c['pid'] == owner['pid'] and c['address'] != owner['address'] and not c['title']
    ]

  def _post(self, event):
    if not event:
      raise RuntimeError('CGEvent creation failed')
    try:
      self.cg.CGEventPost(0, event)
    finally:
      self.cf.CFRelease(event)

  def click(self, address, x, y):
    self.action('focus', address)
    time.sleep(.1)
    point = _CGPoint(x, y)
    # Mouse moved, left down, left up.
    for kind in (5, 1, 2):
      self._post(self.cg.CGEventCreateMouseEvent(None, kind, point, 0))
      time.sleep(.05)

  def chord(self, address, *keys):
    names = [key.upper() for key in keys]
    if any(name not in self._KEYS for name in names):
      raise ValueError(f'Unsupported native key chord: {keys}')
    self.action('focus', address)
    time.sleep(.1)
    flags = 0
    for name in names[:-1]:
      flags |= self._FLAGS.get(name, 0)
    for down in (True, False):
      event = self.cg.CGEventCreateKeyboardEvent(None, self._KEYS[names[-1]], down)
      if event:
        self.cg.CGEventSetFlags(event, flags)
      self._post(event)
      time.sleep(.05)

  def _display_bounds(self):
    count = ctypes.c_uint32()
    displays = (ctypes.c_uint32 * 16)()
    self.cg.CGGetActiveDisplayList.argtypes = [
        ctypes.c_uint32,
        ctypes.POINTER(ctypes.c_uint32),
        ctypes.POINTER(ctypes.c_uint32)
    ]
    self.cg.CGDisplayBounds.argtypes = [ctypes.c_uint32]
    self.cg.CGDisplayBounds.restype = _CGRect
    if self.cg.CGGetActiveDisplayList(16, displays, ctypes.byref(count)):
      raise RuntimeError('CGGetActiveDisplayList failed')
    return [self.cg.CGDisplayBounds(displays[index]) for index in range(count.value)]

  def settled_client(self, address, timeout=5.0):
    """Waits until a window's frame stops changing and lies on a display.

    A window opened on a Space that is not showing makes macOS slide to it,
    and CGWindowList reports shifted frames during the animation (window
    open animations also change the size). Capturing then would read pixels
    from the wrong place or fail for an off-display rectangle.
    """
    deadline = time.monotonic() + timeout
    last, since = None, time.monotonic()
    while time.monotonic() < deadline:
      client = self._client(address)
      frame = (*client['at'], *client['size'])
      if frame != last:
        last, since = frame, time.monotonic()
      elif time.monotonic() - since >= .5:
        x, y, width, height = frame
        if any(b.origin.x <= x and b.origin.y <= y and x + width <= b.origin.x +
               b.size.width and y + height <= b.origin.y + b.size.height
               for b in self._display_bounds()):
          return client
      time.sleep(.05)
    raise RuntimeError(f'Window {address} did not settle on a display: {last}')

  def capture_window(self, address, path, region=None):
    self.settled_client(address)
    super().capture_window(address, path, region)

  # The app renders sRGB and macOS composites it into the display's color
  # space, which screencapture records. Convert back so pixel checks compare
  # rendered sRGB values (the round trip can be off by one per channel).
  SRGB_PROFILE = '/System/Library/ColorSync/Profiles/sRGB Profile.icc'

  def capture(self, rect, path):
    x, y, width, height = rect
    if width <= 0 or height <= 0:
      raise RuntimeError(f'Invalid capture rectangle: {rect}')
    subprocess.run(['screencapture', '-x', '-R', f'{x},{y},{width},{height}',
                    str(path)],
                   check=True)
    subprocess.run(['sips', '--matchTo', self.SRGB_PROFILE,
                    str(path)],
                   check=True,
                   stdout=subprocess.DEVNULL)


def desktop() -> Desktop:
  if sys.platform == 'win32':
    return Windows()
  if sys.platform.startswith('linux'):
    return Hyprland()
  if sys.platform == 'darwin':
    return MacOS()
  raise RuntimeError(f'Native Rust-shell validation is not implemented on {sys.platform}')
