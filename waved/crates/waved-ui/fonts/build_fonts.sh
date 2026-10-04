#!/usr/bin/env bash
# Regenerate the LVGL font sources in src/kit/fonts/ and src/kit/icons.rs from
# pinned upstream fonts. Needs curl, node (npx) and python3. Outputs are
# committed; cargo never runs this script.
set -euo pipefail
cd "$(dirname "$0")"

GOOGLE_FONTS=9710da1eacb3be272583c3224dcb70f9da6eadbb
TABLER=3.48.0
CONV="npx -y lv_font_conv@1.5.3"
OUT=$PWD/../src/kit/fonts
TEXT_RANGE="0x20-0x7E,0xA0-0x17F,0x2013,0x2014,0x2018,0x2019,0x201C,0x201D,0x2022,0x2026,0x2212"
DIGIT_RANGE="0x20,0x25,0x2B,0x2D,0x2E,0x30-0x3A,0x61,0x6D,0x70,0x2212"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

raw() { curl -fsSL -o "$2" "https://raw.githubusercontent.com/google/fonts/$GOOGLE_FONTS/ofl/$1"; }
raw barlowsemicondensed/BarlowSemiCondensed-Medium.ttf   "$WORK/bsc-medium.ttf"
raw barlowsemicondensed/BarlowSemiCondensed-SemiBold.ttf "$WORK/bsc-semibold.ttf"
raw barlow/Barlow-Medium.ttf                             "$WORK/b-medium.ttf"
raw barlow/OFL.txt                                       LICENSE-Barlow.txt
(cd "$WORK" && npm pack --silent "@tabler/icons-webfont@$TABLER" >/dev/null \
  && tar xzf "tabler-icons-webfont-$TABLER.tgz")
cp "$WORK/package/LICENSE" LICENSE-Tabler.txt

# Freeze tabular figures so digits keep their width while a timer runs.
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/pip" -q install fonttools==4.66.1 opentype-feature-freezer==1.32.2
for face in bsc-medium bsc-semibold b-medium; do
  "$WORK/venv/bin/pyftfeatfreeze" -f tnum "$WORK/$face.ttf" "$WORK/$face-tnum.ttf" >/dev/null
done

mkdir -p "$OUT"
# lv_font_conv records its argument list in the generated header, so run it from
# inside $WORK with relative paths: the mktemp directory never leaks into the
# output and regenerating twice gives byte-identical files.
conv() { # name font size range
  (cd "$WORK" && $CONV --font "$2" --size "$3" --bpp 4 --format lvgl --no-compress \
    -r "$4" --lv-font-name "$1" -o "$1.c" >/dev/null)
  mv "$WORK/$1.c" "$OUT/$1.c"
}
conv font_micro_12 bsc-medium-tnum.ttf   12 "$TEXT_RANGE"
conv font_small_13 bsc-medium-tnum.ttf   13 "$TEXT_RANGE"
conv font_body_15  bsc-medium-tnum.ttf   15 "$TEXT_RANGE"
conv font_title_20 bsc-semibold-tnum.ttf 20 "$TEXT_RANGE"
conv font_dial_26  bsc-semibold-tnum.ttf 26 "$TEXT_RANGE"
conv font_num_13   b-medium-tnum.ttf     13 "$TEXT_RANGE"
conv font_num_20   b-medium-tnum.ttf     20 "$TEXT_RANGE"
conv font_num_26   b-medium-tnum.ttf     26 "$DIGIT_RANGE"
conv font_num_48   b-medium-tnum.ttf     48 "$DIGIT_RANGE"

CSS="$WORK/package/dist/tabler-icons.css"
ICON_RANGE=$(python3 icons.py range "$CSS")
for size in 16 20 26 44; do
  conv "font_icons_$size" package/dist/fonts/tabler-icons.ttf "$size" "$ICON_RANGE"
done
python3 icons.py rust "$CSS" > ../src/kit/icons.rs
echo "fonts regenerated in $OUT"
