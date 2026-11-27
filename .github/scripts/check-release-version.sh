#!/usr/bin/env bash
# Checks that a stable release tag matches the versions stored in the repository (docs/RELEASES.md, rule 1).
#
#   check-release-version.sh v1.2.3
#
# One tag, one version number for every app that ships from GitHub (Android and Windows; macOS is set aside until it can be
# built and signed in CI, iOS is distributed through the stores). Run it from the repository root, on the commit that carries
# the tag (fetch the other tags first so that the previous release can be found: `git fetch --tags`). It prints
# `version=...` and `previous_tag=...` on stdout (usable as GitHub Actions outputs) and fails with a message otherwise.
#
# Android:  `versionName` in apps/android/app/build.gradle.kts equals the tag version, and `versionCode` is strictly greater
#           than the one of the previous release (Android refuses to update an app whose versionCode did not grow).
# Windows:  `version` in apps/windows/src-tauri/tauri.conf.json, apps/windows/package.json and apps/windows/src-tauri/Cargo.toml
#           all equal the tag version.
# Both:     the version is strictly greater than the one of the previous release of that app (a published tag is never reused,
#           and nothing goes backwards). The previous release is the highest earlier `vX.Y.Z` tag or, before the grouped
#           releases existed, the highest `android-vX.Y.Z` / `windows-vX.Y.Z` tag of that app.

set -euo pipefail

fail() {
  echo "::error title=Release version::$*" >&2
  exit 1
}

tag="${1:-}"
[ -n "$tag" ] || fail "usage: check-release-version.sh <vX.Y.Z>"

if [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  version="${BASH_REMATCH[1]}"
else
  fail "'$tag' is not a stable release tag (expected vX.Y.Z, three numbers, no suffix)."
fi

gradle_file=apps/android/app/build.gradle.kts
tauri_file=apps/windows/src-tauri/tauri.conf.json
package_file=apps/windows/package.json
cargo_file=apps/windows/src-tauri/Cargo.toml

# `versionName = "x"` / `versionCode = n` of a Gradle file read from the working tree ($2 empty) or from a git ref ($2).
gradle_value() { # <key> [<ref>]
  local text
  if [ -n "${2:-}" ]; then text="$(git show "$2:$gradle_file")"; else text="$(cat "$gradle_file")"; fi
  grep -E "^[[:space:]]*$1[[:space:]]*=" <<<"$text" | head -n 1 | sed -E 's/.*=[[:space:]]*"?([^"[:space:]]+)"?.*/\1/'
}

# Package version of a Cargo.toml (first `version =` of the [package] table).
cargo_package_version() {
  awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/, ""); print; exit}' "$cargo_file"
}

# The most recent earlier release of an app ("" when this is the first one): the highest `vX.Y.Z` tag or `<app>-vX.Y.Z` tag
# other than the one being released, compared by version number.
previous_tag_of() { # <android|windows>
  git tag --list 'v[0-9]*.[0-9]*.[0-9]*' "$1-v[0-9]*.[0-9]*.[0-9]*" | grep -vx "$tag" \
    | awk '{ v = $0; sub(/^[a-z]+-/, "", v); sub(/^v/, "", v); print v, $0 }' | sort -V -k1,1 | tail -n 1 | cut -d' ' -f2 || true
}

# `a` is strictly greater than `b` as a dotted version.
greater() { # <a> <b>
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

# --- Android ---------------------------------------------------------------------------------------------------------------
android_previous="$(previous_tag_of android)"
name="$(gradle_value versionName)"
code="$(gradle_value versionCode)"
[ "$name" = "$version" ] || fail "$gradle_file has versionName \"$name\" but the tag says $version."
[[ "$code" =~ ^[0-9]+$ ]] || fail "versionCode in $gradle_file is not a number ('$code')."
if [ -n "$android_previous" ]; then
  prev_code="$(gradle_value versionCode "$android_previous")"
  prev_name="$(gradle_value versionName "$android_previous")"
  [[ "$prev_code" =~ ^[0-9]+$ ]] || fail "could not read the versionCode of $android_previous."
  [ "$code" -gt "$prev_code" ] || fail "versionCode $code is not greater than $prev_code of $android_previous: Android would refuse the update. Raise versionCode."
  greater "$version" "$prev_name" || fail "version $version is not greater than $prev_name of $android_previous (Android)."
fi

# --- Windows ---------------------------------------------------------------------------------------------------------------
windows_previous="$(previous_tag_of windows)"
tauri="$(jq -r .version "$tauri_file")"
package="$(jq -r .version "$package_file")"
cargo="$(cargo_package_version)"
[ "$tauri" = "$version" ] || fail "$tauri_file has version \"$tauri\" but the tag says $version."
[ "$package" = "$version" ] || fail "$package_file has version \"$package\" but the tag says $version."
[ "$cargo" = "$version" ] || fail "$cargo_file has version \"$cargo\" but the tag says $version."
if [ -n "$windows_previous" ]; then
  prev_version="$(git show "$windows_previous:$tauri_file" | jq -r .version)"
  greater "$version" "$prev_version" || fail "version $version is not greater than $prev_version of $windows_previous (Windows)."
fi

echo "version=$version"
echo "previous_tag=${android_previous:-$windows_previous}"
