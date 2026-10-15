#!/usr/bin/env python3
"""Test the RAM menu edit against a locally decrypted, flattened 6.61 vshmain.

Usage: python3 tests/xmb-menu.py /path/to/vshmain.bin
Firmware is intentionally not included. No PSP or network access is used.
"""
import ctypes
import hashlib
import pathlib
import struct
import subprocess
import sys
import tempfile

source = pathlib.Path(__file__).resolve().parents[1] / "psp/xmb.c"
original = pathlib.Path(sys.argv[1]).read_bytes()
table, table_size, digest = 0x54B20, 0x230, 0x556D0
mask = table + 13 * 16 + 8
icon = 0x55084
minimum = 0x55C87 + len(b"msgtop_sysconf_network\0")
assert len(original) >= minimum
assert hashlib.sha1(original[table:table + table_size]).digest() == original[digest:digest + 20]

with tempfile.TemporaryDirectory(prefix="usbnet-xmb-") as temporary:
    library = pathlib.Path(temporary) / "xmb.so"
    subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                    "-shared", "-fPIC", "-DUSBNET_HOST_TEST", str(source),
                    "-o", str(library)], check=True)
    edit = ctypes.CDLL(str(library)).xmb_network_menu_edit
    edit.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
    edit.restype = ctypes.c_int

    def run(data, enable=1, size=None):
        buffer = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
        result = edit(buffer, len(data) if size is None else size, enable)
        return result, bytes(buffer)

    result, patched = run(original)
    assert result == 1
    expected = bytearray(original)
    struct.pack_into("<I", expected, mask, 0x7FF)
    struct.pack_into("<I", expected, icon + 12, 0x7FF)
    expected[digest:digest + 20] = hashlib.sha1(expected[table:table + table_size]).digest()
    assert patched == expected, "only the Network Settings masks and menu SHA-1 may change"
    assert run(patched) == (0, patched), "repeated enable must leave the image alone"
    assert run(patched, 0) == (1, original), "unload must restore every byte"
    assert run(original, 0) == (0, original)

    offsets = (list(range(table, table + table_size)) + list(range(icon, icon + 20)) +
               list(range(digest, digest + 20)) + [0x55C87])
    for offset in offsets:
        corrupt = bytearray(original)
        corrupt[offset] ^= 1
        assert run(corrupt) == (0, corrupt), f"unexpected byte accepted at {offset:x}"
        corrupt = bytearray(patched)
        corrupt[offset] ^= 1
        assert run(corrupt, 0) == (0, corrupt), f"another patch overwritten at {offset:x}"
    for size in (0, table - 1, digest, minimum - 1):
        assert run(original, size=size) == (0, original)
    for invalid in (-1, 2):
        assert run(original, invalid) == (0, original)
    assert edit(None, len(original), 1) == 0
    print("PASS: menu + texture masks, SHA-1, byte-for-byte restoration, idempotence,")
    print(f"      {2 * len(offsets)} corruption cases, truncated images, null input and invalid modes")
