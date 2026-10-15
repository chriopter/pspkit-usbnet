/* The Street's original Network Settings item, enabled only in RAM. */
#ifndef USBNET_XMB_H
#define USBNET_XMB_H

#include <pspkernel.h>

void xmb_module_start(SceModule *module);
void xmb_stop(void);

#endif
