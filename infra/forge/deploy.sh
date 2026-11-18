#!/usr/bin/env bash
# Tomi relay — example deployment script for a managed server (pull-based deployment).
#
# This is the script a managed-server panel (such as Laravel Forge, see README.md) runs for the
# site (the "Deploy Script" field, or the panel's deploy action). It runs as the site user, in the site
# directory. A copy is kept here so the script lives in the repository; the source of truth is
# the panel's "Deploy Script" field, which must be kept identical to this file.
#
# What it does on each run (repeatable, without side effects):
#   1. reads the latest binary published in the GitHub release `relay-latest` (private repository);
#   2. if it differs from the one running, downloads it, checks its SHA-256 digest and that it is
#      an x86_64 ELF binary;
#   3. installs it by atomic rename, keeping the previous one (tomi-relay.prev);
#   4. restarts the supervisor daemon and checks /healthz;
#   5. automatically reverts to the previous binary if the relay does not come back healthy,
#      UNLESS the new binary has migrated the data (see below).
#
# Refusal to start because of the data: the relay then exits with code 78 (spec/api-v1.md §10.1:
# volume not mounted, symbolic link on a space, space directory or database unreadable, database
# belonging to another space). Going back to an older binary would then answer 409/401 for a live
# space: if the binary in place exits with 78 (`admin status --json --config …`), the script does
# NOT roll back and tells you what to check. This check has three outcomes: refusal 78 (stop),
# "ok" (0, or the legacy layout tomi.db alone, not yet migrated, which the old binary reads fine),
# or "could not conclude" (config.toml missing, timeout 124, killed 137, other code): the script
# WARNS and the automatic rollback continues (the old binary has just been copied from the binary
# in service). A missing `config.toml` skips the check with a warning. `spaces/` must be a
# subdirectory of the data volume, never the mount point itself.
#
# On a second run with the same binary, "nothing to deploy" is only a success if the daemon is
# RUNNING, /healthz answers, and the running process really executes the binary in place (otherwise: error).
#
# The relay is blind: this script never reads the database or the blobs; it only touches the binary
# and the daemon. The data (/home/deploy/tomi-data) is never modified here.
#
# Data migration: the multi-space relay moves the old layout (tomi.db + blobs/) itself, on its
# first start, to spaces/<id>/ (relay.db, blobs/) and creates routing.db. A binary from before this
# migration does not find tomi.db any more: it would start empty ("space_not_initialized") and the
# clients would permanently record their space as deleted. If spaces (spaces/*/relay.db) appear
# during a deployment that had none, the automatic rollback is therefore DISABLED: the script stops
# with an error and leaves the new binary in place (see recipes.md §3). A relay that is still empty
# (no space created yet) has nothing to lose, so the automatic rollback stays active.
#
# SERVER PREREQUISITES (set up once, outside the repository):
#   - a GitHub token with minimal scope (fine-grained, READ-ONLY: Contents = Read-only on the
#     private repository only, short lifetime) in /home/deploy/.config/tomi/github-token (mode 0600,
#     owned by the deploy user). It never goes on a command line (see gh_curl);
#   - `gh` OR `curl` available (gh is used first if it is present and authenticated).
# See infra/forge/README.md §3.

set -Eeuo pipefail
trap 'echo "deploy.sh: error at line ${LINENO} (command: ${BASH_COMMAND})" >&2' ERR

# --- Deployment parameters (adjust to your install) --------------------------
# Required: the GitHub repository that publishes the release, as OWNER/REPO (see the check below).
REPO="${TOMI_RELEASE_REPO:-}"
RELEASE_TAG="${TOMI_RELEASE_TAG:-relay-latest}"
ASSET="tomi-relay-x86_64-unknown-linux-musl"

