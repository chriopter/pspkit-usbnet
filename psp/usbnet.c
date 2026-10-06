/* usbnet: the PSP's network over the USB cable.
 *
 * A kernel PRX with two halves. This file is the cable: one USB function
 * driver, an interface of class 0xfd with a bulk IN and a bulk OUT
 * endpoint. A transfer carries one or more Ethernet frames, each behind its
 * length (u16, little endian); a length of zero ends it. net.c is what
 * makes Sony's network stack use it.
 *
 * The same module however it is loaded (pspsh, an application, an ARK
 * plugin). USB is taken only while a connection over the cable lasts:
 *   - USB already active (PSPLink): the bus is restarted once with
 *     usbhostfs and this driver both on it, and pspsh keeps working. The
 *     driver then stays on the bus, so the cable drops only that once;
 *   - USB not active: the module starts the bus itself and gives it up
 *     when the connection ends.
 * Either way the device is 054c:01c9.
 *
 * Nothing polls: one thread sleeps until a frame arrives, one (in net.c)
 * until the stack has a frame to send.
 *
 * An application asks whether the gateway is there, before and without
 * connecting, through the device "usbnet:" (usbnet_api.h, and the end of
 * this file). Looking takes USB like a connection does, for as long as it
 * looks.
 *
 * Options after the path: "alone" and "beside" say which of the two bus
 * cases it is instead of looking; "nowlan" behaves as if there were no
 * radio (see net.c), for tests.
 */
#include <pspkernel.h>
#include <pspsdk.h>
#include <pspusb.h>
#include <pspusbbus.h>
#include <pspinit.h>
#include <systemctrl.h>
#include <string.h>

#include "usbnet.h"
#include "usbnet_api.h"

PSP_MODULE_INFO("usbnet", PSP_MODULE_KERNEL, 1, 0);

#define DRIVER "UsbnetDriver"
#define USB_PID 0x1c9         /* as PSPLink activates the bus */
#define FRAME_MAX 1514
#define BATCH (16 * 1024)     /* one transfer, either way */

/* ---- the USB function, laid out the way PSPLink's usbhostfs does ---- */

static struct DeviceDescriptor devdesc = {
    .bLength = 18, .bDescriptorType = 1, .bcdUSB = 0x200,
    .bMaxPacketSize = 64, .bcdDevice = 0x100, .bNumConfigurations = 1,
};
static struct ConfigDescriptor confdesc = {
    .bLength = 9, .bDescriptorType = 2, .wTotalLength = 9 + 9 + 2 * 7,
    .bNumInterfaces = 1, .bConfigurationValue = 1, .bmAttributes = 0xC0,
};
static struct InterfaceDescriptor interdesc = {
    .bLength = 9, .bDescriptorType = 4, .bNumEndpoints = 2, .bInterfaceClass = 0xFD,
    .iInterface = 1, /* the driver's string, below; the bus driver numbers it */
};
static struct EndpointDescriptor endpdesc[2] = { /* the bus driver numbers them */
    { .bLength = 7, .bDescriptorType = 5, .bEndpointAddress = 0x81, .bmAttributes = 2 },
    { .bLength = 7, .bDescriptorType = 5, .bEndpointAddress = 0x02, .bmAttributes = 2 },
};
/* The interface's string says which PSP this is: the gateway serves
 * several, each on its own cable, and names them by it. "PSP" alone until
 * the model is known. */
static struct StringDescriptor strp = { 2 + 2 * 3, 3, { 'P', 'S', 'P' } };
static struct UsbEndpoint endp[3] = { { 0, 0, 0 }, { 1, 0, 0 }, { 2, 0, 0 } }; /* control, IN, OUT */
static struct UsbInterface intp = { 0xFFFFFFFF, 0, 1 };
static struct UsbData usbdata[2]; /* high speed, full speed */
static struct UsbDriver driver;

