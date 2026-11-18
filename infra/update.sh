#!/usr/bin/env bash
# Tomi relay — binary update with rollback. Run AS ROOT on the server.
#
#   sudo bash infra/update.sh /path/to/tomi-relay [--skip-health] [--no-backup]
#   sudo bash infra/update.sh --rollback [--force]
#
# Sequence: data backup → old binary copied to tomi-relay.prev →
# new binary installed by atomic rename → restart → health check.
# If the service does not come back (or /healthz does not respond), the script automatically rolls back to the old binary.
#
# Limit (data layout): the multi-space relay migrates the old layout (tomi.db + blobs/) to
# spaces/<id>/ (routing.db, relay.db) by itself at startup. A binary from before that
# migration can no longer find tomi.db: it would start with empty data and answer
# "space_not_initialized"; clients would then record their space as deleted, permanently.
# So as soon as spaces exist (spaces/*/relay.db) when there were none before the update, the automatic
# rollback is DISABLED, and a manual --rollback requires --force. To go back to the old
# version in that case: stop the service, restore the backup taken just before (infra/backup.md),
# then swap the binaries (--rollback --force).
#
# Limit (data directory safeguard): once TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=true is written to
# relay.env, a binary that does not know the key refuses to start (unknown key). Before any rollback
# (automatic or --rollback), if the target binary does not know the key, the line is commented out with a message;
# install.sh restores it when a version that knows the key runs again.
# After a HEALTHY update, update.sh arms this safeguard itself (same test as install.sh: a binary that
# knows the key, routing.db present; relay.env is rewritten through a temporary file, verified, then renamed).
# It only takes effect at the next start. Separately, --rollback (without --force) is refused if the
# data directory contains neither routing.db, nor spaces/, nor tomi.db (volume not mounted?), whether the key
# is armed or not.
#
# Limit (refusal to start because of the data): the relay exits with code 78 when it refuses to
# start because of what its data contains (volume not mounted, symbolic link on a space,
# unreadable folder or database, database belonging to another space; spec/api-v1.md §10.1). Before any rollback
# (automatic or --rollback, with or without --force), the script runs the installed binary under the service
# user and configuration (`admin status --json`) and reads systemd's last exit code: if it is
# 78, the script STOPS without swapping anything, and tells you what to check. Going back to an older binary
# would answer 409 space_not_initialized / 401 for a live space. Any code other than 0 or 78
# (1, 124 = timeout, 137 = killed, runuser missing...) means the check could NOT conclude: the
# script warns; on a manual --rollback it stops (--force overrides this block, but NEVER an established 78);
# on the automatic rollback it warns and continues (tomi-relay.prev has just been copied from the binary in
# service). An intact old layout (tomi.db alone, not yet migrated) is not a refusal: a binary from before the
# migration reads it perfectly well, it blocks nothing.

set -Eeuo pipefail
trap 'echo "update.sh: error on line ${LINENO} (command: ${BASH_COMMAND})" >&2' ERR

SVC="tomi-relay.service"
BIN="/usr/local/bin/tomi-relay"
SVC_USER="tomi"
PREV="${BIN}.prev"
NEW="${BIN}.new"
ENV_FILE="/etc/tomi/relay.env"
BACKUP_BIN="/usr/local/sbin/tomi-backup"

log() { printf '==> %s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: sudo bash infra/update.sh /path/to/tomi-relay [--skip-health] [--no-backup]
       sudo bash infra/update.sh --rollback [--force]

  --skip-health  only checks the state of the service, not /healthz
  --no-backup    does not back up the data before the update
  --rollback     swaps the current binary and tomi-relay.prev (can be replayed to go back)
  --force        with --rollback: swaps even if spaces exist (multi-space layout);
                 only do this if tomi-relay.prev understands that layout, or after restoring
                 the backup taken before the update; also overrides the stop when the "refused because of
                 the data" check could not conclude, or when the data directory looks empty;
                 NEVER overrides an established refusal 78
EOF
}

