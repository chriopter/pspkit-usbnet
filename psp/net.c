/* net.c: between Sony's network stack and its WLAN driver.
 *
 * How the firmware (6.60) works, traced on a PSP-1000 and read in a
 * decompilation: wlan.prx creates an "interface handle"
 * and attaches it to ifhandle.prx as "wlan". The handle holds four
 * callbacks: up, down, send, ioctl. The connection manager (apctl) drives a
 * connection through them: up, ioctl 0x34 scan for the profile's SSID, 0x36
 * join, 0x37 "what am I joined to", 0x38 leave, down. Each of these ends
 * with a signal carrying its result; the stack waits for it without a
 * timeout. Frames to send are taken from the handle's queue (dequeue),
 * received ones are handed up as mbufs (enqueue).
 *
 * This module takes the callbacks' place. A scan finds one more access
 * point, "Hi-Speed USB", and the user makes a connection from it like from
 * any other. A connection with that SSID is answered here and its frames
 * go to usbnet.c; the radio is never switched on. Any other connection
 * reaches wlan.prx as before.
 *
 *  - Nothing is made up in the registry: the connection is a saved one.
 *  - With the WLAN switch off, and on a PSP-E1000 (no WLAN hardware),
 *    wlan.prx reports "switch off" and refuses to attach. Five of its
 *    functions are answered here, and the "wlan" handle is then this
 *    module's own, with no driver behind it.
 */
#include <pspkernel.h>
#include <pspsdk.h>
#include <systemctrl.h>
#include <string.h>

#include "usbnet.h"

#define USB_SSID "Hi-Speed USB" /* as a scan lists it */

static const u8 gateway_mac[6] = { 0x02, 0x50, 0x43, 0x00, 0x00, 0x01 }; /* the PC's side */
static const u8 own_mac[6] = { 0x02, 0x50, 0x53, 0x50, 0x00, 0x01 };     /* without a radio */

/* ---- hooks ---- */

/* A function of another module, entered through here instead. Its first
 * two instructions are kept and run from "tramp" before jumping on, so the
 * original stays callable and can be put back. */
struct hook {
    u32 addr, jump, saved[2], tramp[4];
};

static void flush(void *p, int n)
{
    sceKernelDcacheWritebackInvalidateRange(p, n);
    sceKernelIcacheInvalidateRange(p, n);
}

static void *hook_install(struct hook *h, u32 addr, void *to)
{
    u32 *f = (u32 *)addr;

    h->addr = addr;
    h->jump = 0x08000000 | (((u32)to >> 2) & 0x03ffffff);
    h->saved[0] = h->tramp[0] = f[0];
    h->saved[1] = h->tramp[1] = f[1];
    h->tramp[2] = 0x08000000 | (((addr + 8) >> 2) & 0x03ffffff);
    h->tramp[3] = 0;
    flush(h, sizeof *h);
    f[0] = h->jump;
    f[1] = 0;
    flush(f, 8);
    return h->tramp;
}

/* Only where our jump still stands: the module may be gone, or loaded
 * again elsewhere. */
static void hook_remove(struct hook *h, u32 addr_now)
{
    u32 *f = (u32 *)h->addr;

    if (h->addr && h->addr == addr_now && f[0] == h->jump) {
        f[0] = h->saved[0];
        f[1] = h->saved[1];
        flush(f, 8);
    }
    h->addr = 0;
}

#ifdef TRACE
/* A diagnosis build (make TRACE=1): what the firmware asks is written to
 * ms0:/usbnet-trace.txt, by a thread of its own since the callers' stacks
 * are small. One line per event: a letter and three numbers. */
static struct { char what; u32 a, b, c; } ring[512];
static volatile unsigned ring_in;

static void trace(char what, u32 a, u32 b, u32 c)
{
    int k = pspSdkDisableInterrupts();
    unsigned i = ring_in++ % 512;

    ring[i].what = what; ring[i].a = a; ring[i].b = b; ring[i].c = c;
    pspSdkEnableInterrupts(k);
}

