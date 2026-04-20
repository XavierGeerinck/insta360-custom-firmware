#!/usr/bin/env bash
# Run waved-mic-map on-device: stops waved, runs the tap harness,
# pulls the resulting JSON back to the host.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "building waved-mic-map for aarch64..."
(cd "$REPO_ROOT" && cross build -p waved-mic-map --release --target aarch64-unknown-linux-gnu)

BIN="$REPO_ROOT/target/aarch64-unknown-linux-gnu/release/waved-mic-map"
[[ -x "$BIN" ]] || { echo "binary not found: $BIN"; exit 1; }

echo "pushing to device..."
adb push "$BIN" /data/waved-mic-map
adb shell chmod +x /data/waved-mic-map

echo "stopping waved..."
adb shell 'pkill -9 waved || true'
sleep 1

echo "running tap harness. FOLLOW ON-SCREEN PROMPTS."
adb shell /data/waved-mic-map

echo "pulling result..."
adb pull /tmp/mic_map.json "$REPO_ROOT/mic_map.json"

echo
echo "Result:"
cat "$REPO_ROOT/mic_map.json"
echo
echo "Next step: copy the alsa_to_physical array into the MicGeometry default"
echo "in crates/waved-audio/src/beam/geometry.rs and commit."
