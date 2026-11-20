"""Screenshot of the game window as PNG, without Pillow: owdbg's PrintWindow capture (works with the window in
the background, not minimized) and a small PNG writer."""
import struct
import zlib

from tools.owdbg.game import capture, find_window


def write_png(path, width, height, bgra):
    rows = bytearray()
    for y in range(height):
        row = bgra[y * width * 4:(y + 1) * width * 4]
        rgb = bytearray(width * 3)
        rgb[0::3], rgb[1::3], rgb[2::3] = row[2::4], row[1::4], row[0::4]
        rows += b"\x00" + rgb

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(bytes(rows), 6)))
        f.write(chunk(b"IEND", b""))


def screenshot(pid, path):
    """PNG of the game window of pid; returns (width, height)."""
    hwnd = find_window(pid)
    if not hwnd:
        raise RuntimeError(f"no visible window of PID {pid}")
    width, height, rgb = capture(hwnd)
    write_png(path, width, height, rgb.data)
    return width, height
