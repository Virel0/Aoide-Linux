#!/usr/bin/env bash
#
# Build Aoide for Linux as a .deb from an already built binary.
#
# Usage: packaging/deb/build-deb.sh BINARY VERSION OUTDIR
#   BINARY   target/release/aoide (built with `cargo build --release -p aoide-app`)
#   VERSION  e.g. 1.2.3; the file is OUTDIR/aoide-linux_VERSION_amd64.deb
#
# Run by .github/workflows/release-appimage-deb.yml. A plain dpkg-deb build
# from packaging/deb/control rather than cargo-deb: cargo-deb reads its
# settings from the crate's Cargo.toml, and this keeps packaging out of the
# crates. The binary is installed as it is given; CI strips it when building.
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

_work="$(mktemp -d)"
trap 'rm -rf -- "$_work"' EXIT
_root="$_work/root"

# install(1) without GNU's -D, so this also runs where install has no -D.
put() {
    local mode=$1 from=$2 to=$3
    install -d -m 755 -- "$(dirname -- "$_root/$to")"
    install -m "$mode" -- "$from" "$_root/$to"
}

put 755 "$_binary" usr/bin/aoide-linux
put 644 "$_repo/packaging/arch/$_appid.desktop" "usr/share/applications/$_appid.desktop"
put 644 "$_repo/packaging/flatpak/$_appid.metainfo.xml" "usr/share/metainfo/$_appid.metainfo.xml"
for _size in 16 32 48 64 128 256 512; do
    put 644 "$_repo/packaging/arch/icons/${_size}x${_size}.png" \
        "usr/share/icons/hicolor/${_size}x${_size}/apps/$_appid.png"
done
put 644 "$_here/copyright" usr/share/doc/aoide-linux/copyright
put 644 "$_repo/crates/aoide-app/fonts/Inter-LICENSE.txt" usr/share/doc/aoide-linux/Inter-LICENSE.txt

_size_kib="$(du -sk -- "$_root/usr" | cut -f1)"
install -d -m 755 -- "$_root/DEBIAN"
grep -v '^#' -- "$_here/control" \
    | sed -e "s/@VERSION@/$_version/" -e "s/@INSTALLED_SIZE@/$_size_kib/" \
    > "$_root/DEBIAN/control"
chmod 644 "$_root/DEBIAN/control"

mkdir -p -- "$_outdir"
_out="$_outdir/aoide-linux_${_version}_amd64.deb"
# xz rather than dpkg's newer zstd default, which older dpkg cannot read.
dpkg-deb --root-owner-group -Zxz --build "$_root" "$_out"
echo "==> Wrote $_out"
