"""Draw the Audio Bridge icon (five rounded bars) as PNG and ICO files.

Uses only the standard library so it runs anywhere:
    python scripts/make_icon.py
"""

import os
import struct
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BACKGROUND = (27, 40, 56)
BAR = (94, 194, 255)
BARS = ((10.0, 0.35), (20.0, 0.6), (32.0, 0.85), (44.0, 0.6), (54.0, 0.35))


def coverage(fx, fy):
    """Colour and alpha at a point of the 64x64 design."""
    r = 12.0
    dx = abs(fx - 32.0) - (32.0 - r)
    dy = abs(fy - 32.0) - (32.0 - r)
    outside = (max(dx, 0.0) ** 2 + max(dy, 0.0) ** 2) ** 0.5 - r
    if outside > 0:
        return None
    for cx, h in BARS:
        half = h * 26.0
        ddy = max(abs(fy - 32.0) - half, 0.0)
        if ((fx - cx) ** 2 + ddy ** 2) ** 0.5 <= 3.6:
            return BAR
    return BACKGROUND


def render(size, samples=4):
    rows = []
    scale = 64.0 / size
    for y in range(size):
        row = bytearray()
        for x in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(samples):
                for sx in range(samples):
                    fx = (x + (sx + 0.5) / samples) * scale
                    fy = (y + (sy + 0.5) / samples) * scale
                    colour = coverage(fx, fy)
                    if colour:
                        acc[0] += colour[0]
                        acc[1] += colour[1]
                        acc[2] += colour[2]
                        acc[3] += 255
            n = samples * samples
            alpha = acc[3] // n
            if alpha:
                covered = acc[3] // 255
                row += bytes((acc[0] // covered, acc[1] // covered, acc[2] // covered, alpha))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return rows


def png(size):
    rows = render(size)
    raw = b"".join(b"\0" + row for row in rows)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def ico(sizes):
    images = [png(s) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for size, data in zip(sizes, images):
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return header + entries + b"".join(images)


def main():
    os.makedirs(os.path.join(ROOT, "assets"), exist_ok=True)
    with open(os.path.join(ROOT, "assets", "icon.png"), "wb") as handle:
        handle.write(png(256))
    with open(os.path.join(ROOT, "receiver", "assets", "icon.ico"), "wb") as handle:
        handle.write(ico([16, 24, 32, 48, 64, 256]))
    print("wrote assets/icon.png and receiver/assets/icon.ico")


if __name__ == "__main__":
    main()
