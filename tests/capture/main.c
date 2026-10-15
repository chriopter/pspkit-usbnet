/* Bounded recording using the reviewed safe-shot userspace reader.
 * Static owned RAM; read-only VRAM access; no display hooks or GU calls.
 * Complete frame header and pixels are written together to the Memory Stick.
 * Explicit thread exit avoids the SDK CRT's automatic ExitGame. */
#include <pspkernel.h>
#include <pspdisplay.h>
#include <pspiofilemgr.h>
#include <string.h>
#include <stdlib.h>
PSP_MODULE_INFO("safe_capture", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_MAIN_THREAD_STACK_SIZE_KB(16);
PSP_HEAP_SIZE_KB(0);
static struct { unsigned magic, time, stride, format; unsigned pixels[512*272]; } frame;
static int write_all(int fd, const void *data, int len)
{
    int at = 0;
    while (at < len) { int n = sceIoWrite(fd, (const char *)data + at, len - at); if (n <= 0) return -1; at += n; }
    return 0;
}
int main(int argc, char **argv)
{
    unsigned start = sceKernelGetSystemTimeLow(), limit = 180000000;
    int fd;
    if (argc > 1) { int seconds = atoi(argv[1]); if (seconds > 0 && seconds <= 180) limit = seconds * 1000000u; }
    fd = sceIoOpen("ms0:/PSP/usbnet-capture.raw", PSP_O_WRONLY | PSP_O_CREAT | PSP_O_TRUNC, 0600);
    if (fd >= 0) {
        while ((unsigned)(sceKernelGetSystemTimeLow() - start) < limit) {
            void *top = NULL; int stride = 0, format = -1;
            unsigned at;
            sceDisplayWaitVblankStart(); at = sceKernelGetSystemTimeLow();
            if (sceDisplayGetFrameBuf(&top, &stride, &format, PSP_DISPLAY_SETBUF_IMMEDIATE) >= 0 && top && stride == 512 && format >= 0 && format <= 3) {
                int len = 512 * 272 * (format == PSP_DISPLAY_PIXEL_FORMAT_8888 ? 4 : 2);
                frame.magic = 0x43415031; frame.time = at; frame.stride = stride; frame.format = format;
                memcpy(frame.pixels, (const void *)((unsigned)top | 0x40000000u), len);
                if (write_all(fd, &frame, 16 + len) < 0) break;
            }
            at = sceKernelGetSystemTimeLow() - at;
            if (at < 200000) sceKernelDelayThread(200000 - at);
        }
        sceIoClose(fd);
    }
    sceKernelExitDeleteThread(0);
    for (;;) sceKernelSleepThread();
}
