# USBNet 1.0

Bring your PSP online through its USB cable — including the PSP Street.
Keep the gateway open on your computer, choose **Hi-Speed USB** in the PSP's
normal connection menu, and you're ready to browse, download or play online.

This release brings back the Street's original Network Settings menu and
icon, makes the desktop gateway easier to use, and improves reconnect handling.

## What's new

- **The PSP Street gets its network menu back.** Set up and test the connection
  directly in the XMB, then use the saved profile in PSPDX and supported games.
  The change happens in RAM; no firmware flashing is needed.
- **One download for your computer.** Standalone gateways for Windows,
  Linux x86_64 and ARM64, and macOS on both Intel and Apple Silicon.
- **Clearer connection status.** See when USB is ready, when the PSP has an IP
  address, and when traffic starts flowing. Startup errors explain what to fix.
- **Improved reconnect handling.** The send queue shuts down cleanly when a
  device disconnects, and Windows driver setup preserves existing drivers
  and certificates if a new installation fails.
- **PSPLink can share the cable.** Develop and use USB networking through
  separate interfaces on the same USB connection.

## Get started

1. Put `usbnet.prx` in `ms0:/seplugins/` and enable it in your custom firmware.
2. Download the gateway for your computer below and run it.
3. On the PSP, create a connection by scanning for **Hi-Speed USB**, with
   security set to **None**. Save it and connect as you would with Wi-Fi.

The [README](https://github.com/chriopter/pspkit-usbnet#readme) has the full
setup guide and a real PSP Street XMB demonstration. `usbnet-psp.zip` contains
the plugin package for PSPDX; the individual `usbnet.prx` is available too.

Tested on a real PSP Street with the Linux gateway. All five desktop targets
pass native CI checks, including Windows driver installation and failure
cleanup. Windows 1.0 USB binding and macOS USB connections still need hardware
coverage; the [verification record](https://github.com/chriopter/pspkit-usbnet/blob/main/tests/release-1.0.md)
keeps the exact scope of testing in one place.
