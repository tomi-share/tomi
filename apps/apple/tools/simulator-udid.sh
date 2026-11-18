#!/usr/bin/env bash
# Prints the UDID of the first available iOS simulator named like one of the arguments
# (default: iPhone Duo), newest runtime first.
# `xcodebuild -destination name=...` fails when several simulators share a name; an id never does.
#
# Usage: simulator-udid.sh [--any-iphone] [name ...]
#   --any-iphone  take any iPhone when none of the names is installed — and when no name is given,
#                 go straight to any iPhone. For CI, whose runners do not have the same simulators
#                 as this Mac. Ordinary runtimes (iOS-27-2) come before the ones tied to a single
#                 device (iOS-27-2-iPhone19-4, the iPhone Duo): a dual-screen simulator is slow to
#                 boot and loses the test runner on a CI machine.
set -euo pipefail
fallback=false
names=()
for arg in "$@"; do
  case "$arg" in
    --any-iphone) fallback=true ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) names+=("$arg") ;;
  esac
done
# Without --any-iphone, no name means the simulator of this project.
if [ ${#names[@]} -eq 0 ] && [ "$fallback" = false ]; then names=("iPhone Duo"); fi

udid="$(xcrun simctl list devices available -j | python3 -I -c '
import json, re, sys

fallback = sys.argv[1] == "true"
names = sys.argv[2:]
devices = json.load(sys.stdin)["devices"]
runtimes = [item for item in sorted(devices.items(), reverse=True) if "iOS" in item[0]]

def pick(predicate, pool):
    for _, entries in pool:
        for device in entries:
            if predicate(device):
                print(device["udid"]); sys.exit(0)

for name in names:
    pick(lambda d, n=name: d["name"] == n, runtimes)
if fallback:
    # A runtime of its own (…iOS-27-2-iPhone19-4) belongs to one device; try the ordinary ones first.
    plain = [item for item in runtimes if re.fullmatch(r".*\.iOS-\d+-\d+", item[0])]
    pick(lambda d: d["name"].startswith("iPhone"), plain)
    pick(lambda d: d["name"].startswith("iPhone"), runtimes)
sys.exit(1)
' "$fallback" ${names[@]+"${names[@]}"})" || {
  echo "no available iOS simulator (names tried: ${names[*]-aucun}, fallback: $fallback)" >&2
  echo "see: xcrun simctl list devices available" >&2
  exit 1
}
echo "$udid"
