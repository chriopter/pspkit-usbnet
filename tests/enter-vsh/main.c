/* Enter PSPLink in VSH context from an otherwise clean GAME session.
 * USB is stopped first, as in the verified regular return-xmb helper.
 * No suspension of all user threads, code hooks, flash writes or retries.
 * Never use from a running XMB or after starting network/GUI applications.
 */
#include <pspkernel.h>
#include <pspinit.h>
#include <psploadexec_kernel.h>
#include <pspusb.h>
#include <pspusbbus.h>
#include <string.h>
PSP_MODULE_INFO("enter_vsh", PSP_MODULE_KERNEL, 1, 0);
static char bootfile[] = "ms0:/PSP/GAME/PSPLINK/EBOOT.PBP";
static int worker(SceSize args, void *argp)
{
    int r, stopped_host = 0, stopped_bus = 0;
    struct SceKernelLoadExecVSHParam param;
    (void)args; (void)argp;
    /* Let PSPLink finish replying; no host0 access after this point. */
    sceKernelDelayThread(500000);
    r = sceUsbDeactivate(0x1c9);
    if (r < 0) return r;
    r = sceUsbStop("USBHostFSDriver", 0, NULL);
    if (r < 0) goto restore;
    stopped_host = 1;
    r = sceUsbStop(PSP_USBBUS_DRIVERNAME, 0, NULL);
    if (r < 0) goto restore;
    stopped_bus = 1;
    memset(&param, 0, sizeof param);
    param.size = sizeof param;
    param.args = sizeof bootfile;
    param.argp = bootfile;
    param.key = "vsh";
    r = sceKernelLoadExecVSHMs2(bootfile, &param);
    if (r >= 0) return 0;
restore:
    /* If the firmware rejects the request, restore the existing USB shell.
     * No forced reset, power operation or automatic retry. */
    if (stopped_bus) sceUsbStart(PSP_USBBUS_DRIVERNAME, 0, NULL);
    if (stopped_host) sceUsbStart("USBHostFSDriver", 0, NULL);
    sceUsbActivate(0x1c9);
    return r;
}
int module_start(SceSize args, void *argp)
{
    SceUID thread, fd;
    unsigned char magic[4];
    int n;
    (void)args; (void)argp;
    if (sceKernelInitKeyConfig() != PSP_INIT_KEYCONFIG_GAME ||
        sceKernelFindModuleByName("vsh_module") ||
        sceKernelFindModuleByName("usbnet") ||
        sceKernelFindModuleByName("sceNet_Library") ||
        sceKernelFindModuleByName("sceNetApctl_Library") ||
        !sceKernelFindModuleByName("PSPLINK") ||
        sceKernelFindModuleByName("sceDialogmain_Module") ||
        sceKernelFindModuleByName("sceVshNetconf_Module") ||
        sceUsbGetDrvState("USBStor_Driver") == 1 ||
        sceUsbGetDrvState("USBHostFSDriver") != 1)
        return 1;
    fd = sceIoOpen(bootfile, PSP_O_RDONLY, 0);
    if (fd < 0) return 1;
    n = sceIoRead(fd, magic, sizeof magic);
    sceIoClose(fd);
    if (n != sizeof magic || memcmp(magic, "\0PBP", sizeof magic)) return 1;
    thread = sceKernelCreateThread("enter_vsh", worker, 32, 0x2000, 0, NULL);
    if (thread < 0) return 1;
    if (sceKernelStartThread(thread, 0, NULL) < 0) {
        sceKernelDeleteThread(thread);
        return 1;
    }
    return 0;
}
int module_stop(SceSize args, void *argp)
{
    (void)args; (void)argp;
    /* Do not unload while the exit worker could still be executing. */
    return -1;
}
