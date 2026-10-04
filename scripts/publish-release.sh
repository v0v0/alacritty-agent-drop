#!/usr/bin/env bash
set -euo pipefail
: "${RELEASE_TAG:?}" "${PRERELEASE:?}" "${RUNNER_TEMP:?}"
# Do not publish a moved tag against binaries from a different commit.
expected="$(git rev-parse HEAD)"
git fetch --force origin "refs/tags/$RELEASE_TAG:refs/tags/$RELEASE_TAG"
test "$(git rev-parse "$RELEASE_TAG^{commit}")" = "$expected"
if gh release view "$RELEASE_TAG" --json isDraft --jq .isDraft > "$RUNNER_TEMP/release-draft"; then
  if [ "$(cat "$RUNNER_TEMP/release-draft")" != true ]; then
    echo "Release already published; refusing to overwrite existing assets."
    exit 1
  fi
else
  flags=()
  if [ "$PRERELEASE" = true ]; then flags+=(--prerelease --latest=false); fi
  gh release create "$RELEASE_TAG" --verify-tag --draft --generate-notes \
    --title "agentdrop $RELEASE_TAG" "${flags[@]}"
fi
# Failed draft uploads can be retried; a published release is never overwritten.
gh release upload "$RELEASE_TAG" dist/* --clobber
flags=()
if [ "$PRERELEASE" = true ]; then flags+=(--latest=false); fi
gh release edit "$RELEASE_TAG" --draft=false --prerelease="$PRERELEASE" "${flags[@]}"
