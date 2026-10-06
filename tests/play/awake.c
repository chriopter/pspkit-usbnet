/* awake: keeps a PSP on the bench from falling asleep. Without a button
 * pressed it suspends after some minutes and is gone from USB until somebody
 * slides its switch; a test of hours needs it there.
 *
 *   ldstart host0:/awake.prx
 */
#include <pspkernel.h>
#include <psppower.h>

PSP_MODULE_INFO("awake", PSP_MODULE_KERNEL, 1, 0);

static int awake(SceSize args, void *argp)
{
    for (;;) {
        scePowerTick(0);
        sceKernelDelayThread(10 * 1000 * 1000);
    }
    return 0;
}

int module_start(SceSize args, void *argp)
{
    sceKernelStartThread(sceKernelCreateThread("awake", awake, 40, 0x1000, 0, NULL), 0, NULL);
    return 0;
}
