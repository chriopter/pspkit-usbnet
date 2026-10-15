#!/usr/bin/env python3
"""Encode complete timestamped userspace captures without changing their speed.

Requires Pillow, numpy and ffmpeg. Usage: encode.py frames.raw output.mp4
Optional --last-png writes a still for inspecting the take.
"""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile
import numpy as np
from PIL import Image

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('source', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--last-png', type=Path)
a = p.parse_args()
with tempfile.TemporaryDirectory(prefix='usbnet-frames-') as work:
    work = Path(work)
    stamps = []
    with a.source.open('rb') as stream:
        while header := stream.read(16):
            if len(header) != 16:
                raise SystemExit('incomplete frame header; do not unload an active recorder')
            magic, stamp, stride, fmt = struct.unpack('<4I', header)
            if magic != 0x43415031 or stride != 512 or fmt > 3:
                raise SystemExit('unsupported frame')
            size = stride * 272 * (4 if fmt == 3 else 2)
            raw = stream.read(size)
            if len(raw) != size:
                raise SystemExit('incomplete frame; do not unload an active recorder')
            if fmt == 3:
                image = Image.frombytes('RGBA', (stride, 272), raw).convert('RGB')
            else:
                pixels = np.frombuffer(raw, dtype='<u2').reshape(272, stride)
                bits = (5, 6, 5) if fmt == 0 else (5, 5, 5) if fmt == 1 else (4, 4, 4)
                channels = []
                shift = 0
                for width in bits:
                    mask = (1 << width) - 1
                    channels.append((((pixels >> shift) & mask).astype(np.uint32) * 255 // mask).astype(np.uint8))
                    shift += width
                image = Image.fromarray(np.stack(channels, axis=-1))
            image = image.crop((0, 0, 480, 272))
            image.save(work / f'{len(stamps):05}.png')
            stamps.append(stamp)
    if len(stamps) < 2:
        raise SystemExit('need at least two complete frames')
    durations = [((b - c) & 0xffffffff) / 1e6 for c, b in zip(stamps, stamps[1:])]
    if any(d <= 0 or d > 5 for d in durations):
        raise SystemExit('invalid or discontinuous capture timing')
    if a.last_png:
        image.save(a.last_png)
    lines = ['ffconcat version 1.0']
    for n, duration in enumerate(durations + [durations[-1]]):
        lines.extend([f"file '{n:05}.png'", 'option framerate 1000000', f'duration {duration:.6f}'])
    lines.append(f"file '{len(stamps)-1:05}.png'")
    manifest = work / 'frames.ffconcat'
    manifest.write_text('\n'.join(lines) + '\n')
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-safe', '0', '-i', str(manifest),
                    '-an', '-c:v', 'libx264', '-crf', '17', '-pix_fmt', 'yuv420p',
                    '-r', '30', '-movflags', '+faststart', str(a.output.resolve())], check=True)
    print(f'{len(stamps)} frames, {sum(durations):.3f} seconds of real capture timing')
