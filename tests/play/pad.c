/* pad: presses buttons by a timetable and photographs the screen, so a
 * game can be driven and watched without hands. A kernel plugin, for a
 * test bench only:
 *
 *   umd, ms0:/seplugins/pad.prx, on
 *
 * ms0:/seplugins/pad.txt: one press a line, "from to buttons": milliseconds
 * since the game started, buttons as in pspctrl.h, in hex (cross 4000,
 * circle 2000, start 8, up 10, down 40, left 80, right 20). A first line
 * "shot N" asks for a picture every N milliseconds: ms0:/shots/NNN.raw
 * (16 bytes of header: "SHOT", line width, pixel format, time; then 272
 * lines as they are in video memory). A first line "film N" instead films:
 * a frame every N milliseconds into host0:/film0.raw, film1.raw, ... (PSPLink's host folder),
 * each the time in milliseconds (4 bytes) and 480x272 pixels of 16 bits
 * (5650), whatever the screen's own format.
 */
#include <pspkernel.h>
#include <pspctrl.h>
#include <pspdisplay.h>
#include <systemctrl.h>
#include <string.h>

PSP_MODULE_INFO("pad", PSP_MODULE_KERNEL, 1, 0);

static struct { u32 from, to, buttons; } press[256];
static int presses, shot_every, film_every;
static u32 started;
static int (*read_real)(SceCtrlData *, int), (*peek_real)(SceCtrlData *, int);

static u32 now(void)
{
    return (sceKernelGetSystemTimeLow() - started) / 1000;
}

static int with_presses(int r, SceCtrlData *pad)
{
    u32 t = now();
    int i, k;

    for (i = 0; i < presses; i++)
        if (t >= press[i].from && t < press[i].to)
            for (k = 0; k < r; k++)
                pad[k].Buttons |= press[i].buttons;
    return r;
}

static int on_read(SceCtrlData *pad, int count)
{
    return with_presses(read_real(pad, count), pad);
}

static int on_peek(SceCtrlData *pad, int count)
{
    return with_presses(peek_real(pad, count), pad);
}

static u32 number(const char **p, int base)
{
    u32 v = 0;

    while (**p == ' ')
        ++*p;
    for (;; ++*p) {
        int c = **p, d = c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : 99;

        if (d >= base)
            return v;
        v = v * base + d;
    }
}

static void timetable(void)
{
    static char text[8192];
    SceUID fd = sceIoOpen("ms0:/seplugins/pad.txt", PSP_O_RDONLY, 0);
    int n = fd >= 0 ? sceIoRead(fd, text, sizeof text - 1) : 0;
    const char *p = text;

    if (fd >= 0)
        sceIoClose(fd);
    text[n > 0 ? n : 0] = 0;
    if (!strncmp(p, "shot", 4)) {
        p += 4;
        shot_every = number(&p, 10);
    } else if (!strncmp(p, "film", 4)) {
        p += 4;
        film_every = number(&p, 10);
    }
    while (*p && presses < 256) {
        while (*p == '\n' || *p == '\r')
            p++;
        if (*p >= '0' && *p <= '9') {
            press[presses].from = number(&p, 10);
            press[presses].to = number(&p, 10);
            press[presses].buttons = number(&p, 16);
            presses++;
        }
        while (*p && *p != '\n')
            p++;
    }
}

static int shots(SceSize args, void *argp)
{
    int n = 0;

    sceIoMkdir("ms0:/shots", 0777);
    for (;;) {
        char name[32] = "ms0:/shots/000.raw";
        void *top = NULL;
        int width = 0, format = 0, k1;
        u32 head[4];
        SceUID fd;

        sceKernelDelayThread(shot_every * 1000);
        k1 = pspSdkSetK1(0);
        sceDisplayGetFrameBuf(&top, &width, &format, 0);
        pspSdkSetK1(k1);
        if (!top || width <= 0 || n > 999)
            continue;
        name[11] = '0' + n / 100, name[12] = '0' + n / 10 % 10, name[13] = '0' + n % 10;
        memcpy(head, "SHOT", 4);
        head[1] = width, head[2] = format, head[3] = now();
        fd = sceIoOpen(name, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0777);
        if (fd >= 0) {
            sceIoWrite(fd, head, sizeof head);
            /* through memory of our own: the file system does not read video memory */
            static u8 lines[16 * 512 * 4];
            int line = width * (format == 3 ? 4 : 2), y;

            for (y = 0; y < 272 && line * 16 <= (int)sizeof lines; y += 16) {
                memcpy(lines, (u8 *)(0x40000000 | (u32)top) + y * line, 16 * line);
                sceIoWrite(fd, lines, 16 * line);
            }
            sceIoClose(fd);
            n++;
        }
    }
    return 0;
}

