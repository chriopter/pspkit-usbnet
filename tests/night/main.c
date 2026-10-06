/* night: many connections in a row, one line of result each.
 * A user PRX for PSPLink; it writes host0:/night.log.
 *
 *   ldstart host0:/night.prx <connection, by name or SSID, _ for a space> <rounds> <flags> [adler32 of /10m.bin]
 *
 * A round: connect, do what the flags say, disconnect. Flags:
 *   d  download /10m.bin from 10.77.0.1:8975 and compare its checksum
 *   t  five short requests (/1k.bin), a TCP connection each
 *   u  twenty datagrams of growing size to the echo server 10.77.0.1:8977
 *   s  scan before connecting; "Hi-Speed USB" must be in the list
 *   a  every other round with connection 1 instead (Wi-Fi): connect only
 *   W  every round with connection 1 (Wi-Fi): connect only
 *   w  the download is written to ms0:   b  64 KiB receive buffer
 *   P  512 KiB packet pool   c  333 MHz   k  load usbnet.prx from this folder
 */
#include <pspkernel.h>
#include <pspsdk.h>
#include <psputility.h>
#include <pspnet.h>
#include <pspnet_inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <arpa/inet.h>
#include <pspnet_apctl.h>
#include <pspnet_resolver.h>
#include <kubridge.h>
#include <psppower.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

