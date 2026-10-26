# Releases: signatures and checksums

Every release carries `SHA256SUMS-arch`, `SHA256SUMS-appimage-deb` and
`SHA256SUMS-flatpak`. Once a release key is set up, each file also has a
detached OpenPGP signature beside it (`FILE.sig`): the pacman packages and
repository database, the AppImage, the deb, the Flatpak bundle and the
checksum files. The signing happens in the release jobs, which only download
the built files, sign and attach them; the jobs that compile never see the
key (`packaging/sign.sh`).

## Setting up the release key (once)

On your own computer, not a server:

```
gpg --quick-gen-key "Aoide for Linux releases" ed25519 sign 3y
gpg --list-secret-keys --keyid-format long    # note the fingerprint
gpg --armor --export-secret-keys FINGERPRINT > aoide-release-key.asc
gh secret set AOIDE_SIGNING_KEY --repo Virel0/Aoide-Linux < aoide-release-key.asc
gh secret set AOIDE_SIGNING_PASSPHRASE --repo Virel0/Aoide-Linux   # if it has one
shred -u aoide-release-key.asc 2>/dev/null || rm -P aoide-release-key.asc
gpg --armor --export FINGERPRINT > packaging/signing-key.asc
```

Commit `packaging/signing-key.asc` (the public half) and put the fingerprint
in this file, so people can check they have the right key. Keep a backup of
the secret key somewhere offline; a lost key means a new one, and everyone
importing it again.

Until the secret is set, releases go out unsigned and the release job says so.

## pacman

```
sudo pacman-key --add packaging/signing-key.asc     # or the file from the release
sudo pacman-key --lsign-key FINGERPRINT
```

and in `/etc/pacman.conf`:

```
[aoide-linux]
SigLevel = Required DatabaseRequired
Server = https://github.com/Virel0/Aoide-Linux/releases/latest/download
```

## The other files

```
gpg --import signing-key.asc
gpg --verify aoide-linux.flatpak.sig aoide-linux.flatpak
gpg --verify SHA256SUMS-appimage-deb.sig SHA256SUMS-appimage-deb
sha256sum --check --ignore-missing SHA256SUMS-appimage-deb
```
