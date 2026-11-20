#!/bin/bash
# Build ArduPilot SITL as WebAssembly (board `wasm`) and put it in public/sitl.
# Run on Linux or in WSL. The ArduPilot commit is `sitl_wasm.commit` in scripts/check-upstream.mjs.
# Usage: scripts/build-sitl-wasm.sh [work dir]   (default: ~/arduloops-wasm)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/public/sitl"
WORK="${1:-$HOME/arduloops-wasm}"
EMSCRIPTEN_VERSION="${EMSCRIPTEN_VERSION:-6.0.8}"
EMSDK_ROOT="${EMSDK_ROOT:-$WORK/emsdk}"
SRC="$WORK/ardupilot"

SHA="$(awk '/^  sitl_wasm: \{/,/^  \},/' "$ROOT/scripts/check-upstream.mjs" | sed -n "s/^ *commit: '\([0-9a-f]\{40\}\)'.*/\1/p")"
if [ -z "$SHA" ]; then
  echo "no sitl_wasm.commit in scripts/check-upstream.mjs" >&2
  exit 2
fi

mkdir -p "$WORK"
if [ ! -d "$EMSDK_ROOT/.git" ]; then
  git clone https://github.com/emscripten-core/emsdk.git "$EMSDK_ROOT"
fi
"$EMSDK_ROOT/emsdk" install "$EMSCRIPTEN_VERSION"
"$EMSDK_ROOT/emsdk" activate "$EMSCRIPTEN_VERSION"
# shellcheck disable=SC1091
source "$EMSDK_ROOT/emsdk_env.sh" >/dev/null

if [ ! -d "$SRC/.git" ]; then
  git init -q "$SRC"
  git -C "$SRC" remote add origin https://github.com/ArduPilot/ardupilot.git
fi
git -C "$SRC" fetch -q --depth 1 origin "$SHA"
git -C "$SRC" checkout -q --force "$SHA"
git -C "$SRC" submodule update --init --recursive --depth 1

cd "$SRC"
./waf configure --board wasm
./waf copter plane

mkdir -p "$DEST"
for v in arducopter arduplane; do
  cp -f "build/wasm/bin/$v.js" "build/wasm/bin/$v.wasm" "$DEST/"
done
cp -f COPYING.txt "$DEST/COPYING.txt"
cat > "$DEST/NOTICE.txt" <<EOF
arducopter and arduplane SITL, built as WebAssembly with Emscripten $EMSCRIPTEN_VERSION
from https://github.com/ArduPilot/ardupilot/tree/$SHA (board wasm).
ArduPilot is licensed under the GPLv3 in COPYING.txt.
Rebuild with scripts/build-sitl-wasm.sh.
EOF
ls -l "$DEST"
