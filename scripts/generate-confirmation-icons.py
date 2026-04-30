#!/usr/bin/env python3
"""Generate the three confirmation-screen icon PNGs (36x36, white on transparent).

Pure stdlib — no Pillow / ImageMagick dependency. Writes PNGs in greyscale-alpha
format (color type 4, 8-bit). Run once on a host machine; commit the resulting
PNGs alongside this script.
"""
import struct
import zlib
from pathlib import Path

W, H = 36, 36


def make_canvas():
    return [[(0, 0) for _ in range(W)] for _ in range(H)]


def set_pixel(canvas, x, y):
    if 0 <= x < W and 0 <= y < H:
        canvas[y][x] = (255, 255)


def fill_rect(canvas, x0, y0, x1, y1):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            set_pixel(canvas, x, y)


def stroke_rect(canvas, x0, y0, x1, y1, thickness=2):
    for t in range(thickness):
        for x in range(x0, x1 + 1):
            set_pixel(canvas, x, y0 + t)
            set_pixel(canvas, x, y1 - t)
        for y in range(y0, y1 + 1):
            set_pixel(canvas, x0 + t, y)
            set_pixel(canvas, x1 - t, y)


def draw_line(canvas, x0, y0, x1, y1, thickness=2):
    dx = abs(x1 - x0)
    sx = 1 if x0 < x1 else -1
    dy = -abs(y1 - y0)
    sy = 1 if y0 < y1 else -1
    err = dx + dy
    half = thickness // 2
    while True:
        for ty in range(-half, thickness - half):
            for tx in range(-half, thickness - half):
                set_pixel(canvas, x0 + tx, y0 + ty)
        if x0 == x1 and y0 == y1:
            break
        e2 = 2 * err
        if e2 >= dy:
            err += dy
            x0 += sx
        if e2 <= dx:
            err += dx
            y0 += sy


def fill_triangle(canvas, p0, p1, p2):
    pts = sorted([p0, p1, p2], key=lambda p: p[1])
    (ax, ay), (bx, by), (cx, cy) = pts

    def edge_x(pa, pb, y):
        if pa[1] == pb[1]:
            return pa[0]
        t = (y - pa[1]) / (pb[1] - pa[1])
        return pa[0] + t * (pb[0] - pa[0])

    for y in range(ay, cy + 1):
        if y < by:
            x_left = edge_x((ax, ay), (bx, by), y)
            x_right = edge_x((ax, ay), (cx, cy), y)
        else:
            x_left = edge_x((bx, by), (cx, cy), y)
            x_right = edge_x((ax, ay), (cx, cy), y)
        if x_left > x_right:
            x_left, x_right = x_right, x_left
        for x in range(int(round(x_left)), int(round(x_right)) + 1):
            set_pixel(canvas, x, y)


def write_png(canvas, path):
    raw = bytearray()
    for row in canvas:
        raw.append(0)
        for (lum, alpha) in row:
            raw.append(lum)
            raw.append(alpha)
    compressed = zlib.compress(bytes(raw), level=9)

    def chunk(tag, data):
        out = struct.pack(">I", len(data)) + tag + data
        crc = zlib.crc32(tag + data)
        out += struct.pack(">I", crc)
        return out

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", W, H, 8, 4, 0, 0, 0)
    png = sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", compressed) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def save_icon():
    c = make_canvas()
    stroke_rect(c, 4, 4, 32, 32, thickness=2)
    fill_rect(c, 10, 4, 26, 12)
    fill_rect(c, 8, 22, 28, 30)
    return c


def send_icon():
    """Upload-arrow icon: thick up-pointing arrow with a base bar.

    Reads as "send to remote" — a paper-plane silhouette is too noisy at
    36×36 mono. Vertices designed for clean fill at this resolution.
    """
    c = make_canvas()
    # Arrowhead — filled triangle pointing up.
    fill_triangle(c, (18, 4), (4, 18), (32, 18))
    # Shaft — vertical filled rect under the head.
    fill_rect(c, 14, 18, 22, 28)
    # Base bar — short horizontal "ground" line for the upload metaphor.
    fill_rect(c, 8, 30, 28, 32)
    return c


def discard_icon():
    c = make_canvas()
    stroke_rect(c, 9, 12, 27, 32, thickness=2)
    fill_rect(c, 6, 8, 30, 11)
    fill_rect(c, 14, 5, 22, 8)
    for x in (14, 18, 22):
        draw_line(c, x, 16, x, 28, thickness=1)
    return c


def main():
    here = Path(__file__).resolve().parent
    out = here.parent / "waved" / "crates" / "waved-ui" / "assets" / "icon" / "btn"
    out.mkdir(parents=True, exist_ok=True)

    write_png(save_icon(),    out / "btn_save.png")
    write_png(send_icon(),    out / "btn_send.png")
    write_png(discard_icon(), out / "btn_discard.png")

    print(f"Wrote PNGs in {out}")
    for f in ("btn_save.png", "btn_send.png", "btn_discard.png"):
        size = (out / f).stat().st_size
        print(f"  {f}  {size} bytes")


if __name__ == "__main__":
    main()
