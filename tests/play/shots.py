#!/usr/bin/env python3
# shots.py <folder>: turns pad.prx's NNN.raw into NNN.png and one sheet.png of all of them, small.
import struct, sys
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw

folder = Path(sys.argv[1])
small = []
for raw in sorted(folder.glob("*.raw")):
    data = raw.read_bytes()
    magic, width, fmt, ms = struct.unpack("<4sIII", data[:16])
    if magic != b"SHOT" or not 480 <= width <= 1024:
        continue
    if fmt == 3:    # 8888
        a = np.frombuffer(data, np.uint8, width * 272 * 4, 16).reshape(272, width, 4)[:, :480, :3]
    else:           # 5650 (0), 5551 (1), 4444 (2): shown as 5650
        p = np.frombuffer(data, np.uint16, width * 272, 16).reshape(272, width)[:, :480].astype(np.uint32)
        a = np.dstack(((p & 31) * 255 // 31, (p >> 5 & 63) * 255 // 63, (p >> 11) * 255 // 31)).astype(np.uint8)
    img = Image.fromarray(np.ascontiguousarray(a), "RGB")
    img.save(raw.with_suffix(".png"))
    t = img.resize((240, 136))
    ImageDraw.Draw(t).text((3, 2), f"{raw.stem} {ms / 1000:.0f}s", fill=(255, 255, 0))
    small.append(t)
cols = 5
if small:
    sheet = Image.new("RGB", (cols * 240, -(-len(small) // cols) * 136))
    for i, t in enumerate(small):
        sheet.paste(t, (i % cols * 240, i // cols * 136))
    sheet.save(folder / "sheet.png")
print(len(small), "pictures")