static int trace_thread(SceSize args, void *argp)
{
    static const char hex[] = "0123456789abcdef";
    unsigned out = 0;

    for (;;) {
        sceKernelDelayThread(300 * 1000);
        if (out == ring_in)
            continue;
        SceUID fd = sceIoOpen("ms0:/usbnet-trace.txt", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_APPEND, 0666);
        for (; fd >= 0 && out != ring_in; out++) {
            char line[32], *p = line;
            u32 v[3] = { ring[out % 512].a, ring[out % 512].b, ring[out % 512].c };
            int i, n;

            *p++ = ring[out % 512].what;
            for (i = 0; i < 3; i++)
                for (*p++ = ' ', n = 28; n >= 0; n -= 4)
                    *p++ = hex[(v[i] >> n) & 15];
            *p++ = '\n';
            sceIoWrite(fd, line, p - line);
        }
        if (fd >= 0)
            sceIoClose(fd);
    }
    return 0;
}

/* The profile functions this module does not answer (every other one of
 * sceUtility_netparam_internal, and "latest id"): who calls them, with what.
 * A line "E n nid address" names slot n; "n a b result" is a call. */
#define EXTRAS 16
static struct hook extra_hook[EXTRAS];
static SceUID trace_thid;
static int (*extra_real[EXTRAS])(u32, u32, u32, u32);
#define EXTRA(n) static int on_extra##n(u32 a, u32 b, u32 c, u32 d) \
    { int r = extra_real[n](a, b, c, d); trace('A' + n, a, b, r); return r; }
EXTRA(0) EXTRA(1) EXTRA(2) EXTRA(3) EXTRA(4) EXTRA(5) EXTRA(6) EXTRA(7)
EXTRA(8) EXTRA(9) EXTRA(10) EXTRA(11) EXTRA(12) EXTRA(13) EXTRA(14) EXTRA(15)
static void *const extra_to[EXTRAS] = { on_extra0, on_extra1, on_extra2, on_extra3, on_extra4, on_extra5,
    on_extra6, on_extra7, on_extra8, on_extra9, on_extra10, on_extra11, on_extra12, on_extra13,
    on_extra14, on_extra15 };

static void trace_start(void)
{
    SceModule *m = sceKernelFindModuleByName("sceUtility_Driver");
    u32 latest = sctrlHENFindFunction("sceUtility_Driver", "sceUtility", 0x4FED24D8);
    u8 *e = m ? m->ent_top : NULL, *end = e ? e + m->ent_size : NULL;
    int n = 0;

    if (latest) {
        trace('E', n, 0x4FED24D8, latest);
        extra_real[n] = hook_install(&extra_hook[n], latest, extra_to[n]);
        n++;
    }
    for (; e && e < end && e[8]; e += e[8] * 4) { /* +0 name, +8 length in words, +10 functions, +12 table */
        const char *name = *(const char **)e;
        int vars = e[9], funcs = *(u16 *)(e + 10), f;
        u32 *nids = *(u32 **)(e + 12);

        if (!name || strcmp(name, "sceUtility_netparam_internal"))
            continue;
        for (f = 0; f < funcs && n < EXTRAS; f++) {
            u32 addr = nids[funcs + vars + f];

            if (nids[f] == 0x67C2105B || (*(u32 *)addr >> 26) == 2) /* ours already */
                continue;
            trace('E', n, nids[f], addr);
            extra_real[n] = hook_install(&extra_hook[n], addr, extra_to[n]);
            n++;
        }
    }
    trace_thid = sceKernelCreateThread("usbnet_trace", trace_thread, 30, 0x4000, 0, NULL);
    sceKernelStartThread(trace_thid, 0, NULL);
}

static void trace_stop(void)
{
    int i;

    for (i = 0; i < EXTRAS; i++)
        if (extra_hook[i].addr)
            hook_remove(&extra_hook[i], extra_hook[i].addr);
    sceKernelTerminateDeleteThread(trace_thid);
}
#else
#define trace(what, a, b, c) ((void)0)
#define trace_start() ((void)0)
#define trace_stop() ((void)0)
#endif

/* ---- ifhandle.prx: loaded by whoever starts networking, gone with it ---- */

#define IFHANDLE "sceNet_Service"

