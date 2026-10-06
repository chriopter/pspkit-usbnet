/* alone: takes PSPLink's USB away for a while, so that usbnet.prx finds the
 * port as it is without PSPLink and starts the bus itself, then gives it
 * back. A kernel PRX for PSPLink:
 *
 *   ldstart host0:/alone.prx <seconds>
 *
 * pspsh does not answer meanwhile; what runs in that time writes to ms0:.
 */
#include <pspkernel.h>
#include <pspusb.h>
#include <string.h>

PSP_MODULE_INFO("alone", PSP_MODULE_KERNEL, 1, 0);

#define HOSTFS "USBHostFSDriver"
#define USB_PID 0x1c9

static int seconds = 30;

static int thread(SceSize size, void *argp)
{
    sceKernelDelayThread(1000 * 1000);
    sceUsbDeactivate(USB_PID);
    sceUsbStop(HOSTFS, 0, 0);
    sceUsbStop(PSP_USBBUS_DRIVERNAME, 0, 0);
    sceKernelDelayThread(seconds * 1000 * 1000);
    sceUsbStart(PSP_USBBUS_DRIVERNAME, 0, 0);
    sceUsbStart(HOSTFS, 0, 0);
    sceUsbActivate(USB_PID);
    return 0;
}

int module_start(SceSize args, void *argp)
{
    const char *p = argp, *end = p + args;
    SceUID th;

    p += strlen(p) + 1;
    if (p < end)
        for (seconds = 0; *p >= '0' && *p <= '9'; p++)
            seconds = seconds * 10 + *p - '0';
    th = sceKernelCreateThread("alone", thread, 20, 0x2000, 0, NULL);
    if (th >= 0)
        sceKernelStartThread(th, 0, NULL);
    return 0;
}
