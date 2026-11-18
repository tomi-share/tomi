#!/usr/bin/env bash
# Tomi relay — idempotent installation. Run AS ROOT on the server (Linux + systemd).
#
#   sudo bash infra/install.sh /path/to/tomi-relay [--no-caddy]
#
# What the script does (each step can be replayed without side effects):
#   - creates the `tomi` system user (no shell) and its directories;
#   - installs the binary (only if it is not already installed: for an update, use update.sh);
#   - creates /etc/tomi/relay.env from relay.env.example ONLY if it does not exist;
#   - installs the systemd units and the admin and backup scripts;
#   - installs the Caddyfile if Caddy is present and the current configuration is not customized
#     (--no-caddy on a server where nginx already holds ports 80/443);
#   - enables and starts tomi-relay (and the backup timer if sqlite3 is installed).
#
# Never deletes or overwrites: /etc/tomi/relay.env, the FCM credentials, /var/lib/tomi,
# a customized Caddyfile, or an already installed binary.

set -Eeuo pipefail
trap 'echo "install.sh: error on line ${LINENO} (command: ${BASH_COMMAND})" >&2' ERR

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

SVC_USER="tomi"
DATA_DIR="/var/lib/tomi"
CONF_DIR="/etc/tomi"
ENV_FILE="${CONF_DIR}/relay.env"
BACKUP_DIR="/var/backups/tomi"
BIN_DST="/usr/local/bin/tomi-relay"
SBIN_DIR="/usr/local/sbin"
UNIT_DIR="/etc/systemd/system"
CADDY_DIR="/etc/caddy"

log() { printf '==> %s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: sudo bash infra/install.sh /path/to/tomi-relay [--no-caddy]

  --no-caddy   do not touch the Caddy configuration
EOF
}

# --- Arguments ---------------------------------------------------------------
BINARY=""
SKIP_CADDY=0
while [ "$#" -gt 0 ]; do
    case "$1" in
    --no-caddy) SKIP_CADDY=1 ;;
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
[ -n "$BINARY" ] || {
    usage >&2
    exit 2
}

# --- Preflight checks --------------------------------------------------------
[ "$(id -u)" -eq 0 ] || die "run as root (sudo bash infra/install.sh ...)"
[ "$(uname -s)" = "Linux" ] || die "Linux only"
command -v systemctl >/dev/null 2>&1 || die "systemd is required"
[ -d /run/systemd/system ] || die "systemd does not appear to be the active init system"

for f in systemd/tomi-relay.service systemd/tomi-backup.service systemd/tomi-backup.timer \
    relay.env.example tomi-admin.sh backup.sh caddy/Caddyfile; do
    [ -f "${SCRIPT_DIR}/${f}" ] || die "missing file: ${SCRIPT_DIR}/${f} (copy the whole infra/ folder)"
done

# The binary must be an ELF executable for the same architecture as this machine.
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
check_binary "$BINARY"

# Installs $1 to $2 (mode $3) if the content differs. Returns 0 if the file changed, 1 otherwise.
install_if_changed() {
    local src="$1" dst="$2" mode="$3" owner="${4:-root}" group="${5:-root}"
    if [ -f "$dst" ] && cmp -s "$src" "$dst"; then
        return 1
    fi
    install -m "$mode" -o "$owner" -g "$group" "$src" "$dst"
    return 0
}

health_url() {
    local listen=""
    if [ -f "$ENV_FILE" ]; then
        listen="$(grep -E '^TOMI_RELAY_LISTEN=' "$ENV_FILE" | tail -n 1 | cut -d= -f2- || true)"
    fi
    printf 'http://%s/healthz' "${listen:-127.0.0.1:8080}"
}

# --- User and directories ----------------------------------------------------
log "System user ${SVC_USER}"
if ! getent group "$SVC_USER" >/dev/null; then
    groupadd --system "$SVC_USER"
fi
if ! id -u "$SVC_USER" >/dev/null 2>&1; then
    NOLOGIN="$(command -v nologin || true)"
    NOLOGIN="${NOLOGIN:-/usr/sbin/nologin}"
    useradd --system --gid "$SVC_USER" --home-dir "$DATA_DIR" --no-create-home \
        --shell "$NOLOGIN" --comment "Tomi relay" "$SVC_USER"
else
    echo "    already present"
fi

log "Directories"
# Created only if missing: the permissions and owner of an existing directory are never changed.
mkdir_once() { # path mode owner group
    if [ -d "$1" ]; then
        echo "    $1 already exists (unchanged)"
    else
        install -d -m "$2" -o "$3" -g "$4" "$1"
        echo "    $1 created"
    fi
}
mkdir_once "$DATA_DIR" 0750 "$SVC_USER" "$SVC_USER"
mkdir_once "${DATA_DIR}/public" 0750 "$SVC_USER" "$SVC_USER"
mkdir_once "${DATA_DIR}/public/updates" 0750 "$SVC_USER" "$SVC_USER"
mkdir_once "${DATA_DIR}/public/.well-known" 0750 "$SVC_USER" "$SVC_USER"
mkdir_once "${DATA_DIR}/public/l" 0750 "$SVC_USER" "$SVC_USER"
mkdir_once "$CONF_DIR" 0750 root "$SVC_USER"
mkdir_once "$BACKUP_DIR" 0750 "$SVC_USER" "$SVC_USER"

