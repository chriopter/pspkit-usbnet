#!/usr/bin/env python3
"""Exercise release.sh promotion/rollback without Rust, USB or network."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

source = Path(__file__).resolve().parents[1] / 'release.sh'
with tempfile.TemporaryDirectory(prefix='usbnet-release-check-') as temporary:
    root = Path(temporary)
    shutil.copy2(source, root / 'release.sh')
    (root / 'THIRD_PARTY_LIBUSB_LICENSE').write_text('test licence')
    (root / 'bin').mkdir()
    tools = {
        'rustup': '#!/bin/sh\nexit 0\n',
        'uname': '#!/bin/sh\necho Linux\n',
        'cargo': '''#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
assert '--locked' in args and '--release' in args
target = args[args.index('--target') + 1]
if target == os.environ.get('FAIL_TARGET'): sys.exit(7)
p = pathlib.Path('gateway/target') / target / 'release' / ('pspkit-usbnetd.exe' if 'windows' in target else 'pspkit-usbnetd')
p.parent.mkdir(parents=True, exist_ok=True)
p.write_text('test executable ' + target)
''',
    }
    for name, text in tools.items():
        path = root / 'bin' / name
        path.write_text(text)
        path.chmod(0o755)
    env = dict(os.environ, PATH=str(root / 'bin') + os.pathsep + os.environ['PATH'])
    dist = root / 'dist'
    dist.mkdir()
    (dist / 'previous').write_text('keep me if compilation fails')
    result = subprocess.run(['bash', 'release.sh'], cwd=root,
                            env=dict(env, FAIL_TARGET='aarch64-unknown-linux-musl'), capture_output=True)
    assert result.returncode == 7, result.stderr
    assert (dist / 'previous').read_text() == 'keep me if compilation fails'
    assert not list(root.glob('.dist-*')), 'failed build left staging directories'
    result = subprocess.run(['bash', 'release.sh'], cwd=root, env=env, capture_output=True)
    assert result.returncode == 0, result.stderr
    assert not (dist / 'previous').exists()
    sums = (dist / 'SHA256SUMS-linux').read_text().splitlines()
    assert len(sums) == 3
    for line in sums:
        expected, name = line.split()
        assert hashlib.sha256((dist / name).read_bytes()).hexdigest() == expected
    assert not list(root.glob('.dist-*')), 'successful build left staging directories'
print('PASS: failed compilation retains previous release; successful build promotes all three artifacts and valid checksums')