BINARY=""
SKIP_HEALTH=0
NO_BACKUP=0
ROLLBACK=0
FORCE=0
while [ "$#" -gt 0 ]; do
    case "$1" in
    --skip-health) SKIP_HEALTH=1 ;;
    --no-backup) NO_BACKUP=1 ;;
    --rollback) ROLLBACK=1 ;;
    --force) FORCE=1 ;;
    -h | --help)
        usage
        exit 0
        ;;
    -*)
        usage >&2
        die "unknown option: $1"
        ;;
    *)
        [ -z "$BINARY" ] || die "only one binary expected"
        BINARY="$1"
        ;;
    esac
    shift
done

[ "$(id -u)" -eq 0 ] || die "run as root (sudo bash infra/update.sh ...)"
command -v systemctl >/dev/null 2>&1 || die "systemd is required"
[ -f "$BIN" ] || die "${BIN} does not exist: run infra/install.sh first"

if [ "$ROLLBACK" -eq 0 ]; then
    [ -n "$BINARY" ] || {
        usage >&2
        exit 2
    }
elif [ -n "$BINARY" ]; then
    die "--rollback does not accept a binary"
fi
if [ "$FORCE" -eq 1 ] && [ "$ROLLBACK" -eq 0 ]; then
    die "--force is only used with --rollback"
fi

check_binary() {
    local f="$1" magic machine want
    [ -f "$f" ] || die "binary not found: $f"
    magic="$(head -c 4 "$f" | od -An -c | tr -d ' ')"
    [ "$magic" = "177ELF" ] || die "$f is not a Linux executable (ELF)"
    machine="$(od -An -tx1 -j18 -N1 "$f" | tr -d ' ')"
    case "$(uname -m)" in
    x86_64) want="3e" ;;
    aarch64 | arm64) want="b7" ;;
    *) want="" ;;
    esac
    if [ -n "$want" ] && [ "$machine" != "$want" ]; then
        die "$f is not built for this machine's architecture ($(uname -m))"
    fi
}

health_url() {
    local listen=""
    if [ -f "$ENV_FILE" ]; then
        listen="$(grep -E '^TOMI_RELAY_LISTEN=' "$ENV_FILE" | tail -n 1 | cut -d= -f2- || true)"
    fi
    printf 'http://%s/healthz' "${listen:-127.0.0.1:8080}"
}

# Waits for the service to be active (and /healthz OK unless --skip-health). 0 = healthy, 1 = failure.
wait_healthy() {
    local i
    for i in $(seq 1 30); do
        if systemctl is-active --quiet "$SVC"; then
            if [ "$SKIP_HEALTH" -eq 1 ] || ! command -v curl >/dev/null 2>&1; then
                # No HTTP check possible: require 5 s of stability.
                if [ "$i" -ge 5 ]; then
                    return 0
                fi
            elif curl -fsS --max-time 2 "$(health_url)" >/dev/null 2>&1; then
                return 0
            fi
        fi
        sleep 1
    done
    return 1
}

# Data directory of the relay (same source as the service: /etc/tomi/relay.env).
data_dir() {
    local d=""
    if [ -f "$ENV_FILE" ]; then
        d="$(grep -E '^TOMI_RELAY_DATA_DIR=' "$ENV_FILE" | tail -n 1 | cut -d= -f2- || true)"
    fi
    printf '%s' "${d:-/var/lib/tomi}"
}

# Is there at least one space in the multi-space layout (spaces/<id>/relay.db)?
has_spaces() { compgen -G "$(data_dir)/spaces/*/relay.db" >/dev/null; }

# Does a path exist (even as a broken link)?
exists() { [ -e "$1" ] || [ -L "$1" ]; }

# Does the data directory contain anything from a relay (routing.db, spaces/ or tomi.db)?
# False = it looks empty: volume not mounted, wrong path.
data_dir_has_data() {
    local d
    d="$(data_dir)"
    exists "$d/routing.db" || exists "$d/spaces" || exists "$d/tomi.db"
}

# Is the old layout intact (tomi.db alone, neither spaces/ nor spaces.migrating/)? A binary
# from before the migration reads it perfectly well: this is not a refusal because of the data.
legacy_intact() {
    local d
    d="$(data_dir)"
    exists "$d/tomi.db" && ! exists "$d/spaces" && ! exists "$d/spaces.migrating"
}