static int usb_start(int size, void *p)
{
    int i;

    memset(usbdata, 0, sizeof usbdata);
    for (i = 0; i < 2; i++) {
        struct UsbData *d = &usbdata[i];
        endpdesc[0].wMaxPacketSize = endpdesc[1].wMaxPacketSize = i == 0 ? 512 : 64;
        memcpy(d->devdesc, &devdesc, sizeof devdesc);
        memcpy(d->confdesc.desc, &confdesc, sizeof confdesc);
        memcpy(d->interdesc.desc, &interdesc, sizeof interdesc);
        memcpy(d->endp[0].desc, &endpdesc[0], sizeof endpdesc[0]);
        memcpy(d->endp[1].desc, &endpdesc[1], sizeof endpdesc[1]);
        d->config.pconfdesc = &d->confdesc;
        d->config.pinterfaces = d->confdesc.pinterfaces = &d->interfaces;
        d->config.pinterdesc = d->interfaces.pinterdesc[0] = &d->interdesc;
        d->config.pendp = d->interdesc.pendp = d->endp;
        d->interfaces.intcount = 1;
    }
    driver.devp_hi = usbdata[0].devdesc;
    driver.confp_hi = &usbdata[0].config;
    driver.devp = usbdata[1].devdesc;
    driver.confp = &usbdata[1].config;
    return 0;
}

/* sceKernelGetModel, looked up and not imported: a firmware without it
 * still loads the module. The SDK's NID, which the CFW translates, then
 * the one of 6.60 and 6.61 as it is. */
static void name_the_model(void)
{
    static const char *const names[] = {
        "PSP-1000", "PSP-2000", "PSP-3000", "PSP-3000", "PSP Go", NULL,
        "PSP-3000", NULL, "PSP-3000", NULL, "PSP Street",
    };
    int (*model)(void) = (void *)sctrlHENFindFunction("sceSystemMemoryManager", "SysMemForKernel", 0x6373995D);
    const char *name;
    int m, i;

    if (!model)
        model = (void *)sctrlHENFindFunction("sceSystemMemoryManager", "SysMemForKernel", 0x07C586A1);
    m = model ? model() : -1;
    name = m >= 0 && m < (int)(sizeof names / sizeof names[0]) ? names[m] : NULL;
    if (!name)
        return;
    for (i = 0; name[i] && i < 31; i++)
        strp.bString[i] = name[i];
    strp.bLength = 2 + 2 * i;
}

/* ---- state ---- */

enum { EV_RECEIVED = 1, EV_DETACHED = 2, EV_STOP = 4, EV_LINK_UP = 8, EV_LINK_DOWN = 16, EV_PROBE = 32 };

static SceUID event = -1, thid = -1;
/* Its own flag: the receiving thread's wait clears every bit of "event"
 * when it wakes, and would take a "sent" meant for the sender with it. */
static SceUID sent = -1;
static int on_bus;      /* our driver is started on the bus */
static int own_bus;     /* and the bus driver by this module: the XMB keeps its own running */
static int alone;       /* and we started the bus ourselves */
static int forced;      /* option: 1 alone, 2 beside */
static int registered, armed;
static volatile int linked; /* a connection over the cable wants the bus */
static u32 cancelled;   /* when the receive request was last called back */

static unsigned char in[BATCH] __attribute__((aligned(64)));
static unsigned char out[BATCH] __attribute__((aligned(64)));
static int fill;        /* bytes of "out" built so far */
static struct UsbdDeviceReq rx_req, tx_req;
/* A request is the bus driver's from the call that hands it over until its
 * callback: only then may it be filled in again. A cancelled one comes
 * back through the callback too; should it not, it counts as back after a
 * second or two. */
static volatile int rx_out, tx_out;

/* ---- what the bus driver calls, in interrupt context ---- */

/* The time, in units of 1024 microseconds: near enough a millisecond, and
 * 32 bits of it last seven weeks. */
static u32 now(void)
{
    return (u32)(sceKernelGetSystemTimeWide() >> 10);
}

static int usb_request(int arg1, int arg2, struct DeviceRequest *req)
{
    return -1; /* no class or vendor requests */
}

static int usb_nothing(int arg1, int arg2, int arg3)
{
    return 0;
}

static int usb_detach(int arg1, int arg2, int arg3)
{
    sceKernelSetEventFlag(event, EV_DETACHED);
    return 0;
}