static struct {
    int (*signal)(void *handle, int unused, int result);
    void *(*dequeue)(void *handle);
    int (*enqueue)(void *handle, void *mbuf);
    void *(*alloc)(int size);
    int (*copydata)(void *mbuf, int offset, int len, void *to);
    void (*freem)(void *mbuf);
    void (*free)(void *mbuf);
    void *(*lookup)(const char *name);
    int (*create)(void *handle);
    int (*detach)(void *handle);
    int (*attach)(u32 *handle, const u8 *mac, const char *name); /* the original of the hooked one */
    int (*destroy)(u32 *handle);                                 /* likewise */
} ifh;

static u32 ifh_find(u32 nid)
{
    return sctrlHENFindFunction(IFHANDLE, "sceNetIfhandle_driver", nid);
}

/* ---- the "wlan" handle ---- */

enum { H_UP = 2, H_DOWN, H_SEND, H_IOCTL }; /* where the callbacks are in a handle */
typedef int (*callback)(u32 *handle, u32 a1, u32 a2, u32 a3);

static u32 *handle;           /* the attached handle, or NULL */
static callback radio[6];     /* wlan.prx's callbacks, while the handle is its own */
static u32 own_handle[11];
static int own;               /* the handle is own_handle: no radio behind it */
static volatile int usb_next; /* apctl has just read the USB profile: the connection it starts runs over the cable */
static volatile int over_usb; /* the interface is up over the cable, not the radio */
static volatile int cable;    /* a connection over the cable is up */
static u8 joined[0x5c];       /* the scan entry apctl asked to join, from the BSSID on */
int net_no_radio;             /* option "nowlan": as if wlan.prx had refused, for tests */

static SceUID tx_event = -1, tx_thid = -1;
enum { TX_KICK = 1, TX_STOP = 2 };

/* The ioctl argument is a BSD ifreq: the name, then these. */
#define IFR_IN(ifr) ((u8 *)(ifr)[4])
#define IFR_LEN(ifr) ((ifr)[5])
#define IFR_OUT(ifr) ((u8 *)(ifr)[6])

/* One access point, open, as wlan.prx would report it (96 bytes; the SDK's
 * wlanscan sample names most of the fields). */
static void scan_entry(u8 *e)
{
    memset(e, 0, 96);
    memcpy(e + 4, gateway_mac, 6);           /* BSSID */
    e[10] = 1;                               /* channel */
    e[11] = sizeof USB_SSID - 1;
    memcpy(e + 12, USB_SSID, sizeof USB_SSID - 1);
    e[44] = 1;                               /* infrastructure */
    e[48] = 100;                             /* beacon period */
    e[66] = 0x01;                            /* capabilities: ESS, no privacy */
    memcpy(e + 68, "\x82\x84\x8b\x96", 4);   /* 1, 2, 5.5, 11 Mbit/s */
    e[76] = 100;                             /* signal */
}

/* A scan for everything (the XMB's "Scan") finds the cable too: wlan.prx
 * gets the buffer from its second entry on, and when it reports the scan
 * done (on its own thread, later) ours goes in front. One driver call is
 * under way at a time, pspnet sees to that. */
static struct { u32 *ifr; u8 *out; } scan;
static struct hook signal_hook;

static int on_signal(u32 *h, int kind, int result) /* interrupts may be off: memory only */
{
    u32 *ifr = scan.ifr;

    if (ifr && h == handle) {
        u32 found = result == 0 ? IFR_LEN(ifr) : 0;

        scan.ifr = NULL;
        scan_entry(scan.out);
        *(u32 *)scan.out = found ? (u32)(scan.out + 96) : 0;
        IFR_LEN(ifr) = found + 96;
        result = 0;
    }
    return ifh.signal(h, kind, result);
}

