/* What usbnet.c (the cable) and net.c (Sony's network stack) share. */
#ifndef USBNET_H
#define USBNET_H

#include <psptypes.h>

/* usbnet.c */
void usbnet_link(int up);        /* a connection over the cable begins or ends: take or leave USB */
unsigned char *tx_reserve(void); /* where the next frame is built; NULL: flush first */
void tx_commit(int len);         /* the frame is part of the transfer */
void tx_flush(void);             /* the transfer goes out; returns once it is on the wire */

/* net.c */
extern int net_no_radio;                      /* option "nowlan" */
int net_present(void);                        /* another copy of this module is at work */
int net_start(void);                          /* < 0: not this firmware, nothing hooked */
void net_stop(void);
void net_receive(const u8 *frame, int len);   /* a frame from the cable */
void net_probe(int ask);                      /* the gateway is asked for a sign of life with the next transfer */

#endif
