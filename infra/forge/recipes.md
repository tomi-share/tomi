# Forge recipes — Tomi relay (example managed-server deployment)

Short commands to paste as **Recipes** in the managed-server panel (Server → Recipes), or to run over
SSH on the server. Each one is independent and repeatable. The run user is `deploy`.

Reminder: the relay is blind. None of these recipes reads the content of the spaces; they touch the
binary, the daemon, the public files (updates, assetlinks, pairing page) and the administration
metadata (spaces, invitations, quotas, devices).

Reference paths (deployment on the example server):
- binary: `/home/deploy/relay.example.org/tomi-relay`
- data: `/home/deploy/tomi-data` (the relay is multi-space, one person = one space):
  - `routing.db`: invitations (token fingerprints), quotas per space, FCM health;
  - `spaces/<space_id>/relay.db`: the space's database; `spaces/<space_id>/blobs/`: its blobs;
  - `public/`, `admin.sock`, `relay.lock`.
  There is no `tomi.db` nor `blobs/` at the root any more after the automatic migration on the first
  multi-space start, and no durable `install-token` file.
- public files: `/home/deploy/tomi-data/public`
- backups: `/home/deploy/tomi-backups` (see §7)

---

## 0. Initialise the relay: get the install token

To create the first space, a client needs an **install token**. The relay keeps only its fingerprint
and shows it only once, when it is created: it cannot be "found again", only replaced by a new one.

Panel recipe **"Tomi relay: init (install token)"** (run as user `deploy`), versioned copy:
`infra/forge/recipes/init-relay.sh`. It:

1. checks that the relay is deployed and answers;
2. removes leftovers of a plain-text token (`smoke.log` from a test start, old `install-token`);
3. prints the relay's state;
4. replaces all pending invitations with a single one and **prints its token**
   (`admin invite --rotate`).

Run it: panel → Recipes → "Tomi relay: init" → Run → select the server, or from a workstation through
the panel's API (see below). The token appears in the recipe's output (panel log, or by e-mail if
requested): it is single-use, paste it immediately into "Create your space".

It touches neither existing spaces, nor devices, nor the binary. Repeatable: each run cancels the
previous token. To *add* an invitation without cancelling the others (a new person), use
`admin invite --new` (§2).

**Arm the guard without touching the invitations.** The script has two modes: `init` (default, all of
the above, rotation included) and `arm` (argument `arm`, or the variable `TOMI_INIT_MODE=arm` on the
first line of a second panel recipe that reuses the same script). It checks the relay, prints its state
and sets `require_existing_data_dir = true` in `config.toml` (if the binary knows the key, there is a
`routing.db`, and there is no TOML table), **without cleanup and without rotation**: pending invitations
are never replaced. This is the mode to run after an update; `init` mode is only for getting a new token.
This second recipe is **"Tomi relay: arm (guard)"**; its script is that of `init-relay.sh` preceded by
`export TOMI_INIT_MODE=arm`.

The panel's CLI has no command for recipes: creating and running them goes through the API
(`POST /api/orgs/{org}/recipes`, then `POST /api/orgs/{org}/recipes/{id}/runs` with
`{"servers":[<server id>]}`), with the panel's CLI token (kept in the CLI's configuration file, never in
the repository).

---

## 1. State and health

```bash
# Local HTTP health (Nginx terminates TLS in front). 200 if routing.db AND each space database answer.
curl -fsS http://127.0.0.1:8080/healthz && echo

# Supervisor daemon (replace the name with yours: sudo supervisorctl status).
sudo supervisorctl status | grep -i tomi-relay

# Version of the deployed binary.
/home/deploy/relay.example.org/tomi-relay --version
```

## 2. Relay administration (spaces, install tokens, quotas, devices, reset)

The relay is administered through its Unix socket (`admin.sock`), as the `deploy` user, with the same
configuration as the service. A relay hosts several spaces: commands that target a space take
`--space <id>` (the full identifier or its first 8 characters; optional only if there is a single space,
otherwise the command refuses and lists the spaces).

```bash
BIN=/home/deploy/relay.example.org/tomi-relay
CFG=/home/deploy/tomi-data/config.toml

# List the spaces: identifier, date, devices, usage, quota.
"$BIN" --config "$CFG" admin spaces

# Global state: number of spaces, total quota used, free disk, FCM health. Then a single space.
"$BIN" --config "$CFG" admin status
"$BIN" --config "$CFG" admin status --space <id>

# Devices of a space, then revocation of a lost device (prefix of at least 8 characters).
"$BIN" --config "$CFG" admin devices --space <id>
"$BIN" --config "$CFG" admin revoke --space <id> <prefix>

# Quota of a space: a number of bytes, `none` (no limit) or `default` (back to the relay's default:
# the web panel first, then max_space_bytes in config.toml, then no limit; max_space_bytes absent or 0
# = no limit). The TOTAL quota is max_relay_bytes (1 GiB).
"$BIN" --config "$CFG" admin quota --space <id> 268435456
"$BIN" --config "$CFG" admin quota --space <id> none

# Last resort for ONE space (no device holds its key any more): erases this space alone and forgets the
# tokens already consumed for it. Asks you to retype its first 8 characters.
"$BIN" --config "$CFG" admin reset --space <id>
```

