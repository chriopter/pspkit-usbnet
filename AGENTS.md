# Real PSP Street bench

The connected PSP Street has **no autoboot**. Recovering a freeze requires an
expensive manual restart. Do not run `tests/night.sh`, `tests/campaign.sh`,
`tests/play/play.sh`, relay actions, or automatic recovery loops on it.

Read `../psp-devbook/README.md` and the relevant notes before hardware work.
Stay away from firmware writes. Use read-only inspection and PC verification
first; execute one justified hardware step at a time and verify its result.

`reset vsh` from a running XMB froze this Street on 2026-10-15. Its cause is
unconfirmed. Do not treat PSPLink reset/context transitions as routine test
setup or recovery. A missing command response can also be a USB re-enumeration:
check the device and PSPLink before concluding that the console froze.

Do not return from a user test's `main`: this SDK's CRT calls ExitGame, which
can trigger PSPLink's automatic reset. Park the thread or explicitly end it.
Do not reinitialize an existing network stack. Keep modules resident while
their callbacks, hooks, dialogs or threads are still in use.

For captures use the reviewed userspace framebuffer reader in the devbook,
not PSPLink's `scrshot` or unreviewed RemoteJoy video patches. Native utility
dialogs should be tested with normal console input, without adding controller
hooks solely to avoid a button press.

Session exception, 2026-10-15: the user explicitly approved the bounded
controller helper for the requested XMB/PSPDX recording after the automatic
review rejection was explained. Keep it bench-only and resident while its
hooks are active. This does not authorize resets, power cycles or firmware
writes. The user also approved pushing prepare-1.0 to the public origin and
starting its native platform checks; release publication remains conditional
on the completed hardware verification and recording.