SITE_DIR="${TOMI_SITE_DIR:-/home/deploy/relay.example.org}"
DATA_DIR="${TOMI_DATA_DIR:-/home/deploy/tomi-data}"
BIN="${SITE_DIR}/tomi-relay"
PREV="${BIN}.prev"
NEW="${BIN}.new"
HEALTH_URL="${TOMI_HEALTH_URL:-http://127.0.0.1:8080/healthz}"
# GitHub token (reading private release assets). Three possible sources, see read_token.
TOKEN_FILE="${TOMI_GITHUB_TOKEN_FILE:-/home/deploy/.config/tomi/github-token}"
SITE_ENV="${TOMI_SITE_ENV:-${SITE_DIR}/.env}"

# Name of the relay's supervisor program. The panel names it `daemon-<id>`; it is found on its own
# from its `command=` line, which points to $BIN, or it is forced with TOMI_DAEMON.
DAEMON="${TOMI_DAEMON:-}"

log() { printf '==> %s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

# --- GitHub token ------------------------------------------------------------
# Allows downloading the assets of the private repository's release. Looked up, in order:
#   1. environment variable TOMI_GITHUB_TOKEN (if the panel already exposes it to the script);
#   2. TOMI_GITHUB_TOKEN=... line of the site's .env (set from the panel's Environment tab);
#   3. dedicated file TOKEN_FILE (set up over SSH).
# The value is never printed (no echo, no set -x).
read_token() {
    GH_TOKEN=""

    # 1. Already in the environment.
    if [ -n "${TOMI_GITHUB_TOKEN:-}" ]; then
        GH_TOKEN="$(printf '%s' "$TOMI_GITHUB_TOKEN" | tr -d ' \t\r\n')"
    fi

    # 2. The site's .env (panel → Environment). We read ONLY the key we need, without sourcing the
    #    file (its contents are never executed), and strip quotes/spaces.
    if [ -z "$GH_TOKEN" ] && [ -f "$SITE_ENV" ]; then
        local line
        line="$(grep -E '^[[:space:]]*TOMI_GITHUB_TOKEN[[:space:]]*=' "$SITE_ENV" | tail -n1 || true)"
        if [ -n "$line" ]; then
            GH_TOKEN="${line#*=}"
            GH_TOKEN="$(printf '%s' "$GH_TOKEN" | tr -d ' \t\r\n' | sed -e 's/^"//' -e 's/"$//' -e "s/^'//" -e "s/'$//")"
        fi
    fi

    # 3. Dedicated file.
    if [ -z "$GH_TOKEN" ] && [ -f "$TOKEN_FILE" ]; then
        GH_TOKEN="$(tr -d ' \t\r\n' <"$TOKEN_FILE")"
    fi

    [ -n "$GH_TOKEN" ] ||
        die "GitHub token not found (neither \$TOMI_GITHUB_TOKEN, nor ${SITE_ENV}, nor ${TOKEN_FILE}); see infra/forge/README.md §3"
    # A GitHub token only has safe characters; we refuse anything else (it is written as-is into a
    # curl configuration, see gh_curl).
    case "$GH_TOKEN" in
    *[!A-Za-z0-9_.-]*) die "the GitHub token contains unexpected characters (expected a fine-grained token starting with \"github_pat_…\")" ;;
    esac
}

# curl to the GitHub API with the token, WITHOUT putting it on the command line (visible in `ps`
# and /proc/<pid>/cmdline to any account on the server): the Authorization header is passed on
# standard input as a curl configuration (`-K -`). printf is a builtin: no process carries the
# token as an argument.
gh_curl() {
    printf 'header = "Authorization: Bearer %s"\n' "$GH_TOKEN" | curl -K - "$@"
}

# --- Supervisor daemon resolution --------------------------------------------
resolve_daemon() {
    [ -z "$DAEMON" ] || return 0
    # The program whose `command=` line runs our binary.
    local conf
    conf="$(grep -ils -- "$BIN" /etc/supervisor/conf.d/*.conf 2>/dev/null | head -n1 || true)"
    [ -n "$conf" ] || die "supervisor daemon not found (no conf.d file runs ${BIN}); set TOMI_DAEMON"
    DAEMON="$(sed -n 's/^\[program:\(.*\)\]$/\1/p' "$conf" | head -n1)"
    [ -n "$DAEMON" ] || die "cannot read the program name from ${conf}"
}

