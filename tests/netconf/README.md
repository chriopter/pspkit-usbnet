# Original game infrastructure chooser

`netconf-test.prx` renders Sony's unmodified `sceUtilityNetconf` dialog with
`PSP_NETCONF_ACTION_CONNECTAP`, the normal game-side profile chooser. Select
`Hi-Speed USB` and confirm with the console's normal button.

Run only in a fresh PSPLink GAME context, with no XMB or other network/GUI
application active. The program checks each initialization result, logs to
`host0:/netconf-test.log`, and never calls ExitGame, LoadExec or reset. There
are no controller, video or firmware hooks. Do not load another network
test or unload this module while the dialog or its stack is resident.

```sh
make -C tests/netconf
pspsh -e 'ldstart host0:/tests/netconf/netconf-test.prx'
```

After Sony's dialog closes and finishes shutting down, the program logs its
result and apctl state/IP, displays a result screen and parks its main thread.
The network stack stays resident for inspection. A successful connection
requires result 0 and apctl state 4; merely opening the chooser is not enough.

Checked 2026-10-15 on a real PSP Street: all network initializations and
NetconfInitStart succeeded; Sony's dialog reached VISIBLE; a userspace
framebuffer capture confirmed the normal German connection chooser with
`[Neue Verbindung]` and selected `Hi-Speed USB`. PSPLink remained responsive
with no reported exception. After the user confirmed `Hi-Speed USB`, Sony's
dialog returned result 0, finished shutdown (status 0), and apctl reached
state 4 with IP `10.77.0.2`. The normal game-side chooser and connection path
are verified. The connection and all four network services were then cleanly
terminated before leaving this test session.