# Does the installed binary (new, or the one in service) REFUSE to start because of the DATA? The relay then
# exits with the dedicated code 78 (spec/api-v1.md §10.1): volume not mounted, symbolic link on spaces/<id> or
# relay.db, folder unreadable (permissions), database unreadable or belonging to another space... Going back then to a binary
# from BEFORE multi-space would be worse than staying down: it ignores these spaces, starts "empty" and
# answers 409 space_not_initialized / 401 for a live space (and creates an installation token if there
# is no invitation). The rollback, automatic or --rollback, must therefore not happen.
# Two sources: the last exit code of the service known to systemd (also covers refusals that a read-only
# open does not see), and `admin status --json` run UNDER THE SERVICE'S USER AND CONFIGURATION
# (relay.env, config.toml).
# Result in DATA_STATE: "refused" (78 established), "ok" (the binary read the data, or the old
# layout is intact), "unknown" (the check could not conclude: runuser missing, timeout 124,
# process killed 137, other code). DATA_REFUSED_WHY says why.
EXIT_DATA_REFUSED=78
DATA_STATE=""
DATA_REFUSED_WHY=""
data_check() {
    local status rc=0 out="" line
    local -a args=(PATH=/usr/sbin:/usr/bin:/sbin:/bin)
    DATA_STATE="unknown"
    DATA_REFUSED_WHY=""
    status="$(systemctl show "$SVC" -p ExecMainStatus --value 2>/dev/null || true)"
    # A migration that fails cleanly exits with 78 but leaves the old layout intact (tomi.db alone):
    # a binary from before the migration reads it perfectly well, so the rollback must remain possible.
    if [ "$status" = "$EXIT_DATA_REFUSED" ] && ! legacy_intact; then
        DATA_STATE="refused"
        DATA_REFUSED_WHY="systemd: the service stopped with code ${EXIT_DATA_REFUSED} (refusal to start because of the data); see journalctl -u tomi-relay -n 30 --no-pager"
        return 0
    fi
    if ! command -v runuser >/dev/null 2>&1; then
        DATA_REFUSED_WHY="runuser is missing"
        return 0
    fi
    if [ -f "$ENV_FILE" ]; then
        while IFS= read -r line || [ -n "$line" ]; do
            case "$line" in
            TOMI_RELAY_*=*) args+=("$line") ;;
            esac
        done <"$ENV_FILE"
    fi
    out="$(cd / && timeout 30 runuser -u "$SVC_USER" -- env -i "${args[@]}" "$BIN" admin status --json 2>&1 >/dev/null)" || rc=$?
    if [ "$rc" -eq 0 ]; then
        DATA_STATE="ok"
    elif [ "$rc" -eq "$EXIT_DATA_REFUSED" ]; then
        DATA_STATE="refused"
        DATA_REFUSED_WHY="${out:-code ${EXIT_DATA_REFUSED}}"
    elif legacy_intact; then
        # Non-zero exit, but this is the old layout not yet migrated (admin status refuses to read it
        # without migrating it): nothing to blame on the data.
        DATA_STATE="ok"
    else
        DATA_REFUSED_WHY="admin status exited with code ${rc} (124 = timeout, 137 = killed)${out:+: ${out}}"
    fi
    return 0
}

# Stops the script (without rollback) if the installed binary refuses to start because of the data;
# warns if the check could not conclude.
# $1: what the script was about to do. $2: "manual" (--rollback: stops if the check did not conclude,
# unless --force) or "auto" (automatic rollback: warns and continues). An established 78 ALWAYS stops.
refuse_if_data_refused() {
    data_check
    case "$DATA_STATE" in
    ok) return 0 ;;
    refused)
        die "the installed binary (${BIN}) REFUSES to start because of the DATA (exit code ${EXIT_DATA_REFUSED}): ${DATA_REFUSED_WHY}
    $1 will NOT be done: an older binary would ignore these spaces, start empty and answer 409 space_not_initialized / 401 for a live space. Nothing was changed by this step.
    To check: the volume of $(data_dir) is mounted; $(data_dir)/spaces/ is a SUB-folder of the volume (never the mount point itself); owner ${SVC_USER} and permissions on $(data_dir), spaces/ and each spaces/<id>/ (backup restored as root without chown?); spaces/<id> and spaces/<id>/relay.db are not symbolic links; the disk has no I/O errors.
    Then: sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC} ; journalctl -u tomi-relay -n 100 --no-pager"
        ;;
    esac
    warn "cannot verify whether the installed binary refuses to start because of the data: ${DATA_REFUSED_WHY}"
    if [ "${2:-auto}" = "manual" ] && [ "$FORCE" -eq 0 ]; then
        die "$1 NOT done: the \"refused because of the data\" check could not conclude (see above). Fix the cause (runuser, invalid config.toml, slow disk) and run again. You can override this with --force if you know the data is readable by the old binary; --force NEVER overrides an established refusal 78. Manual check: sudo -u ${SVC_USER} ${BIN} admin status ; echo \$?"
    fi
    warn "continuing anyway ($1 ; ${2:-auto} ; --force=${FORCE})."
}

