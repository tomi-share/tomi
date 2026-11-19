#!/usr/bin/env bash
# Checks that a stable release tag matches the versions stored in the repository (docs/RELEASES.md, rule 1).
#
#   check-release-version.sh android-v1.2.3
#   check-release-version.sh windows-v1.2.3
#
# Run it from the repository root, on the commit that carries the tag (fetch the other tags first so that the previous
# release can be found: `git fetch --tags`). It prints the product and the version it validated on stdout as
# `product=...` and `version=...` (usable as a GitHub Actions output) and fails with a message otherwise.
#
# Android:  `versionName` in apps/android/app/build.gradle.kts equals the tag version, and `versionCode` is strictly greater
#           than the one of the previous android-v* tag (Android refuses to update an app whose versionCode did not grow).
# Windows:  `version` in apps/windows/src-tauri/tauri.conf.json, apps/windows/package.json and apps/windows/src-tauri/Cargo.toml
#           all equal the tag version.
# Both:     the version is strictly greater than the one of the previous tag of the same product (a published tag is never
#           reused, and nothing goes backwards).

set -euo pipefail

fail() {
  echo "::error title=Release version::$*" >&2
  exit 1
}

tag="${1:-}"
[ -n "$tag" ] || fail "usage: check-release-version.sh <android-vX.Y.Z|windows-vX.Y.Z>"

if [[ "$tag" =~ ^(android|windows)-v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  product="${BASH_REMATCH[1]}"
  version="${BASH_REMATCH[2]}"
else
  fail "'$tag' is not a stable release tag (expected android-vX.Y.Z or windows-vX.Y.Z, three numbers, no suffix)."
fi

gradle_file=apps/android/app/build.gradle.kts
tauri_file=apps/windows/src-tauri/tauri.conf.json
package_file=apps/windows/package.json
cargo_file=apps/windows/src-tauri/Cargo.toml

# `versionName = "x"` / `versionCode = n` of a Gradle file read from the working tree ($1 empty) or from a git ref ($1).
gradle_value() { # <key> [<ref>]
  local text
  if [ -n "${2:-}" ]; then text="$(git show "$2:$gradle_file")"; else text="$(cat "$gradle_file")"; fi
  grep -E "^[[:space:]]*$1[[:space:]]*=" <<<"$text" | head -n 1 | sed -E 's/.*=[[:space:]]*"?([^"[:space:]]+)"?.*/\1/'
}

# Package version of a Cargo.toml (first `version =` of the [package] table).
cargo_package_version() {
  awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version[[:space:]]*=/{gsub(/.*=[[:space:]]*"|".*/, ""); print; exit}' "$cargo_file"
}

# The most recent tag of this product other than the one being released ("" when this is the first one).
previous_tag="$(git tag --list "${product}-v*" --sort=-version:refname | grep -vx "$tag" | head -n 1 || true)"

# `a` is strictly greater than `b` as a dotted version.
greater() { # <a> <b>
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

case "$product" in
  android)
    name="$(gradle_value versionName)"
    code="$(gradle_value versionCode)"
    [ "$name" = "$version" ] || fail "$gradle_file has versionName \"$name\" but the tag says $version."
    [[ "$code" =~ ^[0-9]+$ ]] || fail "versionCode in $gradle_file is not a number ('$code')."
    if [ -n "$previous_tag" ]; then
      prev_code="$(gradle_value versionCode "$previous_tag")"
      prev_name="$(gradle_value versionName "$previous_tag")"
      [[ "$prev_code" =~ ^[0-9]+$ ]] || fail "could not read the versionCode of $previous_tag."
      [ "$code" -gt "$prev_code" ] || fail "versionCode $code is not greater than $prev_code of $previous_tag: Android would refuse the update. Raise versionCode."
      greater "$version" "$prev_name" || fail "version $version is not greater than $prev_name of $previous_tag."
    fi
    ;;
  windows)
    tauri="$(jq -r .version "$tauri_file")"
    package="$(jq -r .version "$package_file")"
    cargo="$(cargo_package_version)"
    [ "$tauri" = "$version" ] || fail "$tauri_file has version \"$tauri\" but the tag says $version."
    [ "$package" = "$version" ] || fail "$package_file has version \"$package\" but the tag says $version."
    [ "$cargo" = "$version" ] || fail "$cargo_file has version \"$cargo\" but the tag says $version."
    if [ -n "$previous_tag" ]; then
      prev_version="$(git show "$previous_tag:$tauri_file" | jq -r .version)"
      greater "$version" "$prev_version" || fail "version $version is not greater than $prev_version of $previous_tag."
    fi
    ;;
esac

echo "product=$product"
echo "version=$version"
echo "previous_tag=$previous_tag"