### Install tokens (invitations)

A single-use token creates a space ("Create my space" on the first client). The relay keeps only its
**fingerprint**: a token is visible only **once**, at the moment it is created.

```bash
# Add an invitation for a new person; DISPLAYS the token (once only).
"$BIN" --config "$CFG" admin invite --new

# Replace all pending invitations with a single one; DISPLAYS the token. This is the safe way if the
# token is lost, or to cancel the one the relay wrote to its logs on the first start.
"$BIN" --config "$CFG" admin invite --rotate

# No option: LISTS the pending invitations (short fingerprints, dates). Never shows a token.
"$BIN" --config "$CFG" admin invite
```

Pass the token out of band (in person, through an end-to-end encrypted messenger), never in plain text
in a ticket or an e-mail.

Where does the very first token come from?

- On the **first start of a new relay** (no space, no invitation), the relay creates an invitation and
  writes its token **only once** to its log (line `install token: ...`):
  `grep "install token" /home/deploy/logs/tomi-relay.log*`. The log rotates (5 MB per file, 3 old files
  kept): do not rely on it for long, and this token stays valid without time limit until it is used or
  replaced. If in doubt, run `invite --rotate`; **do it anyway once the first space has been created**:
  this log is created by supervisord (which runs as root) with its own umask. The init recipe tries to set
  it to 0600, but this fails if the file is owned by root (a warning, without stopping); check with
  `ls -l /home/deploy/logs/tomi-relay.log*` on the server, and if needed close the folder
  (`sudo chmod 700 /home/deploy/logs`) or run this block as root. The real safety net is the rotation of
  `init` mode: the token never expires until it is replaced.
- If a file `/home/deploy/tomi-data/install-token` from a previous version exists, the multi-space relay
  imports it as an invitation on its first start and then deletes it: **the same token therefore remains
  valid**, with no new line in the log.

### After `reset --space`

- If other spaces remain, `reset --space` **creates no token**: then run `invite --new` for the person
  concerned. (Only the reset of the last space displays one.)
- **Warn the person beforehand**: their applications will receive an authentication refusal (`401`) and
  show "device not recognised", without offering to start over. They must reset (or reinstall) the
  application, then use the new token.
- Take a backup first if in doubt (§7).

## 3. Deploy / update the binary

Normal deployment (pull from the newest `relay-v*` GitHub release): **through the panel**, not by hand —
the panel's deploy action, the CI's post-release hook, or the Deploy button of the UI. The script run by
the panel is the content of `infra/forge/deploy.sh` (pasted into the site's "Deploy Script" field, with
automatic source deployment disabled); see README §4.

Manual troubleshooting if needed (over SSH on the server): the script is not copied to the server (it
lives in the panel), so run it again through the panel's deploy action.

**The relay exits with code 78 when it refuses to start because of the data** (volume not mounted,
symbolic link on `spaces/<id>` or `relay.db`, directory or database unreadable, database of another
space; spec §10.1). `deploy.sh` then runs `admin status --json --config /home/deploy/tomi-data/config.toml`
before any rollback and, on 78, **does not roll back** to the old binary (it would ignore these spaces
and answer `409`/`401` for a live space): it stops and says what to check (volume mounted, rights of
`deploy` on `/home/deploy/tomi-data`, `spaces/` and each `spaces/<id>/`, no symbolic link). `spaces/`
must be a subdirectory of the volume, never the mount point itself. If supervisor reports an error on
restart but `/healthz` answers, the deployment is not declared successful (it is the old process that is
still running).
This check is **of the same kind** as the one in `update.sh`, not identical: it is skipped (with a
warning) if `config.toml` is missing, and a check that could not conclude (timeout, code 1, 124, 137
outside an intact legacy layout) warns and then lets the automatic rollback continue. On a second run
with the same binary, `deploy.sh` exits 0 only if the daemon is `RUNNING`, `/healthz` answers and the
process really executes the binary in place.

