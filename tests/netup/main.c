/* netup: the test program. Brings Sony's network stack up like an
 * application, connects with a profile and downloads with a checksum.
 * A user PRX for PSPLink; it writes host0:/netup.log.
 *
 *   ldstart host0:/netup.prx <seconds to wait> <profile name or SSID> <ip> <port> <path> [flags]
 *
 * Flags: k load ms0:/seplugins/usbnet.prx itself, r three more rounds of
 * disconnect, connect, download, R 30 rounds of disconnect and connect,
 * w write the download to ms0:, b 64 KiB receive buffer, P 512 KiB packet
 * pool, c 333 MHz, n no checksum, m Memory Stick speed alone.
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
#include <pspwlan.h>
#include <kubridge.h>
#include <psppower.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

PSP_MODULE_INFO("netup", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_HEAP_SIZE_KB(1024);

/* Also into host0:/netup.log: PSPLink sends stdout to one shell only. */
static void say(const char *fmt, ...)
{
    char line[160];
    va_list ap;
    int n;
    SceUID fd;

    va_start(ap, fmt);
    n = vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    printf("%s", line);
    fd = sceIoOpen("host0:/netup.log", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0666);
    if (fd >= 0) {
        sceIoWrite(fd, line, n);
        sceIoClose(fd);
    }
}

/* Runs on apctl's own thread, which has 5.6 KB of stack: only note it. */
static volatile int ev[64][4], nev, shown;

/* Runs on apctl's own thread, which has 5.6 KB of stack: only note it. */
static volatile int ev[64][4], nev, shown;

static void on_event(int old, int new, int event, int error, void *arg)
{
    if (nev < 64) {
        ev[nev][0] = event, ev[nev][1] = old, ev[nev][2] = new, ev[nev][3] = error;
        nev++;
    }
}

/* GET http://ip:port/path, count and checksum (Adler-32) what comes back. */
/* flags: n no checksum, w write to ms0:/usbnet-test.bin, b 256 KiB receive buffer */
static void download(const char *ip, int port, const char *path, const char *flags)
{
    static unsigned char buf[32 * 1024];
    struct sockaddr_in addr = { 0 };
    char req[256];
    unsigned a = 1, b = 0, total = 0, body = 0;
    int fd, n, i, header = 1, hl = 0, sum = !strchr(flags, 'n');
    SceUID out = strchr(flags, 'w') ? sceIoOpen("ms0:/usbnet-test.bin", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0666) : -1;
    u64 t0, t1, last;

    fd = sceNetInetSocket(AF_INET, SOCK_STREAM, 0);
    if (strchr(flags, 'b')) {
        int size = 65535;
        { int r = sceNetInetSetsockopt(fd, SOL_SOCKET, SO_RCVBUF, &size, sizeof size); say("netup: SO_RCVBUF %d = %d (errno %d)\n", size, r, r < 0 ? sceNetInetGetErrno() : 0); }
    }
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    sceNetInetInetAton(ip, &addr.sin_addr);
    t0 = sceKernelGetSystemTimeWide();
    n = sceNetInetConnect(fd, (struct sockaddr *)&addr, sizeof addr);
    say("netup: connect %s:%d = %d (errno %d) after %d ms\n", ip, port, n, n < 0 ? sceNetInetGetErrno() : 0,
        (int)((sceKernelGetSystemTimeWide() - t0) / 1000));
    if (n < 0)
        return;
    n = snprintf(req, sizeof req, "GET %s HTTP/1.0\r\nHost: %s\r\n\r\n", path, ip);
    sceNetInetSend(fd, req, n, 0);
    t0 = last = sceKernelGetSystemTimeWide();
    while ((n = sceNetInetRecv(fd, buf, sizeof buf, 0)) > 0) {
        i = 0;
        if (header) { /* skip to the blank line */
            for (; i < n && header; i++) {
                hl = buf[i] == '\n' ? hl + 1 : buf[i] == '\r' ? hl : 0;
                if (hl == 2)
                    header = 0;
            }
        }
        if (out >= 0)
            sceIoWrite(out, buf + i, n - i);
        if (!sum)
            body += n - i;
        else
            for (; i < n; i++) {
                a += buf[i];
                b += a;
                if ((++body & 0xfff) == 0) {
                    a %= 65521;
                    b %= 65521;
                }
            }
        total += n;
        t1 = sceKernelGetSystemTimeWide();
        if (t1 - last > 5000000) {
            say("netup: %u bytes after %d ms\n", body, (int)((t1 - t0) / 1000));
            last = t1;
        }
    }
    t1 = sceKernelGetSystemTimeWide();
    a %= 65521;
    b %= 65521;
    say("netup: download ended with %d (errno %d): %u body bytes in %d ms, adler32 %08x\n", n,
        n < 0 ? sceNetInetGetErrno() : 0, body, (int)((t1 - t0) / 1000), b << 16 | a);
    sceNetInetClose(fd);
    if (out >= 0) {
        sceIoClose(out);
        say("netup: file closed after %d ms\n", (int)((sceKernelGetSystemTimeWide() - t0) / 1000));
    }
}

