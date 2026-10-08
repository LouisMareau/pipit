#!/usr/bin/env python3
"""Renders the PWA icons (PNG) from the same shapes as web/public/icons/icon.svg.

Pure standard library so it runs anywhere: `python scripts/make-icons.py`.
"""
import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "web" / "public" / "icons"
BG = (0x14, 0x16, 0x1C)
BIRD = (0x5F, 0xD3, 0xBC)
BEAK = (0xF3, 0xC8, 0x6B)


def inside_rounded_rect(x, y, size, radius):
    cx = min(max(x, radius), size - radius)
    cy = min(max(y, radius), size - radius)
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius**2


def inside_ellipse(x, y, cx, cy, rx, ry):
    return ((x - cx) / rx) ** 2 + ((y - cy) / ry) ** 2 <= 1


def inside_triangle(x, y, a, b, c):
    def sign(p1, p2, p3):
        return (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1])

    d1, d2, d3 = sign((x, y), a, b), sign((x, y), b, c), sign((x, y), c, a)
    return not ((d1 < 0 or d2 < 0 or d3 < 0) and (d1 > 0 or d2 > 0 or d3 > 0))


def pixel(u, v, maskable):
    """Colour at SVG coordinates (0-64). Maskable icons keep a solid full bleed."""
    if not maskable and not inside_rounded_rect(u, v, 64, 14):
        return None
    if inside_triangle(u, v, (50, 25), (57, 27), (50, 29)):
        return BEAK
    if (u - 46) ** 2 + (v - 24.5) ** 2 <= 1.4**2:
        return BG
    if inside_ellipse(u, v, 33, 36, 15, 10) or (u - 44) ** 2 + (v - 26) ** 2 <= 49:
        return BIRD
    if inside_triangle(u, v, (18, 34), (8, 28), (20, 38)):
        return BIRD
    for lx0, ly0, lx1, ly1 in ((30, 45, 28, 52), (35, 45, 36, 52)):
        t = max(0, min(1, ((u - lx0) * (lx1 - lx0) + (v - ly0) * (ly1 - ly0)) / ((lx1 - lx0) ** 2 + (ly1 - ly0) ** 2)))
        if math.hypot(u - (lx0 + t * (lx1 - lx0)), v - (ly0 + t * (ly1 - ly0))) <= 1:
            return BEAK
    return BG


def render(size, maskable, supersample=3):
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(supersample):
                for sx in range(supersample):
                    u = (px + (sx + 0.5) / supersample) * 64 / size
                    v = (py + (sy + 0.5) / supersample) * 64 / size
                    c = pixel(u, v, maskable)
                    if c:
                        acc[0] += c[0]
                        acc[1] += c[1]
                        acc[2] += c[2]
                        acc[3] += 255
            n = supersample * supersample
            a = acc[3] // n
            if a:
                row += bytes((acc[0] * 255 // acc[3], acc[1] * 255 // acc[3], acc[2] * 255 // acc[3], a))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return rows


def write_png(path, size, rows):
    raw = b"".join(b"\0" + r for r in rows)

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)
    print(f"wrote {path} ({len(png)} bytes)")


def write_ico(path, size, rows):
    """ICO holding a single PNG-compressed image (Windows accepts PNG entries)."""
    tmp = path.with_suffix(".tmp.png")
    write_png(tmp, size, rows)
    png = tmp.read_bytes()
    tmp.unlink()
    dim = 0 if size >= 256 else size
    header = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), 22)
    path.write_bytes(header + entry + png)
    print(f"wrote {path} ({len(png) + 22} bytes)")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    write_png(OUT / "icon-192.png", 192, render(192, maskable=False))
    write_png(OUT / "icon-512.png", 512, render(512, maskable=True))

    # Desktop (Tauri) icons.
    desktop = Path(__file__).resolve().parent.parent / "desktop" / "src-tauri" / "icons"
    desktop.mkdir(parents=True, exist_ok=True)
    write_png(desktop / "32x32.png", 32, render(32, maskable=False))
    write_png(desktop / "128x128.png", 128, render(128, maskable=False))
    write_png(desktop / "128x128@2x.png", 256, render(256, maskable=False))
    write_png(desktop / "icon.png", 512, render(512, maskable=False))
    write_ico(desktop / "icon.ico", 256, render(256, maskable=False))
