# Street framebuffer recorder

`capture.prx` uses owned userspace RAM to read the displayed framebuffer.
It writes to `ms0:/PSP/usbnet-capture.raw`, never hooks the display, and does
not issue GU commands, use the Media Engine or invoke ExitGame.

Build: `make -C tests/capture`. Start through PSPLink:
`ldstart host0:/tests/capture/capture.prx`. Default duration: 180 seconds,
at most five frames per second. Allow roughly 500 MB of free space.
Wait for completion before copying the file or unloading the module.

The raw stream contains four little-endian u32 words per frame: magic
`0x43415031`, PSP microsecond timestamp, stride (512), pixel format; then
272 rows at that stride, four bytes/pixel for 8888, otherwise two.
`encode.py input.raw output.mp4` validates complete records and preserves
measured intervals. It requires Pillow, numpy and ffmpeg.

The original host0 recorder was verified on the Street XMB, but USBHostFS
writes failed during PSPDX connection recordings. The local-file variant
avoids that transfer dependency; a full connection recording with it has
not yet been verified. Never use truncated footage as connection evidence.
Use normal console input. See the repository AGENTS.md for hardware rules.
