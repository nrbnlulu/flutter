#!/usr/bin/env bash
# Package the standard Android embedding plus the matching compiler inputs.
# rlutter must never combine an upstream flutter.jar with our native engine.
# Usage: package_android_engine_artifacts.sh <out/android_mode_cpu> <destination>
set -euo pipefail
src="$1"
dst="$2"
mkdir -p "$dst/clang_x64" "$dst/gen/flutter/lib/snapshot"

# The native library is linked by Cargo; the Maven jars/POMs supply the same
# engine and its Java dependencies to Flutter's Gradle plugin.
for f in libflutter.so flutter.jar; do
  cp -L "$src/$f" "$dst/$f"
done
for artifact in "$src"/flutter_embedding_*.pom "$src"/arm64_v8a_*.pom; do
  name="$(basename "$artifact" .pom)"
  for extension in pom jar maven-metadata.xml; do
    cp -L "$src/$name.$extension" "$dst/$name.$extension"
  done
done
cp -RL "$src/flutter_patched_sdk" "$dst/"
# Flutter's debug assemble target reads these snapshots from local-engine-out
# even though libflutter.so also embeds its own VM code.
for snapshot in "$src"/gen/flutter/lib/snapshot/*.bin; do
  if [[ -f "$snapshot" ]]; then
    cp -L "$snapshot" "$dst/gen/flutter/lib/snapshot/"
  fi
done
if [[ -e "$src/clang_x64/gen_snapshot" ]]; then
  cp -L "$src/clang_x64/gen_snapshot" "$dst/clang_x64/gen_snapshot"
fi
