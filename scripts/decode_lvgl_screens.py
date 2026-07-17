#!/usr/bin/env python3
"""Decode Insta360 Wave LVGL screen assets and live framebuffer dumps to PNG.

Pure stdlib (struct + zlib) — no Pillow / ImageMagick dependency, matching the
convention in generate-confirmation-icons.py.

Two input formats:

1. LVGL v8 *.bin image  (icon/bin/<seq>/*.bin)
   4-byte lv_img_header_t bitfield (LE uint32):
       cf:5  always_zero:3  reserved:2  w:11  h:21..31
   followed by w*h little-endian RGB565 pixels (cf==4 TRUE_COLOR, LV_COLOR_DEPTH 16).
   These hold LVGL's logical buffer, which on this device == the visual image
   (the 180deg mount and the 180deg flush() rotation cancel — see
   waved/docs/ui/COORDINATE_SYSTEM.md), so no rotation is applied.

2. Raw /dev/fb0 dump  (400x304x2, 243200 bytes, no header)
   The flush() callback byte-swaps each RGB565 pixel and rotates 180deg before
   writing to fb0. To recover the visual (LVGL-logical) image we interpret fb0
   as BIG-endian RGB565 and rotate 180deg back.
"""
import struct
import sys
import zlib
from pathlib import Path

FB_W, FB_H = 400, 304


def rgb565_to_rgb888(px):
    r5 = (px >> 11) & 0x1F
    g6 = (px >> 5) & 0x3F
    b5 = px & 0x1F
    # bit-replication expansion
    return ((r5 << 3) | (r5 >> 2),
            (g6 << 2) | (g6 >> 4),
            (b5 << 3) | (b5 >> 2))


def write_png_rgb(path, w, h, rows):
    """rows: list of h bytearrays, each 3*w bytes (R,G,B...)."""
    def chunk(tag, data):
        out = struct.pack(">I", len(data)) + tag + data
        return out + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = bytearray()
    for row in rows:
        raw.append(0)  # filter type 0
        raw.extend(row)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    Path(path).write_bytes(png)


def decode_lvgl_bin(path):
    data = Path(path).read_bytes()
    v = struct.unpack("<I", data[:4])[0]
    cf = v & 0x1F
    w = (v >> 10) & 0x7FF
    h = (v >> 21) & 0x7FF
    if cf != 4:
        raise ValueError(f"{path}: unsupported cf={cf} (only TRUE_COLOR/4 handled)")
    px = data[4:4 + w * h * 2]
    rows = []
    for y in range(h):
        row = bytearray(3 * w)
        base = y * w * 2
        for x in range(w):
            p = px[base + 2 * x] | (px[base + 2 * x + 1] << 8)
            r, g, b = rgb565_to_rgb888(p)
            row[3 * x] = r
            row[3 * x + 1] = g
            row[3 * x + 2] = b
        rows.append(row)
    return w, h, rows


def decode_fb0(path, w=FB_W, h=FB_H):
    data = Path(path).read_bytes()[: w * h * 2]
    # big-endian RGB565, then rotate 180deg
    pix = [[None] * w for _ in range(h)]
    for y in range(h):
        base = y * w * 2
        for x in range(w):
            p = (data[base + 2 * x] << 8) | data[base + 2 * x + 1]  # BE
            pix[y][x] = rgb565_to_rgb888(p)
    rows = []
    for y in range(h):
        row = bytearray(3 * w)
        sy = h - 1 - y
        for x in range(w):
            r, g, b = pix[sy][w - 1 - x]  # rotate 180
            row[3 * x] = r
            row[3 * x + 1] = g
            row[3 * x + 2] = b
        rows.append(row)
    return w, h, rows


def main():
    mode = sys.argv[1]
    src = sys.argv[2]
    dst = sys.argv[3]
    if mode == "bin":
        w, h, rows = decode_lvgl_bin(src)
    elif mode == "fb0":
        w, h, rows = decode_fb0(src)
    else:
        sys.exit(f"unknown mode {mode}")
    write_png_rgb(dst, w, h, rows)
    print(f"{dst}  ({w}x{h})")


if __name__ == "__main__":
    main()
