//! pspkit-usbnetd: the PC side of "Ethernet over the PSP's USB cable".
//!
//! The PSP exchanges raw Ethernet frames with the PC over two USB bulk
//! endpoints. This crate is the LAN behind that cable, entirely in userspace:
//! it answers ARP, is the DHCP server, forwards DNS and relays TCP and UDP
//! through ordinary sockets (a "slirp"-style gateway). No TAP/TUN, no root.
//!
//! * [`device`]  - the `FrameDevice` trait and an in-memory implementation
//! * [`usb`]     - the USB implementation (libusb through `rusb`)
//! * [`gateway`] - the gateway itself, independent of USB
//! * [`dhcp`], [`packet`] - wire formats

pub mod device;
pub mod dhcp;
pub mod gateway;
pub mod log;
pub mod packet;
pub mod status;
pub mod ui;
pub mod usb;