static int on_ioctl(u32 *h, u32 cmd, u32 a2, u32 a3)
{
    u32 *ifr = (u32 *)a2;
    u8 *in = ifr ? IFR_IN(ifr) : NULL, *out = ifr ? IFR_OUT(ifr) : NULL;

    trace('i', cmd, over_usb, own);
    if (cmd == 0x34 && in && out && IFR_LEN(ifr) >= 96) {
        int everything = in[0x18] == 0;

        cable = in[0x18] == sizeof USB_SSID - 1 && !memcmp(in + 0x1c, USB_SSID, sizeof USB_SSID - 1);
        if (cable || (own && everything)) {
            scan_entry(out);
            IFR_LEN(ifr) = 96;
        } else if (own) {
            IFR_LEN(ifr) = 0; /* without a radio nothing else is found */
        } else if (everything && signal_hook.addr && IFR_LEN(ifr) >= 2 * 96) {
            u32 rest[2] = { (u32)in, (u32)(out + 96) }; /* wlan.prx copies the two before it returns */

            IFR_LEN(ifr) -= 96;
            scan.out = out;
            scan.ifr = ifr;
            return radio[H_IOCTL](h, cmd, a2, (u32)rest);
        }
    }
    if (!cable && !own)
        return radio[H_IOCTL](h, cmd, a2, a3);
    if (cmd == 0x36 && in) /* join: the scan entry without its link */
        memcpy(joined, in, sizeof joined);
    if (cmd == 0x37 && in) /* what am I joined to */
        memcpy(in, joined, sizeof joined);
    if (cmd == 0x38)       /* leave */
        cable = 0;
    ifh.signal(h, 0, 0); /* keys, multicast, power saving: nothing to do on a cable */
    return 0;
}

static int on_send(u32 *h, u32 a1, u32 a2, u32 a3)
{
    if (!over_usb && !own)
        return radio[H_SEND](h, a1, a2, a3);
    sceKernelSetEventFlag(tx_event, TX_KICK);
    return 0;
}

/* "Up" switches the radio on in wlan.prx and signals once it is. For the
 * cable the radio stays off and USB is taken instead. Without a radio
 * there is nothing to switch on; a scan may follow, and finds the cable. */
static int on_up(u32 *h, u32 a1, u32 a2, u32 a3)
{
    trace('u', usb_next, own, 0);
    if (usb_next && !over_usb) {
        over_usb = 1;
        usbnet_link(1);
    }
    if (!over_usb && !own)
        return radio[H_UP](h, a1, a2, a3);
    ifh.signal(h, 0, 0);
    return 0;
}

static int on_down(u32 *h, u32 a1, u32 a2, u32 a3)
{
    int usb = over_usb;

    trace('d', usb, own, 0);
    over_usb = usb_next = 0;
    if (!usb && !own)
        return radio[H_DOWN](h, a1, a2, a3);
    cable = 0;
    if (usb)
        usbnet_link(0);
    ifh.signal(h, 0, 0);
    return 0;
}

static const callback ours[6] = { [H_UP] = on_up, [H_DOWN] = on_down, [H_SEND] = on_send, [H_IOCTL] = on_ioctl };

/* wlan.prx has attached its handle: its callbacks are kept, ours go in. */
static void interpose(u32 *h)
{
    int i;

    if (h == handle || !h[H_UP] || !h[H_DOWN] || !h[H_SEND] || !h[H_IOCTL])
        return;
    for (i = H_UP; i <= H_IOCTL; i++) {
        radio[i] = (callback)h[i];
        h[i] = (u32)ours[i];
    }
    handle = h;
    scan.ifr = NULL;
    cable = over_usb = own = 0;
}

static int on_attach(u32 *h, const u8 *mac, const char *name)
{
    int r = ifh.attach(h, mac, name);

    trace('a', (u32)h, r, 0);
    if (r == 0 && name && !strcmp(name, "wlan") && h != own_handle)
        interpose(h);
    return r;
}

static int on_destroy(u32 *h)
{
    if (h == handle) {
        handle = NULL;
        scan.ifr = NULL;
        cable = over_usb = 0;
    }
    return ifh.destroy(h);
}

/* ---- frames ---- */

static int (*wlan_ether)(u8 *mac);

/* "Who has 10.77.0.1?", asked as the PSP the gateway hands 10.77.0.2: the
 * gateway answers that as soon as it has the cable, connection or not, and
 * usbnet.c takes any frame from it as the sign that it is there. An
 * ordinary request: one from 0.0.0.0 is an address probe, which not every
 * stack answers. It goes out through this thread like every frame, so a
 * connection's own are not disturbed. */
static volatile int arp_wanted;

