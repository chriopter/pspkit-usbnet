/* probe: the test of the device "usbnet:" (psp/usbnet_api.h), and what an
 * application's use of it looks like. A user PRX for PSPLink.
 *
 *   ldstart host0:/probe.prx [rounds] [ms] [flags] [log] [wait]
 *
 * It asks the version and the state, makes three calls that must be
 * refused, then looks for the gateway "rounds" times (20), "ms" each (6000),
 * a line of result each: what came back, how long it took, the state after.
 * Flags (_ for none):
 *   2  a second thread asks at the same time, every round
 * log: where the lines go (host0:/probe.log); wait: seconds before the first
 * round, for a test that takes PSPLink's USB away meanwhile (alone.prx),
 * which wants the log on ms0:.
 */
#include <pspkernel.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../../psp/usbnet_api.h"

PSP_MODULE_INFO("probe", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_HEAP_SIZE_KB(256);

static const char *log_to = "host0:/probe.log";

static void say(const char *fmt, ...)
{
    char line[200];
    va_list ap;
    int n;
    SceUID fd;

    va_start(ap, fmt);
    n = vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    fd = sceIoOpen(log_to, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0666);
    if (fd >= 0) {
        sceIoWrite(fd, line, n);
        sceIoClose(fd);
    }
}

static int now_ms(void)
{
    return (int)(sceKernelGetSystemTimeWide() / 1000);
}

static int ask(unsigned cmd, void *in, int len)
{
    return sceIoDevctl(USBNET_DEVICE, cmd, in, len, NULL, 0);
}

static unsigned ms = 6000;
static volatile int second_said, second_took;

static int second(SceSize size, void *argp)
{
    int since = now_ms();

    second_said = ask(USBNET_PROBE, &ms, sizeof ms);
    second_took = now_ms() - since;
    return 0;
}

int main(int argc, char *argv[])
{
    int rounds = argc > 1 ? atoi(argv[1]) : 20, there = 0, answered = 0, i, r;
    const char *flags = argc > 3 ? argv[3] : "";
    unsigned two = 2;

    if (argc > 2)
        ms = strtoul(argv[2], NULL, 10);
    if (argc > 4)
        log_to = argv[4];
    r = ask(USBNET_VERSION, NULL, 0);
    say("probe: version %d (%08x), state %x\n", r, r, ask(USBNET_STATE, NULL, 0));
    if (r < 0) {
        say("probe: no device, finished 0 of %d\n", rounds);
        return sceKernelSelfStopUnloadModule(1, 0, NULL);
    }
    /* A command it does not have, an argument of the wrong size, and one
     * that points into the kernel: all three refused. */
    say("probe: refused %d %d %08x\n", ask(99, NULL, 0), ask(USBNET_PROBE, &two, 2),
        ask(USBNET_PROBE, (void *)0x88000000, 4));
    if (argc > 5)
        sceKernelDelayThread(atoi(argv[5]) * 1000 * 1000);
    for (i = 0; i < rounds; i++) {
        SceUID th = strchr(flags, '2') ? sceKernelCreateThread("probe2", second, 32, 0x4000, PSP_THREAD_ATTR_USER, NULL) : -1;
        int since = now_ms(), took;

        if (th >= 0)
            sceKernelStartThread(th, 0, NULL);
        r = ask(USBNET_PROBE, &ms, sizeof ms);
        took = now_ms() - since;
        answered += r == 0 || r == 1;
        there += r == 1;
        if (th >= 0) {
            sceKernelWaitThreadEnd(th, NULL);
            sceKernelDeleteThread(th);
            say("probe: %d: %d in %d ms, the second %d in %d ms, state %x\n", i + 1, r, took, second_said,
                second_took, ask(USBNET_STATE, NULL, 0));
        } else {
            say("probe: %d: %d in %d ms, state %x\n", i + 1, r, took, ask(USBNET_STATE, NULL, 0));
        }
        sceKernelDelayThread(300 * 1000);
    }
    two = 0;
    say("probe: finished %d of %d, there %d, heard lately %d, state %x\n", answered, rounds, there,
        ask(USBNET_PROBE, &two, sizeof two), ask(USBNET_STATE, NULL, 0));
    return sceKernelSelfStopUnloadModule(1, 0, NULL); /* PSPLink stays */
}