/* How fast the Memory Stick takes data: 32 MiB from RAM, in 32 KiB writes. */
static void msbench(void)
{
    static unsigned char block[32 * 1024];
    SceUID fd = sceIoOpen("ms0:/usbnet-test.bin", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0666);
    u64 t0 = sceKernelGetSystemTimeWide();
    int i;

    if (fd < 0) {
        say("netup: msbench open %08x\n", fd);
        return;
    }
    for (i = 0; i < 1024; i++)
        sceIoWrite(fd, block, sizeof block);
    sceIoClose(fd);
    say("netup: msbench 33554432 bytes in %d ms\n", (int)((sceKernelGetSystemTimeWide() - t0) / 1000));
    sceIoRemove("ms0:/usbnet-test.bin");
}

int main(int argc, char *argv[])
{
    int wait = argc > 1 ? atoi(argv[1]) : 15, r, i, state = -1, last = -2, profile = 1;

    /* As an application would: load the driver itself, from its own folder,
     * unless it is there already (flag k). */
    if (argc > 6 && strchr(argv[6], 'k')) {
        char path[256];
        const char *slash = strrchr(argv[0], '/');
        int status = 0;
        SceUID mod;
        snprintf(path, sizeof path, "%.*susbnet.prx", slash ? (int)(slash - argv[0]) + 1 : 0, argv[0]);
        mod = kuKernelLoadModule(path, 0, NULL);
        say("netup: kuKernelLoadModule(%s) %08x\n", path, mod);
        if (mod >= 0)
            say("netup: start %08x\n", sceKernelStartModule(mod, strlen(path) + 1, path, &status, NULL));
        sceKernelDelayThread(8 * 1000 * 1000); /* the bus restarts beside PSPLink */
    }

    say("netup: load common %08x\n", sceUtilityLoadNetModule(PSP_NET_MODULE_COMMON));
    say("netup: load inet %08x\n", sceUtilityLoadNetModule(PSP_NET_MODULE_INET));
    /* pspSdkInetInit gives the stack 128 KiB for packets. A 64 KiB socket
     * buffer of full frames alone pins about 100 KiB of that (2.3 KiB a
     * frame), and a fast link fills it: flag P asks for 512 KiB instead. */
    if (argc > 6 && strchr(argv[6], 'P')) {
        say("netup: sceNetInit 512 KiB %08x\n", sceNetInit(0x80000, 0x20, 0x1000, 0x20, 0x1000));
        say("netup: inet %08x resolver %08x apctl %08x\n", sceNetInetInit(), sceNetResolverInit(),
            sceNetApctlInit(0x1600, 0x42));
    } else {
        say("netup: pspSdkInetInit %08x\n", pspSdkInetInit());
    }
    say("netup: waiting %d s\n", wait);
    sceKernelDelayThread(wait * 1000 * 1000);
    /* The saved connections; the one named by argv[2] (default: number 1). */
    for (i = 1; i <= 10; i++) {
        netData name, ssid;
        if (sceUtilityCheckNetParam(i) != 0)
            continue;
        sceUtilityGetNetParam(i, PSP_NETPARAM_NAME, &name);
        sceUtilityGetNetParam(i, PSP_NETPARAM_SSID, &ssid);
        say("netup: profile %d \"%s\" ssid \"%s\"\n", i, name.asString, ssid.asString);
        /* by name or by SSID: a name with spaces does not survive pspsh's arguments */
        if (argc > 2 && (!strcmp(argv[2], name.asString) || !strcmp(argv[2], ssid.asString)))
            profile = i;
    }
    sceNetApctlAddHandler(on_event, NULL);
    r = sceNetApctlConnect(profile);
    say("netup: sceNetApctlConnect(%d) %08x\n", profile, r);
    for (i = 0; i < 300 && !(state == 4 && argc > 5); i++) {
        for (; shown < nev; shown++)
            say("netup: event %d, state %d -> %d, error %08x\n", ev[shown][0], ev[shown][1],
                ev[shown][2], ev[shown][3]);
        for (; shown < nev; shown++)
            say("netup: event %d, state %d -> %d, error %08x\n", ev[shown][0], ev[shown][1],
                ev[shown][2], ev[shown][3]);
        r = sceNetApctlGetState(&state);
        if (state != last) {
            union SceNetApctlInfo info;
            if (state == 4 && sceNetApctlGetInfo(PSP_NET_APCTL_INFO_IP, &info) == 0)
                say("netup: ip %s\n", info.ip);
            say("netup: state %d (%08x) at %d ms\n", state, r, i * 100);
            last = state;
        }
        sceKernelDelayThread(100 * 1000);
    }
    say("netup: done watching\n");
    {
        const char *flags = argc > 6 ? argv[6] : "";
        if (strchr(flags, 'c'))
            say("netup: 333 MHz = %d\n", scePowerSetClockFrequency(333, 333, 166));
        if (strchr(flags, 'm'))
            msbench();
        if (argc > 5 && state == 4 && !strchr(flags, 'R'))
            download(argv[3], atoi(argv[4]), argv[5], flags);
        /* flag r: leave and join again three times, a download each time */
        /* flag R: thirty times, without the downloads */
        for (i = 0; argc > 5 && (strchr(flags, 'r') || strchr(flags, 'R')) && i < (strchr(flags, 'R') ? 30 : 3); i++) {
            int t;
            say("netup: disconnect %08x\n", sceNetApctlDisconnect());
            for (t = 0, state = -1; t < 100 && state != 0; t++) { /* until it has really left */
                sceNetApctlGetState(&state);
                sceKernelDelayThread(100 * 1000);
            }
            say("netup: reconnect %08x after %d ms\n", sceNetApctlConnect(profile), t * 100);
            for (t = 0, state = -1; t < 150 && state != 4; t++) {
                sceNetApctlGetState(&state);
                sceKernelDelayThread(100 * 1000);
            }
            say("netup: round %d state %d after %d ms\n", i + 1, state, t * 100);
            if (state == 4 && !strchr(flags, 'R'))
                download(argv[3], atoi(argv[4]), argv[5], flags);
        }
        if (strchr(flags, 'r') || strchr(flags, 'R')) { /* leave before anyone unloads the driver */
            sceNetApctlDisconnect();
            for (i = 0, state = -1; i < 100 && state != 0; i++) {
                sceNetApctlGetState(&state);
                sceKernelDelayThread(100 * 1000);
            }
        }
        say("netup: finished\n");
        if (strchr(flags, 'w'))
            sceIoRemove("ms0:/usbnet-test.bin");
    }
    sceKernelSleepThread();
    return 0;
}
