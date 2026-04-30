#!/usr/bin/env bash
# Push the confirmation-screen icons to the device's pspapp asset tree.
# Mirrors the convention used by other btn_*.png assets (see
# waved/crates/waved-ui/src/screens/recording.rs:486).
set -euo pipefail

SRC="$(cd "$(dirname "$0")/.." && pwd)/waved/crates/waved-ui/assets/icon/btn"
DST="/etc/pspapp/lvgl/icon/btn"

for f in btn_save.png btn_send.png btn_discard.png; do
  adb push "$SRC/$f" "$DST/$f"
done
