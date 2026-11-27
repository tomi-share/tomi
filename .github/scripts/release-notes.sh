#!/usr/bin/env bash
# Writes the notes of a stable release on stdout (docs/RELEASES.md, rules 3 to 5).
#
#   release-notes.sh v1.2.3 > notes.md
#
# One release carries every app built by CI (Android and Windows): the notes list the changes and how to verify each file. A partial
# release builds only one of them and carries the other over, unchanged, from the previous release that had it: the notes say so.
#
# Environment (all optional except where stated):
#   GITHUB_REPOSITORY      owner/name, used in the verification commands (default: tomi-share/tomi)
#   RELEASE_SHA            the commit the release was built from (default: the commit of the tag)
#   RELEASE_RUN_URL        URL of the workflow run that built the files
#   ANDROID_CERT_SHA256    SHA-256 fingerprint of the signing certificate (Android; public information)
#   ASSET_APK / ASSET_EXE  published file names, for the verification commands
#   WINDOWS_SIGNED         "true" when the installer was signed through SignPath
#   BUILT                  apps built by this release, comma separated (default: android,windows)
#   ANDROID_CARRIED        tag of the release the APK was carried over from (empty: built by this release)
#   WINDOWS_CARRIED        same for the installer
#
# The notes carry no secret: commit subjects of a public repository, public fingerprints and commands. Run it where the
# previous tags are fetched (`git fetch --tags`, full history) so that the change list can start at the previous release.

set -euo pipefail

tag="${1:?usage: release-notes.sh <vX.Y.Z>}"
repo="${GITHUB_REPOSITORY:-tomi-share/tomi}"
sha="${RELEASE_SHA:-$(git rev-list -n 1 "$tag" 2>/dev/null || git rev-parse HEAD)}"
built="${BUILT:-android,windows}"
android_carried="${ANDROID_CARRIED:-}"
windows_carried="${WINDOWS_CARRIED:-}"

if [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  version="${BASH_REMATCH[1]}"
else
  echo "'$tag' is not a stable release tag" >&2
  exit 1
fi

title="Tomi ${version}"
paths=(crates/core spec)
case ",${built}," in *,android,*) paths=(apps/android "${paths[@]}") ;; esac
case ",${built}," in *,windows,*) paths=(apps/windows "${paths[@]}") ;; esac

joined="$(IFS=,; echo "${paths[*]}")"; joined="${joined//,/, }"
# The previous release: the highest earlier `vX.Y.Z` tag or, before the grouped releases, `android-vX.Y.Z` / `windows-vX.Y.Z`.
previous_tag="$(git tag --list 'v[0-9]*.[0-9]*.[0-9]*' 'android-v[0-9]*.[0-9]*.[0-9]*' 'windows-v[0-9]*.[0-9]*.[0-9]*' | grep -vx "$tag" \
  | awk '{ v = $0; sub(/^[a-z]+-/, "", v); sub(/^v/, "", v); print v, $0 }' | sort -V -k1,1 | tail -n 1 | cut -d' ' -f2 || true)"
api_version="$(grep -E '^pub const API_VERSION' crates/core/src/format.rs | sed -E 's/.*=[[:space:]]*([0-9]+).*/\1/')"
relay_version="$(awk '/^\[workspace.package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/, ""); print; exit}' Cargo.toml)"

echo "# ${title}"
echo
echo "Built from commit \`${sha}\`${RELEASE_RUN_URL:+ by ${RELEASE_RUN_URL}}."
echo
echo "This release carries the Android app and the Windows installer."
if [ -n "$android_carried" ] || [ -n "$windows_carried" ]; then
  echo
  echo "It is a partial release: only ${built//,/, } was built from this commit, the other file is carried over from an earlier release:"
  [ -z "$android_carried" ] || echo "- The Android APK is **not rebuilt**: it is the file of \`${android_carried}\`, carried over unchanged (its own version number is in its file name)."
  [ -z "$windows_carried" ] || echo "- The Windows installer is **not rebuilt**: it is the file of \`${windows_carried}\`, carried over unchanged (its own version number is in its file name)."
fi
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
  range="${previous_tag}..${sha}"
else
  echo "First release (commits touching ${joined}):"
  range="${sha}"
fi
echo
# Subjects only, newest first; long histories are cut so that the notes stay readable.
total="$(git rev-list --count "$range" -- "${paths[@]}")"
git log "$range" --no-merges --format='- %s' -n 60 -- "${paths[@]}"
if [ "$total" -gt 60 ]; then
  echo "- ... and $((total - 60)) more (see the commit history)."
fi
echo

apk="${ASSET_APK:-tomi-android-${version}.apk}"
exe="${ASSET_EXE:-tomi-windows-${version}-setup.exe}"

echo "## Verify your download"
echo
echo "### Android"
echo
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
  echo
fi
echo "### Windows"
echo
echo '```sh'
echo "sha256sum --check ${exe}.sha256"
echo "gh attestation verify ${exe} --repo ${repo}"
echo '```'
echo
echo "## Code signing policy"
echo
if [ -n "$windows_carried" ]; then
  echo "The Windows installer is the one of \`${windows_carried}\`, carried over unchanged: its code signing status is the one stated in the notes of that release."
elif [ "${WINDOWS_SIGNED:-false}" = "true" ]; then
  echo "Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)."
  echo
  echo "The installer is code-signed; Windows shows SignPath Foundation as the publisher. Who may sign, how a request is approved and what the program does with your data: [code signing policy](https://github.com/${repo}/blob/${tag}/docs/CODE_SIGNING_POLICY.md). Even so, SmartScreen may warn at first: reputation builds up with downloads."
else
  echo "The Windows installer is not code-signed yet: Windows SmartScreen may warn. Verifying the attestation tells you which workflow and commit built the file. The project's plan is in the [code signing policy](https://github.com/${repo}/blob/${tag}/docs/CODE_SIGNING_POLICY.md)."
fi
echo
echo "Details: [docs/RELEASES.md](https://github.com/${repo}/blob/${tag}/docs/RELEASES.md#verifying-a-download)."
