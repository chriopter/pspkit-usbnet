#!/usr/bin/env bash
# release.sh [version] : builds pspkit-usbnetd as single executables that need
# nothing installed, into dist/. On Linux: Linux x86_64 and aarch64 (static,
# musl) and Windows x86_64, cross-compiled with zig. On macOS: one universal
# binary. libusb is compiled in on all of them.
#
# Needs rustup; on Linux also zig and cargo-zigbuild (pip install ziglang cargo-zigbuild).
set -euo pipefail
cd "$(dirname "$0")"
version=${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' gateway/Cargo.toml | head -1)}
rm -rf dist && mkdir dist

build() { # target, name in dist/
    rustup target add "$1" >/dev/null
    $cargo --release --manifest-path gateway/Cargo.toml --target "$1"
    local exe=gateway/target/$1/release/pspkit-usbnetd
    [ -f "$exe.exe" ] && exe=$exe.exe
    cp "$exe" "dist/$2"
}

if [ "$(uname)" = Darwin ]; then
    cargo="cargo build"
    build x86_64-apple-darwin x64 && build aarch64-apple-darwin arm64
    lipo -create dist/x64 dist/arm64 -output "dist/pspkit-usbnetd-$version-macos"
    rm dist/x64 dist/arm64
else
    cargo="cargo zigbuild"
    build x86_64-unknown-linux-musl "pspkit-usbnetd-$version-linux-x86_64"
    build aarch64-unknown-linux-musl "pspkit-usbnetd-$version-linux-aarch64"
    build x86_64-pc-windows-gnu "pspkit-usbnetd-$version-windows-x86_64.exe"
fi
cp THIRD_PARTY_LIBUSB_LICENSE dist/
( cd dist && shasum -a 256 pspkit-usbnetd-* | tee "SHA256SUMS-$(uname -s | tr A-Z a-z)" )