static int rx_done(struct UsbdDeviceReq *r, int arg2, int arg3)
{
    rx_out = 0;
    sceKernelSetEventFlag(event, EV_RECEIVED);
    return 0;
}

static int tx_done(struct UsbdDeviceReq *r, int arg2, int arg3)
{
    tx_out = 0;
    sceKernelSetEventFlag(sent, 1);
    return 0;
}

static struct UsbDriver driver = {
    DRIVER, 3, endp, &intp, NULL, NULL, NULL, NULL, &strp,
    usb_request, usb_nothing, (void *)usb_nothing, usb_detach, 0, usb_start, (void *)usb_nothing, NULL,
};

/* ---- transfers ---- */

static void arm_receive(void)
{
    sceKernelDcacheInvalidateRange(in, sizeof in);
    memset(&rx_req, 0, sizeof rx_req);
    rx_req.endp = &endp[2];
    rx_req.data = in;
    rx_req.size = sizeof in;
    rx_req.func = rx_done;
    rx_out = armed = 1; /* before the call: the callback may come before it returns */
    if (sceUsbbdReqRecv(&rx_req) < 0)
        rx_out = armed = 0;
}

static void disarm(void)
{
    if (rx_out)
        sceUsbbdReqCancelAll(&endp[2]);
    cancelled = now();
    armed = 0;
}

unsigned char *tx_reserve(void)
{
    return fill + 2 + FRAME_MAX <= BATCH - 4 ? out + fill + 2 : NULL;
}

void tx_commit(int len)
{
    out[fill] = len;
    out[fill + 1] = len >> 8;
    fill += 2 + len;
}

void tx_flush(void)
{
    SceUInt timeout = 1000 * 1000;
    u32 bits;
    int len = fill;

    fill = 0;
    if (!len || tx_out) /* the last transfer is not back yet: the stack sends these frames again */
        return;
    if (len % 64 == 0) /* a transfer ends in a short packet: a zero length does it */
        out[len] = out[len + 1] = 0, len += 2;
    sceKernelDcacheWritebackRange(out, (len + 63) & ~63);
    memset(&tx_req, 0, sizeof tx_req);
    tx_req.endp = &endp[1];
    tx_req.data = out;
    tx_req.size = len;
    tx_req.func = tx_done;
    sceKernelClearEventFlag(sent, 0);
    tx_out = 1;
    if (sceUsbbdReqSend(&tx_req) < 0) {
        tx_out = 0;
    } else if (sceKernelWaitEventFlag(sent, 1, PSP_EVENT_WAITOR | PSP_EVENT_WAITCLEAR, &bits, &timeout) < 0
               && tx_out) { /* still out: the bus going down takes it back itself */
        sceUsbbdReqCancelAll(&endp[1]); /* nobody listening: the stack sends again */
        timeout = 1000 * 1000;
        sceKernelWaitEventFlag(sent, 1, PSP_EVENT_WAITOR | PSP_EVENT_WAITCLEAR, &bits, &timeout);
        tx_out = 0;
    }
}

/* ---- the bus ---- */

static int connected(void)
{
    return sceUsbGetState() & PSP_USB_CONNECTION_ESTABLISHED;
}

static int storage(void)
{
    return sceUsbGetDrvState("USBStor_Driver") == 1; /* started */
}

static void bus_up(void)
{
    int i;

    if (on_bus || storage())
        return; /* the XMB's USB connection has the port: it keeps it */
    if (forced == 1 || (!forced && !(sceUsbGetState() & PSP_USB_ACTIVATED))) {
        own_bus = sceUsbStart(PSP_USBBUS_DRIVERNAME, 0, 0) >= 0;
        sceUsbStart(DRIVER, 0, 0);
        sceUsbActivate(USB_PID);
        on_bus = alone = 1;
        return;
    }
    /* Beside PSPLink: the bus restarts with both drivers. Should the PC not
     * take the two together, PSPLink gets it back alone. */
    sceUsbDeactivate(USB_PID);
    sceUsbStart(DRIVER, 0, 0);
    sceUsbActivate(USB_PID);
    for (i = 0; i < 150 && !connected(); i++)
        sceKernelDelayThread(100 * 1000);
    on_bus = connected();
    if (!on_bus) {
        sceUsbDeactivate(USB_PID);
        sceUsbStop(DRIVER, 0, 0);
        sceUsbActivate(USB_PID);
    }
}

