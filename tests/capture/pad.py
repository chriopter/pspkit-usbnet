#!/usr/bin/env python3
"""Send one expiring press to the bench helper, never a reset."""
import argparse
import os
from pathlib import Path
import struct
import time

BUTTONS = dict(up=0x10, right=0x20, down=0x40, left=0x80, cross=0x4000,
               circle=0x2000, square=0x8000, triangle=0x1000, start=8, select=1,
               l=0x100, r=0x200)
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('buttons', choices=BUTTONS, nargs='+')
p.add_argument('--ms', type=int, default=180)
a = p.parse_args()
if not 1 <= a.ms <= 1000:
    p.error('--ms must be 1..1000')
path = Path(__file__).with_name('command.bin')
temp = path.with_suffix('.tmp')
mask = 0
for key in a.buttons:
    mask |= BUTTONS[key]
with temp.open('wb') as f:
    f.write(struct.pack('<4I', 0x50414431, time.time_ns() & 0xffffffff, mask, a.ms))
    f.flush()
    os.fsync(f.fileno())
os.replace(temp, path)
time.sleep(a.ms / 1000 + .15)
print(' + '.join(a.buttons))
