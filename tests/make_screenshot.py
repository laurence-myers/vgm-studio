"""Generate tests/screenshot.png: a 320x200, 8-bit palette PNG standing in for a
DOS game's title screen. Indexed colour and mode 13h dimensions, so the pack
inspector's facts read like a real capture."""
import struct, zlib, sys

W, H = 320, 200

# A small VGA-ish palette.
PALETTE = [
    (0x07, 0x0C, 0x18),  # 0 near-black
    (0x10, 0x1A, 0x33),  # 1 deep navy
    (0x1B, 0x2E, 0x52),  # 2 navy
    (0x2A, 0x3C, 0x6B),  # 3 mid blue
    (0xF2, 0xC7, 0x66),  # 4 gold
    (0x7A, 0x4A, 0x10),  # 5 gold shadow
    (0x8F, 0xB6, 0xE6),  # 6 pale blue
    (0x6F, 0xE2, 0x8A),  # 7 green
    (0xE0, 0x4A, 0x4A),  # 8 red
    (0x3E, 0x52, 0x73),  # 9 rule
]

px = [[0] * W for _ in range(H)]

# Sky: a banded vertical gradient, darkest at the bottom.
for y in range(H):
    t = y / (H - 1)
    band = 3 if t < 0.28 else 2 if t < 0.55 else 1 if t < 0.78 else 0
    for x in range(W):
        px[y][x] = band

# A 5x7 block font, enough for the two words on the card.
GLYPHS = {
    "C": ["01110", "10001", "10000", "10000", "10000", "10001", "01110"],
    "O": ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
    "L": ["10000", "10000", "10000", "10000", "10000", "10000", "11111"],
    "G": ["01110", "10001", "10000", "10111", "10001", "10001", "01111"],
    "A": ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
    "M": ["10001", "11011", "10101", "10101", "10001", "10001", "10001"],
    "E": ["11111", "10000", "10000", "11110", "10000", "10000", "11111"],
    "S": ["01111", "10000", "10000", "01110", "00001", "10001", "01110"],
    "I": ["111", "010", "010", "010", "010", "010", "111"],
    "R": ["11110", "10001", "10001", "11110", "10100", "10010", "10001"],
    "N": ["10001", "11001", "10101", "10011", "10001", "10001", "10001"],
    "V": ["10001", "10001", "10001", "10001", "10001", "01010", "00100"],
    "-": ["000", "000", "000", "111", "000", "000", "000"],
    " ": ["00", "00", "00", "00", "00", "00", "00"],
}


def draw_text(text, left, top, scale, ink, shadow=None):
    x = left
    for ch in text:
        rows = GLYPHS[ch]
        w = len(rows[0])
        for ry, row in enumerate(rows):
            for rx, bit in enumerate(row):
                if bit != "1":
                    continue
                for sy in range(scale):
                    for sx in range(scale):
                        py, pxx = top + ry * scale + sy, x + rx * scale + sx
                        if shadow is not None and 0 <= py + scale < H:
                            px[py + scale][pxx] = shadow
                        if 0 <= py < H and 0 <= pxx < W:
                            px[py][pxx] = ink
        x += (w + 1) * scale
    return x


# Title, centred-ish, with a drop shadow.
draw_text("COOL GAME", 62, 52, 4, 4, 5)
draw_text("SIERRA ON-LINE", 74, 108, 2, 6)

# A HUD bar across the bottom, with a rule above it.
for x in range(W):
    px[152][x] = 9
for y in range(153, H):
    for x in range(W):
        px[y][x] = 1
draw_text("SCORE", 10, 176, 2, 7)
draw_text("LIVES", 250, 176, 2, 8)

raw = b"".join(b"\x00" + bytes(row) for row in px)


def chunk(tag, data):
    body = tag + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))


png = b"\x89PNG\r\n\x1a\n"
png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 3, 0, 0, 0))
png += chunk(b"PLTE", b"".join(bytes(c) for c in PALETTE))
png += chunk(b"IDAT", zlib.compress(raw, 9))
png += chunk(b"IEND", b"")

out = sys.argv[1]
with open(out, "wb") as f:
    f.write(png)
print(f"wrote {out}: {len(png)} bytes, {W}x{H}, 8-bit palette")
