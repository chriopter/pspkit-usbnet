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

`encode.py frames.raw output.mp4` validates complete records and preserves
the measured intervals when encoding. Requires Pillow, numpy and ffmpeg.
It refuses partial or discontinuous captures; it never invents connection
events or compresses waiting time.

`input/` is a separate kernel bench helper based on the two user syscall
replacements in `tests/play/pad.c`. It does not capture video. `pad.py` writes
one bounded press; each press expires after at most one second. Its module
refuses unload while syscall entries point into it. The user explicitly
approved the recording exception on 2026-10-15 after the initial automatic
review rejection. Hardware input verification is still pending. Never
install it as part of USBNet.

The non-hooking firmware emulation alternative was also checked on this
Street: `sceCtrl_driver` NID `0xDF53E160`, validated slot/argument stores,
12 samples, user and kernel buttons, with an explicit clear in `finally`.
Calls returned success but did not move the XMB. No controller hook was
loaded. Do not treat this ineffective API as a tested recording input path.
