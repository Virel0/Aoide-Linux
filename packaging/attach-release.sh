#!/usr/bin/env bash
# Attaches files to the GitHub release for TAG, creating the release from the
# tag's own message first if there isn't one yet. Several release jobs run at
# once for one tag, so creating may lose a race to another job: that's fine,
# as long as the release then exists. Files that don't exist are skipped
# (signatures, before a release key is set up).
#
#   GH_TOKEN, GH_REPO       set by the workflow
#   packaging/attach-release.sh TAG FILE...
set -euo pipefail

tag=$1
shift
files=()
for file in "$@"; do
    [[ -e $file ]] && files+=("$file")
done
if ((${#files[@]} == 0)); then
    echo "::error::nothing to attach to $tag"
    exit 1
fi

if ! gh release view "$tag" >/dev/null 2>&1; then
    gh release create "$tag" --verify-tag --title "$tag" --notes-from-tag \
        || gh release view "$tag" >/dev/null
fi
gh release upload "$tag" "${files[@]}" --clobber
printf 'attached %s\n' "${files[@]}"
