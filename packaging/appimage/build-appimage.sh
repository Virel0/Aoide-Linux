#!/usr/bin/env bash
#
# Build Aoide for Linux as an AppImage from an already built binary.
#
# Usage: packaging/appimage/build-appimage.sh BINARY VERSION OUTDIR
#   BINARY   target/release/aoide (built with `cargo build --release -p aoide-app`)
#   VERSION  e.g. 1.2.3; the file is OUTDIR/aoide-linux-VERSION-x86_64.AppImage
#
# Linux x86_64 only. Run by .github/workflows/release-appimage-deb.yml.
#
# The tool is appimagetool (AppImage/appimagetool, MIT, maintained by the
# AppImage project), with the static type2 runtime (AppImage/type2-runtime,
# MIT), both pinned to a release and checked against the SHA-256 GitHub
# publishes for it. linuxdeploy is not used: what it adds over appimagetool is
# copying the libraries the binary links, and this binary links nothing an
# AppImage should carry:
#
#   libc, libm, libgcc_s, libasound  on the AppImage exclude list: they must
#                                    be the system's (glibc, the sound setup)
#   libdbus-1                        on every desktop with a session bus,
#                                    which the app needs anyway
#
# Everything else the window needs (Wayland or X11, xkbcommon, Vulkan or
# EGL) is loaded at run time and must also be the system's, to match the
# display server and the GPU driver. The release workflow fails if the binary
# starts linking anything else, so this list is not silently outgrown.
#
# The binary's glibc is the oldest it runs on, so CI builds on Ubuntu 22.04
# (glibc 2.35).
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 BINARY VERSION OUTDIR" >&2
    exit 2
fi
_binary=$1
_version=$2
_outdir=$3

_here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
_repo="$(cd -- "$_here/../.." && pwd)"
_appid='io.github.Virel0.Aoide.Linux'

_tool_url='https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage'
_tool_sha256='ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0'
_runtime_url='https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64'
_runtime_sha256='2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d'

_cache="${XDG_CACHE_HOME:-$HOME/.cache}/aoide-linux-appimage"
mkdir -p -- "$_cache" "$_outdir"

# Download URL to the cache unless a file with the expected hash is there.
fetch() {
    local url=$1 sha256=$2 dest
    dest="$_cache/$sha256-$(basename -- "$url")"
    if [[ ! -f $dest ]] || ! echo "$sha256  $dest" | sha256sum --check --status; then
        curl --fail --silent --show-error --location --retry 3 --output "$dest.part" "$url"
        if ! echo "$sha256  $dest.part" | sha256sum --check --status; then
            echo "error: $url does not match its pinned SHA-256" >&2
            rm -f -- "$dest.part"
            exit 1
        fi
        mv -- "$dest.part" "$dest"
    fi
    printf '%s\n' "$dest"
}

_tool="$(fetch "$_tool_url" "$_tool_sha256")"
_runtime="$(fetch "$_runtime_url" "$_runtime_sha256")"
chmod +x -- "$_tool"

_work="$(mktemp -d)"
trap 'rm -rf -- "$_work"' EXIT
_appdir="$_work/AppDir"

install -Dm755 "$_binary" "$_appdir/usr/bin/aoide-linux"
install -Dm755 "$_here/AppRun" "$_appdir/AppRun"
install -Dm644 "$_repo/packaging/arch/$_appid.desktop" "$_appdir/usr/share/applications/$_appid.desktop"
install -Dm644 "$_repo/packaging/flatpak/$_appid.metainfo.xml" "$_appdir/usr/share/metainfo/$_appid.metainfo.xml"
for _size in 16 32 48 64 128 256 512; do
    install -Dm644 "$_repo/packaging/arch/icons/${_size}x${_size}.png" \
        "$_appdir/usr/share/icons/hicolor/${_size}x${_size}/apps/$_appid.png"
done
install -Dm644 "$_repo/LICENSE" "$_appdir/usr/share/doc/aoide-linux/LICENSE"
install -Dm644 "$_repo/crates/aoide-app/fonts/Inter-LICENSE.txt" \
    "$_appdir/usr/share/doc/aoide-linux/Inter-LICENSE.txt"
# appimagetool wants the entry and its icon at the top of the AppDir too.
ln -s "usr/share/applications/$_appid.desktop" "$_appdir/$_appid.desktop"
ln -s "usr/share/icons/hicolor/256x256/apps/$_appid.png" "$_appdir/$_appid.png"

_out="$_outdir/aoide-linux-$_version-x86_64.AppImage"
# APPIMAGE_EXTRACT_AND_RUN: appimagetool is itself an AppImage, and CI
# machines and containers often have no FUSE to mount it with.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 VERSION="$_version" \
    "$_tool" --runtime-file "$_runtime" "$_appdir" "$_out"
echo "==> Wrote $_out"
