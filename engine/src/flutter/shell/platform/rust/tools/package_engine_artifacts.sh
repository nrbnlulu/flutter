#!/usr/bin/env bash
# Copies the subset of an engine out directory (out/host_<mode>) that the
# Flutter tool needs to treat it as a local engine for the Rust shell:
# the engine library plus the exactly-matching kernel compiler pieces.
# Usage: package_engine_artifacts.sh <out/host_mode> <destination>
set -euo pipefail

src="$1"
dst="$2"
mkdir -p "$dst/gen/flutter/lib/snapshot"

for f in libflutter_rust_engine.so flutter_rust_shell_runner icudtl.dat gen_snapshot font-subset impellerc libtessellator.so; do
  [[ -e "$src/$f" ]] && cp -L "$src/$f" "$dst/$f"
done
cp -RL "$src/flutter_patched_sdk" "$dst/"
cp -L "$src"/gen/flutter/lib/snapshot/*.bin "$dst/gen/flutter/lib/snapshot/"
# Keep the complete matching SDK: assemble also needs its libraries and
# metadata, and DDS/dartdev can gain additional runtime dependencies.
cp -RL "$src/dart-sdk" "$dst/"
cp -RL "$src/shader_lib" "$dst/"
mkdir -p "$dst/gen/dart-pkg"
cp -RL "$src/gen/dart-pkg/sky_engine" "$dst/gen/dart-pkg/"
