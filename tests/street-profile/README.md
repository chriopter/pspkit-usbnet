# USB profile on a PSP Street

User-mode PSPLink helper: inspect saved network profiles, or add a normal
`Hi-Speed USB` profile with DHCP in the first unused slot. Existing profiles
are preserved. No network-stack initialization, ExitGame, reset, or firmware
installation. Profile creation saves normal network settings.

```sh
make -C tests/street-profile
pspsh -e 'ldstart host0:/tests/street-profile/street-profile.prx'
```

Read `host0:/street-profile.log` and verify PSPLink still answers. The helper
ends its own thread and stays loaded; stop/unload it before the next invocation:

```sh
pspsh -e 'modstun @street_profile'
pspsh -e 'ldstart host0:/tests/street-profile/street-profile.prx create'
```

The log must end with a successful save and a read-back of `Hi-Speed USB`.
The helper stops on unexpected errors or if all ten slots are occupied.

On a Street without autoboot, do not run the automated reset/power-cycle
harness. Use normal PSPLink without additionally loading the XMB for network
tests. A test returning from `main` reaches the CRT's `sceKernelExitGame`
and can trigger PSPLink's automatic reset; `tests/night` now parks its main
thread on both success and missing-profile paths. Do not reinitialize a
network stack left resident by a previous test.

Verified 2026-10-15 on a real PSP Street, ARK, PSPLink 3.2.1 and Linux gateway
0.1.10: profile created and read back; one `night` round with `dtu` passed
(10 MiB Adler-32 `a392ab8d`, five short TCP requests, twenty UDP echoes).
A separate bounded helper reused the initialized stack: DNS for `example.com`,
HTTP 200, clean disconnect. PSPLink answered and reported no exception after
each completed test. No reboot or power cycle in this run. Automatic loading
of the installed plugin on the next boot remains untested.
