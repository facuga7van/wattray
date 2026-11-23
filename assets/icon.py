"""Generates wattray.ico from 16x16 pixel art, scaled nearest-neighbor. Run: python assets/icon.py"""
import struct
import zlib
from pathlib import Path

ART = [
    "................",
    "................",
    "................",
    "kkkkkkkkkkkkkk..",
    "kwwwwwwwwwwwwk..",
    "kwkkkkkkkkkkwkkk",
    "kwkggggggkkkwwwk",
    "kwkggggggkkkwwwk",
    "kwkggggggkkkwwwk",
    "kwkggggggkkkwwwk",
    "kwkkkkkkkkkkwkkk",
    "kwwwwwwwwwwwwk..",
    "kkkkkkkkkkkkkk..",
    "................",
    "................",
    "................",
]
COLORS = {".": (0, 0, 0, 0), "k": (0x1A, 0x1A, 0x1A, 255), "w": (0xF2, 0xF2, 0xF2, 255), "g": (0x6C, 0xCB, 0x5F, 255)}


def png(scale: int) -> bytes:
    rows = b"".join(
        b"\0" + b"".join(bytes(COLORS[c]) * scale for c in line) for line in ART for _ in range(scale)
    )
    size = 16 * scale

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


images = [png(s) for s in (1, 2, 3, 16)]  # 16, 32, 48, 256 px
offset = 6 + 16 * len(images)
ico = struct.pack("<HHH", 0, 1, len(images))
for scale, data in zip((1, 2, 3, 16), images):
    side = 16 * scale % 256  # 0 means 256
    ico += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
Path(__file__).with_name("wattray.ico").write_bytes(ico + b"".join(images))
