/* What an application may ask usbnet.prx: the device "usbnet:".
 *
 *   int r = sceIoDevctl("usbnet:", USBNET_PROBE, &ms, sizeof ms, NULL, 0);
 *
 * Nothing is imported from the plugin. Without it, or with one older than
 * this, the call fails with 0x80020321 (no such device). This file is all
 * there is to it: copy it. */
#ifndef USBNET_API_H
#define USBNET_API_H

#define USBNET_DEVICE "usbnet:"

/* Commands. The answer is the call's result. */
#define USBNET_VERSION 1 /* the number of this interface: USBNET_API */
#define USBNET_STATE   2 /* at once: USBNET_CABLE | ... as things are */
#define USBNET_PROBE   3 /* in: u32, how long to look in ms (up to 10000).
                            Returns once the gateway has answered or the time
                            is up: 1 there, 0 not there, or an error below.
                            Takes USB for as long as it looks, unless a
                            connection has it. 0 ms does not look: 1 if the
                            gateway was heard in the last five seconds. */

#define USBNET_API 1

/* USBNET_STATE */
#define USBNET_CABLE   1 /* the cable is in a host (known while USB is active) */
#define USBNET_BUS     2 /* the plugin is on the bus and the host has taken it */
#define USBNET_LINK    4 /* a connection over the cable is up */
#define USBNET_GATEWAY 8 /* the gateway was heard in the last five seconds */

/* Errors */
#define USBNET_NO_CABLE (-1) /* the cable is not in a host */
#define USBNET_BUSY     (-2) /* the USB connection (storage) has the port */
#define USBNET_STOPPED  (-3) /* the plugin is being unloaded */
#define USBNET_INVALID  (-4) /* a command or an argument it does not know */

#endif
