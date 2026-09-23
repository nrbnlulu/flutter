#!/usr/bin/env bash
# Copies the subset of an engine out directory (out/host_<mode>) that the
# Flutter tool needs to treat it as a local engine for the Rust shell:
# the engine library plus the exactly-matching kernel compiler pieces.
# Usage: package_engine_artifacts.sh <out/host_mode> <destination>
set -euo pipefail

src="$1"
dst="$2"
mkdir -p "$dst/gen/flutter/lib/snapshot" "$dst/dart-sdk/bin/snapshots"

for f in libflutter_rust_engine.so flutter_rust_shell_runner icudtl.dat gen_snapshot font-subset; do
  [[ -e "$src/$f" ]] && cp -L "$src/$f" "$dst/$f"
done
cp -RL "$src/flutter_patched_sdk" "$dst/"
cp -L "$src"/gen/flutter/lib/snapshot/*.bin "$dst/gen/flutter/lib/snapshot/"
cp -L "$src/dart-sdk/bin/dart" "$src/dart-sdk/bin/dartaotruntime" "$dst/dart-sdk/bin/"
cp -L "$src/dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot" "$dst/dart-sdk/bin/snapshots/"
