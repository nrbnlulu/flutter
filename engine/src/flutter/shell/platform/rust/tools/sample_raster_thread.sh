#!/usr/bin/env bash
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sample the FlutterRust.ras raster thread with eu-stack and print a histogram
# of the top inclusive frames.
#
# Usage:
#   sample_raster_thread.sh [--wait-stage N] [--output FILE] [--samples N] [--interval-ms N]
#
# --wait-stage N   Wait for the benchmark stage with N children to begin
#                  before collecting samples (e.g. 12). If omitted, starts
#                  sampling immediately once the process appears.
# --output FILE    Write raw stack samples to FILE (default: /tmp/raster_stacks.txt)
# --samples N      Number of stack snapshots to collect (default: 200)
# --interval-ms N  Milliseconds between snapshots (default: 20)
# --pid N          Sample this exact runner process (otherwise discover it)
# --status FILE    Benchmark status file, required with --wait-stage
#
# ptrace_scope=1 on this machine; the runner must be launched through
# benchmark_windowing.py --allow-stack-sampling, or tools/exec_ptracer.py,
# for eu-stack to attach. Run profiling separately from acceptance measurements.

set -euo pipefail

WAIT_STAGE=""
OUTPUT="/tmp/raster_stacks.txt"
SAMPLES=200
INTERVAL_MS=20
STATUS=""
pid=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --wait-stage) WAIT_STAGE="$2"; shift 2 ;;
    --output) OUTPUT="$2"; shift 2 ;;
    --samples) SAMPLES="$2"; shift 2 ;;
    --interval-ms) INTERVAL_MS="$2"; shift 2 ;;
    --status) STATUS="$2"; shift 2 ;;
    --pid) pid="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -z "$WAIT_STAGE" || -n "$STATUS" ]] || { echo "--wait-stage requires --status FILE" >&2; exit 1; }

: > "$OUTPUT"

# Wait for the runner process. The binary name is truncated to 15 chars by
# the kernel (comm); use "flutter_rust_sh" instead of the full path.
echo "[sampler] waiting for flutter_rust_sh process..." | tee -a "$OUTPUT"
if [[ -z "$pid" ]]; then
  until pid=$(pgrep -x flutter_rust_sh 2>/dev/null | head -1) && [[ -n "$pid" ]]; do
    sleep 0.1
  done
fi
echo "pid=$pid" >> "$OUTPUT"

# Locate the raster thread by comm name.
until tid=$(for t in /proc/$pid/task/*; do
               [[ "$(cat "$t/comm" 2>/dev/null)" == "FlutterRust.ras" ]] && basename "$t"
             done | head -1) && [[ -n "$tid" ]]; do
  sleep 0.05
  # Make sure process still alive.
  kill -0 "$pid" 2>/dev/null || { echo "[sampler] process exited before raster thread found"; exit 0; }
done
echo "tid=$tid" | tee -a "$OUTPUT"

# If a benchmark stage is requested, wait for the status file.
if [[ -n "$WAIT_STAGE" ]]; then
  echo "[sampler] waiting for stage count=$WAIT_STAGE..." | tee -a "$OUTPUT"
  until rg -q "start count=$WAIT_STAGE" "$STATUS" 2>/dev/null; do
    sleep 0.1
    kill -0 "$pid" 2>/dev/null || { echo "[sampler] process exited while waiting for stage"; exit 0; }
  done
  echo "[sampler] stage $WAIT_STAGE active, starting $SAMPLES samples" | tee -a "$OUTPUT"
fi

# Collect stack snapshots.
SLEEP_S=$(python3 -c 'import sys; print(int(sys.argv[1]) / 1000)' "$INTERVAL_MS")
for i in $(seq 1 "$SAMPLES"); do
  eu-stack -p "$pid" -m -i | awk -v t="TID $tid:" \
    '$0==t{p=1;print;next} /^TID /{p=0} p' >> "$OUTPUT"
  echo "----" >> "$OUTPUT"
  sleep "$SLEEP_S"
done
echo "[sampler] done — $SAMPLES samples in $OUTPUT" | tee -a "$OUTPUT"

# Print histogram of top inclusive frames.
python3 - "$OUTPUT" <<'PYEOF'
import re, sys, collections
txt = open(sys.argv[1]).read().split('----')
incl = collections.Counter(); n = 0
for s in txt:
    frames = [l for l in s.splitlines() if l.startswith('#')]
    if not frames:
        continue
    n += 1
    names = set()
    for l in frames[:60]:
        m = re.match(r'#\d+\s+0x[0-9a-f]+\s+(.*?)\s*-\s+(\S+)$', l)
        if m:
            fn = (m.group(1) or '?')[:70]
            lib = m.group(2).split('/')[-1][:22]
            names.add(f"{fn} @{lib}")
    for name in names:
        incl[name] += 1
print(f"\n{n} samples")
if not n:
    sys.exit("No raster stack samples captured")
print(f"{'Count':>6}  {'%':>5}  Frame")
print("-" * 90)
for k, v in incl.most_common(30):
    print(f"{v:>6}  {v/n*100:>4.0f}%  {k}")
PYEOF
