/* 6.61 vshmain's menu has 35 {category, item, model mask, region mask}
 * records. Bit 10 hides Network Settings on the Street. Its constructor
 * SHA-1 checks the entire table and transforms it if the digest differs:
 * changing the mask alone corrupts the menu. A second table filters the
 * icon's three textures. Validate the menu, icon record and stored digest
 * before changing the two masks and the matching menu digest.
 * No executable instructions, model queries or files are patched.
 *
 * Offsets are from the decrypted 6.61 updater. Unrecognized layouts and
 * tables already changed by another plugin are deliberately left alone.
 */
#include <stddef.h>
#include <stdint.h>
#include <string.h>

enum {
    TABLE = 0x54b20, ICON = 0x55084, HASH = 0x556d0, NAME = 0x55c87,
    ROWS = 35, NETWORK = 13, STREET_BIT = 0x400,
    IMAGE_MIN = NAME + sizeof("msgtop_sysconf_network")
};

static const uint32_t original[ROWS][4] = {
    {0, 0x00, 0x7ff, 0xffffffff}, {0, 0x01, 0x7ff, 0xffffffff},
    {0, 0x02, 0x7ff, 0xffffffff}, {0, 0x03, 0x7ff, 0xffffffff},
    {0, 0x04, 0x7ff, 0xffffffff}, {0, 0x05, 0x7ff, 0xffffffff},
    {0, 0x06, 0x7ff, 0xffffffff}, {0, 0x07, 0x7ff, 0xffffffff},
    {0, 0x08, 0x2b0, 0xffffffff}, {0, 0x09, 0x3fe, 0xffffffff},
    {0, 0x0a, 0x7ff, 0xffffffff}, {0, 0x0b, 0x7ff, 0xffffffff},
    {0, 0x0c, 0x3ff, 0xffffffff}, {0, 0x0d, 0x3ff, 0xffffffff},
    {2, 0x23, 0x7ff, 0xffffffff}, {3, 0x32, 0x3ff, 0x2b0},
    {3, 0x2f, 0x7ff, 0xffffffff}, {1, 0x27, 0x3fe, 0x8},
    {1, 0x2d, 0x3fe, 0x40},       {1, 0x30, 0x7ff, 0x8},
    {1, 0x31, 0x7ff, 0x2b0},     {1, 0x33, 0x3ff, 0x8},
    {5, 0x17, 0x3ff, 0xffffffff}, {5, 0x18, 0x54f, 0xffffffff},
    {6, 0x25, 0x3ff, 0xffffffff}, {6, 0x21, 0x3ff, 0xffffffff},
    {6, 0x26, 0x3fe, 0xffffffff}, {6, 0x24, 0x3ff, 0xffffffff},
    {6, 0x28, 0x3ff, 0xffffffff}, {6, 0x22, 0x3ff, 0xffffffff},
    {6, 0x20, 0x3ff, 0xffffffff}, {6, 0x29, 0x3ff, 0xffffffff},
    {7, 0x2a, 0x3ff, 0xffffffff}, {7, 0x2c, 0x3ff, 0xffffffff},
    {7, 0x2e, 0x3ff, 0xffffffff},
};

static const unsigned char digest[2][20] = {
    {0xb8,0x5b,0x9f,0x5b,0x56,0x10,0x6e,0xe0,0xcb,0xca,
     0x2f,0xd8,0x03,0xf3,0x45,0x3e,0x12,0x42,0xbc,0xc1},
    {0x68,0x51,0x28,0xcb,0x55,0xf0,0x76,0x51,0x04,0x50,
     0x67,0x73,0xdd,0x05,0xc2,0x9c,0x27,0x73,0x76,0x90},
};

/* The texture loader has its own model filter, separate from the menu.
 * The three names select the original normal/dim/glowing icon textures. */
static const uint32_t original_icon[5] = {
    0x00004443, 0x00005243, 0x00004644, 0x3ff, 0xffffffff,
};

