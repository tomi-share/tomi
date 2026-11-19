#!/usr/bin/env bash
# Fails (non-zero exit) unless the given APK is fit to be PUBLISHED:
#   - it is not debuggable (aapt2 `dump badging` and the manifest attribute, two independent readings);
#   - it is signed (apksigner verifies it; an unsigned APK makes apksigner fail);
#   - it has exactly one signer, whose certificate is not an Android debug certificate;
#   - if an expected certificate SHA-256 is given, the signer's certificate is exactly that one.
#
# Usage: verify-android-apk.sh <apk> [expected-cert-sha256]
#   expected-cert-sha256: hex, with or without colons, any case. Optional but recommended.
#
# Fails closed: a missing tool or an output that cannot be parsed is a failure, never a pass.
# Needs the Android SDK build-tools (aapt2, apksigner) under $ANDROID_HOME or $ANDROID_SDK_ROOT, and a JDK for apksigner.
# Used by .github/workflows/build-apps.yml, both after the build and again before publishing.
set -euo pipefail

fail() {
  echo "::error title=APK check::$*" >&2
  echo "APK check FAILED: $*" >&2
  exit 1
}

apk="${1:-}"
[ -n "$apk" ] || fail "usage: verify-android-apk.sh <apk> [expected-cert-sha256]"
[ -f "$apk" ] || fail "no such file: $apk"
expected="$(printf '%s' "${2:-}" | tr -d ': \r\n' | tr 'A-F' 'a-f')"
if [ -n "$expected" ] && ! printf '%s' "$expected" | grep -Eq '^[0-9a-f]{64}$'; then
  fail "the expected certificate fingerprint is not a SHA-256 (64 hex characters)"
fi

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$sdk" ] && [ -d "$sdk/build-tools" ] || fail "Android SDK build-tools not found (ANDROID_HOME='${sdk}')"
bt="$(ls -1 "$sdk/build-tools" | sort -V | tail -n 1)"
aapt2="$sdk/build-tools/$bt/aapt2"
apksigner="$sdk/build-tools/$bt/apksigner"
[ -x "$aapt2" ] || fail "aapt2 not found in build-tools $bt"
[ -x "$apksigner" ] || fail "apksigner not found in build-tools $bt"
echo "Using build-tools $bt on $apk"

# 1. Not debuggable. Two readings of the same fact, so that one parsing surprise cannot make the check pass.
badging="$("$aapt2" dump badging "$apk")" || fail "aapt2 dump badging failed"
grep -q '^package: name=' <<<"$badging" || fail "aapt2 dump badging returned no package line (unexpected output)"
if grep -q 'application-debuggable' <<<"$badging"; then
  fail "the APK is DEBUGGABLE (application-debuggable)"
fi

manifest="$("$aapt2" dump xmltree --file AndroidManifest.xml "$apk")" || fail "aapt2 dump xmltree failed"
grep -q 'E: manifest' <<<"$manifest" || fail "aapt2 dump xmltree returned no manifest element (unexpected output)"
debuggable_lines="$(grep -Ei 'debuggable' <<<"$manifest" || true)"
if grep -Eiq '=true|=0xffffffff|"true"' <<<"$debuggable_lines"; then
  fail "the manifest sets android:debuggable to true"
fi

# 2. Signature: verified, one signer, not a debug certificate, and (if given) the expected one.
signer="$("$apksigner" verify --verbose --print-certs "$apk")" || fail "apksigner could not verify the APK (unsigned or invalid signature)"
grep -Eq 'Verified using v[234](\.[0-9]+)? scheme.*: true' <<<"$signer" || fail "no APK signature scheme v2 or later verified"
grep -q '^Number of signers: 1$' <<<"$signer" || fail "the APK must have exactly one signer"
# The line prefix depends on the build-tools version and on the scheme: "Signer #1 certificate DN: ..." (older) or
# "V3.0 Signer: certificate DN: ..." (newer). Accept both, and require that all the lines agree on ONE certificate.
dn="$(sed -nE 's/^(Signer #1|V[0-9.]+ Signer:) certificate DN: //p' <<<"$signer" | tr -d '\r')"
digest="$(sed -nE 's/^(Signer #1|V[0-9.]+ Signer:) certificate SHA-256 digest: //p' <<<"$signer" | tr -d ': \r' | tr 'A-F' 'a-f')"
[ -n "$dn" ] && [ -n "$digest" ] || fail "could not read the signer certificate (unexpected apksigner output)"
[ "$(sort -u <<<"$digest" | wc -l)" -eq 1 ] || fail "the signature schemes report different certificates"
digest="$(sort -u <<<"$digest")"
echo "Signer: $dn"
echo "SHA-256: $digest"
if grep -Eiq 'CN=Android Debug|O=Android,.*C=US' <<<"$dn"; then
  fail "the APK is signed with an Android DEBUG certificate ($dn)"
fi
if [ -n "$expected" ]; then
  [ "$digest" = "$expected" ] || fail "the signer certificate is not the expected one (expected $expected, got $digest)"
  echo "Certificate matches the expected fingerprint."
else
  echo "::notice title=APK check::No expected certificate fingerprint given: only 'not a debug certificate' was checked."
fi

echo "APK check passed: not debuggable, signed by a non-debug certificate."