supervisor() { sudo supervisorctl "$@"; }

# Is there at least one space in the multi-space layout (spaces/<id>/relay.db)?
has_spaces() { compgen -G "${DATA_DIR}/spaces/*/relay.db" >/dev/null; }

# Does a path exist (even a broken link)?
exists() { [ -e "$1" ] || [ -L "$1" ]; }

# Is the legacy layout intact (tomi.db alone, neither spaces/ nor spaces.migrating/)? A binary from
# before the migration reads it perfectly well: this is not a refusal because of the data.
legacy_intact() {
    exists "${DATA_DIR}/tomi.db" && ! exists "${DATA_DIR}/spaces" && ! exists "${DATA_DIR}/spaces.migrating"
}

# Does the binary in place ($BIN) REFUSE to start because of the DATA? The relay then exits with the
# dedicated code 78 (spec/api-v1.md §10.1): volume not mounted, symbolic link on spaces/<id> or
# relay.db, directory or database unreadable (permissions), database of another space... Rolling back
# to a binary from BEFORE multi-space would be worse than staying down: it ignores these spaces,
# starts "empty" and answers 409 space_not_initialized / 401 for a live space (and creates an install
# token if there is no invitation). The test is `admin status --json` with the daemon's configuration
# (--config) and the deploy user (the daemon's user), from its directory (`directory=` of the daemon).
# Result in DATA_STATE: "refused" (78 established), "ok" (the binary read the data, or the legacy
# layout is intact), "unknown" (the check could not conclude: config.toml missing, timeout 124,
# process killed 137, other code). DATA_REFUSED_WHY says why.
EXIT_DATA_REFUSED=78
CONFIG_FILE="${TOMI_CONFIG:-${DATA_DIR}/config.toml}"
DATA_STATE=""
DATA_REFUSED_WHY=""
data_check() {
    local rc=0 out=""
    DATA_STATE="unknown"
    DATA_REFUSED_WHY=""
    if [ ! -f "$CONFIG_FILE" ]; then
        DATA_REFUSED_WHY="${CONFIG_FILE} is missing: the check (admin status --config) is SKIPPED"
        return 0
    fi
    out="$(cd "$DATA_DIR" 2>/dev/null || cd /; timeout 30 "$BIN" --config "$CONFIG_FILE" admin status --json 2>&1 >/dev/null)" || rc=$?
    if [ "$rc" -eq 0 ]; then
        DATA_STATE="ok"
    elif [ "$rc" -eq "$EXIT_DATA_REFUSED" ]; then
        DATA_STATE="refused"
        DATA_REFUSED_WHY="${out:-code ${EXIT_DATA_REFUSED}}"
    elif legacy_intact; then
        # Non-zero exit, but this is the legacy layout not yet migrated (admin status refuses to read
        # it without migrating it): nothing to reproach the data for.
        DATA_STATE="ok"
    else
        DATA_REFUSED_WHY="admin status exited with code ${rc} (124 = timeout, 137 = killed)${out:+ : ${out}}"
    fi
    return 0
}

