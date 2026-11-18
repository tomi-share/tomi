#!/usr/bin/env bash
# Prints the UDID of the first available iOS simulator named "$1" (default: iPhone Duo).
# `xcodebuild -destination name=...` fails when several simulators share a name; an id never does.
set -euo pipefail
name="${1:-iPhone Duo}"
udid="$(xcrun simctl list devices available -j | python3 -I -c '
import json, sys
name = sys.argv[1]
devices = json.load(sys.stdin)["devices"]
for runtime, entries in sorted(devices.items(), reverse=True):
    if "iOS" not in runtime:
        continue
    for d in entries:
        if d["name"] == name:
            print(d["udid"]); sys.exit(0)
sys.exit(1)
' "$name")" || { echo "no available simulator named '$name' (xcrun simctl list devices available)" >&2; exit 1; }
echo "$udid"
