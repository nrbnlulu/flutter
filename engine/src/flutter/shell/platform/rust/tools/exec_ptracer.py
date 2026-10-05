#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Exec a profiling runner with eu-stack attachment allowed at ptrace_scope=1."""

import ctypes
import os
import sys

if len(sys.argv) < 2:
  sys.exit("Usage: exec_ptracer.py RUNNER [ARG ...]")
libc = ctypes.CDLL(None, use_errno=True)
# PR_SET_PTRACER, PR_SET_PTRACER_ANY. This opt-in permission survives exec.
if libc.prctl(0x59616d61, ctypes.c_ulong(-1 & ((1 << 64) - 1)), 0, 0, 0) != 0:
  error = ctypes.get_errno()
  raise OSError(error, os.strerror(error))
os.execv(sys.argv[1], sys.argv[1:])
