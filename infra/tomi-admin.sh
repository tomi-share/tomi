#!/usr/bin/env bash
# Tomi — runs `tomi-relay admin ...` as the service user, with the service configuration.
# Installed by install.sh under /usr/local/sbin/tomi-admin.
#
#   sudo tomi-admin status [--space <id>]
#   sudo tomi-admin spaces
#   sudo tomi-admin devices [--space <id>]
#   sudo tomi-admin invite [--new]
#   sudo tomi-admin quota --space <id> <octets|none>
#   sudo tomi-admin revoke [--space <id>] <device>
#   sudo tomi-admin reset [--space <id>]
#
# All arguments are passed through unchanged to the binary: the exact subcommands and options are
# defined by it (`tomi-relay admin --help`); this script does not know any of them.

set -Eeuo pipefail

ENV_FILE="/etc/tomi/relay.env"
SVC_USER="tomi"
BIN="/usr/local/bin/tomi-relay"

if [ "$#" -eq 0 ]; then
    echo "Usage: sudo tomi-admin <status|spaces|devices|invite|quota|revoke|reset> [arguments]" >&2
    echo "       (details: sudo tomi-admin --help)" >&2
    exit 2
fi

if [ "$(id -u)" -ne 0 ]; then
    exec sudo -- "$0" "$@"
fi

[ -r "$ENV_FILE" ] || {
    echo "tomi-admin: ${ENV_FILE} not found or not readable" >&2
    exit 1
}

# Same environment as the service (relay.env only contains simple KEY=value lines).
set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

cd "${TOMI_RELAY_DATA_DIR:-/var/lib/tomi}"
exec runuser -u "$SVC_USER" -- "$BIN" admin "$@"