# Does binary $1 know TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR? Tested on the binary ALONE: it runs
# with an empty environment (env -i) except for a throwaway, empty data directory, so without a reachable
# admin socket (a running relay would answer in its place, with ITS version) and without
# loading relay.env. The key is set to false: a binary that does not know it fails (unknown key,
# non-zero exit code); a binary that knows it reads the state of this empty folder and exits 0 without
# writing anything. When in doubt (failure, timeout), the answer is "no". Limit: the binary still reads
# /etc/tomi-relay/config.toml if it exists (env -i does not exclude it); an invalid config makes the
# test fail like an unknown key. The messages below say so (the logic stays "no" when in doubt).
binary_knows_guard() {
    local tmp rc=0
    tmp="$(mktemp -d)" || return 1
    timeout 30 env -i TOMI_RELAY_DATA_DIR="$tmp" TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=false \
        "$1" admin status --json >/dev/null 2>&1 || rc=$?
    [ -z "$tmp" ] || rm -rf -- "$tmp"
    [ "$rc" -eq 0 ]
}

# An unknown key makes startup fail: a binary from before TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR
# would refuse to start again once install.sh has written it into relay.env. Before a rollback
# to such a binary, the line is commented out (with an explanation); it is not
# deleted. The safeguard is then no longer active: install.sh restores it (it adds the line if it
# is missing) when a version that knows it runs again.
#
# Order: the modified relay.env is first PREPARED in a file next to it (STAGED_ENV, empty if nothing to do),
# then the binaries are swapped, then the prepared file replaces relay.env. If the swap
# fails, relay.env has not moved; if the replacement fails, the swap is undone. Never one without
# the other.
STAGED_ENV=""
stage_guard_disarm() {
    STAGED_ENV=""
    [ -f "$ENV_FILE" ] || return 0
    grep -qE '^TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=' "$ENV_FILE" || return 0
    if binary_knows_guard "$PREV"; then
        return 0
    fi
    local t="${ENV_FILE}.update.$$" before after matches
    local fail="could not comment out TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR in ${ENV_FILE} (rollback cancelled, nothing was changed): ${PREV} does not know this key and would refuse to start. Remove the line by hand, then run again. (The \"does not know the key\" test also fails if /etc/tomi-relay/config.toml exists and is invalid: check it with \"${PREV} admin status --json\".)"
    # An empty file (disk full, cp or sed failing) must NEVER replace relay.env: every step is
    # tested (set -e has no effect in a function called from an \"if\"), then the
    # result is verified (non-empty, and exactly one more line per line commented out).
    cp -p -- "$ENV_FILE" "$t" || {
        rm -f -- "$t"
        die "$fail"
    }
    sed -i -E 's/^(TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=.*)$/# Commented out by update.sh before a rollback: the previous binary does not know this key and would refuse to start.\n#\1/' "$t" || {
        rm -f -- "$t"
        die "$fail"
    }
    before="$(grep -c '' "$ENV_FILE" || true)"
    after="$(grep -c '' "$t" || true)"
    matches="$(grep -cE '^TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=' "$ENV_FILE" || true)"
    if [ ! -s "$t" ] || [ "$after" -ne $((before + matches)) ] || grep -qE '^TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=' "$t"; then
        rm -f -- "$t"
        die "$fail"
    fi
    STAGED_ENV="$t"
}

