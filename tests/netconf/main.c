/* Exercise Sony's actual game-side infrastructure chooser under PSPLink.
 * Start only in a fresh GAME context with no other network/GUI application.
 * Operate Sony's dialog with the console buttons. Never calls ExitGame,
 * LoadExec or reset. Stays resident after restoring the original display.
 */
#include <pspkernel.h>
#include <pspdisplay.h>
#include <pspdebug.h>
#include <pspgu.h>
#include <psputility.h>
#include <pspnet.h>
#include <pspnet_inet.h>
#include <pspnet_resolver.h>
#include <pspnet_apctl.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
PSP_MODULE_INFO("usbnet_netconf_test", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_MAIN_THREAD_STACK_SIZE_KB(32);
PSP_HEAP_SIZE_KB(512);
static unsigned int list[4096] __attribute__((aligned(16)));
static pspUtilityNetconfData conf;
static void say(const char *fmt, ...)
{
    char line[240]; va_list ap; int n; SceUID fd;
    va_start(ap, fmt); n = vsnprintf(line, sizeof line, fmt, ap); va_end(ap);
    fd = sceIoOpen("host0:/netconf-test.log", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0666);
    if (fd >= 0) { if (n > 0 && n < (int)sizeof line) sceIoWrite(fd, line, n); sceIoClose(fd); }
}
static void park(void) __attribute__((noreturn));
static void park(void) { for (;;) sceKernelSleepThread(); }
static void background(void)
{
    sceGuStart(GU_DIRECT, list);
    sceGuClearColor(0xff201008);
    sceGuClear(GU_COLOR_BUFFER_BIT);
    sceGuFinish(); sceGuSync(0, 0);
}
int main(void)
{
    int r, state, last = -99, shutdown = 0;
    void *old_frame = NULL; int old_stride = 0, old_format = 0;
    say("start: fresh GAME context required\n");
    r = sceUtilityLoadNetModule(PSP_NET_MODULE_COMMON); say("load common %08x\n", r); if (r < 0) park();
    r = sceUtilityLoadNetModule(PSP_NET_MODULE_INET); say("load inet %08x\n", r); if (r < 0) park();
    r = sceNetInit(128 * 1024, 0x20, 0x1000, 0x20, 0x1000); say("net init %08x\n", r); if (r < 0) park();
    r = sceNetInetInit(); say("inet init %08x\n", r); if (r < 0) park();
    r = sceNetResolverInit(); say("resolver init %08x\n", r); if (r < 0) park();
    r = sceNetApctlInit(0x1600, 0x42); say("apctl init %08x\n", r); if (r < 0) park();
    sceDisplayGetFrameBuf(&old_frame, &old_stride, &old_format, PSP_DISPLAY_SETBUF_IMMEDIATE);
    sceGuInit(); sceGuStart(GU_DIRECT, list);
    sceGuDrawBuffer(GU_PSM_8888, (void *)0, 512);
    sceGuDispBuffer(480, 272, (void *)(512 * 272 * 4), 512);
    sceGuOffset(2048 - 240, 2048 - 136); sceGuViewport(2048, 2048, 480, 272);
    sceGuScissor(0, 0, 480, 272); sceGuEnable(GU_SCISSOR_TEST);
    sceGuFinish(); sceGuSync(0, 0); sceDisplayWaitVblankStart(); sceGuDisplay(GU_TRUE);
    memset(&conf, 0, sizeof conf); conf.base.size = sizeof conf;
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_LANGUAGE, &conf.base.language);
    sceUtilityGetSystemParamInt(PSP_SYSTEMPARAM_ID_INT_BUTTON_SWAP, &conf.base.buttonSwap);
    conf.base.graphicsThread = 17; conf.base.accessThread = 19;
    conf.base.fontThread = 18; conf.base.soundThread = 16;
    conf.action = PSP_NETCONF_ACTION_CONNECTAP;
    say("opening Sony infrastructure chooser\n");
    r = sceUtilityNetconfInitStart(&conf); say("netconf init %08x\n", r);
    if (r >= 0) {
        do {
            background();
            state = sceUtilityNetconfGetStatus();
            if (state != last) {say("dialog status %d\n", state); last = state;}
            if (state == PSP_UTILITY_DIALOG_VISIBLE) sceUtilityNetconfUpdate(1);
            else if (state == PSP_UTILITY_DIALOG_QUIT && !shutdown) {
                r = sceUtilityNetconfShutdownStart(); say("shutdown %08x\n", r);
                if (r >= 0) shutdown = 1;
            }
            if (state < 0) {say("unexpected dialog status; keeping resources resident\n"); park();}
            sceDisplayWaitVblankStart(); sceGuSwapBuffers();
        } while (state != PSP_UTILITY_DIALOG_NONE);
        say("dialog result %08x\n", conf.base.result);
        r = sceNetApctlGetState(&state); say("apctl result %08x state %d\n", r, state);
        if (r >= 0 && state == 4) {
            union SceNetApctlInfo info;
            if (!sceNetApctlGetInfo(PSP_NET_APCTL_INFO_IP, &info)) say("connected IP %s\n", info.ip);
            say("Sony game infrastructure chooser connected successfully\n");
        }
    }
    sceGuDisplay(GU_FALSE); sceGuTerm();
    if (old_frame) sceDisplaySetFrameBuf(old_frame, old_stride, old_format, PSP_DISPLAY_SETBUF_NEXTFRAME);
    pspDebugScreenInit();
    pspDebugScreenPrintf("USB network test\nSony dialog result: 0x%08x\n", conf.base.result);
    if (sceNetApctlGetState(&state) >= 0 && state == 4)
        pspDebugScreenPrintf("Connected. PSPLink remains available.\n");
    else
        pspDebugScreenPrintf("Not connected. PSPLink remains available.\n");
    /* Leave the stack resident and available for inspection, like night.prx.
     * No unloading or reinitialization while these modules are resident. */
    say("finished; display restored; no ExitGame\n"); park();
}