# --- Configuration -----------------------------------------------------------
log "Configuration ${ENV_FILE}"
FIRST_CONFIG=0
if [ -f "$ENV_FILE" ]; then
    echo "    already exists: kept as is"
else
    install -m 0640 -o root -g "$SVC_USER" "${SCRIPT_DIR}/relay.env.example" "$ENV_FILE"
    FIRST_CONFIG=1
    echo "    created from relay.env.example (review it)"
fi

# --- Binary and scripts ------------------------------------------------------
log "Binary ${BIN_DST}"
if [ ! -f "$BIN_DST" ]; then
    install -m 0755 -o root -g root "$BINARY" "$BIN_DST"
    echo "    installed"
elif cmp -s "$BINARY" "$BIN_DST"; then
    echo "    already up to date"
else
    warn "a different binary is already installed: not replaced. To update: bash infra/update.sh ${BINARY}"
fi

log "Admin and backup scripts"
for pair in "tomi-admin.sh:tomi-admin" "backup.sh:tomi-backup"; do
    src="${pair%%:*}"
    dst="${SBIN_DIR}/${pair##*:}"
    if install_if_changed "${SCRIPT_DIR}/${src}" "$dst" 0755; then
        echo "    ${dst} installed/updated"
    else
        echo "    ${dst} up to date"
    fi
done

# --- systemd units -----------------------------------------------------------
log "systemd units"
UNITS_CHANGED=0
RELAY_UNIT_CHANGED=0
for u in tomi-relay.service tomi-backup.service tomi-backup.timer; do
    if install_if_changed "${SCRIPT_DIR}/systemd/${u}" "${UNIT_DIR}/${u}" 0644; then
        echo "    ${u} installed/updated"
        UNITS_CHANGED=1
        if [ "$u" = "tomi-relay.service" ]; then
            RELAY_UNIT_CHANGED=1
        fi
    else
        echo "    ${u} up to date"
    fi
done
if [ "$UNITS_CHANGED" -eq 1 ]; then
    systemctl daemon-reload
fi

# --- Starting the relay ------------------------------------------------------
log "tomi-relay service"
systemctl enable tomi-relay.service >/dev/null 2>&1
if systemctl is-active --quiet tomi-relay.service; then
    if [ "$RELAY_UNIT_CHANGED" -eq 1 ]; then
        systemctl restart tomi-relay.service
        echo "    restarted (unit changed)"
    else
        echo "    already active"
    fi
else
    systemctl start tomi-relay.service
    echo "    started"
fi

# --- Backup ------------------------------------------------------------------
log "Nightly backup"
if command -v sqlite3 >/dev/null 2>&1; then
    systemctl enable --now tomi-backup.timer >/dev/null 2>&1
    echo "    timer enabled (archives in ${BACKUP_DIR}, see infra/backup.md)"
else
    warn "sqlite3 is missing: backup not enabled. Install it (apt install sqlite3 / dnf install sqlite), then run this script again."
fi

# --- Caddy -------------------------------------------------------------------
CADDY_CHANGED=0
if [ "$SKIP_CADDY" -eq 1 ]; then
    log "Caddy: skipped (--no-caddy)"
elif ! command -v caddy >/dev/null 2>&1; then
    warn "Caddy is not installed: no TLS. If nginx already serves this server, run again with --no-caddy and follow infra/README.md (\"Server already served by nginx\"). Otherwise install Caddy (same file, step 4), then run this script again."
else
    log "Caddy"
    SRC_CADDY="${SCRIPT_DIR}/caddy/Caddyfile"
    DST_CADDY="${CADDY_DIR}/Caddyfile"
    mkdir -p "$CADDY_DIR"
    if [ ! -f "$DST_CADDY" ]; then
        install -m 0644 -o root -g root "$SRC_CADDY" "$DST_CADDY"
        CADDY_CHANGED=1
        echo "    Caddyfile installed"
    elif cmp -s "$SRC_CADDY" "$DST_CADDY"; then
        echo "    Caddyfile already up to date"
    elif grep -q "easy way to configure your Caddy web server" "$DST_CADDY"; then
        # Example file shipped by the package: back it up, then replace it.
        [ -f "${DST_CADDY}.dist" ] || cp -p "$DST_CADDY" "${DST_CADDY}.dist"
        install -m 0644 -o root -g root "$SRC_CADDY" "$DST_CADDY"
        CADDY_CHANGED=1
        echo "    package Caddyfile replaced (backup: ${DST_CADDY}.dist)"
    else
        install -m 0644 -o root -g root "$SRC_CADDY" "${DST_CADDY}.tomi"
        warn "a customized Caddyfile exists: not modified. Our version is in ${DST_CADDY}.tomi, to be merged by hand."
    fi

    if [ "$CADDY_CHANGED" -eq 1 ]; then
        if id -u caddy >/dev/null 2>&1; then
            runuser -u caddy -- caddy validate --config "$DST_CADDY" --adapter caddyfile >/dev/null ||
                die "invalid Caddyfile (caddy validate failed)"
        else
            caddy validate --config "$DST_CADDY" --adapter caddyfile >/dev/null ||
                die "invalid Caddyfile (caddy validate failed)"
        fi
    fi

    if systemctl cat caddy.service >/dev/null 2>&1; then
        systemctl enable caddy.service >/dev/null 2>&1
        if systemctl is-active --quiet caddy.service; then
            if [ "$CADDY_CHANGED" -eq 1 ]; then
                systemctl reload caddy.service
                echo "    Caddy reloaded"
            fi
        else
            systemctl start caddy.service
            echo "    Caddy started"
        fi
    else
        warn "caddy.service unit not found: start Caddy yourself with ${DST_CADDY}."
    fi
