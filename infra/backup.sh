#!/usr/bin/env bash
# Tomi relay — backup of the data directory (see infra/backup.md).
# Installed by install.sh under /usr/local/sbin/tomi-backup; run every night by
# tomi-backup.timer (user `tomi`, environment of /etc/tomi/relay.env).
# To run it by hand with the same rights and settings as the schedule:
#   sudo systemctl start tomi-backup.service
#
# The script needs neither root nor the `tomi` user: it only has to read the data directory
# and write to the destination (sqlite3 opens databases read-only (-readonly) but must
# be able to write their -shm file: so the account that runs the relay). On a host where the relay
# runs under another account, set TOMI_RELAY_DATA_DIR and TOMI_BACKUP_DIR accordingly, for example:
#   TOMI_RELAY_DATA_DIR=/srv/tomi-data TOMI_BACKUP_DIR=/srv/tomi-backups tomi-backup
#
# Data layout (the relay is multi-space):
#   <data>/routing.db                          global routing database
#   <data>/spaces/<space_id>/relay.db          one SQLite database per space
#   <data>/spaces/<space_id>/blobs/<2 hex>/…   encrypted blobs of the space
# Former layout (a single space: <data>/tomi.db + <data>/blobs/) is still supported as long as
# it is present (the relay migrates it by itself at startup).
#
# Each database is copied with sqlite3's `.backup` command (online backup API: a consistent copy
# even while the relay is writing). NEVER copy a live .db / -wal / -shm file with cp:
# the result may be corrupted.
#
# Blobs are NOT copied into a working folder: tar reads them directly at their location, AFTER the
# database snapshots. No hard links either: a blob being uploaded is completed by appends, so a hard link
# would grow afterwards and the snapshot would no longer be frozen. The disk therefore only holds a
# temporary copy of the (small) databases and the archive.
# Disk peak of a backup ≈ data + TOMI_BACKUP_KEEP archives + 1 archive being written.
#
# Variables (all optional):
#   TOMI_RELAY_DATA_DIR         relay data directory               (default /var/lib/tomi)
#   TOMI_BACKUP_DIR             archive destination                (default /var/backups/tomi)
#   TOMI_BACKUP_KEEP            local archives kept                (default 2, minimum 1)
#   TOMI_BACKUP_MIN_FREE_BYTES  free space to preserve, in bytes, once the archive is written
#                               (default 2147483648 = 2 GiB); below that, the backup is refused.
#                               The relay refuses uploads when free disk space drops below its
#                               `min_free_disk_bytes` (1 GiB by default): with the same margin, a
#                               backup close to the limit would make uploads be refused (`relay_full`)
#                               for as long as it runs. 2 GiB leaves 1 GiB more than the relay's floor.
#   TOMI_BACKUP_DB              legacy layout only: SQLite file to back up
#                               (default: auto-detection in the data directory)
#
# Exit codes: 0 success; 1 error; 3 not enough disk space (nothing was written).

set -Eeuo pipefail
umask 077

DATA_DIR="${TOMI_RELAY_DATA_DIR:-/var/lib/tomi}"
OUT_DIR="${TOMI_BACKUP_DIR:-/var/backups/tomi}"
KEEP="${TOMI_BACKUP_KEEP:-2}"
MIN_FREE="${TOMI_BACKUP_MIN_FREE_BYTES:-2147483648}"

EXIT_NO_SPACE=3

die() {
    printf 'tomi-backup: %s\n' "$*" >&2
    exit 1
}
warn() { printf 'tomi-backup: warning: %s\n' "$*" >&2; }
is_uint() {
    case "$1" in
    '' | *[!0-9]*) return 1 ;;
    esac
    return 0
}
mib() { echo $((($1 + 1048575) / 1048576)); }

command -v sqlite3 >/dev/null 2>&1 || die "sqlite3 is required (apt install sqlite3 / dnf install sqlite)"

is_uint "$KEEP" || die "TOMI_BACKUP_KEEP must be an integer"
[ "$KEEP" -ge 1 ] || die "TOMI_BACKUP_KEEP must be at least 1"
is_uint "$MIN_FREE" || die "TOMI_BACKUP_MIN_FREE_BYTES must be an integer (bytes)"
[ -d "$DATA_DIR" ] || die "data directory not found: ${DATA_DIR}"
[ -d "$OUT_DIR" ] || die "destination missing: ${OUT_DIR}"

