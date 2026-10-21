#!/usr/bin/env bash
# Copies the subset of an Android engine out directory (out/android_<mode>_<cpu>)
# that rlutter links into a project: the engine library, ICU data, the
# platform dill, the debug VM snapshots, and (release) the host-run AOT
# compiler. Keep in sync with ENGINE_ENTRIES in rlutter/src/sdk.rs.
# Usage: package_android_engine_artifacts.sh <out/android_mode_cpu> <destination>
set -euo pipefail

src="$1"
dst="$2"
mkdir -p "$dst/gen/flutter/lib/snapshot" "$dst/clang_x64"

for f in libflutter_rust_engine.so icudtl.dat; do
  cp -L "$src/$f" "$dst/$f"
done
cp -RL "$src/flutter_patched_sdk" "$dst/"
# Debug engines carry the VM snapshots; release ones have none.
cp -L "$src"/gen/flutter/lib/snapshot/*.bin "$dst/gen/flutter/lib/snapshot/" 2>/dev/null || true
[[ -e "$src/clang_x64/gen_snapshot" ]] && cp -L "$src/clang_x64/gen_snapshot" "$dst/clang_x64/gen_snapshot"
exit 0