# Stops the script (without rollback) if the binary in place refuses to start because of the data;
# warns if the check could not conclude (the automatic rollback then continues: the old binary has
# just been copied from the binary in service, and staying down would not help).
refuse_if_data_refused() {
    data_check
    case "$DATA_STATE" in
    ok) return 0 ;;
    refused)
        die "the binary in place (${BIN}) REFUSES to start because of the DATA (exit code ${EXIT_DATA_REFUSED}): ${DATA_REFUSED_WHY}
    The rollback to the old binary is NOT done: it would ignore these spaces, start empty and answer 409 space_not_initialized / 401 for a live space.
    To check: the volume of ${DATA_DIR} is mounted; ${DATA_DIR}/spaces/ is a SUBDIRECTORY of the volume (never the mount point itself); owner is the deploy user with permissions on ${DATA_DIR}, spaces/ and each spaces/<id>/; spaces/<id> and spaces/<id>/relay.db are not symbolic links; the disk has no I/O errors.
    Then: sudo supervisorctl restart ${DAEMON}: ; sudo supervisorctl tail -1000 ${DAEMON} stdout"
        ;;
    esac
    warn "cannot check whether the binary in place refuses to start because of the data: ${DATA_REFUSED_WHY}. The automatic rollback continues without this check. Manual check: cd ${DATA_DIR} && ${BIN} --config ${CONFIG_FILE} admin status ; echo \$?  (78 = refusal because of the data: do not roll back)."
}

# The daemon's process (PID): empty if the daemon is not RUNNING.
daemon_pid() {
    local out
    out="$( (supervisor status "${DAEMON}:" 2>/dev/null || supervisor status "$DAEMON" 2>/dev/null) || true)"
    printf '%s\n' "$out" | sed -n 's/.*[[:space:]]RUNNING[[:space:]]*pid \([0-9][0-9]*\),.*/\1/p' | head -n 1 || true
}

# Does process $1 run the binary in place? 0 = yes; 1 = no (the OLD process is still running);
# 2 = cannot be verified (/proc/<pid>/exe unreadable).
PROC_DIR="/proc"
running_binary_matches() {
    [ -r "${PROC_DIR}/${1}/exe" ] || return 2
    if cmp -s "${PROC_DIR}/${1}/exe" "$BIN"; then
        return 0
    fi
    return 1
}

restart_daemon() {
    # `restart <name>:` covers the numprocs groups (daemon-xxxx:daemon-xxxx_00).
    # Returns 1 if supervisor reports an error: a binary that exits right away makes `restart` report
    # an error even though the binaries have already been swapped. The caller does NOT die on that:
    # it is wait_healthy that decides (and, on the first attempt, the rollback).
    supervisor restart "${DAEMON}:" >/dev/null 2>&1 ||
        supervisor restart "$DAEMON" >/dev/null 2>&1 ||
        return 1
}