# Swaps $BIN and $PREV with three renames. Undoes what was done if one of them fails.
# Return code: 0 = swapped; 1 = failure, binaries PUT BACK in place (nothing changed);
# 2 = failure AND putting back also failed (intermediate state: see swap_hint).
swap_binaries() {
    mv -f -- "$BIN" "${BIN}.swap" || return 1
    if ! mv -f -- "$PREV" "$BIN"; then
        mv -f -- "${BIN}.swap" "$BIN" && return 1
        return 2
    fi
    if ! mv -f -- "${BIN}.swap" "$PREV"; then
        if mv -f -- "$BIN" "$PREV" && mv -f -- "${BIN}.swap" "$BIN"; then
            return 1
        fi
        return 2
    fi
}

# Describes the state left by a swap whose putting back failed, with the recovery commands.
# $1 = context (the content of ${BIN}.swap is not the same):
#   "swap": first swap. Before it, ${BIN} = "A" (in service) and ${PREV} = "B" (the old one);
#           ${BIN}.swap contains A. Recovery goal: A in ${BIN}, B in ${PREV} (the state before).
#   "undo": undoing the swap, after relay.env could not be modified. The first swap had been done:
#           ${BIN} = B (the OLD one), ${PREV} = A. The undo put B into ${BIN}.swap.
#           Recovery goal: A in ${BIN}, B in ${PREV} (the state before the rollback).
swap_hint() {
    local ctx="${1:-swap}" ls_out
    ls_out="$(ls -l -- "$BIN" "${BIN}.swap" "$PREV" 2>&1 || true)"
    if [ "$ctx" = "undo" ]; then
        if [ ! -e "$BIN" ] && [ -e "${BIN}.swap" ] && [ -e "$PREV" ]; then
            printf 'current state: %s is MISSING; %s.swap contains the OLD version (the one we were rolling back to), %s the version that was in service before the rollback. Recovery (restore the earlier state): sudo mv -f %s %s && sudo mv -f %s.swap %s (then sudo systemctl restart %s if the service is stopped).\n%s' \
                "$BIN" "$BIN" "$PREV" "$PREV" "$BIN" "$BIN" "$PREV" "$SVC" "$ls_out"
        elif [ -e "$BIN" ] && [ -e "${BIN}.swap" ] && [ ! -e "$PREV" ]; then
            printf 'current state: %s contains the version that was in service before the rollback, %s.swap contains the OLD version (the one we were rolling back to), %s is MISSING. Recovery (restore the earlier state): sudo mv -f %s.swap %s.\n%s' \
                "$BIN" "$BIN" "$PREV" "$BIN" "$PREV" "$ls_out"
        else
            printf 'current state uncertain, read it before running any command. Before the rollback, %s was the version in service and %s the old one; the first swap had been done, its undo failed; %s.swap, if it exists, contains the OLD version. Files present:\n%s' \
                "$BIN" "$PREV" "$BIN" "$ls_out"
        fi
        return 0
    fi
    if [ ! -e "$BIN" ] && [ -e "${BIN}.swap" ]; then
        printf 'current state: %s is MISSING; the version that was in service is in %s.swap, the other one in %s. Recovery: sudo mv -f %s.swap %s (then sudo systemctl restart %s if the service is stopped).\n%s' \
            "$BIN" "$BIN" "$PREV" "$BIN" "$BIN" "$SVC" "$ls_out"
    elif [ -e "$BIN" ] && [ -e "${BIN}.swap" ] && [ ! -e "$PREV" ]; then
        printf 'current state: %s contains the OLD version (the one of %s before the swap), the version that was in service is in %s.swap, %s is MISSING. Recovery: sudo mv -f %s %s && sudo mv -f %s.swap %s.\n%s' \
            "$BIN" "$PREV" "$BIN" "$PREV" "$BIN" "$PREV" "$BIN" "$BIN" "$ls_out"
    else
        printf 'current state uncertain, read it before running any command. Before the swap, %s was the version in service and %s the old one; a file %s.swap is the version that was in service. Files present:\n%s' \
            "$BIN" "$PREV" "$BIN" "$ls_out"
    fi
}

