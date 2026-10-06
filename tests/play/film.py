#!/usr/bin/env python3
# film.py <out.mp4> <film0.raw> [film1.raw ...]: pad.prx's film (time + 480x272 of 16 bits a frame) as a video, in real time.
import subprocess, sys
import numpy as np
size = 4 + 480 * 272 * 2; frames = []
for b in (raw[i * size:(i + 1) * size] for raw in (open(f, 'rb').read() for f in sys.argv[2:]) for i in range(len(raw) // size)):
    p = np.frombuffer(b, np.uint16, 480 * 272, 4).reshape(272, 480).astype(np.uint32)
    rgb = np.dstack(((p & 31) * 255 // 31, (p >> 5 & 63) * 255 // 63, (p >> 11) * 255 // 31)).astype(np.uint8)
    frames.append((int.from_bytes(b[:4], 'little'), rgb))
fps = 10; n = len(frames)
ff = subprocess.Popen(['ffmpeg', '-y', '-loglevel', 'error', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-s', '480x272', '-r', str(fps), '-i', '-',
                       '-vf', 'scale=960:544:flags=neighbor', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '18', sys.argv[1]], stdin=subprocess.PIPE)
for i, (t, rgb) in enumerate(frames):
    until = frames[i + 1][0] if i + 1 < n else t + 500
    for _ in range(max(1, round((until - t) * fps / 1000))):
        ff.stdin.write(rgb.tobytes())
ff.stdin.close(); ff.wait()
print(n, 'frames,', (frames[-1][0] - frames[0][0]) // 1000 if n else 0, 's')
