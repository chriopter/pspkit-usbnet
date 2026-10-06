/* httpsnight: downloads over HTTPS from the internet, many in a row, one
 * line of result each. A user PRX for PSPLink; it writes host0:/night.log
 * like tests/night, so night.sh's counting fits.
 *
 *   ldstart host0:/httpsnight.prx <connection, _ for a space> <rounds> <url> <bytes> <adler32>
 *
 * A round: connect, GET the URL with pspkit-https (DNS, TLS 1.3, the
 * certificate checked), compare length and checksum, disconnect.
 */
#include <pspkernel.h>
#include <pspsdk.h>
#include <psputility.h>
#include <pspnet_apctl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "pspkit-https/entropy.h"
#include "pspkit-https/https.h"

PSP_MODULE_INFO("httpsnight", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_HEAP_SIZE_KB(-1024);
PSP_MAIN_THREAD_STACK_SIZE_KB(256);

static void say(const char *fmt, ...)
{
    char line[240];
    va_list ap;
    int n;
    SceUID fd;

    va_start(ap, fmt);
    n = vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    fd = sceIoOpen("host0:/night.log", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0666);
    if (fd >= 0) {
        sceIoWrite(fd, line, n);
        sceIoClose(fd);
    }
}

static int same(const char *arg, const char *text)
{
    for (; *arg && *text; arg++, text++)
        if (*arg != *text && !(*arg == '_' && *text == ' '))
            return 0;
    return *arg == *text;
}

static int wait_state(int wanted, int tenths)
{
    int state = -1, t;

    for (t = 0; t < tenths && state != wanted; t++) {
        sceNetApctlGetState(&state);
        sceKernelDelayThread(100 * 1000);
    }
    return state == wanted ? 0 : -1;
}

static struct { unsigned a, b, n; } sum;

static int on_data(void *ctx, const void *data, size_t len)
{
    const unsigned char *p = data;
    size_t i;

    for (i = 0; i < len; i++) {
        sum.a += p[i];
        sum.b += sum.a;
        if ((++sum.n & 0xfff) == 0) {
            sum.a %= 65521;
            sum.b %= 65521;
        }
    }
    return 0;
}

int main(int argc, char *argv[])
{
    int rounds = argc > 2 ? atoi(argv[2]) : 1, profile = 0, i, ok = 0;
    unsigned want_len = argc > 4 ? strtoul(argv[4], NULL, 10) : 0, want_sum = argc > 5 ? strtoul(argv[5], NULL, 16) : 0;

    entropy_init();
    entropy_allow_unswept(); /* a test: its keys protect nothing */
    https_net_init();
    for (i = 1; i <= 10; i++) {
        netData name, ssid;

        if (sceUtilityCheckNetParam(i) != 0)
            continue;
        sceUtilityGetNetParam(i, PSP_NETPARAM_NAME, &name);
        sceUtilityGetNetParam(i, PSP_NETPARAM_SSID, &ssid);
        if (argc > 1 && (same(argv[1], name.asString) || same(argv[1], ssid.asString)))
            profile = i;
    }
    for (i = 1; profile && argc > 3 && i <= rounds; i++) {
        struct https_result result;
        u64 t0 = sceKernelGetSystemTimeWide();
        enum https_outcome r = HTTPS_FAILED;
        unsigned got;
        int up = sceNetApctlConnect(profile) >= 0 && wait_state(4, 300) == 0 && https_net_connect() >= 0;

        memset(&result, 0, sizeof result);
        sum.a = 1, sum.b = sum.n = 0;
        if (up)
            r = https_get(argv[3], on_data, NULL, NULL, NULL, &result);
        got = (sum.b % 65521) << 16 | sum.a % 65521;
        if (up && r == HTTPS_COMPLETE && result.status == 200 && sum.n == want_len && got == want_sum) {
            ok++;
            say("night: round %d ok: https %u bytes, handshake %u ms, all %d ms\n", i, sum.n, result.handshake_ms,
                (int)((sceKernelGetSystemTimeWide() - t0) / 1000));
        } else {
            say("night: round %d FAIL: %s, outcome %d, status %ld, %u bytes, adler32 %08x, phase %s\n", i,
                up ? "https" : "no connection", r, result.status, sum.n, got, https_get_phase());
        }
        https_close_idle(); /* the library's own disconnect ends its network for good: leave by apctl */
        sceNetApctlDisconnect();
        wait_state(0, 150);
    }
    say("night: finished %d of %d\n", ok, rounds);
    sceKernelSleepThread();
    return 0;
}
