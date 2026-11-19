#!/usr/bin/env bash
# Writes the notes of a stable release on stdout (docs/RELEASES.md, rules 3 to 5).
#
#   release-notes.sh android-v1.2.3 > notes.md
#
# Environment (all optional except where stated):
#   GITHUB_REPOSITORY      owner/name, used in the verification commands (default: tomi-share/tomi)
#   RELEASE_SHA            the commit the release was built from (default: the commit of the tag)
#   RELEASE_RUN_URL        URL of the workflow run that built the files
#   ANDROID_CERT_SHA256    SHA-256 fingerprint of the signing certificate (Android; public information)
#   ASSET_APK / ASSET_EXE  published file names, for the verification commands
#
# The notes carry no secret: commit subjects of a public repository, public fingerprints and commands. Run it where the
# previous tags are fetched (`git fetch --tags`, full history) so that the change list can start at the previous release.

set -euo pipefail

tag="${1:?usage: release-notes.sh <android-vX.Y.Z|windows-vX.Y.Z>}"
repo="${GITHUB_REPOSITORY:-tomi-share/tomi}"
sha="${RELEASE_SHA:-$(git rev-list -n 1 "$tag")}"

if [[ "$tag" =~ ^(android|windows)-v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  product="${BASH_REMATCH[1]}"
  version="${BASH_REMATCH[2]}"
else
  echo "'$tag' is not a stable release tag" >&2
  exit 1
fi

case "$product" in
  android) title="Tomi for Android"; paths=(apps/android crates/core spec) ;;
  windows) title="Tomi for Windows"; paths=(apps/windows crates/core spec) ;;
esac

joined="$(IFS=,; echo "${paths[*]}")"; joined="${joined//,/, }"
previous_tag="$(git tag --list "${product}-v*" --sort=-version:refname | grep -vx "$tag" | head -n 1 || true)"
api_version="$(grep -E '^pub const API_VERSION' crates/core/src/format.rs | sed -E 's/.*=[[:space:]]*([0-9]+).*/\1/')"
relay_version="$(awk '/^\[workspace.package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/, ""); print; exit}' Cargo.toml)"

echo "# ${title} ${version}"
echo
echo "Built from commit \`${sha}\`${RELEASE_RUN_URL:+ by ${RELEASE_RUN_URL}}."
echo

echo "## Compatibility"
echo
echo "- Protocol: Tomi format v1 and relay API v${api_version} (frozen test vectors in \`spec/vectors/v1\` and \`spec/vectors/pairing-v1\`)."
echo "- Relay: works with any \`tomi-relay\` that speaks API v${api_version}. The relay at the time of this release is ${relay_version}."
echo "- Devices of one space can run different app versions as long as they speak the same protocol version."
echo

echo "## Changes"
echo
if [ -n "$previous_tag" ]; then
  echo "Since \`${previous_tag}\` (commits touching ${joined}):"
  range="${previous_tag}..${tag}"
else
  echo "First release of this product (commits touching ${joined}):"
  range="${tag}"
fi
echo
# Subjects only, newest first; long histories are cut so that the notes stay readable.
total="$(git rev-list --count "$range" -- "${paths[@]}")"
git log "$range" --no-merges --format='- %s' -n 60 -- "${paths[@]}"
if [ "$total" -gt 60 ]; then
  echo "- ... and $((total - 60)) more (see the commit history)."
fi
echo

echo "## Verify your download"
echo
case "$product" in
  android)
    apk="${ASSET_APK:-tomi-android-${version}.apk}"
    echo '```sh'
    echo "sha256sum --check ${apk}.sha256"
    echo "gh attestation verify ${apk} --repo ${repo}"
    echo "apksigner verify --print-certs ${apk}   # the certificate must be the one below"
    echo '```'
    echo
    if [ -n "${ANDROID_CERT_SHA256:-}" ]; then
      echo "Signing certificate SHA-256: \`${ANDROID_CERT_SHA256}\`"
      echo
      echo "Android refuses to update an app signed by another key: a build signed with a different certificate is not an update of this one."
    fi
    ;;
  windows)
    exe="${ASSET_EXE:-tomi-windows-${version}-setup.exe}"
    echo '```sh'
    echo "sha256sum --check ${exe}.sha256"
    echo "gh attestation verify ${exe} --repo ${repo}"
    echo '```'
    echo
    echo "## Code signing policy"
    echo
    if [ "${WINDOWS_SIGNED:-false}" = "true" ]; then
      echo "Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)."
      echo
      echo "The installer is code-signed; Windows shows SignPath Foundation as the publisher. Who may sign, how a request is approved and what the program does with your data: [code signing policy](https://github.com/${repo}/blob/${tag}/docs/CODE_SIGNING_POLICY.md). Even so, SmartScreen may warn at first: reputation builds up with downloads."
    else
      echo "The installer is not code-signed yet: Windows SmartScreen may warn. Verifying the attestation tells you which workflow and commit built the file. The project's plan is in the [code signing policy](https://github.com/${repo}/blob/${tag}/docs/CODE_SIGNING_POLICY.md)."
    fi
    ;;
esac
echo
echo "Details: [docs/RELEASES.md](https://github.com/${repo}/blob/${tag}/docs/RELEASES.md#verifying-a-download)."
