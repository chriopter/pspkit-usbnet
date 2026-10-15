#!/usr/bin/env bash
# Build standalone gateways. Linux: static x86_64 + ARM64 and Windows x86_64
# via cargo-zigbuild. macOS: universal Apple Silicon + Intel.
# Failed builds preserve the preceding dist/ directory.
set -euo pipefail
cd "$(dirname "$0")"

case "$(uname -s)" in
    Darwin) cargo=(cargo build) ;;
    Linux) cargo=(cargo zigbuild) ;;
    *) echo 'Build on Linux or macOS; native Windows builds use cargo build --release.' >&2; exit 1 ;;
esac
if [ -L dist ] || { [ -e dist ] && [ ! -d dist ]; }; then
    echo 'dist must be a directory, not a symlink or file.' >&2
    exit 1
fi
stage=$(mktemp -d "$PWD/.dist-build.XXXXXX")
previous=
cleanup() {
    local status=$?
    if [ -n "$previous" ] && [ -d "$previous/dist" ] && [ ! -e dist ]; then
        mv "$previous/dist" dist
    fi
    [ ! -d "$stage" ] || rm -rf -- "$stage"
    [ -z "$previous" ] || rm -rf -- "$previous"
    exit "$status"
}
trap cleanup EXIT

build() {
    rustup target add "$1" >/dev/null
    "${cargo[@]}" --locked --release --manifest-path gateway/Cargo.toml --target "$1"
    local exe=gateway/target/$1/release/pspkit-usbnetd
    [ ! -f "$exe.exe" ] || exe=$exe.exe
    cp "$exe" "$stage/$2"
}

if [ "$(uname -s)" = Darwin ]; then
    build x86_64-apple-darwin x64
    build aarch64-apple-darwin arm64
    lipo -create "$stage/x64" "$stage/arm64" -output "$stage/pspkit-usbnetd-macos"
    rm "$stage/x64" "$stage/arm64"
else
    build x86_64-unknown-linux-musl pspkit-usbnetd-linux-x86_64
    build aarch64-unknown-linux-musl pspkit-usbnetd-linux-aarch64
    build x86_64-pc-windows-gnu pspkit-usbnetd-windows-x86_64.exe
fi
cp THIRD_PARTY_LIBUSB_LICENSE "$stage/"
(cd "$stage" && shasum -a 256 pspkit-usbnetd-* > "SHA256SUMS-$(uname -s | tr '[:upper:]' '[:lower:]')")
previous=$(mktemp -d "$PWD/.dist-previous.XXXXXX")
[ ! -d dist ] || mv dist "$previous/dist"
mv "$stage" dist
cat "dist/SHA256SUMS-$(uname -s | tr '[:upper:]' '[:lower:]')"
