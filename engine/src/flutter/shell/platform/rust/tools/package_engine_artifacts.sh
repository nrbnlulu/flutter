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
# Read engine-compiled metadata without starting an isolate or GPU device.
python3 - "$dst" <<'PY'
import ctypes
import json
from pathlib import Path
import sys
root = Path(sys.argv[1]).resolve()
engine = ctypes.CDLL(str(root / 'libflutter_rust_engine.so'))
engine.FlutterRustShellGetEngineAbiVersion.restype = ctypes.c_uint32
engine.FlutterRustShellGetBuildMode.restype = ctypes.c_char_p
engine.FlutterRustShellGetEngineRevision.restype = ctypes.c_char_p
value = {'formatVersion': 1, 'shellAbi': engine.FlutterRustShellGetEngineAbiVersion(),
         'buildMode': engine.FlutterRustShellGetBuildMode().decode(),
         'engineRevision': engine.FlutterRustShellGetEngineRevision().decode()}
(root / 'engine-manifest.json').write_text(json.dumps(value, indent=2) + '\n')
PY