/* Off the bus. Beside PSPLink that happens only when the module goes: the
 * cable would drop at the end of every connection otherwise. */
static void bus_down(int for_good)
{
    int i;

    if (!on_bus || (!alone && !for_good))
        return;
    disarm();
    if (tx_out)
        sceUsbbdReqCancelAll(&endp[1]);
    for (i = 0; i < 20 && (rx_out || tx_out); i++) /* both requests back before the driver goes */
        sceKernelDelayThread(10 * 1000);
    sceUsbDeactivate(USB_PID);
    sceUsbStop(DRIVER, 0, 0);
    rx_out = tx_out = 0;
    if (alone && own_bus)
        sceUsbStop(PSP_USBBUS_DRIVERNAME, 0, 0);
    else if (!alone)
        sceUsbActivate(USB_PID);                 /* back to usbhostfs alone */
    on_bus = alone = 0;
}

/* In the XMB a cable going in starts the "USB Connection" by itself (the
 * setting "USB Auto Connect"), and the cable to the gateway is always in:
 * that would take the port from under a connection. Sony's bus driver
 * tells the XMB about the cable through one callback; while this module is
 * loaded it is not told. "USB Connection" chosen by hand works as ever. */
static int cable_not_told(SceUID callback, int state)
{
    return 0;
}

static void xmb_is_told_of_the_cable(int told)
{
    SceModule *usb = sceKernelFindModuleByName("sceUSB_Driver");
    u32 notify = sctrlHENFindFunction("sceThreadManager", "ThreadManForKernel", 0xC11BA8C4);

    if (usb && notify && sceKernelInitKeyConfig() == PSP_INIT_KEYCONFIG_VSH)
        sctrlHookImportByNID(usb, "ThreadManForKernel", 0xC11BA8C4, told ? (void *)notify : cable_not_told);
}

void usbnet_link(int up)
{
    sceKernelSetEventFlag(event, up ? EV_LINK_UP : EV_LINK_DOWN);
}

/* ---- is the gateway there? ----
 *
 * Asked through the device at the end of this file, by any thread; done
 * here, on the cable's thread, so that an asker that is killed while it
 * waits leaves nothing behind. The bus comes up as for a connection, and
 * once the host has taken the device the gateway is asked for a sign of
 * life twice a second. Any frame from the cable is that sign: only the
 * gateway talks on this interface. net.c hands a frame to the stack only
 * while a connection is up, so the answers go no further than here.
 *
 * A connection beginning while this looks finds the bus up and keeps it;
 * one that is up already is not touched, the question travels with its
 * frames. Otherwise the bus goes down again at the end, the way it does
 * after a connection (beside PSPLink that means it stays). */

#define PROBE_MAX 10000     /* ms an asker may look */
#define ARP_EVERY 500
#define NO_CABLE_AFTER 1500 /* the port says "no cable" this long after the bus came up */
#define HEARD_FOR 5000      /* how long "the gateway was heard" is said */

static volatile u32 heard, probe_from, probe_until, failed_at; /* times, see now() */
static volatile int heard_any, probe_asked, failed;
static volatile int callers, stopping;
static int probing;
static u32 probe_began, arp_at;

/* Over; error: what the askers are told where it could not look at all. */
static void probe_end(int error)
{
    int k = pspSdkDisableInterrupts();

    if (error) {
        failed = error;
        failed_at = now();
        probe_asked = 0;
    }
    pspSdkEnableInterrupts(k);
    net_probe(0);
    probing = 0;
    if (!linked)
        bus_down(0);
}

static void probe_begin(void)
{
    if (probing || !probe_asked)
        return; /* the one under way answers this asker too; or it has, already */
    probing = 1;
    probe_began = now();
    arp_at = probe_began - ARP_EVERY;
    if (on_bus)
        return;
    if (storage()) {
        probe_end(USBNET_BUSY);
        return;
    }
    bus_up();
    if (!on_bus) /* beside PSPLink, and the PC did not take the two */
        probe_end(USBNET_NO_CABLE);
}