static void put_arp(void)
{
    static const u8 head[] = { 0, 1, 8, 0, 6, 4, 0, 1 }, none[6]; /* Ethernet, IPv4, request */
    u8 mac[6], *f = tx_reserve();

    if (!f) {
        tx_flush();
        if (!(f = tx_reserve()))
            return;
    }
    if (!wlan_ether || wlan_ether(mac) < 0 || !memcmp(mac, none, 6))
        memcpy(mac, own_mac, 6);
    memset(f, 0, 60);
    memset(f, 0xff, 6);
    memcpy(f + 6, mac, 6);
    f[12] = 0x08, f[13] = 0x06;
    memcpy(f + 14, head, 8);
    memcpy(f + 22, mac, 6);
    memcpy(f + 28, "\x0a\x4d\x00\x02", 4);
    memcpy(f + 38, "\x0a\x4d\x00\x01", 4);
    tx_commit(60);
}

/* What the stack wants sent: taken from its queue, copied flat, put on the
 * cable together. Without a connection there is nowhere to send it. */
static int tx_thread(SceSize size, void *argp)
{
    u32 bits;

    while (sceKernelWaitEventFlag(tx_event, TX_KICK | TX_STOP, PSP_EVENT_WAITOR | PSP_EVENT_WAITCLEAR,
                                  &bits, NULL) >= 0 && !(bits & TX_STOP)) {
        u32 *m;

        while (handle && (cable || own) && (m = ifh.dequeue(handle)) != NULL) {
            int len = (((u16 *)m)[9] & 2) ? (int)m[6] : 0; /* packet header: total length */
            u8 *to = NULL;

            if (cable && len >= 14 && len <= 1514 && !(to = tx_reserve())) {
                tx_flush(); /* the transfer is full */
                to = tx_reserve();
            }
            if (to) {
                ifh.copydata(m, 0, len, to);
                tx_commit(len);
            }
            ifh.freem(m);
        }
        if (arp_wanted) {
            arp_wanted = 0;
            put_arp();
        }
        tx_flush();
    }
    return 0;
}

void net_probe(int ask)
{
    arp_wanted = ask;
    if (ask)
        sceKernelSetEventFlag(tx_event, TX_KICK);
}

/* A frame from the cable goes up as an mbuf from the stack's own pool,
 * built the way wlan.prx builds it: 256 bytes that hold up to 206 of data,
 * or point at a 2048-byte cluster. */
void net_receive(const u8 *frame, int len)
{
    u32 *m;
    u8 *data;

    if (!cable || !handle || len < 14 || len > 1514 || !(m = ifh.alloc(256)))
        return;
    m[0] = m[1] = 0;          /* next, next packet */
    ((u16 *)m)[8] = 1;        /* type: data */
    ((u16 *)m)[9] = 2;        /* flags: packet header */
    m[6] = len;               /* the whole packet */
    m[7] = m[8] = 0;
    data = (u8 *)m + 48;
    if (len > 206) {
        if (!(data = ifh.alloc(2048))) {
            ifh.free(m);
            return;
        }
        m[12] = (u32)data;
        ((u16 *)m)[9] |= 9;   /* external storage */
        m[13] = m[14] = 0;
        m[15] = 2048;
        m[17] = m[18] = (u32)m;
    }
    data += 2;                /* the IP header lands on a word boundary */
    m[2] = (u32)data;
    m[3] = len;               /* this mbuf */
    memcpy(data, frame, len);
    ifh.enqueue(handle, m);
}

/* ---- wlan.prx without a radio ---- */

static int (*wlan_attach)(void), (*wlan_detach)(void);

static int on_switch(void)
{
    trace('s', 0, 0, 0);
    return 1; /* a cable needs no WLAN switch */
}

static int on_ether(u8 *mac)
{
    static const u8 none[6];
    int r = wlan_ether(mac);

    if (mac && (r < 0 || !memcmp(mac, none, 6))) {
        memcpy(mac, own_mac, 6);
        r = 0;
    }
    return r;
}

/* The two below arrive through a syscall from apctl: k1 says "user", and
 * ifhandle would refuse this module's kernel addresses. */
#define NO_RADIO ((int)0x80410D0C) /* sceWlanDevAttach: WLAN switch off, or no WLAN hardware */

