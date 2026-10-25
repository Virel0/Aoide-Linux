#!/usr/bin/env bash
# Signs release files with the release key: a detached OpenPGP signature,
# FILE.sig, beside each FILE. pacman reads FILE.sig for packages and the
# repository database; the rest are checked with `gpg --verify FILE.sig FILE`.
#
#   AOIDE_SIGNING_KEY         the armored secret key (a repository secret)
#   AOIDE_SIGNING_PASSPHRASE  its passphrase, if it has one
#
#   packaging/sign.sh FILE...
#
# Run only in the release jobs, which build nothing: no crate's build script
# ever runs where the key is. Without a key it says so and signs nothing.
set -euo pipefail

if [[ -z ${AOIDE_SIGNING_KEY:-} ]]; then
    echo "::notice::No AOIDE_SIGNING_KEY secret: the release goes out unsigned (docs/releases.md)."
    exit 0
fi
if (($# == 0)); then
    echo "usage: $0 FILE..." >&2
    exit 2
fi

GNUPGHOME="$(mktemp -d)"
export GNUPGHOME
trap 'gpgconf --kill gpg-agent 2>/dev/null || true; rm -rf "$GNUPGHOME"' EXIT
chmod 700 "$GNUPGHOME"

printf '%s\n' "$AOIDE_SIGNING_KEY" | gpg --batch --quiet --import
fingerprint="$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "fpr" { print $10; exit }')"
echo "Signing with ${fingerprint}"

for file in "$@"; do
    gpg --batch --yes --pinentry-mode loopback \
        --passphrase "${AOIDE_SIGNING_PASSPHRASE:-}" \
        --local-user "$fingerprint" \
        --detach-sign --output "$file.sig" "$file"
    gpg --batch --quiet --verify "$file.sig" "$file"
    echo "signed $file"
done
