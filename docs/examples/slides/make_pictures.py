#!/usr/bin/env python3
"""Write the example deck's pictures (synthetic, no inputs): a dusk-sky
gradient and a chart-like pattern, as PNG, with only the standard library.

    python3 docs/examples/slides/make_pictures.py
"""

import pathlib
import struct
import zlib

HERE = pathlib.Path(__file__).resolve().parent


def png(path, w, h, pixel):
    rows = b"".join(
        b"\x00" + b"".join(bytes(pixel(x, y)) for x in range(w)) for y in range(h)
    )

    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(rows, 9))
    data += chunk(b"IEND", b"")
    path.write_bytes(data)


def mix(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def sky(x, y):
    # Navy at the top to warm orange at the horizon, with a dark ground.
    w, h = 480, 270
    horizon = int(h * 0.72)
    if y > horizon:
        # Rolling hills.
        hill = horizon + 18 * (1 + __import__("math").sin(x / 37.0)) // 2
        if y > hill:
            return (24, 30, 48)
    t = y / horizon
    return mix((18, 32, 77), (242, 140, 64), min(1.0, t * t))


def bars(x, y):
    w, h = 320, 320
    bg = (236, 241, 248)
    for i, v in enumerate([0.35, 0.55, 0.48, 0.72, 0.9]):
        left = 30 + i * 56
        if left <= x < left + 40 and y >= h - 20 - int(v * (h - 60)) and y < h - 20:
            return mix((79, 129, 189), (31, 56, 100), i / 4)
    if y == h - 20 and 20 <= x < w - 20:
        return (120, 130, 145)
    return bg


png(HERE / "sky.png", 480, 270, sky)
png(HERE / "bars.png", 320, 320, bars)
