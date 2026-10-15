# USBNet 1.0 verification

Release-ready: yes

## Automated checks

The release workflow builds the PSP plugin and standalone gateways, then
runs protocol and CLI tests on Linux x86_64 and ARM64, Windows x86_64, and
macOS Intel and Apple Silicon. Publication requires every job to pass.
Runners without a USB bus verify the explicit startup diagnostic instead
of a hardware connection. Windows checks installation and failure cleanup
on a disposable runner.

Local checks: 30 Rust tests; clippy with warnings denied; staged release
build and checksum checks. Windows 11 additionally passed the native
protocol tests and installer success/failure checks in an isolated VM.
The guarded Street XMB patch passed 1202 malformed-layout cases on the PC.

## Hardware and footage

- PSP Street, firmware 6.61 with ARK: native XMB Network Settings and icon,
  saved Hi-Speed USB profile, connection test and game-side Sony connection
  dialog. The trimmed XMB recording is in `assets/connecting-psp.mp4`.
- Linux 1.0 gateway alongside PSPLink: 20/20 probes with two concurrent
  callers per round; 20–21 ms responses. XMB and PSPDX sessions transferred
  traffic through the gateway while PSPLink remained reachable.
- Earlier Street checks covered DHCP, DNS, HTTP, a 10 MiB checksum-verified
  download, five TCP requests and twenty UDP echoes.
- Earlier PSP-1000/Go runs covered PSPDX catalogs, installs and online play;
  Windows 0.1.10 passed 20 checksum-verified downloads with a PSP-1000.

## Limits

The 1.0 recording covers the XMB; a complete new PSPDX video is deferred.
USBHostFS framebuffer recording stopped on USB write errors during PSPDX
connection attempts. A local-memory-stick recorder is prepared, but its
full connection capture is not verified. This is not a crash-free guarantee.
Windows 1.0 USB binding, macOS USB connections and multiple physical PSPs
at once have not been tested. The smaller Windows/Linux README clips show
0.1.10 and are illustrations, not evidence for current binaries.