static int on_wlan_attach(void)
{
    int r = net_no_radio ? NO_RADIO : wlan_attach(), k1, i;

    trace('w', r, own, 0);
    /* Only that one answer means "no radio". The others are the driver's
     * own business: 0x80410D0E the chip is still powering up (apctl asks
     * again), 0x80410D0F its handle is attached already. A second "wlan"
     * handle beside the driver's brings the console down. */
    if (r != NO_RADIO || own || !ifh.create)
        return r;
    k1 = pspSdkSetK1(0);
    memset(own_handle, 0, sizeof own_handle);
    if (ifh.create(own_handle) >= 0) {
        for (i = H_UP; i <= H_IOCTL; i++)
            own_handle[i] = (u32)ours[i];
        if (ifh.attach(own_handle, own_mac, "wlan") >= 0) {
            handle = own_handle;
            cable = 0;
            own = 1;
            r = 0;
        } else {
            ifh.destroy(own_handle);
        }
    }
    pspSdkSetK1(k1);
    return r;
}

static void own_detach(void)
{
    handle = NULL;
    cable = over_usb = own = 0;
    ifh.detach(own_handle);
    ifh.destroy(own_handle);
}

static int on_wlan_detach(void)
{
    int k1;

    if (!own)
        return wlan_detach();
    k1 = pspSdkSetK1(0);
    usbnet_link(0);
    own_detach();
    pspSdkSetK1(k1);
    return 0;
}

/* ---- which connection is for the cable ---- */

/* The one whose SSID is the cable's: a saved connection like any other,
 * made from the scan's entry. apctl reads the whole profile before it
 * connects, parameter 8 only it and only through this function; lists
 * read names and SSIDs. */
static int (*get_param)(int id, int param, void *data);

static int on_get_param(int id, int param, void *data)
{
    trace('g', id, param, 0);
    if (param == 8) {
        char ssid[0x80];
        int k1 = pspSdkSetK1(0); /* a kernel buffer in the caller's system call */

        usb_next = get_param(id, 1, ssid) >= 0 && !strcmp(ssid, USB_SSID);
        pspSdkSetK1(k1);
    }
    return get_param(id, param, data);
}

/* ---- putting it all in, and taking it out ---- */

/* The functions hooked for good: in modules that are there from boot. */
static struct fixed {
    const char *module, *library;
    u32 nid;
    void *to, **original;
    struct hook hook;
} fixed[] = {
    { "sceUtility_Driver", "sceUtility", 0x434D4B3A, on_get_param, (void **)&get_param },
    { "sceWlan_Driver", "sceWlanDrv", 0xD7763699, on_switch, NULL },    /* sceWlanGetSwitchState */
    { "sceWlan_Driver", "sceWlanDrv", 0x93440B11, on_switch, NULL },    /* sceWlanDevIsPowerOn */
    { "sceWlan_Driver", "sceWlanDrv", 0x0C622081, on_ether, (void **)&wlan_ether },
    { "sceWlan_Driver", "sceWlanDrv_lib", 0x482CAE9A, on_wlan_attach, (void **)&wlan_attach },
    { "sceWlan_Driver", "sceWlanDrv_lib", 0xC9A8CAB7, on_wlan_detach, (void **)&wlan_detach },
};
#define N_FIXED ((int)(sizeof fixed / sizeof fixed[0]))

static u32 fixed_addr(const struct fixed *f)
{
    return sctrlHENFindFunction(f->module, f->library, f->nid);
}

static struct hook attach_hook, destroy_hook;