# --- Health check ------------------------------------------------------------
wait_healthy() {
    local i
    for i in $(seq 1 30); do
        if curl -fsS --max-time 2 "$HEALTH_URL" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

check_binary() {
    local f="$1" magic machine
    [ -s "$f" ] || die "empty or missing binary: $f"
    magic="$(head -c 4 "$f" | od -An -c | tr -d ' ')"
    [ "$magic" = "177ELF" ] || die "$f is not a Linux executable (ELF)"
    machine="$(od -An -tx1 -j18 -N1 "$f" | tr -d ' ')"
    [ "$machine" = "3e" ] || die "$f is not x86_64 (ELF machine byte = ${machine})"
}

# --- Release download --------------------------------------------------------
# Fills $DL_DIR with $ASSET and $ASSET.sha256 from the release $RELEASE_TAG.
download_release() {
    DL_DIR="$(mktemp -d)"
    if command -v gh >/dev/null 2>&1 && GH_TOKEN="$GH_TOKEN" gh auth status >/dev/null 2>&1; then
        log "Downloading via gh (${REPO}, ${RELEASE_TAG})"
        GH_TOKEN="$GH_TOKEN" gh release download "$RELEASE_TAG" -R "$REPO" \
            -p "$ASSET" -p "$ASSET.sha256" -D "$DL_DIR" --clobber
    else
        # Path used on the example server, where gh is not installed. Release ASSETS go through the
        # HTTPS API (a read-only SSH deploy key does not cover them): hence the token.
        log "Downloading via the GitHub API (${REPO}, ${RELEASE_TAG})"
        command -v jq >/dev/null 2>&1 || die "jq is required to read the GitHub API (apt-get install jq)"
        local api="https://api.github.com/repos/${REPO}/releases/tags/${RELEASE_TAG}"
        local meta
        meta="$(gh_curl -fsSL \
            -H "Accept: application/vnd.github+json" \
            -H "X-GitHub-Api-Version: 2022-11-28" "$api")" ||
            die "release ${RELEASE_TAG} not found (valid token? repository ${REPO}?)"
        for name in "$ASSET" "$ASSET.sha256"; do
            local id
            # jq reads the asset id by its name, whatever the JSON formatting.
            id="$(printf '%s' "$meta" | jq -r --arg n "$name" \
                '.assets[] | select(.name == $n) | .id' 2>/dev/null | head -n1)"
            [ -n "$id" ] && [ "$id" != "null" ] ||
                die "asset ${name} missing from release ${RELEASE_TAG}"
            gh_curl -fsSL \
                -H "Accept: application/octet-stream" \
                -H "X-GitHub-Api-Version: 2022-11-28" \
                "https://api.github.com/repos/${REPO}/releases/assets/${id}" \
                -o "${DL_DIR}/${name}" || die "download of ${name} failed"
        done
    fi
    [ -s "${DL_DIR}/${ASSET}" ] || die "binary not downloaded"
    [ -s "${DL_DIR}/${ASSET}.sha256" ] || die "digest not downloaded"
}

cleanup() { [ -n "${DL_DIR:-}" ] && rm -rf "$DL_DIR" || true; }
trap cleanup EXIT

# --- Main flow ---------------------------------------------------------------
log "Deploying the relay (site ${SITE_DIR})"
[ -n "$REPO" ] || die "TOMI_RELEASE_REPO must be set to the GitHub repository that publishes the release, as OWNER/REPO (for example in the panel's Environment, or exported before running this script)"
command -v curl >/dev/null 2>&1 || die "curl is required"
[ -d "$SITE_DIR" ] || die "site directory missing: ${SITE_DIR}"
read_token
resolve_daemon
log "Supervisor daemon: ${DAEMON}"

download_release

# Integrity check: the published digest must match the downloaded binary.
# This guarantees the INTEGRITY of the transfer, not its AUTHENTICITY: the digest comes from the same
# release as the binary, so whoever can modify the release can modify both. Next step: sign the
# release (minisign or a GitHub attestation) and verify the signature here (infra/forge/README.md §8).
( cd "$DL_DIR" && sha256sum --check "${ASSET}.sha256" ) >/dev/null ||
    die "invalid SHA-256 digest: corrupted download, nothing has been installed"
check_binary "${DL_DIR}/${ASSET}"

if [ -f "$BIN" ] && cmp -s "${DL_DIR}/${ASSET}" "$BIN"; then
    # "Nothing to deploy" is only a success if the relay is REALLY healthy: a second run after a failure,
    # or after a refused restart (the old process is still running), must not exit 0.
    log "The binary in place is already the latest published one: checking the relay's state."
    PID="$(daemon_pid)"
    if [ -z "$PID" ]; then
        die "the binary in place is already the latest published one, but the daemon ${DAEMON} is NOT running (RUNNING): nothing is in service. Read: sudo supervisorctl status ; sudo supervisorctl tail -1000 ${DAEMON} stdout ; then sudo supervisorctl restart ${DAEMON}:"
    fi
    if ! wait_healthy; then
        die "the binary in place is already the latest published one, but ${HEALTH_URL} does not answer. Read: sudo supervisorctl tail -1000 ${DAEMON} stdout ; then sudo supervisorctl restart ${DAEMON}:"
    fi
    RUNNING_RC=0
    running_binary_matches "$PID" || RUNNING_RC=$?
    if [ "$RUNNING_RC" -eq 1 ]; then
        die "the binary in place is already the latest published one, but the running process (PID ${PID}) executes ANOTHER binary: the OLD process is still running (a restart was refused?). Do: sudo supervisorctl restart ${DAEMON}: ; sudo supervisorctl tail -1000 ${DAEMON} stdout"
    elif [ "$RUNNING_RC" -ne 0 ]; then
        warn "cannot verify that the running process (PID ${PID}) executes the binary in place (/proc unreadable); the daemon is RUNNING and ${HEALTH_URL} answers."
    fi
    log "Nothing to deploy: the relay is running, answers, and executes the binary in place."
    exit 0
fi

# Did the spaces already exist in the new layout? If not, a successful start of the new binary may
# have migrated the data, and the automatic rollback is no longer safe (see the header).
HAD_SPACES=0
if has_spaces; then
    HAD_SPACES=1
fi

log "Installing the new binary (atomic, previous one kept as .prev)"
install -m 0755 "${DL_DIR}/${ASSET}" "$NEW"
[ -f "$BIN" ] && cp -p "$BIN" "$PREV"
mv -f "$NEW" "$BIN"

log "Restarting the daemon"
# A refusal from supervisor is not fatal here (the new binary is installed, the health check decides),
# but it is recorded: if the relay answers anyway, it is the OLD process that is still running and the
# deployment has NOT succeeded.
RESTART_FAILED=0
restart_daemon || {
    RESTART_FAILED=1
    warn "supervisor reported an error when restarting ${DAEMON}: the health check decides (rollback if needed)."
}

if wait_healthy; then
    if [ "$RESTART_FAILED" -eq 1 ]; then
        die "supervisor reported an error when restarting ${DAEMON}, yet /healthz answers: it is the OLD process that is still running; the new binary (${BIN}) is NOT in service. Deployment NOT successful, no rollback done. Read: sudo supervisorctl status ; sudo supervisorctl tail -1000 ${DAEMON} stdout ; then sudo supervisorctl restart ${DAEMON}:"
    fi
    log "Deployment successful; /healthz answers. Previous binary: ${PREV}"
    exit 0
fi

# Does the new binary refuse to start because of the data? Then NO rollback: the old binary would ignore
# these spaces (see data_refused). Before the "spaces/ appeared" test below.
refuse_if_data_refused

if [ "$HAD_SPACES" -eq 0 ] && has_spaces; then
    warn "the relay is not healthy with the new binary, but the data changed layout (spaces/ appeared): automatic rollback DISABLED, the old binary would not read them."
    die "the new binary stays in place. Logs: sudo supervisorctl tail -1000 ${DAEMON} stdout (or /home/deploy/logs/tomi-relay.log). To go back to the old version: stop the daemon, restore the backup taken before (infra/backup.md), then put tomi-relay.prev back in place (infra/forge/recipes.md §3)."
fi

if [ ! -f "$PREV" ]; then
    die "the relay is not healthy with the new binary, and there is no old binary (${PREV} missing: first deployment): no rollback possible. Logs: sudo supervisorctl tail -1000 ${DAEMON} stdout"
fi

warn "the relay is not healthy with the new binary: rolling back to the old one."
mv -f "$BIN" "${BIN}.ko"
mv -f "$PREV" "$BIN"
mv -f "${BIN}.ko" "$PREV"
RESTART_FAILED=0
restart_daemon || {
    RESTART_FAILED=1
    warn "supervisor reported an error when restarting ${DAEMON} after the rollback: the health check decides."
}
if wait_healthy; then
    if [ "$RESTART_FAILED" -eq 1 ]; then
        die "rollback: the binaries are swapped (the old version is in place, the refused version is in ${PREV}), supervisor reported an error on restart but /healthz answers: check that the running process is indeed the old one (sudo supervisorctl status) before concluding."
    fi
    die "rollback done, the old version is running. The refused version is in ${PREV}."
fi
die "the relay is failing after the rollback. Logs: sudo supervisorctl tail -1000 ${DAEMON} stdout"
