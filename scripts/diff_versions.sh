#!/usr/bin/env bash
# Diff decompiled .c output across firmware versions.
# Walks each of the 6 target binaries, computes per-version SHA + line count,
# and produces a unified diff between consecutive versions.
#
# Output:
#   firmwares/fw_v3.0.0.1/decompiled/SUMMARY.md
#   firmwares/fw_v3.0.0.1/decompiled/diff_<binary>_<oldver>_to_<newver>.diff
#
# The "consecutive" order is the chronological consumer release line plus
# the business build folded in next to the closest consumer version:
#   fw_v1.0.13.6  -> fw_business_v1.0.2.6  -> fw_v1.1.6.6  -> fw_v3.0.0.1
# (Business is grouped between v1.0.13.6 and v1.1.6.6 because its binaries
# are derived from a 1.0.x base.)
set -euo pipefail

ROOT="/Users/xaviergeerinck/Projects/insta360"
OUT_DIR="$ROOT/firmwares/fw_v3.0.0.1/decompiled"
SUMMARY="$OUT_DIR/SUMMARY.md"

VERS=(
  "fw_v1.0.13.6"
  "fw_business_v1.0.2.6"
  "fw_v1.1.6.6"
  "fw_v3.0.0.1"
)

BINARIES=(
  "libInstaAudio.so"
  "libcloud_proto_https.so"
  "libdatabase.so"
  "libins_entry.so"
  "libui.so"
  "pspapp"
)

{
echo "# Decompiled-firmware comparison"
echo
echo "Generated: $(date '+%F %T')"
echo
echo "## Version order (chronological-ish)"
printf '  - %s\n' "${VERS[@]}"
echo
echo "## Per-binary stats"
echo
printf '| binary | %s |\n' "$(IFS=' | '; echo "${VERS[*]}")"
printf '|---|%s|\n' "$(printf -- '---|%.0s' "${VERS[@]}")"
for bin in "${BINARIES[@]}"; do
  row="| $bin |"
  for v in "${VERS[@]}"; do
    f="$ROOT/firmwares/$v/decompiled/$bin.c"
    if [ -f "$f" ]; then
      lines=$(wc -l < "$f" | tr -d ' ')
      bytes=$(wc -c < "$f" | tr -d ' ')
      funcs=$(grep -c '^// Function at ' "$f" || true)
      row="$row ${funcs}f / ${lines}L / $((bytes/1024))K |"
    else
      row="$row — |"
    fi
  done
  echo "$row"
done
echo
echo "## Diffs (consecutive)"
for bin in "${BINARIES[@]}"; do
  echo
  echo "### \`$bin\`"
  for i in $(seq 0 $((${#VERS[@]}-2))); do
    a="${VERS[$i]}"
    b="${VERS[$i+1]}"
    fa="$ROOT/firmwares/$a/decompiled/$bin.c"
    fb="$ROOT/firmwares/$b/decompiled/$bin.c"
    if [ ! -f "$fa" ] || [ ! -f "$fb" ]; then
      echo "- $a → $b: skipped (missing input)"
      continue
    fi
    diff_path="$OUT_DIR/diff_${bin}_${a}_to_${b}.diff"
    # Drop the function header comment line (// Function at 0x...) to reduce
    # spurious diffs from address shifts; we keep the bodies.
    diff -u \
      <(grep -v '^// Function at ' "$fa") \
      <(grep -v '^// Function at ' "$fb") \
      > "$diff_path" || true
    added=$(grep -c '^+[^+]' "$diff_path" || true)
    removed=$(grep -c '^-[^-]' "$diff_path" || true)
    bytes=$(wc -c < "$diff_path" | tr -d ' ')
    rel_diff="${diff_path#$ROOT/}"
    echo "- $a → $b: +${added} / -${removed} lines, $((bytes/1024))K  →  \`$rel_diff\`"
  done
done
} > "$SUMMARY"

echo "wrote $SUMMARY"