fi

# --- Final check -------------------------------------------------------------
log "Health check"
HEALTH_OK=0
for _ in $(seq 1 20); do
    if ! systemctl is-active --quiet tomi-relay.service; then
        sleep 1
        continue
    fi
    if command -v curl >/dev/null 2>&1; then
        if curl -fsS --max-time 2 "$(health_url)" >/dev/null 2>&1; then
            HEALTH_OK=1
            break
        fi
    else
        HEALTH_OK=1
        break
    fi
    sleep 1
done

if ! systemctl is-active --quiet tomi-relay.service; then
    systemctl --no-pager --full status tomi-relay.service || true
    die "tomi-relay is not active. See: journalctl -u tomi-relay -n 100 --no-pager"
fi
if [ "$HEALTH_OK" -eq 1 ]; then
    echo "    tomi-relay active, $(health_url) responds"
else
    warn "tomi-relay is active but $(health_url) does not respond (does the relay implement /healthz?)."
fi

# --- "Unmounted volume" safeguard ---------------------------------------------
# After a successful first start, the data directory contains routing.db: from then on,
# a relay that finds it empty (unmounted volume, wrong path) must refuse to start rather than
# treat itself as new. Added only once; an existing value (true or false) is respected.
# Only with a binary that knows the key (an unknown variable would make startup fail).
log "Data directory safeguard"
data_dir_of_env() {
    local d=""
    if [ -f "$ENV_FILE" ]; then
        d="$(grep -E '^TOMI_RELAY_DATA_DIR=' "$ENV_FILE" | tail -n 1 | cut -d= -f2- || true)"
    fi
    printf '%s' "${d:-$DATA_DIR}"
}
# Does binary $1 know the key? Tested on the binary ALONE (not on the running relay, whose admin socket
# would answer with ITS version): empty environment (env -i), throwaway empty data directory,
# relay.env not read. A binary that does not know the key fails (unknown key).
binary_knows_guard() {
    local tmp rc=0
    tmp="$(mktemp -d)" || return 1
    timeout 30 env -i TOMI_RELAY_DATA_DIR="$tmp" TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=false \
        "$1" admin status --json >/dev/null 2>&1 || rc=$?
    [ -z "$tmp" ] || rm -rf -- "$tmp"
    [ "$rc" -eq 0 ]
}
if grep -qE '^TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=' "$ENV_FILE"; then
    echo "    already set in ${ENV_FILE} (unchanged)"
elif [ "$HEALTH_OK" -ne 1 ]; then
    warn "relay not verified: TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR not added (run this script again once the relay is healthy)."
elif [ ! -e "$(data_dir_of_env)/routing.db" ]; then
    warn "$(data_dir_of_env)/routing.db is missing: TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR not added."
elif ! binary_knows_guard "$BIN_DST"; then
    warn "the installed binary does not know TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR yet: not added (update, then run this script again)."
else
    printf '\n# Added by install.sh after the first successful start (see relay.env.example).\nTOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=true\n' >>"$ENV_FILE"
    echo "    TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=true added (applied at the next start)"
fi

CONFIG_NOTE=""
if [ "$FIRST_CONFIG" -eq 1 ]; then
    CONFIG_NOTE=" (just created)"
fi

cat <<EOF

Installation complete.

Next steps:
  1. Review ${ENV_FILE}${CONFIG_NOTE}.
  2. Get the installation token. On the very first start of a new relay, it is written
     only once to the log (line "install token: ..."):
       sudo journalctl -u tomi-relay --no-pager | grep "install token"
     The relay keeps only its fingerprint. If you no longer have it, or to get a token that is
     guaranteed to work, replace the pending invitations with a single new one and display its token
     (the token from the log then stops working):
       sudo tomi-admin invite --rotate
     ("sudo tomi-admin invite" without options only LISTS the invitations and never shows a token;
     "invite --new" adds one more, for another person.)
     The token from the log never expires and stays readable by root and by the adm group: after
     creating the first space, run "sudo tomi-admin invite --rotate" to cancel it.
  3. Relay status:  sudo tomi-admin status
Details: infra/README.md
EOF
