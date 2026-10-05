#!/usr/bin/env python3
# Copyright 2026 The Flutter Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Compare multi-view benchmark runs and print KEEP or REVERT.

Usage:
  compare_benchmarks.py BASE_DIR [BASE_DIR ...] -- CAND_DIR [CAND_DIR ...]

The primary metric is raster p50 at the 8- and 12-child stages.
A change is KEEP if:
  - Raster p50 at the 8- or 12-child stage improves by more than noise.
  - No guard metric regresses by more than its noise at any stage.

Noise for a metric is the larger of 5% and twice the spread over the baseline
runs: 2 * (max - min) / median.
"""

import argparse
import json
import statistics
import sys
from pathlib import Path

GUARD_METRICS = [
    # (stage_key, path, lower_is_better, label)
    # stage_key matches a stage by children count; None means all stages.
    (None, "cpu_percent", True, "process CPU%"),
    (None, "timings.build_ms.p95", True, "build p95 ms"),
    (None, "timings.raster_ms.p50", True, "raster p50 ms"),
    (None, "timings.raster_ms.p95", True, "raster p95 ms"),
    (None, "timings.raster_queue_ms.p95", True, "raster queue p95 ms"),
    (None, "implicit_native_timings.acquire_ms.p95", True, "acquire p95 ms"),
    (None, "implicit_native_timings.handoff_ms.p95", True, "handoff p95 ms"),
    (None, "implicit_native_timings.present_ms.p95", True, "present p95 ms"),
]
PRIMARY_STAGES = {8, 12}
PRIMARY_METRIC_PATH = "timings.raster_ms.p50"
PRIMARY_LOWER_IS_BETTER = True


def load_summary(directory: Path) -> list[dict]:
  summary_path = directory / "summary.json"
  if not summary_path.exists():
    summary_path = directory / "partial-summary.json"
  return json.loads(summary_path.read_text())


def get_by_path(obj: dict, path: str):
  """Traverse nested dict using dot-separated path; return None if missing."""
  parts = path.split(".")
  for part in parts:
    if not isinstance(obj, dict) or part not in obj:
      return None
    obj = obj[part]
  return obj


def stage_key(stage: dict) -> int:
  return stage["children"]


def collect_metric(runs: list[list[dict]], children: int, metric_path: str) -> list[float]:
  values = []
  for run in runs:
    for stage in run:
      if stage["children"] == children:
        val = get_by_path(stage, metric_path)
        if val is not None:
          values.append(float(val))
  return values


def noise(values: list[float]) -> float:
  """Noise threshold: larger of 5% and 2*(max-min)/median."""
  if len(values) < 2:
    return 0.05
  med = statistics.median(values)
  if med == 0:
    return 0.05
  spread = 2 * (max(values) - min(values)) / med
  return max(0.05, spread)


def summarize(values: list[float]) -> dict:
  if not values:
    return {"n": 0, "median": None, "min": None, "max": None}
  med = statistics.median(values)
  return {"n": len(values), "median": med, "min": min(values), "max": max(values)}


def compare_metric(
    base_values: list[float],
    cand_values: list[float],
    lower_is_better: bool,
    label: str,
    children: int,
) -> dict:
  b = summarize(base_values)
  c = summarize(cand_values)
  if b["median"] is None or c["median"] is None:
    return {"label": label, "children": children, "verdict": "MISSING", "b": b, "c": c}

  threshold = noise(base_values)
  b_med = b["median"]
  c_med = c["median"]
  if b_med == 0:
    rel = 0.0
  else:
    rel = (c_med - b_med) / b_med

  # positive rel = candidate increased; negative = decreased
  if lower_is_better:
    improvement = rel < -threshold
    regression = rel > threshold
  else:
    improvement = rel > threshold
    regression = rel < -threshold

  return {
      "label": label,
      "children": children,
      "base": b,
      "cand": c,
      "rel_change": rel,
      "threshold": threshold,
      "improvement": improvement,
      "regression": regression,
      "verdict": "IMPROVED" if improvement else ("REGRESSED" if regression else "UNCHANGED"),
  }


def fmt_rel(rel: float) -> str:
  sign = "+" if rel >= 0 else ""
  return f"{sign}{rel * 100:.1f}%"


def fmt_median(v) -> str:
  if v is None:
    return "n/a"
  return f"{v:.2f}"


def main():
  # Split args at --
  argv = sys.argv[1:]
  if "--" not in argv:
    print("Usage: compare_benchmarks.py BASE_DIR... -- CAND_DIR...")
    sys.exit(2)
  sep = argv.index("--")
  base_dirs = [Path(d) for d in argv[:sep]]
  cand_dirs = [Path(d) for d in argv[sep + 1:]]

  if not base_dirs or not cand_dirs:
    print("Need at least one base and one candidate directory.")
    sys.exit(2)

  base_runs = [load_summary(d) for d in base_dirs]
  cand_runs = [load_summary(d) for d in cand_dirs]

  all_children = sorted({s["children"] for run in base_runs for s in run})

  print(
      f"\n{'Stage':>8}  {'Metric':<30}  {'Base med':>10}  {'Cand med':>10}  {'Δ':>8}  {'Thr':>6}  Verdict"
  )
  print("-" * 90)

  improvements = []
  regressions = []

  for children in all_children:
    for _stage_key, metric_path, lower_is_better, label in GUARD_METRICS:
      base_vals = collect_metric(base_runs, children, metric_path)
      cand_vals = collect_metric(cand_runs, children, metric_path)
      if not base_vals and not cand_vals:
        continue
      result = compare_metric(base_vals, cand_vals, lower_is_better, label, children)
      verdict = result["verdict"]
      delta_str = fmt_rel(result["rel_change"]) if result.get("rel_change") is not None else "n/a"
      thr_str = f"±{result.get('threshold', 0) * 100:.0f}%"
      print(
          f"{children:>8}  {label:<30}  "
          f"{fmt_median(result['base']['median']):>10}  "
          f"{fmt_median(result['cand']['median']):>10}  "
          f"{delta_str:>8}  {thr_str:>6}  {verdict}"
      )
      if result["improvement"] and children in PRIMARY_STAGES and metric_path == PRIMARY_METRIC_PATH:
        improvements.append(result)
      if result["regression"]:
        regressions.append(result)

  print()

  # Overall verdict
  keep_primary = any(
      r["children"] in PRIMARY_STAGES and r.get("improvement") and PRIMARY_METRIC_PATH in
      (r.get("label") or "")
      # Re-check via metric path in the full result dict
      for r in improvements
  )
  # Simpler: count improvements where metric path matches
  primary_improved = bool(improvements)

  if regressions:
    print("❌  REVERT: guard metric regressions detected:")
    for r in regressions:
      print(
          f"   {r['children']}-child {r['label']}: {fmt_rel(r['rel_change'])} (threshold ±{r['threshold']*100:.0f}%)"
      )
    print()
    sys.exit(1)
  elif primary_improved:
    print("✅  KEEP: primary metric improved beyond noise with no guard regressions.")
    print(f"   Improved stages: {', '.join(str(r['children']) for r in improvements)}")
    print()
    sys.exit(0)
  else:
    print("⚠️   REVERT: no primary metric improvement detected beyond noise.")
    print()
    sys.exit(1)


if __name__ == "__main__":
  main()