**The rollback depends on the data layout.** The first start of the multi-space relay moves `tomi.db` and
`blobs/` into `spaces/<id>/`. A binary from before this migration would find nothing: it would start
empty (`space_not_initialized`) and the clients would record their space as deleted. `deploy.sh` therefore
does not roll back on its own to the old binary if spaces (`spaces/*/relay.db`) appeared during the
deployment. In that case, to go back to the old version: stop the daemon, restore the backup taken before
(§7 and `infra/backup.md`, which covers `tomi.db` and `blobs/`), then put the old binary back as below.

```bash
# Immediate rollback to the previous binary (guarded by deploy.sh).
# Do NOT do it if spaces exist (ls /home/deploy/tomi-data/spaces/*/relay.db) and tomi-relay.prev predates
# the multi-space relay, unless the old backup has been restored.
# The block stops by itself (without swapping anything) if /home/deploy/tomi-data looks empty (no
# routing.db, no spaces/, no tomi.db: volume not mounted, the old binary would think it is new), or if
# `admin status` exits with anything other than 0 (or 1 with the legacy layout intact, "not migrated yet"):
# 78 = refusal to start because of the data, rolling back would be worse; any other code = read first.
cd /home/deploy/relay.example.org
D=/home/deploy/tomi-data
rc=0; ( cd "$D" && /home/deploy/relay.example.org/tomi-relay --config "$D/config.toml" admin status >/dev/null ) || rc=$?
legacy=no; [ -e "$D/tomi.db" ] && [ ! -e "$D/spaces" ] && [ ! -e "$D/spaces.migrating" ] && legacy=yes
if [ ! -e "$D/routing.db" ] && [ ! -e "$D/spaces" ] && [ ! -e "$D/tomi.db" ]; then
  echo "STOP: $D looks empty (volume not mounted?). No rollback."
elif [ "$rc" -ne 0 ] && ! { [ "$rc" -eq 1 ] && [ "$legacy" = yes ]; }; then
  echo "STOP: admin status exited with code $rc (78 = refusal because of the data). No rollback."
elif [ -f tomi-relay.prev ]; then
  mv -f tomi-relay tomi-relay.ko && mv -f tomi-relay.prev tomi-relay && mv -f tomi-relay.ko tomi-relay.prev
  sudo supervisorctl restart tomi-relay: 2>/dev/null || sudo supervisorctl restart tomi-relay
  curl -fsS http://127.0.0.1:8080/healthz && echo
else
  echo "No tomi-relay.prev: nothing to put back."
fi
```

## 4. Public files (App Links, pairing page, updates)

Served by the relay from `public/`. Public by design (the app is useless without a space).
Never put a signing key here.

```bash
PUB=/home/deploy/tomi-data/public

# Android App Links: SHA-256 fingerprint of the APK's signing certificate.
install -d -m 0755 "$PUB/.well-known"
install -m 0644 ~/assetlinks.json "$PUB/.well-known/assetlinks.json"
curl -fsS https://relay.example.org/.well-known/assetlinks.json | head -c 200; echo

# Static "Install Tomi" page for the pairing links (/l/pair). No script, no third party.
install -d -m 0755 "$PUB/l"
install -m 0644 ~/l-index.html "$PUB/l/index.html"

# APK (drop latest.json LAST, once the installer is in place).
install -d -m 0755 "$PUB/updates/android"
install -m 0644 ~/tomi-<version>.apk "$PUB/updates/android/"
install -m 0644 ~/android-latest.json "$PUB/updates/android/latest.json"
```

## 5. Android notifications (FCM)

```bash
# Drop the service account JSON (secret: readable by deploy only).
install -d -m 0700 /home/deploy/.config/tomi
install -m 0600 ~/fcm-service-account.json /home/deploy/.config/tomi/fcm-service-account.json
shred -u ~/fcm-service-account.json

# Uncomment fcm_service_account_file in config.toml (pointing to this file), then:
sudo supervisorctl restart tomi-relay: 2>/dev/null || sudo supervisorctl restart tomi-relay
```

## 5 bis. iPhone notifications (APNs)

Before: on developer.apple.com, "Certificates, Identifiers & Profiles" › "Keys", create a key with "Apple Push
Notifications service (APNs)"; note its identifier (10 characters) and download the `AuthKey_<id>.p8` file
(possible only once). The team identifier (10 characters) is under "Membership".

```bash
# Drop the key (secret: readable by deploy only).
install -d -m 0700 /home/deploy/.config/tomi
install -m 0600 ~/AuthKey_XXXXXXXXXX.p8 /home/deploy/.config/tomi/apns-key.p8
shred -u ~/AuthKey_XXXXXXXXXX.p8

# Uncomment apns_key_file, apns_key_id and apns_team_id in config.toml, then:
sudo supervisorctl restart tomi-relay: 2>/dev/null || sudo supervisorctl restart tomi-relay
```

The relay must be deployed with this support **before** the iOS apps that register an `apns` token (an older
version refuses this provider: the app keeps working, without push).

