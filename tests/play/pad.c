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
 * lines as they are in video memory).
 */
#include <pspkernel.h>
#include <pspctrl.h>
#include <pspdisplay.h>
#include <systemctrl.h>
#include <string.h>

PSP_MODULE_INFO("pad", PSP_MODULE_KERNEL, 1, 0);

static struct { u32 from, to, buttons; } press[256];
static int presses, shot_every;
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
    if (shot_every > 0)
        sceKernelStartThread(sceKernelCreateThread("pad_shots", shots, 40, 0x4000, 0, NULL), 0, NULL);
    return 0;
}