/* ifhandle.prx has just been loaded, or was there already. */
static void ifhandle_arrived(void)
{
    static const struct { u32 nid; void **to; } wanted[] = {
        { 0xF94BAF52, (void **)&ifh.signal }, { 0xE2F4F1C9, (void **)&ifh.dequeue },
        { 0xC28F6FF2, (void **)&ifh.enqueue }, { 0x15CFE3C0, (void **)&ifh.alloc },
        { 0x9A6261EC, (void **)&ifh.copydata }, { 0xF56FAC82, (void **)&ifh.freem },
        { 0xF8825DC4, (void **)&ifh.free }, { 0x9CBA24D4, (void **)&ifh.lookup },
        { 0x16042084, (void **)&ifh.create }, { 0x54D1AEA1, (void **)&ifh.detach },
    };
    u32 attach = ifh_find(0xAE81C0CB), destroy = ifh_find(0xC9344A59), *h;
    int i, missing = !attach || !destroy;

    handle = NULL;
    cable = over_usb = own = 0;
    scan.ifr = NULL;
    signal_hook.addr = 0; /* the module it was in is gone */
    for (i = 0; i < (int)(sizeof wanted / sizeof wanted[0]); i++)
        if (!(*wanted[i].to = (void *)ifh_find(wanted[i].nid)))
            missing = 1;
    if (missing) {
        memset(&ifh, 0, sizeof ifh); /* another firmware: Wi-Fi works as ever, the cable does not */
        return;
    }
    ifh.signal = hook_install(&signal_hook, (u32)ifh.signal, on_signal);
    ifh.attach = hook_install(&attach_hook, attach, on_attach);
    ifh.destroy = hook_install(&destroy_hook, destroy, on_destroy);
    if ((h = ifh.lookup("wlan")) != NULL) /* loaded while a connection program already runs */
        interpose(h);
}

static int (*next_start_handler)(SceModule *);

static int on_module_start(SceModule *module)
{
    trace('m', *(u32 *)module->modname, *(u32 *)(module->modname + 4), *(u32 *)(module->modname + 8));
    if (!strcmp(module->modname, IFHANDLE))
        ifhandle_arrived();
    return next_start_handler ? next_start_handler(module) : 0;
}

/* Loaded twice (as a plugin, and again by an application): the second
 * sees the first one's jump at the head of sceUtilityGetNetParam and does
 * not stay. */
int net_present(void)
{
    u32 get = fixed_addr(&fixed[0]);

    return get && (*(u32 *)get >> 26) == 2;
}

int net_start(void)
{
    int i;

    if (!fixed_addr(&fixed[0]))
        return -1; /* another firmware: nothing to hook, nothing hooked */
    tx_event = sceKernelCreateEventFlag("usbnet_tx", 0, 0, NULL);
    tx_thid = sceKernelCreateThread("usbnet_tx", tx_thread, 18, 0x4000, 0, NULL);
    if (tx_event < 0 || tx_thid < 0) {
        if (tx_thid >= 0)
            sceKernelDeleteThread(tx_thid);
        if (tx_event >= 0)
            sceKernelDeleteEventFlag(tx_event);
        return -1;
    }
    sceKernelStartThread(tx_thid, 0, NULL);
    for (i = 0; i < N_FIXED; i++) {
        u32 addr = fixed_addr(&fixed[i]);
        void *original = addr ? hook_install(&fixed[i].hook, addr, fixed[i].to) : NULL;

        if (fixed[i].original)
            *fixed[i].original = original;
    }
    trace_start();
    if (ifh_find(0xAE81C0CB))
        ifhandle_arrived();
    next_start_handler = sctrlHENSetStartModuleHandler(on_module_start);
    return 0;
}

/* Everything put into other modules is taken out again, so that this
 * module can be unloaded. */
void net_stop(void)
{
    SceUInt timeout = 2 * 1000 * 1000;
    int i, k, ifhandle_there = ifh.lookup && ifh_find(0x9CBA24D4) == (u32)ifh.lookup;

    sctrlHENSetStartModuleHandler(next_start_handler);
    trace_stop();
    if (own && ifhandle_there)
        own_detach();
    k = pspSdkDisableInterrupts();
    if (handle && ifhandle_there && ifh.lookup("wlan") == handle)
        for (i = H_UP; i <= H_IOCTL; i++)
            handle[i] = (u32)radio[i];
    handle = NULL;
    cable = over_usb = own = 0;
    hook_remove(&signal_hook, ifh_find(0xF94BAF52));
    hook_remove(&attach_hook, ifh_find(0xAE81C0CB));
    hook_remove(&destroy_hook, ifh_find(0xC9344A59));
    for (i = 0; i < N_FIXED; i++)
        hook_remove(&fixed[i].hook, fixed_addr(&fixed[i]));
    pspSdkEnableInterrupts(k);
    sceKernelSetEventFlag(tx_event, TX_STOP);
    sceKernelWaitThreadEnd(tx_thid, &timeout);
    sceKernelTerminateDeleteThread(tx_thid);
    sceKernelDeleteEventFlag(tx_event);
}
