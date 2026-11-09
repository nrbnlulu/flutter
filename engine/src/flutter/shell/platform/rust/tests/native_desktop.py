# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Native desktop automation used by Rust-shell runtime validation.

Coordinates are physical screen pixels. Windows are always selected by PID by
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


class Desktop(ABC):

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
    if self.lua and action in ('close', 'float', 'resize', 'focus'):
      options = f'window = {json.dumps(selector)}'
      if action == 'resize':
        options += f', x = {width}, y = {height}, relative = false'
      elif action == 'float':
        options += ', action = "enable"'
      command = ['hyprctl', 'dispatch', f'hl.dsp.window.{action}({{{options}}})']
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


def desktop() -> Desktop:
  if sys.platform == 'win32':
    return Windows()
  if sys.platform.startswith('linux'):
    return Hyprland()
  raise RuntimeError(f'Native Rust-shell validation is not implemented on {sys.platform}')
