/* launch: starts the ISO named in host0:/launch.txt, the way ARK's launcher
 * does. A kernel PRX for PSPLink (which ends with it): ldstart host0:/launch.prx
 */
#include <pspkernel.h>
#include <psploadexec_kernel.h>
#include <systemctrl.h>
#include <systemctrl_se.h>
#include <pspusb.h>
#include <pspusbbus.h>
#include <string.h>

PSP_MODULE_INFO("launch", PSP_MODULE_KERNEL, 1, 0);

/* In a thread of its own, and with USB shut down first: PSPLink's host
 * file system would hold the restart up. */
static int launch(SceSize args, void *argp)
{
    static char iso[256], eboot[] = "disc0:/PSP_GAME/SYSDIR/EBOOT.BIN";
    struct SceKernelLoadExecVSHParam param;
    SceUID fd = sceIoOpen("host0:/launch.txt", PSP_O_RDONLY, 0);
    int n = fd >= 0 ? sceIoRead(fd, iso, sizeof iso - 1) : 0;

    if (fd >= 0)
        sceIoClose(fd);
    while (n > 0 && (iso[n - 1] == '\n' || iso[n - 1] == '\r'))
        n--;
    if (n <= 0)
        return 1;
    iso[n] = 0;
    sceKernelDelayThread(500 * 1000);
    sceUsbDeactivate(0x1c9);
    sceUsbStop("USBHostFSDriver", 0, 0);
    sceUsbStop(PSP_USBBUS_DRIVERNAME, 0, 0);
    memset(&param, 0, sizeof param);
    param.size = sizeof param;
    param.key = "umdemu";
    param.argp = eboot;
    param.args = sizeof eboot;
    sctrlSESetDiscType(0x10);        /* a game */
    sctrlSESetBootConfFileIndex(3);  /* the inferno ISO driver */
    sctrlSESetUmdFile(iso);
    sctrlKernelLoadExecVSHWithApitype(0x123, iso, &param);
    return 0;
}

int module_start(SceSize args, void *argp)
{
    sceKernelStartThread(sceKernelCreateThread("launch", launch, 32, 0x4000, 0, NULL), 0, NULL);
    return 0;
}