PSP_MODULE_INFO("night", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_HEAP_SIZE_KB(1024);

int sceNetApctlScanUser(void);
int sceNetApctlGetBSSDescIDListUser(int *size, void *list);
int sceNetApctlGetBSSDescEntryUser(int id, int code, void *data);

#define GATEWAY "10.77.0.1"
#define USB_SSID "Hi-Speed USB"

static const char *flags = "";
static unsigned want_sum;
static char why[96];

static void say(const char *fmt, ...)
{
    char line[200];
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

static int fail(const char *fmt, ...)
{
    va_list ap;

    va_start(ap, fmt);
    vsnprintf(why, sizeof why, fmt, ap);
    va_end(ap);
    return -1;
}

static int now_ms(void)
{
    return (int)(sceKernelGetSystemTimeWide() / 1000);
}

static int same(const char *arg, const char *text)
{
    for (; *arg && *text; arg++, text++)
        if (*arg != *text && !(*arg == '_' && *text == ' '))
            return 0;
    return *arg == *text;
}

static volatile int last_error;

static void on_event(int old, int new, int event, int error, void *arg)
{
    if (error)
        last_error = error;
}

static int wait_state(int wanted, int tenths)
{
    int state = -1, t;

    for (t = 0; t < tenths; t++) {
        sceNetApctlGetState(&state);
        if (state == wanted)
            return t * 100;
        if (wanted == 4 && state == 0 && last_error) /* given up */
            break;
        sceKernelDelayThread(100 * 1000);
    }
    return fail("state %d, not %d, after %d s, last error %08x", state, wanted, tenths / 10, last_error);
}

static int tcp_socket(int port)
{
    struct sockaddr_in addr = { 0 };
    int fd = sceNetInetSocket(AF_INET, SOCK_STREAM, 0), timeout = 20 * 1000 * 1000, size = 65535;

    if (fd < 0)
        return fail("socket %08x", fd);
    sceNetInetSetsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
    sceNetInetSetsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof timeout);
    if (strchr(flags, 'b'))
        sceNetInetSetsockopt(fd, SOL_SOCKET, SO_RCVBUF, &size, sizeof size);
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    sceNetInetInetAton(GATEWAY, &addr.sin_addr);
    if (sceNetInetConnect(fd, (struct sockaddr *)&addr, sizeof addr) < 0) {
        int e = sceNetInetGetErrno();

        sceNetInetClose(fd);
        return fail("tcp connect errno %d", e);
    }
    return fd;
}

/* GET path; returns the body's length, its Adler-32 in *sum. */
static int http_get(const char *path, unsigned *sum, int to_stick)
{
    static unsigned char buf[32 * 1024];
    char req[128];
    unsigned a = 1, b = 0;
    int fd = tcp_socket(8975), n, i, header = 1, blank = 0, body = 0;
    SceUID out = to_stick ? sceIoOpen("ms0:/usbnet-test.bin", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0666) : -1;

    if (fd < 0)
        return -1;
    n = snprintf(req, sizeof req, "GET %s HTTP/1.0\r\nHost: " GATEWAY "\r\n\r\n", path);
    sceNetInetSend(fd, req, n, 0);
    while ((n = sceNetInetRecv(fd, buf, sizeof buf, 0)) > 0) {
        for (i = 0; i < n && header; i++) {
            blank = buf[i] == '\n' ? blank + 1 : buf[i] == '\r' ? blank : 0;
            header = blank < 2;
        }
        if (out >= 0)
            sceIoWrite(out, buf + i, n - i);
        for (; i < n; i++) {
            a += buf[i];
            b += a;
            if ((++body & 0xfff) == 0) {
                a %= 65521;
                b %= 65521;
            }
        }
    }
    i = n < 0 ? sceNetInetGetErrno() : 0;
    sceNetInetClose(fd);
    if (out >= 0)
        sceIoClose(out);
    *sum = (b % 65521) << 16 | a % 65521;
    return n < 0 ? fail("recv errno %d after %d bytes", i, body) : body;
}

static int udp_echo(void)
{
    static unsigned char out[1500], in[1500];
    struct sockaddr_in to = { 0 }, from;
    int fd = sceNetInetSocket(AF_INET, SOCK_DGRAM, 0), timeout = 5 * 1000 * 1000, i, k, n, r = 0;
    socklen_t len;

    if (fd < 0)
        return fail("udp socket %08x", fd);
    sceNetInetSetsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
    to.sin_family = AF_INET;
    to.sin_port = htons(8977);
    sceNetInetInetAton(GATEWAY, &to.sin_addr);
    for (i = 0; i < 20 && r == 0; i++) {
        int size = 1 + i * 73; /* 1 .. 1388 bytes */

        for (k = 0; k < size; k++)
            out[k] = (unsigned char)(k * 7 + i);
        sceNetInetSendto(fd, out, size, 0, (struct sockaddr *)&to, sizeof to);
        len = sizeof from;
        n = sceNetInetRecvfrom(fd, in, sizeof in, 0, (struct sockaddr *)&from, &len);
        if (n != size || memcmp(in, out, size))
            r = fail("udp datagram %d: %d bytes back, not %d (errno %d)", i, n, size, n < 0 ? sceNetInetGetErrno() : 0);
        else if (from.sin_addr.s_addr != to.sin_addr.s_addr || from.sin_port != to.sin_port)
            r = fail("udp answer from the wrong address");
    }
    sceNetInetClose(fd);
    return r;
}

static int scan(void)
{
    struct { void *next; int id; } list[20];
    int size = sizeof list, r, n;

    if ((r = sceNetApctlScanUser()) < 0)
        return fail("scan %08x", r);
    sceKernelDelayThread(7 * 1000 * 1000);
    memset(list, 0, sizeof list);
    if ((r = sceNetApctlGetBSSDescIDListUser(&size, list)) < 0)
        return fail("scan list %08x", r);
    for (n = 0; n < size / 8 && n < 20; n++) {
        char ssid[40] = "";

        sceNetApctlGetBSSDescEntryUser(list[n].id, 1, ssid);
        if (!strcmp(ssid, USB_SSID))
            return size / 8;
    }
    return fail("the scan's %d entries have no " USB_SSID, size / 8);
}

static int one_round(int profile, int wifi)
{
    unsigned sum;
    int r, i, t0 = now_ms(), connect_ms, found = 0;

    last_error = 0;
    if (strchr(flags, 's') && (found = scan()) < 0)
        return -1;
    if ((r = sceNetApctlConnect(wifi ? 1 : profile)) < 0)
        return fail("connect %08x", r);
    if ((connect_ms = wait_state(4, 300)) < 0)
        return -1;
    connect_ms = now_ms() - t0;
    if (!wifi) {
        if (strchr(flags, 'd')) {
            if ((r = http_get("/10m.bin", &sum, strchr(flags, 'w') != NULL)) < 0)
                return -1;
            if (r != 10485760 || sum != want_sum)
                return fail("download %d bytes, adler32 %08x", r, sum);
        }
        for (i = 0; strchr(flags, 't') && i < 5; i++)
            if ((r = http_get("/1k.bin", &sum, 0)) != 1024)
                return r < 0 ? -1 : fail("short request %d: %d bytes", i, r);
        if (strchr(flags, 'u') && udp_echo() < 0)
            return -1;
    }
    r = now_ms() - t0;
    sceNetApctlDisconnect();
    if (wait_state(0, 150) < 0)
        return -1;
    return snprintf(why, sizeof why, "%s connect %d ms, all %d ms%s", wifi ? "wifi" : "usb", connect_ms, r,
                    found ? ", scan found it" : "");
}

int main(int argc, char *argv[])
{
    int rounds = argc > 2 ? atoi(argv[2]) : 1, profile = 0, i, ok = 0;

    flags = argc > 3 ? argv[3] : "";
    want_sum = argc > 4 ? strtoul(argv[4], NULL, 16) : 0;
    if (strchr(flags, 'k')) {
        char path[256];
        const char *slash = strrchr(argv[0], '/');
        SceUID mod;
        int status = 0;

        snprintf(path, sizeof path, "%.*susbnet.prx", slash ? (int)(slash - argv[0]) + 1 : 0, argv[0]);
        mod = kuKernelLoadModule(path, 0, NULL);
        if (mod >= 0)
            sceKernelStartModule(mod, strlen(path) + 1, path, &status, NULL);
        say("night: usbnet.prx loaded here: %08x\n", mod);
    }
    if (strchr(flags, 'c'))
        scePowerSetClockFrequency(333, 333, 166);
    sceUtilityLoadNetModule(PSP_NET_MODULE_COMMON);
    sceUtilityLoadNetModule(PSP_NET_MODULE_INET);
    if (strchr(flags, 'P')) {
        sceNetInit(0x80000, 0x20, 0x1000, 0x20, 0x1000);
        sceNetInetInit();
        sceNetResolverInit();
        sceNetApctlInit(0x1600, 0x42);
    } else {
        pspSdkInetInit();
    }
    for (i = 1; i <= 10; i++) {
        netData name, ssid;

        if (sceUtilityCheckNetParam(i) != 0)
            continue;
        sceUtilityGetNetParam(i, PSP_NETPARAM_NAME, &name);
        sceUtilityGetNetParam(i, PSP_NETPARAM_SSID, &ssid);
        if (argc > 1 && (same(argv[1], name.asString) || same(argv[1], ssid.asString)))
            profile = i;
    }
    if (!profile) {
        say("night: no connection \"%s\"\nnight: finished 0 of %d\n", argc > 1 ? argv[1] : "", rounds);
        return 0;
    }
    sceNetApctlAddHandler(on_event, NULL);
    for (i = 1; i <= rounds; i++) {
        int r = one_round(profile, strchr(flags, 'W') || (strchr(flags, 'a') && i % 2 == 0));

        ok += r >= 0;
        say("night: round %d %s: %s\n", i, r >= 0 ? "ok" : "FAIL", why);
        if (r < 0) { /* leave whatever state it is in, and give it a moment */
            sceNetApctlDisconnect();
            wait_state(0, 150);
            sceKernelDelayThread(2 * 1000 * 1000);
        }
    }
    say("night: finished %d of %d\n", ok, rounds);
    sceKernelSleepThread();
    return 0;
}