## 6. Logs

```bash
# Through the panel: the daemon's tab, or the panel's CLI log commands.
# Over SSH:
tail -f /home/deploy/logs/tomi-relay.log            # daemon stdout (JSON)
sudo supervisorctl tail -f tomi-relay stdout
```

## 7. Backup (panel cron, user `deploy`, no sudo)

The example Forge deployment had no backup. The repository's `infra/backup.sh` script suits it as is: it
needs neither root nor the `tomi` user. It copies `routing.db` and each `spaces/*/relay.db` with SQLite's
online backup API (`.backup`, consistent while the relay writes), checks each copy, then archives the
copies and reads the blobs in place (`tar`) into `tomi-AAAAMMJJTHHMMSSZ.tar.gz`, written first as
`.partial` then renamed. Never copy the `.db`, `-wal` or `-shm` files by hand.

**Setup (once)**:

```bash
# From a workstation (the repository's copy is authoritative; redo it when backup.sh changes):
#   scp infra/backup.sh deploy@<server host>:backup.sh
# On the server, as deploy:
command -v sqlite3 >/dev/null || sudo apt-get install -y sqlite3     # the only sudo need, once
install -d -m 0700 /home/deploy/tomi-backups /home/deploy/.local/bin /home/deploy/logs
install -m 0755 ~/backup.sh /home/deploy/.local/bin/tomi-backup
rm ~/backup.sh

# Test by hand (same variables as the cron):
TOMI_RELAY_DATA_DIR=/home/deploy/tomi-data TOMI_BACKUP_DIR=/home/deploy/tomi-backups \
  /home/deploy/.local/bin/tomi-backup
ls -lh /home/deploy/tomi-backups
```

**Scheduling** (panel → Server → Scheduler → New Job, user `deploy`, every night, for example at 03:30)
— command to enter:

```bash
TOMI_RELAY_DATA_DIR=/home/deploy/tomi-data TOMI_BACKUP_DIR=/home/deploy/tomi-backups /home/deploy/.local/bin/tomi-backup >> /home/deploy/logs/tomi-backup.log 2>&1
```

The script exits with code 0 (success), 1 (error) or 3 (not enough disk space, nothing was written);
the result is in `/home/deploy/logs/tomi-backup.log`. By default it keeps 2 archives
(`TOMI_BACKUP_KEEP=2`; add it to the command to change this).

**Disk budget.** The example server has about 7.5 GB free, shared with other sites. The relay is capped
at 1 GiB of data in total (`max_relay_bytes`). A backup's peak is about `(KEEP + 2) × data`: at most
4 GiB at full capacity with `KEEP=2` (2 archives kept, 1 being written, plus the data), 3 GiB with
`KEEP=1`. The script refuses to start, without writing anything (code 3), if the free space does not cover
its estimate **plus a 2 GiB margin** (`TOMI_BACKUP_MIN_FREE_BYTES`): this margin keeps the disk above the
1 GiB floor below which the relay refuses uploads (`min_free_disk_bytes`). At full capacity with `KEEP=2`,
about 4 GiB must be free before the backup, and 3.5 GB remain at its peak on 7.5 GB: it is tight. If the
free space of the server drops (`df -h /home/deploy`), switch to `TOMI_BACKUP_KEEP=1` rather than
lowering the margin. With data much smaller than 1 GiB, this is not an issue.

**Off-machine copy.** The archives are on the same disk as the data: they protect against an operator
error, not against the loss of the server. From time to time, copy the latest one to a workstation
(it is `0600`, owned by `deploy`):

```powershell
scp deploy@<server host>:tomi-backups/tomi-<date>.tar.gz D:\backups\tomi\
```

The archives contain no plain-text content (everything is end-to-end encrypted), but they reveal the
metadata (space and device identifiers, times, sizes): see `infra/backup.md`.
The FCM JSON file is deliberately not backed up (it is a secret: store it in your password manager),
nor `config.toml` (versioned in `infra/forge/`).

**Restore**: same procedure as `infra/backup.md` ("Full restore" and "Restore ONE space"), with these
substitutions: `/var/lib/tomi` → `/home/deploy/tomi-data`, `/var/backups/tomi` → `/home/deploy/tomi-backups`,
owner `tomi:tomi` → `deploy:deploy`, and instead of `systemctl stop|start tomi-relay`:
`sudo supervisorctl stop tomi-relay:` then `sudo supervisorctl start tomi-relay:`. Do not restore
`routing.db` to find a space again (the relay discovers spaces through `spaces/*/relay.db`); after a full
restore, run `admin invite` and then, if tokens were issued or consumed since the backup,
`admin invite --rotate` (§2).
