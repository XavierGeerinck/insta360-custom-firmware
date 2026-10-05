#!/usr/bin/env python3
"""Convert a 400x304 golden PNG into a raw image for the Wave framebuffer.

The panel takes big-endian RGB565 and is mounted upside down, so the image is
rotated 180 degrees (docs/DISPLAY_HARDWARE.md, "flush() implementation").

    scripts/png-to-fb.py crates/waved-ui/golden/kit_home.png target/kit_home.fb
"""
import struct
import sys
import zlib

W, H = 400, 304


def read_rgb(path):
    """Rows of RGB bytes from an 8-bit, non-interlaced RGB PNG."""
    with open(path, "rb") as handle:
        data = handle.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit(f"{path}: not a PNG")
    pos, idat, header = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if (width, height, depth, color, interlace) != (W, H, 8, 2, 0):
        sys.exit(f"{path}: expected a {W}x{H} 8-bit RGB PNG, got {header}")
    raw = zlib.decompress(idat)
    stride = W * 3
    rows, prev = [], bytearray(stride)
    for y in range(H):
        start = y * (stride + 1)
        kind = raw[start]
        line = bytearray(raw[start + 1:start + 1 + stride])
        for i in range(stride):
            a = line[i - 3] if i >= 3 else 0
            b = prev[i]
            c = prev[i - 3] if i >= 3 else 0
            if kind == 1:
                line[i] = (line[i] + a) & 0xFF
            elif kind == 2:
                line[i] = (line[i] + b) & 0xFF
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 0xFF
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 0xFF
        rows.append(line)
        prev = line
    return rows


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: png-to-fb.py <golden.png> <out.fb>")
    rows = read_rgb(sys.argv[1])
    out = bytearray()
    for y in reversed(range(H)):
        line = rows[y]
        for x in reversed(range(W)):
            r, g, b = line[x * 3:x * 3 + 3]
            out += struct.pack(">H", ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3))
    with open(sys.argv[2], "wb") as handle:
        handle.write(out)


if __name__ == "__main__":
    main()
