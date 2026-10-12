#!/usr/bin/env python3
"""Build the boot splash (deploy/splash.fb) from the start-up screen golden.

The bootloader's Insta360 screen stays up until something draws over it, and
waved draws its own start-up screen only ~10-15 s after power-on.
deploy/S03splash writes this image to the panel as soon as /data is mounted,
so boot shows waved's start-up ring almost at once. The golden's version line
is a placeholder, so it is blanked here.

    scripts/make-splash.py crates/waved-ui/golden/boot.png deploy/splash.fb
"""
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("png_to_fb", os.path.join(HERE, "png-to-fb.py"))
png_to_fb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(png_to_fb)

# The version line under "Starting" (rows 133-141, columns 171-226 in the
# golden), with a margin; the ring is 60 px further out on these rows.
BLANK_ROWS = range(130, 146)
BLANK_COLS = range(150, 251)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: make-splash.py <boot.png> <out.fb>")
    rows = png_to_fb.read_rgb(sys.argv[1])
    for y in BLANK_ROWS:
        for x in BLANK_COLS:
            rows[y][x * 3:x * 3 + 3] = b"\x00\x00\x00"
    out = bytearray()
    for y in reversed(range(png_to_fb.H)):
        line = rows[y]
        for x in reversed(range(png_to_fb.W)):
            r, g, b = line[x * 3:x * 3 + 3]
            out += png_to_fb.struct.pack(">H", ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3))
    with open(sys.argv[2], "wb") as handle:
        handle.write(out)


if __name__ == "__main__":
    main()
