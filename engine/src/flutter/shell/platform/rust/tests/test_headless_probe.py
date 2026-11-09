#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Run the Phase 0 engine probe and retain PNGs, logs and socket evidence."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import struct
import zlib


def save_png(path, width, height, rgba):
    assert len(rgba) == width * height * 4
    def chunk(kind, payload):
        return (struct.pack('>I', len(payload)) + kind + payload +
                struct.pack('>I', zlib.crc32(kind + payload)))
    # Vulkan render targets store premultiplied channels; PNG uses straight alpha.
    rgba = bytearray(rgba)
    for offset in range(0, len(rgba), 4):
        alpha = rgba[offset + 3]
        if alpha == 0:
            rgba[offset:offset + 3] = b'\0\0\0'
        elif alpha != 255:
            for channel in range(3):
                rgba[offset + channel] = min(255, (rgba[offset + channel] * 255 + alpha // 2) // alpha)
    stride = width * 4
    rows = b''.join(b'\0' + rgba[y:y + stride] for y in range(0, len(rgba), stride))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' +
                     chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0)) +
                     chunk(b'gAMA', struct.pack('>I', 100000)) +
                     chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runner', type=Path, required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--icu', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--strace', type=Path, required=True)
    parser.add_argument('--icd', type=Path)
    parser.add_argument('--driver-library-dir', type=Path)
    args = parser.parse_args()
    output = args.artifacts.resolve()
    output.mkdir(parents=True, exist_ok=True)
    # Each run gets a fresh directory so stale frames cannot satisfy the gate.
    frames = output / 'frames'
    frames.mkdir(exist_ok=False)
    environment = {k: v for k, v in os.environ.items()
                   if k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
    environment.update(WGPU_VALIDATION='1', WGPU_DEBUG='1')
    if args.icd:
        environment['VK_DRIVER_FILES'] = str(args.icd.resolve())
    if args.driver_library_dir:
        environment['LD_LIBRARY_PATH'] = str(args.driver_library_dir.resolve())
    command = [str(args.strace.resolve()), '-f', '-e', 'trace=connect', '-o',
               str(output / 'connect.trace'), str(args.runner.resolve()),
               str(args.assets.resolve()), str(args.icu.resolve()), str(frames)]
    report = {'command': command, 'status': 'failed'}
    try:
        with (output / 'stdout.log').open('w') as stdout, (output / 'stderr.log').open('w') as stderr:
            process = subprocess.Popen(command, env=environment, stdout=stdout,
                                       stderr=stderr, start_new_session=True)
            try:
                process.wait(timeout=60)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, 9)
                    process.wait()
        report['exitCode'] = process.returncode
        logs = (output / 'stderr.log').read_text() + (output / 'stdout.log').read_text()
        assert process.returncode == 0, logs[-10000:]
        assert not re.search(r'Validation Error|VUID-|UNASSIGNED-|validation failed|Unhandled Exception|\[ERROR:', logs), logs[-10000:]
        sockets = (output / 'connect.trace').read_text()
        assert not re.search(r'\.X11-unix|wayland-[0-9]+', sockets), sockets
        captured = []
        for raw in sorted(frames.glob('*.rgba')):
            match = re.fullmatch(r'frame-(\d+)-(\d+)x(\d+)\.rgba', raw.name)
            assert match, raw
            number, width, height = map(int, match.groups())
            save_png(raw.with_suffix('.png'), width, height, raw.read_bytes())
            captured.append({'frame': number, 'width': width, 'height': height,
                             'png': str(raw.with_suffix('.png').relative_to(output))})
        assert len(captured) >= 12, captured
        assert {(f['width'], f['height']) for f in captured} == {(801, 601), (1001, 751)}, captured
        report.update(status='passed', frames=captured,
                      colorConvention='RGBA8 linear UNORM; straight-alpha PNG with gAMA=1.0',
                      validation=True, displaySocketConnections=0)
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"PASS: {len(captured)} Flutter frames; report: {output / 'report.json'}")


if __name__ == '__main__':
    main()