swap_with_prev() {
    [ -f "$PREV" ] || die "${PREV} does not exist: nothing to roll back to"
    stage_guard_disarm
    local rc=0
    swap_binaries || rc=$?
    if [ "$rc" -eq 1 ]; then
        [ -z "$STAGED_ENV" ] || rm -f -- "$STAGED_ENV"
        die "the swap of ${BIN} and ${PREV} failed; the binaries were put back in place and ${ENV_FILE} was not modified; the service was not restarted"
    elif [ "$rc" -ne 0 ]; then
        [ -z "$STAGED_ENV" ] || rm -f -- "$STAGED_ENV"
        die "the swap of ${BIN} and ${PREV} failed AND putting back also failed: the binaries are NOT in the state from before. ${ENV_FILE} was not modified; the service was not restarted.
$(swap_hint swap)"
    fi
    if [ -n "$STAGED_ENV" ]; then
        if mv -f -- "$STAGED_ENV" "$ENV_FILE"; then
            warn "${BIN} (the old version) does not know TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR (an unknown key would prevent it from starting): the line was commented out in ${ENV_FILE}. The \"unmounted volume\" safeguard is inactive while this version runs; run infra/install.sh again once you are back on a version that knows it. (If you believe this version knows the key: the test also fails when /etc/tomi-relay/config.toml exists and is invalid; fix it, then try again.)"
        else
            rm -f -- "$STAGED_ENV"
            rc=0
            swap_binaries || rc=$?
            if [ "$rc" -eq 0 ]; then
                die "could not modify ${ENV_FILE}: the binary swap was undone (the binaries are put back in place, the service was not restarted), nothing changed. Remove the TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR line by hand if you want to go back to a version that does not know it."
            fi
            if [ "$rc" -eq 1 ]; then
                die "could not modify ${ENV_FILE}, and the binary swap could NOT be undone (nothing moved during this second attempt): ${BIN} now contains the OLD version and ${PREV} the withdrawn version. The service was not restarted. To return to the earlier state: sudo bash infra/update.sh --rollback --force (after removing the TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR line from ${ENV_FILE} if needed), or swap the two files by hand.
$(ls -l -- "$BIN" "$PREV" 2>&1 || true)"
            fi
            die "could not modify ${ENV_FILE}, and the binary swap could NOT be undone: ${BIN} and ${PREV} are in an intermediate state. The service was not restarted.
$(swap_hint undo)"
        fi
    fi
    # The service has just failed several times (new binary exiting straight away): systemd's restart
    # limit (StartLimitBurst) may have been reached, and a "restart" would then be refused even though the
    # binaries are already swapped. So the failure counter is cleared.
    systemctl reset-failed "$SVC" || true
    if ! systemctl restart "$SVC"; then
        warn "systemctl restart ${SVC} was refused or failed (the binaries are swapped: ${BIN} is now the old version)."
        return 1
    fi
}

# Is the process in service running the installed binary? 0 = yes; 1 = no (the OLD process is still
# running: a restart was refused or did not happen); 2 = cannot be verified (no PID, /proc unreadable).
PROC_DIR="/proc"
running_binary_matches() {
    local pid
    pid="$(systemctl show "$SVC" -p MainPID --value 2>/dev/null || true)"
    case "$pid" in
    '' | 0 | *[!0-9]*) return 2 ;;
    esac
    [ -r "${PROC_DIR}/${pid}/exe" ] || return 2
    if cmp -s "${PROC_DIR}/${pid}/exe" "$BIN"; then
        return 0
    fi
    return 1
}

