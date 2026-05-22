#!/usr/bin/env bash
# Decompile the 6 target binaries from one firmware version using Ghidra headless.
# Usage: decompile_version.sh <version_dir_name>
#   e.g. decompile_version.sh fw_v3.0.0.1
# Outputs:
#   firmwares/<version_dir_name>/decompiled/<binary>.c   (one per binary)
#   firmwares/<version_dir_name>/decompiled/_log.txt     (per-version Ghidra log)
set -euo pipefail

VER="${1:?missing version arg, e.g. fw_v3.0.0.1}"
ROOT="/Users/xaviergeerinck/Projects/insta360"
GHIDRA="/opt/homebrew/Cellar/ghidra/12.0.4/libexec/support/analyzeHeadless"
export JAVA_HOME="/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"
export PATH="$JAVA_HOME/bin:$PATH"
SCRIPT_DIR="$ROOT/scripts"
ROOTFS="$ROOT/firmwares/$VER/extracted/rootfs"
OUT_DIR="$ROOT/firmwares/$VER/decompiled"
LOG="$OUT_DIR/_log.txt"
PROJ_ROOT="/tmp/ghidra_projects/$VER"

BINARIES=(
  "usr/lib/libInstaAudio.so"
  "usr/lib/libcloud_proto_https.so"
  "usr/lib/libdatabase.so"
  "usr/lib/libins_entry.so"
  "usr/lib/libui.so"
  "usr/bin/pspapp"
)

mkdir -p "$OUT_DIR" "$PROJ_ROOT"
: > "$LOG"
echo "[$(date '+%F %T')] version=$VER rootfs=$ROOTFS out=$OUT_DIR" | tee -a "$LOG"

for rel in "${BINARIES[@]}"; do
  src="$ROOTFS/$rel"
  name="$(basename "$rel")"
  out="$OUT_DIR/$name.c"
  if [ ! -f "$src" ]; then
    echo "  SKIP missing: $src" | tee -a "$LOG"
    continue
  fi
  if [ -s "$out" ]; then
    echo "  SKIP already exists: $out ($(wc -c < "$out") bytes)" | tee -a "$LOG"
    continue
  fi
  echo "[$(date '+%F %T')] decompiling $name ..." | tee -a "$LOG"
  proj_name="proj_${name//./_}"
  # Each binary in its own Ghidra project, deleted after to save disk.
  "$GHIDRA" "$PROJ_ROOT" "$proj_name" \
    -import "$src" \
    -overwrite \
    -scriptPath "$SCRIPT_DIR" \
    -postScript decompile_one.java "$OUT_DIR" \
    -deleteProject \
    -scriptlog "$LOG" \
    >> "$LOG" 2>&1 || echo "  ERROR on $name (rc=$?)" | tee -a "$LOG"
  if [ -s "$out" ]; then
    echo "  DONE $name -> $(wc -c < "$out") bytes" | tee -a "$LOG"
  else
    echo "  FAIL $name (no output)" | tee -a "$LOG"
  fi
done

echo "[$(date '+%F %T')] $VER COMPLETE" | tee -a "$LOG"
