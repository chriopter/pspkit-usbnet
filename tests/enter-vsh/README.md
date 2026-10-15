# PSPLink VSH context for the Street recording

Bench-only helper, never part of the installed USBNet plugin. It launches the
existing PSPLink EBOOT through the firmware with the `vsh` key after ordered
USB teardown. Unlike PSPLink reset it does not suspend all user threads.

Requires a fresh GAME session, no XMB, no loaded network libraries or Sony
dialog, USBNet unloaded, USBHostFS started and storage inactive. The existing
EBOOT must have the PBP header. Failed USB teardown or LoadExec restores the
previous USB shell without retrying. Never unload its worker or use it from
an already running XMB.

Checked 2026-10-15: built with pspdev v20261001. PC mocks verified the guards,
EBOOT check, LoadExec arguments, USB order and four failure paths. One real
Street transition succeeded; PSPLink reconnected, VshCtrl/VshBridge loaded,
then the original vshmain started and the XMB rendered. This is one successful
transition, not a guarantee against future hangs.
