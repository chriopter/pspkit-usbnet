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
 * Options after the path: "alone" and "beside" say which of the two bus
 * cases it is instead of looking; "nowlan" behaves as if there were no
 * radio (see net.c), for tests.
 */
#include <pspkernel.h>
#include <pspusb.h>
#include <pspusbbus.h>
#include <pspinit.h>
#include <systemctrl.h>
#include <string.h>

#include "usbnet.h"

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
};
static struct EndpointDescriptor endpdesc[2] = { /* the bus driver numbers them */
    { .bLength = 7, .bDescriptorType = 5, .bEndpointAddress = 0x81, .bmAttributes = 2 },
    { .bLength = 7, .bDescriptorType = 5, .bEndpointAddress = 0x02, .bmAttributes = 2 },
};
static unsigned char strp[] = { 0x8, 0x3, 'P', 0, 'S', 0, 'P', 0 };
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

/* ---- state ---- */

enum { EV_RECEIVED = 1, EV_DETACHED = 2, EV_STOP = 4, EV_LINK_UP = 8, EV_LINK_DOWN = 16 };

static SceUID event = -1, thid = -1;
/* Its own flag: the receiving thread's wait clears every bit of "event"
 * when it wakes, and would take a "sent" meant for the sender with it. */
static SceUID sent = -1;
static int on_bus;      /* our driver is started on the bus */
static int own_bus;     /* and the bus driver by this module: the XMB keeps its own running */
static int alone;       /* and we started the bus ourselves */
static int forced;      /* option: 1 alone, 2 beside */
static int registered, armed;

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
    DRIVER, 3, endp, &intp, NULL, NULL, NULL, NULL, (struct StringDescriptor *)strp,
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
    } else if (sceKernelWaitEventFlag(sent, 1, PSP_EVENT_WAITOR | PSP_EVENT_WAITCLEAR, &bits, &timeout) < 0) {
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

static void bus_up(void)
{
    int i;

    if (on_bus || sceUsbGetDrvState("USBStor_Driver") == 1) /* started */
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

static int cable_thread(SceSize size, void *argp)
{
    int waited = 0; /* turns of the loop with a cancelled request not back */

    registered = sceUsbbdRegister(&driver) >= 0;
    for (;;) {
        /* Armed, or off the bus, only an event wakes this up. On the bus
         * with the cable out it looks once a second for the PC. */
        SceUInt timeout = 1000 * 1000;
        u32 bits = 0;

        if (rx_out && !armed && ++waited > 2)
            rx_out = 0;
        if (on_bus && !armed && !rx_out && connected()) {
            waited = 0;
            arm_receive();
        }
        if (sceKernelWaitEventFlag(event, 0xff, PSP_EVENT_WAITOR | PSP_EVENT_WAITCLEAR, &bits,
                                   on_bus && !armed ? &timeout : NULL) < 0)
            continue;
        if (bits & EV_STOP)
            break;
        if (bits & EV_LINK_DOWN)
            bus_down(0);
        if (bits & EV_LINK_UP)
            bus_up();
        if (bits & EV_DETACHED) {
            disarm();
        } else if ((bits & EV_RECEIVED) && armed && on_bus) {
            int n = rx_req.retcode == 0 ? rx_req.recvsize : 0, at = 0;

            armed = 0;
            while (at + 2 <= n) { /* the frames of this transfer */
                int len = in[at] | in[at + 1] << 8;
                if (len == 0 || at + 2 + len > n)
                    break;
                net_receive(in + at + 2, len);
                at += 2 + len;
            }
        }
    }
    return 0;
}

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
    return 0;
}

/* For development: a plugin stays for good. Unload only without a
 * connection: with the radio absent the interface the stack is using would
 * go with the module.
 *
 * The hooks first, then the thread, then the bus as it was found, all
 * before returning and from this thread (left to the module's own thread,
 * the bus did not always come back on 6.60). Beside PSPLink the cable drops
 * once more in here. */
int module_stop(SceSize args, void *argp)
{
    SceUInt timeout = 2 * 1000 * 1000;

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
