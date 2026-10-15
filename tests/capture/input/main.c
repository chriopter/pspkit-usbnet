/* Bench input only. No display/driver hooks, no resets, no flash writes.
 * Patches the same two user syscalls as tests/play/pad.c. Always resident
 * while the syscall table references this module. Each press expires. */
#include <pspkernel.h>
#include <pspctrl.h>
#include <pspsdk.h>
#include <systemctrl.h>
PSP_MODULE_INFO("capture_input", PSP_MODULE_KERNEL, 1, 0);
static int (*read_real)(SceCtrlData *, int), (*peek_real)(SceCtrlData *, int);
static volatile unsigned buttons, until;
static int inject(int result, SceCtrlData *pad, int count)
{
    unsigned b = buttons, end = until;
    int i;
    if (result > 0 && result <= count && result <= 64 &&
        (int)(end - sceKernelGetSystemTimeLow()) > 0)
        for (i = 0; i < result; ++i) pad[i].Buttons |= b;
    return result;
}
static int on_read(SceCtrlData *pad, int n) { return inject(read_real(pad, n), pad, n); }
static int on_peek(SceCtrlData *pad, int n) { return inject(peek_real(pad, n), pad, n); }
static int worker(SceSize args, void *argp)
{
    unsigned seen = 0;
    (void)args; (void)argp;
    for (;;) {
        unsigned cmd[4];
        int n = 0, k;
        SceUID fd = sceIoOpen("host0:/tests/capture/command.bin", PSP_O_RDONLY, 0);
        if (fd >= 0) { n = sceIoRead(fd, cmd, sizeof cmd); sceIoClose(fd); }
        if (n == sizeof cmd && cmd[0] == 0x50414431 && cmd[1] != seen && cmd[3] <= 1000) {
            seen = cmd[1];
            k = pspSdkDisableInterrupts();
            buttons = cmd[2] & 0xf3f9;
            until = sceKernelGetSystemTimeLow() + cmd[3] * 1000;
            pspSdkEnableInterrupts(k);
        }
        sceKernelDelayThread(50000);
    }
    return 0;
}
int module_start(SceSize args, void *argp)
{
    SceUID th;
    unsigned read = sctrlHENFindFunction("sceController_Service", "sceCtrl", 0x1F803938);
    unsigned peek = sctrlHENFindFunction("sceController_Service", "sceCtrl", 0x3A622550);
    (void)args; (void)argp;
    if (!read || !peek || read == peek) return 1;
    th = sceKernelCreateThread("capture_input", worker, 48, 0x2000, 0, NULL);
    if (th < 0) return 1;
    if (sceKernelStartThread(th, 0, NULL) < 0) { sceKernelDeleteThread(th); return 1; }
    read_real = (void *)read; peek_real = (void *)peek;
    sctrlHENPatchSyscall((void *)read, on_read);
    sctrlHENPatchSyscall((void *)peek, on_peek);
    return 0;
}
int module_stop(SceSize args, void *argp) { (void)args; (void)argp; return -1; }
