# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Check compositor command encoding, deterministic captures, and Win32 control."""
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zlib

from native_desktop import Hyprland, desktop, png_rgba
from test_texture import wait_until


class PortableTest(unittest.TestCase):

  def test_png_preserves_pixels_and_has_no_variable_metadata(self):
    with tempfile.TemporaryDirectory() as work:
      a, b = Path(work) / 'a.png', Path(work) / 'b.png'
      pixels = bytes([255, 0, 0, 255, 0, 255, 0, 255])
      png_rgba(a, 2, 1, pixels)
      png_rgba(b, 2, 1, pixels)
      self.assertEqual(a.read_bytes(), b.read_bytes())
      content = a.read_bytes()
      offset, data = 8, b''
      while offset < len(content):
        length = struct.unpack('!I', content[offset:offset + 4])[0]
        kind = content[offset + 4:offset + 8]
        if kind == b'IDAT':
          data += content[offset + 8:offset + 8 + length]
        offset += length + 12
      self.assertEqual(zlib.decompress(data), b'\0' + pixels)

  def test_hyprland_lua_escapes_window_selector_and_resizes_absolutely(self):
    with patch('native_desktop.shutil.which',
               return_value='/bin/hyprctl'), patch('native_desktop.subprocess.run') as run:
      run.return_value.returncode = 0
      api = Hyprland()
      api.action('resize', '0x123', width=911, height=677)
      command = run.call_args.args[0]
      self.assertEqual(command[:2], ['hyprctl', 'dispatch'])
      self.assertIn('window = "address:0x123"', command[2])
      self.assertIn('x = 911, y = 677, relative = false', command[2])

  def test_failed_close_is_not_retried_using_a_different_protocol(self):
    with patch('native_desktop.shutil.which',
               return_value='/bin/hyprctl'), patch('native_desktop.subprocess.run') as run:
      run.return_value.returncode = 0
      api = Hyprland()
      run.reset_mock()
      run.side_effect = subprocess.CalledProcessError(1, ['hyprctl'])
      with self.assertRaises(subprocess.CalledProcessError):
        api.action('close', '0x123')
      self.assertEqual(run.call_count, 1)

  def test_legacy_hyprland_uses_exact_size(self):
    with patch('native_desktop.shutil.which',
               return_value='/bin/hyprctl'), patch('native_desktop.subprocess.run') as run:
      run.return_value.returncode = 1
      api = Hyprland()
      api.action('resize', '0x123', width=640, height=480)
      self.assertEqual(
          run.call_args.args[0],
          ['hyprctl', 'dispatch', 'resizewindowpixel', 'exact 640 480,address:0x123']
      )


@unittest.skipUnless(sys.platform == 'win32', 'Win32 native automation')
class WindowsTest(unittest.TestCase):

  def test_occlusion_is_rejected_before_captured_pixels_are_compared(self):
    api = desktop()
    with patch.object(api.user, 'SetWindowPos', return_value=True), \
         patch.object(api.user, 'WindowFromPoint', return_value=2), \
         patch.object(api.user, 'GetAncestor', return_value=2), \
         patch.object(api, 'clients', return_value=[{'address': '1', 'at': [0, 0], 'size': [100, 100]}]), \
         patch.object(api, 'capture') as capture, \
         patch('native_desktop.time.sleep'):
      with self.assertRaisesRegex(RuntimeError, 'occluded'):
        api.capture_window('1', Path('unused.png'))
      capture.assert_not_called()

  def test_pid_selection_resize_capture_and_clean_close(self):
    try:
      import tkinter
    except ImportError:
      self.skipTest('Python Tk is unavailable')
    api = desktop()
    source = 'import tkinter as t; root=t.Tk(); root.title("Rust desktop adapter test"); root.geometry("320x240"); root.mainloop()'
    process = subprocess.Popen([sys.executable, '-c', source])
    try:

      def find():
        return next((
            c for c in api.clients()
            if c['pid'] == process.pid and c['title'] == 'Rust desktop adapter test'
        ), None)

      client = wait_until(find, process)
      self.assertIsNotNone(client)
      address = client['address']
      api.action('resize', address, width=500, height=400)

      def changed():
        current = find()
        return current if current and current['size'] != client['size'] else None

      resized = wait_until(changed, process)
      self.assertIsNotNone(resized)
      api.action('hide', address)
      api.action('restore', address)
      with tempfile.TemporaryDirectory() as work:
        capture = Path(work) / 'capture.png'
        api.capture((*resized['at'], *resized['size']), capture)
        self.assertTrue(capture.read_bytes().startswith(b'\x89PNG\r\n\x1a\n'))
      api.action('close', address)
      self.assertEqual(process.wait(timeout=5), 0)
    finally:
      if process.poll() is None:
        process.kill()
        process.wait()

  def test_modal_resize_when_interactive_foreground_input_is_available(self):
    try:
      import tkinter
    except ImportError:
      self.skipTest('Python Tk is unavailable')
    api = desktop()
    process = subprocess.Popen([
        sys.executable, '-c',
        'import tkinter as t; r=t.Tk(); r.title("Rust modal resize test"); r.mainloop()'
    ])
    try:

      def find():
        return next((
            c for c in api.clients()
            if c['pid'] == process.pid and c['title'] == 'Rust modal resize test'
        ), None)

      client = wait_until(find, process)
      self.assertIsNotNone(client)
      try:
        api.action('focus', client['address'])
      except RuntimeError as error:
        self.skipTest(f'Interactive foreground input is unavailable: {error}')
      self.assertTrue(
          api.live_resize(client['address'], lambda: find()['size'] if find() else None)
      )
      api.action('close', client['address'])
      self.assertEqual(process.wait(timeout=5), 0)
    finally:
      if process.poll() is None:
        process.kill()
        process.wait()


if __name__ == '__main__':
  unittest.main()