/* While filming, host0:/pad.live ("count buttons", buttons in hex) presses
 * from the PC: each new count is one press of 200 ms. */
static void live(void)
{
    static u32 seen;
    char text[32];
    SceUID fd = sceIoOpen("host0:/pad.live", PSP_O_RDONLY, 0);
    int n = fd >= 0 ? sceIoRead(fd, text, sizeof text - 1) : 0;
    const char *p = text;
    u32 count;

    if (fd >= 0)
        sceIoClose(fd);
    text[n > 0 ? n : 0] = 0;
    count = number(&p, 10);
    if (count != seen && presses < 256) {
        seen = count;
        press[presses].from = now();
        press[presses].to = now() + 200;
        press[presses].buttons = number(&p, 16);
        presses++;
    }
}

static int film(SceSize args, void *argp)
{
    static u16 part[480 * 34];   /* an eighth of the screen: the kernel has no room for a whole one */
    char name[] = "host0:/film0.raw";   /* a new file whenever the USB bus restarted under the old one */
    SceUID fd = -1;

    for (;;) {
        void *top = NULL;
        int width = 0, format = 0, k1 = pspSdkSetK1(0), x, y;
        u32 at = now();

        sceDisplayGetFrameBuf(&top, &width, &format, 0);
        pspSdkSetK1(k1);
        if (fd < 0 && name[11] <= '9') {
            fd = sceIoOpen(name, PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0777);
            if (fd >= 0)
                name[11]++;
        }
        if (fd >= 0 && top && width >= 480) {
            int bad = sceIoWrite(fd, &at, 4) != 4;

            for (y = 0; y < 272; y++) {
                u8 *line = (u8 *)(0x40000000 | (u32)top) + y * width * (format == 3 ? 4 : 2);
                u16 *to = part + (y % 34) * 480;

                if (format == 3)
                    for (x = 0; x < 480; x++) {
                        u32 p = ((u32 *)line)[x];
                        to[x] = (p >> 3 & 31) | (p >> 10 & 63) << 5 | (p >> 19 & 31) << 11;
                    }
                else
                    memcpy(to, line, 480 * 2);
                if (y % 34 == 33 && !bad)
                    bad = sceIoWrite(fd, part, sizeof part) != sizeof part;
            }
            if (bad) {
                sceIoClose(fd);
                fd = -1;
            }
        }
        live();
        at = now() - at;
        sceKernelDelayThread((at < (u32)film_every ? film_every - at : 1) * 1000);
    }
    return 0;
}

int module_start(SceSize args, void *argp)
{
    u32 read = sctrlHENFindFunction("sceController_Service", "sceCtrl", 0x1F803938);
    u32 peek = sctrlHENFindFunction("sceController_Service", "sceCtrl", 0x3A622550);

    started = sceKernelGetSystemTimeLow();
    timetable();
    if (read && peek) {
        read_real = (void *)read;
        peek_real = (void *)peek;
        sctrlHENPatchSyscall((void *)read, on_read);
        sctrlHENPatchSyscall((void *)peek, on_peek);
    }
    if (film_every > 0)
        sceKernelStartThread(sceKernelCreateThread("pad_film", film, 40, 0x4000, 0, NULL), 0, NULL);
    if (shot_every > 0)
        sceKernelStartThread(sceKernelCreateThread("pad_shots", shots, 40, 0x4000, 0, NULL), 0, NULL);
    return 0;
}
