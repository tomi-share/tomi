#!/usr/bin/env bash
# Tomi relay — "Tomi relay: init" recipe (example managed-server deployment; run as the `deploy` user).
#
# Run once after the first deployment, or when the install token has been lost:
#   - checks that the relay is deployed and answers;
#   - removes leftovers of a plain-text token (smoke.log from a test start, old install-token file);
#   - closes the data directory to the group and to others (chmod -R go-rwx, also in arm mode);
#   - prints the relay's state;
#   - replaces all pending invitations with ONE and PRINTS its token.
#
# The token is printed only once: the relay keeps only its fingerprint. It appears in the recipe's
# output (panel log, or by e-mail if requested): it is single-use, it is only needed to create a
# space; paste it right away into "Create your space", after which it is consumed.
#
# Repeatable. Does not touch existing spaces, devices or the binary.
# Versioned copy of the panel recipe; see infra/forge/recipes.md §0.
#
# Two modes (an argument, or the TOMI_INIT_MODE variable for a panel recipe that passes no argument):
#   init (default): everything above, including the ROTATION of invitations;
#   arm           : checks the relay, prints its state and arms the "volume not mounted" guard
#                   (require_existing_data_dir) WITHOUT doing anything else: no cleanup, no rotation;
#                   pending invitations are NEVER replaced. Use it to arm the guard after an update.

set -euo pipefail

MODE="${1:-${TOMI_INIT_MODE:-init}}"
case "$MODE" in
init | arm) ;;
*)
    printf 'ERROR: unknown mode "%s" (init or arm)\n' "$MODE" >&2
    exit 2
    ;;
esac

BIN="${TOMI_BIN:-/home/deploy/relay.example.org/tomi-relay}"
DATA="${TOMI_DATA_DIR:-/home/deploy/tomi-data}"
CFG="${TOMI_CONFIG:-${DATA}/config.toml}"
HEALTH_URL="${TOMI_HEALTH_URL:-http://127.0.0.1:8080/healthz}"

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

# Does binary $1 know require_existing_data_dir? The test is on the binary ALONE (not on the running
# relay, whose admin socket would answer with ITS version): empty environment (env -i), throwaway empty
# data directory, deployment configuration (--config) not read. A binary that does not know the key
# fails (unknown key).
binary_knows_guard() {
    local tmp rc=0
    tmp="$(mktemp -d)" || return 1
    timeout 30 env -i TOMI_RELAY_DATA_DIR="$tmp" TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=false \
        "$1" admin status --json >/dev/null 2>&1 || rc=$?
    [ -z "$tmp" ] || rm -rf -- "$tmp"
    [ "$rc" -eq 0 ]
}

[ -x "$BIN" ] || die "binary missing (${BIN}): deploy the relay first (run the deploy script via the panel)"
[ -f "$CFG" ] || die "configuration missing (${CFG}): see infra/forge/README.md §2"
curl -fsS --max-time 5 "$HEALTH_URL" >/dev/null ||
    die "the relay does not answer on ${HEALTH_URL}: sudo supervisorctl status"

if [ "$MODE" = init ]; then
    echo "==> Removing plain-text tokens"
    removed=0
    for f in "${DATA}/smoke.log" "${DATA}/install-token"; do
        if [ -f "$f" ]; then
            shred -u "$f" 2>/dev/null || rm -f "$f"
            echo "    removed: ${f}"
            removed=1
        fi
    done
    [ "$removed" -eq 1 ] || echo "    nothing to remove"
fi

# Permissions of the data directory: the `deploy` user is shared with the panel and the recipes, and
# files created before `umask=077` (supervisor-daemon.conf) may be 0644. Repeatable.
echo "==> Data directory permissions"
if chmod -R go-rwx -- "$DATA"; then
    echo "    ${DATA}: closed to the group and to others (chmod -R go-rwx)"
else
    echo "    WARNING: chmod -R go-rwx ${DATA} failed (see infra/forge/README.md §8)" >&2
fi

# Daemon log: the relay writes once, on the first start of a new relay, a line "install token: …"
# (an invitation that never expires). Supervisord creates this file with its own umask (often 022:
# readable by everyone), and the daemon's `umask=077` does not apply to it. We therefore close it
# (current log and old ones). The token it contains is cancelled anyway by the rotation of init mode;
# in arm mode (no rotation) it stays valid: run `init` to replace it.
LOGS="${TOMI_LOG_FILE:-/home/deploy/logs/tomi-relay.log}"
echo "==> Relay log permissions"
for f in "$LOGS" "$LOGS".[0-9]*; do
    [ -f "$f" ] || continue
    if chmod 600 -- "$f" 2>/dev/null; then
        echo "    ${f}: 0600"
    else
        # Expected if the file is owned by root (supervisord opens it itself): not a failure.
        echo "    WARNING: ${f} could not be set to 0600 (owner: $(stat -c %U -- "$f" 2>/dev/null || echo '?'))." >&2
        echo "    The logged token is cancelled by the rotation of init mode. Check: ls -l ${f}* ;" >&2
        echo "    if needed: sudo chmod 700 $(dirname -- "$f") or run this block as root." >&2
    fi
done

echo "==> Relay state"
"$BIN" --config "$CFG" admin status

# "Volume not mounted" guard (spec/api-v1.md §8.6): the relay has started and answers, its data
# directory contains routing.db; from now on it will refuse to start on an empty directory instead of
# assuming it is new. Written only once; a value already present is respected.
# (Arming never replaces invitations: rotation is the next step, init mode only.)
# Only with a binary that knows the key (an unknown key would make startup fail), and only if the file
# has no TOML table (the key must stay at the root level).
echo "==> Data directory guard"
if grep -qE '^[[:space:]]*require_existing_data_dir[[:space:]]*=' "$CFG"; then
    echo "    already set in ${CFG} (unchanged)"
elif [ ! -e "${DATA}/routing.db" ]; then
    echo "    ${DATA}/routing.db missing: not added"
elif grep -qE '^[[:space:]]*\[' "$CFG"; then
    echo "    ${CFG} contains a TOML table: add \"require_existing_data_dir = true\" by hand, before it"
elif ! binary_knows_guard "$BIN"; then
    echo "    the deployed binary does not know require_existing_data_dir yet: not added"
else
    printf '\n# Added by the "Tomi relay: init" recipe after a successful start (see config.toml in the repository).\nrequire_existing_data_dir = true\n' >>"$CFG"
    echo "    require_existing_data_dir = true added (applied at the next start)"
fi

if [ "$MODE" = arm ]; then
    echo
    echo "Arm mode: guard handled, pending invitations UNCHANGED (no rotation)."
    exit 0
fi

echo "==> Install token (printed ONCE, replaces pending invitations)"
"$BIN" --config "$CFG" admin invite --rotate

echo
echo "Paste this token into \"Create your space\" (Android or Windows). It is single-use."
