# Street Network Settings in the original XMB

`psp/xmb.c` enables only the Network Settings record (`category 0`, `item 13`)
when the model is the Street (10), before `vsh_module` begins execution.
The existing module-start handler is reused. A running XMB is not modified.

From the decrypted Sony 6.61 updater's `vshmain.prx`, flattened at base zero:

| Data | Offset | Value |
| --- | --- | --- |
| 35 menu records, four words each | `0x54b20` | 560 bytes |
| Network Settings model mask | `0x54bf8` | `0x3ff` → `0x7ff` |
| Icon texture record (`CD`, `CR`, `DF`, model/region masks) | `0x55084` | 20 bytes |
| Icon texture model mask | `0x55090` | `0x3ff` → `0x7ff` |
| SHA-1 of the entire table | `0x556d0` | 20 bytes |
| Network Settings label | `0x55c87` | `msgtop_sysconf_network` |

The XMB constructor at `0x1fc84` computes the table's SHA-1 and compares it
with the stored digest. On mismatch it transforms the table; changing just
the mask is unsafe. The menu constructor at `0x20654` then filters each
record by the real model's bit and the region mask.

The original-theme texture loader at `0x2d210` independently filters 20-byte
icon records by their model and region masks. Its settings group starts at
`0x54f80`; record 13 names `CD`, `CR`, `DF` and excludes the Street. Enabling
the menu without this second mask produces the observed missing icon.

The plugin requires all 140 menu words, all five icon words, the stored digest
and the label to match. Each range must be inside an actual module segment.
Only the two Street bits and the matching menu SHA-1 change, in RAM. Other
models and unknown or already modified layouts are left alone. On unload,
the original data are restored only if the same module is still resident
and all the plugin's expected data still match. No references to plugin code
are placed in the XMB.

PC verification against the locally extracted 6.61 module:

```sh
python3 tests/xmb-menu.py /path/to/flattened/vshmain.bin
make -C psp
```

The test independently recomputes SHA-1, compares every changed byte, restores
the original image, and checks repeated calls, 1202 corrupted-image cases,
truncated images, null input and invalid modes. The firmware is not included.

Checked: 2026-10-15 — PC tests passed on Sony 6.61 vshmain; PSP build passed
with pspdev v20261001. On the real Street, the user confirmed the original
Network Settings entry is visible after the controlled VSH-context change
and XMB start; its icon is missing. PSPLink remained responsive with no
reported exception. A later `reset vsh` from the already running XMB froze
and required a manual restart. Do not repeat that transition as a routine
activation method on the Street. The freeze's cause is not established.
The icon extension was subsequently built, PC-tested and installed. After a
normal firmware return from a cleaned GAME session, the user confirmed the
original XMB Network Settings entry and its icon. See
[`return-xmb/README.md`](return-xmb/README.md) for that separate exit path.

The Street's `topmenu_icon.rco` is byte-identical to the shared 6.61 updater
resource (SHA-256 `ef5a1759600ca36b8f1339a06bf0fffb5dbcb16e72d81b8d17b5d14e92e57de2`).
Decompressed resource labels include `CD`, `CR`, `DF`, the three icon names
in the Network Settings menu descriptor. The resources have not been removed
from the Street firmware.
