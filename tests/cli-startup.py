#!/usr/bin/env python3
"""Smoke-test the actual gateway binary on an isolated CI runner without PSPs.

Do not run on a hardware bench: this starts the USB gateway.
Usage: python3 tests/cli-startup.py /path/to/pspkit-usbnetd
"""
import os
from pathlib import Path
import subprocess
import sys
import time

exe = str(Path(sys.argv[1]).resolve())
env = dict(os.environ, TERM='dumb', NO_COLOR='1')
process = subprocess.Popen([exe], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, env=env)
try:
    time.sleep(2)
    if process.poll() is not None:
        out, err = process.communicate()
        # Hosted Linux VMs may expose no USB controllers. In that environment
        # libusb's static usbfs backend cannot initialize; verify the diagnostic.
        usbfs = Path('/dev/bus/usb')
        no_usb = sys.platform.startswith('linux') and (not usbfs.exists() or not any(usbfs.iterdir()))
        if no_usb:
            assert process.returncode == 1, (out, err)
            assert b'USB initialization failed' in err, (out, err)
            assert b'libusb does not start' in err, (out, err)
            assert b'panicked' not in err and b'\x1b' not in out, (out, err)
            print('PASS: actual release binary reports unavailable USB on this runner')
            sys.exit(0)
        raise AssertionError(f'gateway exited during USB startup: {(out, err)}')
    second = subprocess.run([exe], stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=10)
    assert second.returncode == 1, f'second instance: {second.returncode}'
    assert b'already running' in second.stderr, second.stderr
finally:
    if process.poll() is None:
        process.terminate()
    try:
        out, err = process.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        out, err = process.communicate()
        raise RuntimeError('gateway would not terminate')
assert b'USB' in out and b'ok' in out, (out, err)
assert b'waiting:' in out, (out, err)
assert b'\x1b' not in out, 'escape codes in noninteractive output'
assert b'panicked' not in err, err
print('PASS: actual release binary, libusb startup, plain status, singleton and termination')