# Only one backup at a time (timer + update.sh): two in parallel would double the disk peak.
if command -v flock >/dev/null 2>&1; then
    exec 9>"${OUT_DIR}/.lock"
    flock -n 9 || die "another backup is already running"
fi

# Leftovers of a run killed outright (OOM, power cut): never counted as archives, but they take up disk space.
find "$OUT_DIR" -maxdepth 1 \( -name '.work.*' -o -name 'tomi-*.tar.gz.partial' \) -mmin +1440 \
    -exec rm -rf -- {} + 2>/dev/null || true

# --- 1. Inventory: which databases, which blobs ------------------------------------------------
# SRC_DBS[i] is copied to <work>/REL_DBS[i], which is also its path inside the archive.
# BLOB_RELS: blob folders (paths relative to DATA_DIR), archived as they are.
SRC_DBS=()
REL_DBS=()
BLOB_RELS=()

DB_PATH="${TOMI_BACKUP_DB:-}"
if [ -z "$DB_PATH" ] && { [ -f "${DATA_DIR}/routing.db" ] || [ -d "${DATA_DIR}/spaces" ]; }; then
    LAYOUT="spaces"
else
    LAYOUT="legacy"
fi

if [ "$LAYOUT" = "spaces" ]; then
    if [ -f "${DATA_DIR}/routing.db" ]; then
        SRC_DBS+=("${DATA_DIR}/routing.db")
        REL_DBS+=("routing.db")
    else
        warn "routing.db missing: only the space databases are backed up"
    fi
    if [ -f "${DATA_DIR}/tomi.db" ] || [ -d "${DATA_DIR}/blobs" ]; then
        warn "former layout (tomi.db, blobs/) still present next to the new one: ignored; has the relay's migration finished?"
    fi
    shopt -s nullglob
    for dir in "${DATA_DIR}"/spaces/*/; do
        dir="${dir%/}"
        id="$(basename -- "$dir")"
        case "$id" in
        *[!0-9A-Za-z_-]*) die "unexpected space name: ${id}" ;;
        esac
        if [ ! -f "${dir}/relay.db" ]; then
            warn "space ${id} has no relay.db: skipped"
            continue
        fi
        SRC_DBS+=("${dir}/relay.db")
        REL_DBS+=("spaces/${id}/relay.db")
        if [ -d "${dir}/blobs" ]; then
            BLOB_RELS+=("spaces/${id}/blobs")
        fi
    done
    shopt -u nullglob
else
    # The SQLite file name is not fixed: take the only .db / .sqlite / .sqlite3 at the root
    # of the data directory, unless TOMI_BACKUP_DB says otherwise.
    if [ -z "$DB_PATH" ]; then
        shopt -s nullglob
        candidates=("${DATA_DIR}"/*.db "${DATA_DIR}"/*.sqlite "${DATA_DIR}"/*.sqlite3)
        shopt -u nullglob
        case "${#candidates[@]}" in
        1) DB_PATH="${candidates[0]}" ;;
        0) die "no SQLite database in ${DATA_DIR} (has the relay already started? otherwise set TOMI_BACKUP_DB)" ;;
        *) die "several databases in ${DATA_DIR}: set TOMI_BACKUP_DB (${candidates[*]})" ;;
        esac
    fi
    [ -f "$DB_PATH" ] || die "database not found: ${DB_PATH}"
    SRC_DBS+=("$DB_PATH")
    REL_DBS+=("$(basename -- "$DB_PATH")")
    if [ -d "${DATA_DIR}/blobs" ]; then
        BLOB_RELS+=("blobs")
    fi
fi

[ "${#SRC_DBS[@]}" -gt 0 ] || die "no database to back up in ${DATA_DIR} (no space created yet?)"

# --- 2. Safeguard: available space -------------------------------------------------------------
# Temporary writes: the database snapshots (≤ size of the database + its -wal); then
# the archive, at worst as large as databases + blobs (encrypted blobs are incompressible).
# The tar headers and the gzip overhead are covered by the MIN_FREE margin.
size_of() { stat -c %s -- "$1" 2>/dev/null || echo 0; }

DB_BYTES=0
for src in "${SRC_DBS[@]}"; do
    DB_BYTES=$((DB_BYTES + $(size_of "$src")))
    if [ -f "${src}-wal" ]; then
        DB_BYTES=$((DB_BYTES + $(size_of "${src}-wal")))
    fi
done
BLOB_BYTES=0
for rel in ${BLOB_RELS[@]+"${BLOB_RELS[@]}"}; do
    sz="$(du -sb -- "${DATA_DIR}/${rel}" 2>/dev/null | cut -f1 || true)"
    BLOB_BYTES=$((BLOB_BYTES + ${sz:-0}))
done

ARCHIVE_EST=$((DB_BYTES + BLOB_BYTES))
NEEDED=$((DB_BYTES + ARCHIVE_EST))
FREE_KB="$(df -Pk -- "$OUT_DIR" | awk 'NR == 2 { print $4 }')"
is_uint "$FREE_KB" || die "cannot read the free space of ${OUT_DIR}"
FREE=$((FREE_KB * 1024))

if [ "$FREE" -lt $((NEEDED + MIN_FREE)) ]; then
    printf 'tomi-backup: not enough disk space, nothing was written.\n' >&2
    printf '  free on %s: %s MiB\n' "$OUT_DIR" "$(mib "$FREE")" >&2
    printf '  required:   %s MiB (database snapshots %s + archive %s at most) + margin of %s MiB\n' \
        "$(mib "$NEEDED")" "$(mib "$DB_BYTES")" "$(mib "$ARCHIVE_EST")" "$(mib "$MIN_FREE")" >&2
    printf '  Free up space, delete old archives (TOMI_BACKUP_KEEP=%s), or lower TOMI_BACKUP_MIN_FREE_BYTES.\n' "$KEEP" >&2
    exit "$EXIT_NO_SPACE"
fi

# --- 3. Database snapshots (first) -------------------------------------------------------------
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
FINAL="${OUT_DIR}/tomi-${STAMP}.tar.gz"
WORK="$(mktemp -d "${OUT_DIR}/.work.XXXXXX")"
cleanup() { rm -rf -- "$WORK" "${FINAL}.partial"; }
trap cleanup EXIT
case "$WORK" in
*"'"*) die "the work path must not contain an apostrophe: ${WORK}" ;;
esac

# Consistency limit: the databases are not frozen at the SAME instant. routing.db (copied first) and
# each space database are each consistent, but taken a few seconds apart, relay running: an invitation or a
# device → space match created in between may be missing from one without being missing from the other.
# No consequence on restore: the relay rebuilds the matches from the space databases; only invitations
# need checking (infra/backup.md).
#
# Opened READ-ONLY (`-readonly`, never created): a space deleted (`reset`) between the inventory and the
# snapshot must neither make the whole backup fail nor let sqlite3 re-create an empty relay.db in the
# live folder. A vanished space is skipped with a warning, along with its blobs. Other failures remain fatal.
SNAP_RELS=()
GONE_IDS=()
for i in "${!SRC_DBS[@]}"; do
    src="${SRC_DBS[$i]}"
    rel="${REL_DBS[$i]}"
    # Re-check right before opening (the inventory is a few moments old).
    if [ ! -f "$src" ]; then
        case "$rel" in
        spaces/*/relay.db)
            gid="${rel#spaces/}"
            GONE_IDS+=("${gid%%/*}")
            warn "${rel} disappeared since the inventory (space deleted?): skipped"
            continue
            ;;
        *) die "${rel} disappeared during the backup" ;;
        esac
    fi
    dst="${WORK}/${rel}"
    mkdir -p -- "$(dirname -- "$dst")"
    if ! sqlite3 -readonly "$src" ".timeout 10000" ".backup '${dst}'"; then
        # Disappeared during the copy: same treatment; otherwise the error is real.
        if [ ! -f "$src" ]; then
            case "$rel" in
            spaces/*/relay.db)
                gid="${rel#spaces/}"
                GONE_IDS+=("${gid%%/*}")
                warn "${rel} disappeared during the copy (space deleted?): skipped"
                rm -f -- "$dst"
                continue
                ;;
            esac
        fi
        die "copying ${rel} failed"
    fi
    check="$(sqlite3 "$dst" "PRAGMA integrity_check;")"
    [ "$check" = "ok" ] || die "the copy of ${rel} fails the integrity check: ${check}"
    SNAP_RELS+=("$rel")
done

# Remove from the archive the blobs of vanished spaces (tar would fail on a missing folder).
if [ "${#GONE_IDS[@]}" -gt 0 ]; then
    KEPT_BLOBS=()
    for b in ${BLOB_RELS[@]+"${BLOB_RELS[@]}"}; do
        skip=0
        for g in "${GONE_IDS[@]}"; do
            [ "$b" = "spaces/${g}/blobs" ] && skip=1
        done
        [ "$skip" -eq 1 ] || KEPT_BLOBS+=("$b")
    done
    BLOB_RELS=(${KEPT_BLOBS[@]+"${KEPT_BLOBS[@]}"})
fi
[ "${#SNAP_RELS[@]}" -gt 0 ] || die "no database could be copied"

# --- 4. Archive: snapshots + blobs read in place (after the databases) -------------------------
# One last filter pass: a space deleted (reset) after its relay.db was copied successfully would make
# tar fail ("Cannot stat", exit code 2, outside BENIGN_RE). The archive then keeps its relay.db (see backup.md).
if [ "${#BLOB_RELS[@]}" -gt 0 ]; then
    STILL_BLOBS=()
    for b in "${BLOB_RELS[@]}"; do
        if [ -d "$DATA_DIR/$b" ]; then
            STILL_BLOBS+=("$b")
        else
            warn "blob folder disappeared before archiving, skipped: ${b}"
        fi
    done
    BLOB_RELS=(${STILL_BLOBS[@]+"${STILL_BLOBS[@]}"})
fi
# The final rename is atomic: never a half-written archive under the final name.
TAR_ARGS=(-czf "${FINAL}.partial" -C "$WORK" "${SNAP_RELS[@]}")
if [ "${#BLOB_RELS[@]}" -gt 0 ]; then
    TAR_ARGS+=(-C "$DATA_DIR" "${BLOB_RELS[@]}")
fi
#
# Blobs live while they are being read: an upload in progress completes them ("file changed as we read it",
# tar exit code 1), an abandoned upload part may be truncated ("File shrank", exit code 2) or a purged blob
# may disappear ("File removed"). These three cases have no consequence for completed blobs. Exit code 2 is
# therefore accepted ONLY if all of tar's messages are of this kind: any other error (disk full, permissions,
# unreadable file) remains fatal. Messages are in English (LC_ALL=C) so they can be filtered.
TAR_ERR="${WORK}/tar.err"
rc=0
LC_ALL=C tar "${TAR_ARGS[@]}" 2>"$TAR_ERR" || rc=$?
cat -- "$TAR_ERR" >&2
BENIGN_RE=': (file changed as we read it|File shrank by [0-9]+ bytes; padding with zeros|File removed before we read it)$'
case "$rc" in
0) ;;
1) warn "some blobs changed while being read (upload in progress or purge): no consequence, the archive remains usable" ;;
2)
    other="$(grep -Ev "$BENIGN_RE|^tar: Exiting with failure status due to previous errors\$" "$TAR_ERR" || true)"
    if [ -n "$other" ]; then
        die "tar failed (code 2): ${other}"
    fi
    warn "some blobs changed, shrank or disappeared while being read (abandoned upload or purge): no consequence for completed blobs, the archive remains usable"
    ;;
*) die "tar failed (code ${rc})" ;;
esac
tar -tzf "${FINAL}.partial" >/dev/null || die "the archive written is unreadable"
mv -- "${FINAL}.partial" "$FINAL"
echo "tomi-backup: ${FINAL} ($(du -h -- "$FINAL" | cut -f1), ${#SNAP_RELS[@]} database(s), layout ${LAYOUT})"

# --- 5. Rotation -------------------------------------------------------------------------------
# The names contain a UTC timestamp: alphabetical order is chronological order.
mapfile -t ARCHIVES < <(find "$OUT_DIR" -maxdepth 1 -type f -name 'tomi-*.tar.gz' | sort)
COUNT="${#ARCHIVES[@]}"
if [ "$COUNT" -gt "$KEEP" ]; then
    for old in "${ARCHIVES[@]:0:COUNT-KEEP}"; do
        rm -f -- "$old"
        echo "tomi-backup: deleted ${old}"
    done
fi