/* Shared with the PC test. Only these checked data ranges are accessed. */
int xmb_network_menu_edit(unsigned char *image, size_t size, int enable)
{
    uint32_t (*table)[4];
    unsigned i, j;

    if (!image || ((uintptr_t)image & 3) || size < IMAGE_MIN ||
        (enable != 0 && enable != 1))
        return 0;
    table = (void *)(image + TABLE);
    if (memcmp(image + NAME, "msgtop_sysconf_network", sizeof("msgtop_sysconf_network")) ||
        memcmp(image + HASH, digest[!enable], sizeof digest[0]))
        return 0;
    for (i = 0; i < ROWS; i++)
        for (j = 0; j < 4; j++) {
            uint32_t want = original[i][j];

            if (!enable && i == NETWORK && j == 2)
                want |= STREET_BIT;
            if (table[i][j] != want)
                return 0;
        }
    for (i = 0; i < 5; i++) {
        uint32_t want = original_icon[i], value;

        if (!enable && i == 3)
            want |= STREET_BIT;
        memcpy(&value, image + ICON + 4 * i, sizeof value);
        if (value != want)
            return 0;
    }
    table[NETWORK][2] = original[NETWORK][2] | (enable ? STREET_BIT : 0);
    ((uint32_t *)(image + ICON))[3] = original_icon[3] | (enable ? STREET_BIT : 0);
    memcpy(image + HASH, digest[enable], sizeof digest[0]);
    return 1;
}

#ifndef USBNET_HOST_TEST
#include <pspkernel.h>
#include <pspsdk.h>
#include <systemctrl.h>
#include "xmb.h"

static SceUID patched_uid = -1;
static u32 patched_base;

static int contains(SceModule *module, unsigned offset, unsigned length)
{
    unsigned i;
    u32 address = module->text_addr + offset;

    if (address < module->text_addr)
        return 0;
    for (i = 0; i < module->nsegment && i < 4; i++)
        if (address >= module->segmentaddr[i] &&
            address - module->segmentaddr[i] <= module->segmentsize[i] &&
            length <= module->segmentsize[i] - (address - module->segmentaddr[i]))
            return 1;
    return 0;
}

static int edit_module(SceModule *module, int enable)
{
    int changed, interrupts;

    if (!contains(module, TABLE, sizeof original) ||
        !contains(module, ICON, sizeof original_icon) ||
        !contains(module, HASH, sizeof digest[0]) ||
        !contains(module, NAME, sizeof("msgtop_sysconf_network")))
        return 0;
    interrupts = pspSdkDisableInterrupts();
    changed = xmb_network_menu_edit((void *)module->text_addr, IMAGE_MIN, enable);
    if (changed) {
        sceKernelDcacheWritebackInvalidateRange((void *)(module->text_addr + TABLE), sizeof original);
        sceKernelDcacheWritebackInvalidateRange((void *)(module->text_addr + ICON), sizeof original_icon);
        sceKernelDcacheWritebackInvalidateRange((void *)(module->text_addr + HASH), sizeof digest[0]);
    }
    pspSdkEnableInterrupts(interrupts);
    return changed;
}

void xmb_module_start(SceModule *module)
{
    int (*model)(void);

    if (strcmp(module->modname, "vsh_module"))
        return;
    model = (void *)sctrlHENFindFunction("sceSystemMemoryManager", "SysMemForKernel", 0x6373995D);
    if (!model)
        model = (void *)sctrlHENFindFunction("sceSystemMemoryManager", "SysMemForKernel", 0x07C586A1);
    if (!model || model() != 10)
        return;
    /* Called by the loader before XMB startup, never on an already running
     * XMB: its menu lists would need rebuilding and are not touched here. */
    if (edit_module(module, 1)) {
        patched_uid = module->modid;
        patched_base = module->text_addr;
    }
}

void xmb_stop(void)
{
    SceModule *module = sceKernelFindModuleByName("vsh_module");

    if (module && module->modid == patched_uid && module->text_addr == patched_base)
        edit_module(module, 0);
    patched_uid = -1;
    patched_base = 0;
}
#endif