# Arms the "unmounted volume" safeguard after a HEALTHY update (same test as install.sh: a value
# already present, true or false, is respected; a binary that knows the key and a routing.db are required). relay.env
# is rewritten through a temporary file, verified (non-empty, old content intact as a prefix, last line
# = the key), then renamed: never an empty or truncated relay.env. Takes effect at the next start. Never
# makes the update fail: in case of trouble, it warns.
arm_guard() {
    local key="TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR" t size mounts
    [ -f "$ENV_FILE" ] || return 0
    if grep -qE "^${key}=" "$ENV_FILE"; then
        return 0
    fi
    if ! exists "$(data_dir)/routing.db"; then
        warn "$(data_dir)/routing.db is missing: ${key} not armed."
        return 0
    fi
    if ! binary_knows_guard "$BIN"; then
        warn "the installed binary does not know ${key}: not armed (an unknown key would prevent it from starting)."
        return 0
    fi
    t="${ENV_FILE}.arm.$$"
    if ! cp -p -- "$ENV_FILE" "$t"; then
        rm -f -- "$t"
        warn "could not prepare ${ENV_FILE}: ${key} not armed (run infra/install.sh again)."
        return 0
    fi
    size="$(wc -c <"$ENV_FILE" | tr -d ' ')"
    if ! printf '\n# Added by update.sh after a healthy update (see relay.env.example).\n%s=true\n' "$key" >>"$t" ||
        [ ! -s "$t" ] || ! cmp -s -n "$size" "$ENV_FILE" "$t" || [ "$(tail -n 1 "$t")" != "${key}=true" ]; then
        rm -f -- "$t"
        warn "the rewrite of ${ENV_FILE} could not be verified: ${key} not armed, file unchanged (run infra/install.sh again)."
        return 0
    fi
    if ! mv -f -- "$t" "$ENV_FILE"; then
        rm -f -- "$t"
        warn "could not replace ${ENV_FILE}: ${key} not armed, file unchanged."
        return 0
    fi
    log "${key}=true armed in ${ENV_FILE} (applied at the next start)"
    mounts="$(systemctl show "$SVC" -p RequiresMountsFor --value 2>/dev/null || true)"
    if [ -z "$mounts" ]; then
        warn "the installed unit ${SVC} has no RequiresMountsFor=: update.sh NEVER replaces the systemd unit (only infra/install.sh installs it). Without it, the service may start before the volume is mounted; only the safeguard protects it then. Run infra/install.sh again to update it."
    fi
    return 0
}

if [ "$ROLLBACK" -eq 1 ]; then
    # First of all: does the running binary refuse to start because of the data? Then rolling back
    # would change nothing there (or worse). This check also covers an unmounted volume, where has_spaces
    # is false and where --force would not be required below. Even with --force for an established 78; an
    # inconclusive check stops the manual rollback, unless --force.
    refuse_if_data_refused "The rollback" manual
    # A data directory that looks empty (neither routing.db, nor spaces/, nor tomi.db): unmounted volume or
    # wrong path, even if the safeguard is not armed (relay.env without the key, or a binary that does not know it).
    # An older binary would think it is new there.
    if [ "$FORCE" -eq 0 ] && ! data_dir_has_data; then
        die "$(data_dir) contains neither routing.db, nor spaces/, nor tomi.db: volume not mounted or wrong TOMI_RELAY_DATA_DIR? The rollback was not done (an older binary would think it is new there). Mount the volume, or fix the path in ${ENV_FILE}, then run again; use --force only for a truly empty installation."
    fi
    if [ "$FORCE" -eq 0 ] && has_spaces; then
        die "spaces exist in $(data_dir)/spaces/: if ${PREV} predates the multi-space relay, it would not read them, would start empty, and clients would record their space as deleted. If ${PREV} understands this layout (or after restoring the backup taken before the update, infra/backup.md), run again with --rollback --force"
    fi
    log "Rollback: swapping ${BIN} and ${PREV}"
    if ! swap_with_prev; then
        die "rollback: the binaries are swapped but the service could not be restarted: sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC} ; journalctl -u tomi-relay -n 100 --no-pager"
    fi
    if wait_healthy; then
        log "Service healthy with the previous binary. (${PREV} now contains the withdrawn version.)"
        exit 0
    fi
    die "the service is not healthy after the rollback: journalctl -u tomi-relay -n 100 --no-pager"
fi

check_binary "$BINARY"

if cmp -s "$BINARY" "$BIN"; then
    # "Nothing to do" only counts as success if the service is REALLY healthy: a second run after a failure,
    # or after a refused restart (the old process is still running), must not exit 0.
    log "The installed binary is already identical: checking the state of the service."
    if ! systemctl is-active --quiet "$SVC"; then
        die "the installed binary is already the right one, but the service ${SVC} is NOT active: nothing is running. Read: journalctl -u tomi-relay -n 100 --no-pager ; then sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC}"
    fi
    if ! wait_healthy; then
        die "the installed binary is already the right one, but the service does not respond healthily (/healthz). Read: journalctl -u tomi-relay -n 100 --no-pager ; then sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC}"
    fi
    RUNNING_RC=0
    running_binary_matches || RUNNING_RC=$?
    if [ "$RUNNING_RC" -eq 1 ]; then
        die "the installed binary is already the right one, but the process in service runs a DIFFERENT binary (the OLD process is still running: a restart was refused?). Do: sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC} ; journalctl -u tomi-relay -n 100 --no-pager"
    elif [ "$RUNNING_RC" -ne 0 ]; then
        warn "cannot verify that the process in service runs the installed binary (PID or /proc unreadable); the service is active and responds."
    fi
    log "Nothing to do: the service is active, responds, and runs the installed binary."
    exit 0