static void probe_step(void)
{
    u32 t = now();
    /* Decided with the askers shut out: one that comes after this is a
     * new probe, one that came before is counted in. */
    int k = pspSdkDisableInterrupts();
    int over = (heard_any && (int)(heard - probe_from) >= 0) || (int)(t - probe_until) >= 0;

    if (over)
        probe_asked = 0;
    pspSdkEnableInterrupts(k);
    if (over) {
        probe_end(0);
    } else if (connected()) {
        if (t - arp_at >= ARP_EVERY) {
            arp_at = t;
            net_probe(1);
        }
    } else if (alone && !(sceUsbGetState() & PSP_USB_CABLE_CONNECTED) && t - probe_began >= NO_CABLE_AFTER) {
        probe_end(USBNET_NO_CABLE);
    }
}

/* An asker's side, on its own thread. It holds nothing while it waits. */
static int probe(u32 ms)
{
    u32 start = now(), span;
    int k;

    if (ms == 0)
        return heard_any && start - heard < HEARD_FOR;
    span = (ms > PROBE_MAX ? PROBE_MAX : ms) * 125 / 128;
    k = pspSdkDisableInterrupts();
    if (!probe_asked || (int)(start + span - probe_until) > 0)
        probe_until = start + span;
    probe_from = start;
    probe_asked = 1;
    pspSdkEnableInterrupts(k);
    sceKernelSetEventFlag(event, EV_PROBE);
    for (;;) {
        if (stopping)
            return USBNET_STOPPED;
        if (heard_any && (int)(heard - start) >= 0)
            return 1;
        if (failed && (int)(failed_at - start) >= 0)
            return failed;
        if (now() - start >= span)
            return 0;
        sceKernelDelayThread(20 * 1000);
    }
}

static int cable_thread(SceSize size, void *argp)
{
    name_the_model();
    registered = sceUsbbdRegister(&driver) >= 0;
    for (;;) {
        /* Armed, or off the bus, only an event wakes this up. On the bus
         * with the cable out it looks once a second for the PC, and while
         * it looks for the gateway twenty times. */
        SceUInt timeout;
        u32 bits = 0;

        if (rx_out && !armed && now() - cancelled > 2000)
            rx_out = 0;
        if (on_bus && !armed && !rx_out && connected())
            arm_receive();
        if (probing)
            probe_step();
        timeout = probing ? 50 * 1000 : 1000 * 1000;
        if (sceKernelWaitEventFlag(event, 0xff, PSP_EVENT_WAITOR | PSP_EVENT_WAITCLEAR, &bits,
                                   probing || (on_bus && !armed) ? &timeout : NULL) < 0)
            continue;
        if (bits & EV_STOP)
            break;
        if (bits & EV_LINK_DOWN) {
            linked = 0;
            if (!probing) /* else when it has looked */
                bus_down(0);
        }
        if (bits & EV_LINK_UP) {
            linked = 1;
            bus_up();
        }
        if (bits & EV_PROBE)
            probe_begin();
        if (bits & EV_DETACHED) {
            disarm();
        } else if ((bits & EV_RECEIVED) && armed && on_bus) {
            int n = rx_req.retcode == 0 ? rx_req.recvsize : 0, at = 0;

            armed = 0;
            while (at + 2 <= n) { /* the frames of this transfer */
                int len = in[at] | in[at + 1] << 8;
                if (len == 0 || at + 2 + len > n)
                    break;
                heard = now();
                heard_any = 1;
                net_receive(in + at + 2, len);
                at += 2 + len;
            }
        }
    }
    return 0;
}

/* ---- the device "usbnet:" ---- */

static int state(void)
{
    int usb = sceUsbGetState(), s = 0;

    if (usb < 0)
        usb = 0;
    if (usb & PSP_USB_CABLE_CONNECTED)
        s |= USBNET_CABLE;
    if (on_bus && (usb & PSP_USB_CONNECTION_ESTABLISHED))
        s |= USBNET_BUS;
    if (linked)
        s |= USBNET_LINK;
    if (heard_any && now() - heard < HEARD_FOR)
        s |= USBNET_GATEWAY;
    return s;
}

