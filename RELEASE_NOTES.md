# USBNet 1.0

Connect your PSP to the internet over USB using its normal network menus.
Choose **Hi-Speed USB** in the XMB, PSPDX or a game's connection dialog, and
keep the gateway running on your computer.

- PSP Street: restores the original Network Settings entry and its icon in
  RAM. No firmware flashing or replacement Sony assets.
- Standalone gateways for Linux x86_64 and ARM64, Windows x86_64, and macOS
  on Apple Silicon and Intel.
- Clearer USB and connection status, actionable startup errors, and a
  corrected send queue for disconnects and reconnects.
- Windows setup uses unique temporary installer paths and preserves existing
  driver packages and certificates if a new installation fails.
- USBNet and PSPLink use separate interfaces on the same USB cable.

See the README for downloads, setup, real hardware footage and the scope of
the recorded verification. Linux may need USB permission through sudo;
Windows may need the gateway's WinUSB setup on first use.

Verified on a PSP Street with the Linux gateway, including the original
XMB connection menu and PSPLink access. Native CI checks the standalone
binaries on all five OS/architecture targets. Physical USB testing on
macOS and Windows 1.0 remains outstanding; see the verification record
for the full scope.