fi

# Backup beforehand (reuses the hardened backup unit, which waits for the work to finish).
if [ "$NO_BACKUP" -eq 1 ]; then
    warn "backup skipped (--no-backup)"
elif [ -x "$BACKUP_BIN" ] && systemctl cat tomi-backup.service >/dev/null 2>&1 && command -v sqlite3 >/dev/null 2>&1; then
    log "Backup before update"
    systemctl start tomi-backup.service || die "the backup failed: update cancelled (or use --no-backup)"
else
    warn "backup unavailable (sqlite3 or unit missing): updating without a prior backup"
fi

# Did spaces already exist? If not, a successful start of the new binary may have migrated the
# data: the automatic rollback is then no longer safe (see the header).
HAD_SPACES=0
if has_spaces; then
    HAD_SPACES=1
fi

log "Installing the new binary"
install -m 0755 -o root -g root "$BINARY" "$NEW"
cp -p "$BIN" "$PREV"
mv -f "$NEW" "$BIN"

log "Restarting"
# A refused restart is not fatal here: the new binary is installed, and wait_healthy decides
# (failure => automatic rollback below). But it is recorded: if the relay responds anyway, it is
# the OLD process still running, and the update has NOT succeeded.
RESTART_FAILED=0
systemctl reset-failed "$SVC" || true
systemctl restart "$SVC" || {
    RESTART_FAILED=1
    warn "systemctl restart ${SVC} failed: the health check decides."
}

if wait_healthy; then
    if [ "$RESTART_FAILED" -eq 1 ]; then
        # Manual rollback is only advised if the data did not change layout: otherwise
        # the old binary would not read it (see the header).
        if [ "$HAD_SPACES" -eq 0 ] && has_spaces; then
            ADVICE="Do NOT run --rollback: spaces have appeared (spaces/), the old binary would not read them (restore the earlier backup first, infra/backup.md)."
        else
            ADVICE="(or sudo bash infra/update.sh --rollback to put the old binary back.)"
        fi
        die "systemctl restart ${SVC} failed and yet the service responds: it is the OLD process still running; the new binary (${BIN}) is NOT in service. Update NOT successful, no rollback done. Read: journalctl -u tomi-relay -n 100 --no-pager ; then sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC} ${ADVICE}"
    fi
    log "Update successful. Old binary kept: ${PREV} (to roll back: sudo bash infra/update.sh --rollback)"
    arm_guard
    exit 0
fi

# Does the new binary refuse to start because of the data? Then NO rollback: the old
# binary would ignore these spaces (see data_check). Before the "spaces/ appeared" test below.
journalctl -u tomi-relay -n 30 --no-pager || true
refuse_if_data_refused "The automatic rollback to the old binary" auto

if [ "$HAD_SPACES" -eq 0 ] && has_spaces; then
    warn "the service is not healthy with the new binary, but the data changed layout (spaces/ appeared): automatic rollback DISABLED, the old binary would not read them."
    die "the new binary stays installed (${BIN}). Read: journalctl -u tomi-relay -n 100 --no-pager. To go back to the old version: sudo systemctl stop tomi-relay ; restore the backup taken before the update (infra/backup.md) ; sudo bash infra/update.sh --rollback --force"
fi

warn "the service is not healthy with the new binary: automatic rollback to the old one."
if ! swap_with_prev; then
    die "rollback: the binaries are swapped (the old version is in place, the refused version is in ${PREV}) but the service could not be restarted: sudo systemctl reset-failed ${SVC} ; sudo systemctl restart ${SVC} ; journalctl -u tomi-relay -n 100 --no-pager"
fi
if wait_healthy; then
    die "rollback done, the old version is running. The refused version is in ${PREV}."
fi
die "the service is still failing after the rollback. Restore the backup (infra/backup.md) and read: journalctl -u tomi-relay -n 100"