/* Called on the asker's thread, through sceIoDevctl. From an application
 * k1 says "user": what it points at must not be the kernel's, and the
 * kernel's own functions are called below as the kernel. */
static int dev_devctl(PspIoDrvFileArg *arg, const char *name, unsigned int cmd, void *indata, int inlen,
                      void *outdata, int outlen)
{
    u32 k1 = pspSdkSetK1(0), ms;
    int r = USBNET_INVALID, k = pspSdkDisableInterrupts();

    callers++;
    pspSdkEnableInterrupts(k);
    if (stopping) {
        r = USBNET_STOPPED;
    } else if (cmd == USBNET_VERSION) {
        r = USBNET_API;
    } else if (cmd == USBNET_STATE) {
        r = state();
    } else if (cmd == USBNET_PROBE && indata && inlen == 4
               && !(k1 && (((u32)indata | ((u32)indata + 3)) & 0x80000000))) {
        memcpy(&ms, indata, 4);
        r = probe(ms);
    }
    k = pspSdkDisableInterrupts();
    callers--;
    pspSdkEnableInterrupts(k);
    pspSdkSetK1(k1);
    return r;
}

static int dev_nothing(PspIoDrvArg *arg)
{
    return 0;
}

static PspIoDrvFuncs dev_funcs = { .IoInit = dev_nothing, .IoExit = dev_nothing, .IoDevctl = dev_devctl };
static PspIoDrv dev = { "usbnet", 0x10, 0x800, "USBNET", &dev_funcs };
static int dev_added;

int module_start(SceSize args, void *argp)
{
    const char *p = argp, *end = p + args;

    if (net_present())
        return 1; /* do not stay loaded */
    for (p += strlen(p) + 1; p < end; p += strlen(p) + 1) {
        if (!strcmp(p, "alone"))
            forced = 1;
        if (!strcmp(p, "beside"))
            forced = 2;
        if (!strcmp(p, "nowlan"))
            net_no_radio = 1;
    }
    /* Everything this module needs first; the hooks into the firmware only
     * once nothing can fail any more, or they would point into a module
     * that is about to be unloaded. */
    event = sceKernelCreateEventFlag("usbnet", 0x200, 0, NULL);
    sent = sceKernelCreateEventFlag("usbnet_sent", 0, 0, NULL);
    thid = sceKernelCreateThread("usbnet", cable_thread, 16, 0x4000, 0, NULL);
    if (event < 0 || sent < 0 || thid < 0 || net_start() < 0) {
        if (thid >= 0)
            sceKernelDeleteThread(thid);
        if (event >= 0)
            sceKernelDeleteEventFlag(event);
        if (sent >= 0)
            sceKernelDeleteEventFlag(sent);
        return 1;
    }
    xmb_is_told_of_the_cable(0);
    sceKernelStartThread(thid, 0, NULL);
    dev_added = sceIoAddDrv(&dev) >= 0; /* without it everything else works as ever */
    return 0;
}

/* For development: a plugin stays for good. Unload only without a
 * connection: with the radio absent the interface the stack is using would
 * go with the module.
 *
 * The device first, and whoever is asking it sent home; then the hooks,
 * the thread, the bus as it was found, all before returning and from this
 * thread (left to the module's own thread, the bus did not always come back
 * on 6.60). Beside PSPLink the cable drops once more in here. */
int module_stop(SceSize args, void *argp)
{
    SceUInt timeout = 2 * 1000 * 1000;
    int i;

    stopping = 1;
    if (dev_added)
        sceIoDelDrv("usbnet");
    for (i = 0; i < 100 && callers; i++)
        sceKernelDelayThread(20 * 1000);
    net_stop();
    xmb_is_told_of_the_cable(1);
    sceKernelSetEventFlag(event, EV_STOP);
    sceKernelWaitThreadEnd(thid, &timeout);
    sceKernelTerminateDeleteThread(thid);
    bus_down(1);
    if (registered)
        sceUsbbdUnregister(&driver);
    sceKernelDeleteEventFlag(event);
    sceKernelDeleteEventFlag(sent);
    return 0;
}
