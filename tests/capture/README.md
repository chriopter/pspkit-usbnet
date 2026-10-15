# Street recording helpers

`capture.prx` extends the devbook's reviewed safe-shot userspace reader into a
bounded recording. It owns its RAM buffer, reads VRAM and writes timestamped
full frames to `host0:/tests/capture/frames.raw`. It does not hook the display,
issue GU commands, touch the Media Engine or invoke ExitGame.

Build with the PSP SDK: `make -C tests/capture`.
Run via PSPLink: `ldstart host0:/tests/capture/capture.prx 30` (1–180 seconds).
The raw stream contains four little-endian u32 words per frame: magic
`0x43415031`, PSP microsecond timestamp, stride (512), PSP pixel format; then
272 rows at that stride, 4 bytes/pixel for 8888, otherwise 2. Timing is real,
not a claim that ten frames are captured each second.

Checked 2026-10-15 on the Street's original XMB beside PSPLink: a three-second
capture produced 26 complete 8888 frames spanning 2.920 seconds. XMB rendering
and PSPLink remained available. Stop/unload only after the recording has
finished and its final frame is complete.

`input/` is a separate kernel bench helper based on the two user syscall
replacements in `tests/play/pad.c`. It does not capture video. `pad.py` writes
one bounded press; each press expires after at most one second. Its module
refuses unload while syscall entries point into it. **Not approved or tested
on this Street yet:** automatic approval rejected loading it under AGENTS.md;
a specific user exception is pending. Never install it as part of USBNet.
