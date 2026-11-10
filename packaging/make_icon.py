#!/usr/bin/env python3
"""Draws the application icon (a simple network symbol) as a PNG without extra libraries."""
import struct, sys, zlib

def png(path, size):
    S = size
    px = bytearray()
    def inside_round_rect(x, y, r):
        cx = min(max(x, r), S - 1 - r)
        cy = min(max(y, r), S - 1 - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r
    nodes = [(0.5, 0.24), (0.24, 0.72), (0.5, 0.72), (0.76, 0.72)]
    def near_line(x, y, a, b, w):
        ax, ay = a[0] * S, a[1] * S
        bx, by = b[0] * S, b[1] * S
        dx, dy = bx - ax, by - ay
        t = max(0, min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
        return (x - ax - t * dx) ** 2 + (y - ay - t * dy) ** 2 <= w * w
    bus_y = 0.50
    for y in range(S):
        px.append(0)
        for x in range(S):
            r, g, b, a = 0, 0, 0, 0
            if inside_round_rect(x, y, S * 0.18):
                k = y / S
                r, g, b, a = int(0 + 20 * k), int(90 - 30 * k), int(170 - 40 * k), 255
                lw = S * 0.035
                white = False
                if near_line(x, y, (0.5, 0.24), (0.5, bus_y), lw):
                    white = True
                if near_line(x, y, (0.24, bus_y), (0.76, bus_y), lw):
                    white = True
                for nx, _ in nodes[1:]:
                    if near_line(x, y, (nx, bus_y), (nx, 0.72), lw):
                        white = True
                for nx, ny in nodes:
                    hw = S * (0.11 if ny < 0.5 else 0.085)
                    if abs(x - nx * S) <= hw and abs(y - ny * S) <= hw * 0.75:
                        white = True
                if white:
                    r, g, b = 255, 255, 255
            px += bytes((r, g, b, a))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(bytes(px), 9)) + chunk(b"IEND", b"")
    open(path, "wb").write(data)

if __name__ == "__main__":
    png(sys.argv[1], int(sys.argv[2]))
