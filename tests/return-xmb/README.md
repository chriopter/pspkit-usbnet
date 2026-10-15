# Regular firmware return to XMB

This helper calls `sceKernelExitVSHVSH(NULL)` from a dedicated kernel thread
after stopping USB. It does not re-execute PSPLink, suspend user threads,
patch code, or write firmware. It is distinct from PSPLink's `reset vsh`,
which froze the Street when used from its running XMB.

Preconditions, checked before starting the worker:

- GAME context; no running XMB.
- No active Sony dialog modules.
- usbnet already unloaded after disconnecting and terminating the test stack.
- USBHostFS started; USB storage inactive.

ARK's `launcher` setting must be off for a real XMB return. On this Street,
only `always, launcher, on` was changed to `off`, with the original settings
backed up on the Memory Stick as
`PSP/SAVEDATA/ARK_01234/SETTINGS_BEFORE_USBNET_XMB.TXT`. This also means future
ordinary starts use the XMB instead of automatically launching PSPLink.

```sh
make -C tests/return-xmb
pspsh -e 'ldstart host0:/tests/return-xmb/return-xmb.prx'
```

The shell replies before the worker disconnects USB. A successful firmware
exit ends PSPLink; a lost USB connection alone does not prove that the XMB
has rendered. Confirm the console's screen. On a returned USB-stop or firmware
error, the worker attempts to restore USB and never retries or resets.
The module refuses stop/unload to protect its executing worker.

Checked: 2026-10-15 — built with pspdev v20261001; PC mock checks passed for
all preconditions, successful call order and four failure paths. Executed
once from the cleaned Street GAME session at the user's request. USB ended
as expected; the user confirmed the normal XMB and the Network Settings icon.
