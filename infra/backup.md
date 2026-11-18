# Tomi relay — backup and restore

The relay is multi-space (one person = one space). Layout under `/var/lib/tomi`:

```
routing.db                              global routing database
spaces/<space_id>/relay.db              one SQLite database (WAL) per space
spaces/<space_id>/blobs/<2 hex>/<id>    encrypted blobs of the space (incompressible)
public/                                 public files (not backed up)
```

The former layout (a single `tomi.db` and `blobs/` at the root) is migrated automatically by the relay at
startup. `tomi-backup` can still back it up as long as it is present.

## What to back up

| Item | Where | Backed up by `tomi-backup`? | Note |
|------|-------|-----------------------------|------|
| Routing database | `/var/lib/tomi/routing.db` | yes | consistent copy via `.backup` |
| Space databases (devices, items, `seq` log) | `/var/lib/tomi/spaces/<id>/relay.db` | yes | one `.backup` copy per space |
| Encrypted blobs | `/var/lib/tomi/spaces/<id>/blobs/` | yes | read in place by `tar`, after the databases |
| Public files (`updates/`, `.well-known/`, `l/`) | `/var/lib/tomi/public/` | **no** | APKs, installers and manifests are rebuilt from the repository |
| Admin socket, lock | `admin.sock`, `relay.lock` | **no** | ephemeral. Installation tokens are not files: only their fingerprints live in `routing.db` |
| Configuration | `/etc/tomi/relay.env` | **no** | contains no secret; keep a copy in your notes |
| FCM credentials | `/etc/tomi/fcm-service-account.json` | **no, on purpose** | this is a secret: store it in your password manager, never in a data archive |

With the multi-space layout, `tomi-backup` has nothing to guess. It backs up `routing.db` and
each `spaces/*/relay.db`. A space folder without `relay.db` is skipped with a warning. With
the former layout, it takes the only `.db`/`.sqlite`/`.sqlite3` file in `/var/lib/tomi`; if there
are several or none, it stops with a message (setting: `TOMI_BACKUP_DB=` in `/etc/tomi/relay.env`).

## Why not a simple copy of the folder?

The databases use WAL mode: recent writes live in the `-wal` file, not yet in the main
file. Copying these files while the relay is running (`cp`, `rsync`, a snapshot without
quiescing) can yield an inconsistent or corrupted database. `tomi-backup` therefore uses SQLite's
online backup API, for each database:

```bash
sqlite3 /var/lib/tomi/spaces/<id>/relay.db ".backup '/path/to/copy.db'"
```

An equivalent alternative, which also produces a compacted file:

```bash
sqlite3 /var/lib/tomi/spaces/<id>/relay.db "VACUUM INTO '/path/to/copy.db'"
```

Both are safe while the relay is writing. The script then checks each copy with
`PRAGMA integrity_check`, and writes an archive `tomi-YYYYMMDDTHHMMSSZ.tar.gz` containing the copies
of the databases (never the `-wal`/`-shm` files: a `.backup` copy is complete by itself) and the
blob folders.

Blobs are archived **after** the databases: the archive thus contains the blobs that the snapshots
reference, except those that disappear in between (see below). An upload in progress is completed by appends
while it is being read: `tar` reports it ("file changed as we read it", code 1), and the script accepts
it with a warning. This only concerns an upload that was unfinished at the time of the backup.

Other changes during reading are accepted with a warning (the script reads the output of `tar`
and accepts exit code 2 only if **all** of its messages are of this kind; any other error, such as a full disk or an
unreadable file, makes the backup fail):

- "File shrank … padding with zeros": an abandoned upload part was truncated during reading.
  An unfinished blob is unusable anyway;
- "File removed before we read it": a blob was deleted between the database snapshot and reading.

**Case of a space deleted between the copy and `tar`**: if a space is reset (`reset`) after the copy
of its `relay.db` but before archiving, the script skips its blob folder (it no longer exists)
while the archive contains its `relay.db`. That space would **come back to life** when this
archive is restored: check `tomi-admin spaces` after any restore and redo the `reset` if needed.

**Case of a blob purged between the snapshot and `tar`**: if the relay purges a blob (expired, deleted or
purged item) after the snapshot of its database but before `tar` reads it, the archive contains the database that
references it (a "complete" blob row) but not the file. After a restore, downloading that
blob fails with `500`, for that item only: it had been deleted or purged on the relay
just after the snapshot. This is rare (the window lasts a few seconds) and has no consequence for the
other items; no action is needed.

## Disk space: the peak of a backup

The former script copied all blobs into a working folder before making the archive, and kept
14 archives: the peak was about 17 times the data. The current script:

- **no longer copies blobs**: `tar` reads them where they are. No hard links either, because a
  blob being uploaded grows by appends, so a hard link would not freeze anything. Only the copies
  of the databases, which are small and deleted at the end, remain in the working folder;
- keeps **2 archives** by default (`TOMI_BACKUP_KEEP=2`);
- **refuses to start** if there is not enough space (see below).

For D bytes of data, the peak is about D + `KEEP` archives + 1 archive being written, that is
`(KEEP + 2) × D` (an archive weighs roughly D: encrypted blobs do not compress, and rotation
happens after the new archive is written):

| `TOMI_BACKUP_KEEP` | Peak | For 1 GiB of data |
|--------------------|------|-------------------|
| 1 | 3 × D | ~3 GiB |
| **2 (default)** | 4 × D | ~4 GiB |
| 14 (former default) | 16 × D | ~16 GiB |

The relay is capped at 1 GiB in total (`TOMI_RELAY_MAX_RELAY_BYTES`). On a small server, budget
roughly four times the data size for the data and its copies: 2 local archives fit, 14 do not.

**Why 2 and not 1.** A single local archive is overwritten by the next one: if a backup succeeds but
captures a bad state (a database damaged without a visible error, a space reset by mistake the day
before), nothing from before remains. With 2, you keep the state from the day before. Three archives or more
each cost one D for a gain that disappears as soon as you send the archives off the machine (see below),
which is the real answer to history. Note: `update.sh` runs a backup before each
update, which goes through the rotation; on the day of an update, both archives may therefore both be dated that day.

### Safeguard

Before writing anything, the script estimates what it will occupy: the size of the databases (with their
`-wal`) for the temporary copies, plus databases + blobs for the archive in the worst case (without any
compression gain). If the free space of `/var/backups/tomi` does not cover this estimate **plus a margin
of 2 GiB**, it stops without writing anything, shows what is missing and exits with **code 3** (the unit
`tomi-backup.service` fails, visible in `systemctl --failed`; `update.sh` then cancels the
update, like for any failed backup). The margin is set with
`TOMI_BACKUP_MIN_FREE_BYTES` in `/etc/tomi/relay.env`. The estimate is an upper bound: the
`tar` headers and the gzip overhead are covered by the margin.

**Why 2 GiB and not 1.** The relay itself refuses uploads (`relay_full`, "not enough free disk") when free disk
space drops below its `min_free_disk_bytes` floor, 1 GiB by default. With a backup margin equal to that
floor, a backup that stops just above the margin leaves the disk exactly at the relay's threshold, and uploads are
refused for as long as writing takes. With 2 GiB, at least 1 GiB more than that floor remains once the archive is written.
If your machine is very tight (see the peak above), reduce the number of local archives (`TOMI_BACKUP_KEEP=1`)
rather than lowering this margin, and send the archives off the machine.

The script also takes a lock (`flock`): two backups in parallel (timer and `update.sh`)
would double the peak.

## Scheduling

`install.sh` enables `tomi-backup.timer` (every night around 03:30, if `sqlite3` is installed):

```bash
systemctl list-timers tomi-backup.timer      # next run
sudo systemctl start tomi-backup.service     # immediate backup
journalctl -u tomi-backup -n 20 --no-pager   # result
sudo ls -lh /var/backups/tomi                # archives (2 kept by default)
```

`update.sh` also runs a backup before each replacement of the binary.

## A local backup does not protect you from losing the disk: off-site copy (S3), strongly recommended

The archives in `/var/backups/tomi` are on the same disk (EBS) as the data: they cover an
operator error or a damaged database, not the loss of the instance, nor a full disk. On a
small disk shared with other services, an off-site copy is also what allows you to keep
few archives locally. **Set it up as soon as the service goes live.**

The simplest option, which stores no key on the machine: **S3 with an instance IAM role** restricted
to a single bucket.

1. Create a private bucket (public access blocked, default encryption enabled), with a lifecycle
   rule that expires objects after 30 days, for example.
2. Create a minimal IAM policy, attached to the instance role (instance profile):

   ```json
   {
     "Version": "2012-10-17",
     "Statement": [
       { "Effect": "Allow", "Action": "s3:PutObject", "Resource": "arn:aws:s3:::BUCKET-NAME/tomi/*" }
     ]
   }
   ```

   No read or delete rights: a compromised instance cannot erase the backups.
   To restore, you read the bucket from your own account.
3. Install AWS CLI v2 on the instance (`aws --version` to check; it takes credentials from the
   instance role only). Not verified on the reference host: neither the presence of the AWS CLI
   nor that of an instance role.
4. Create `/usr/local/sbin/tomi-backup-offsite` (mode `0755`):

   ```bash
   #!/usr/bin/env bash
   set -Eeuo pipefail
   latest="$(find /var/backups/tomi -maxdepth 1 -name 'tomi-*.tar.gz' | sort | tail -n 1)"
   [ -n "$latest" ] || { echo "no archive" >&2; exit 1; }
   exec aws s3 cp --no-progress "$latest" "s3://BUCKET-NAME/tomi/"
   ```

5. Create `/etc/systemd/system/tomi-backup-offsite.service`:

   ```ini
   [Unit]
   Description=Tomi: off-site copy of the latest archive
   After=tomi-backup.service

   [Service]
   Type=oneshot
   User=tomi
   Group=tomi
   ExecStart=/usr/local/sbin/tomi-backup-offsite
   NoNewPrivileges=yes
   ProtectSystem=strict
   ProtectHome=yes
   PrivateTmp=yes
   ```

   then make it follow the backup, with an override of `tomi-backup.service`
   (`sudo systemctl edit tomi-backup.service`):

   ```ini
   [Unit]
   OnSuccess=tomi-backup-offsite.service
   ```

   `OnSuccess=` requires systemd 249 or later (`systemctl --version`). These units are an example: they
   have not been tested on the instance. The backup unit cannot send to S3 itself,
   because its hardening forbids network access (`RestrictAddressFamilies=AF_UNIX`).

With the archive sent every night, `TOMI_BACKUP_KEEP=1` is enough if the disk is really tight (peak
3 × D); keep 2 otherwise.

Other options, less automated:

- from a Windows PC, in two steps (the folder is `0750 tomi:tomi`, so it is unreadable for the
  SSH user): on the server `sudo cp /var/backups/tomi/tomi-<date>.tar.gz ~ && sudo chown $USER ~/tomi-<date>.tar.gz`,
  then on the PC `scp user@relay.example.org:tomi-<date>.tar.gz D:\backups\tomi\`;
- a scheduled EBS snapshot (AWS Data Lifecycle Manager). A snapshot taken while running is consistent
  "as after a power cut": SQLite in WAL mode can recover from it, but the `tomi-backup`
  archive remains the reference.

### Everything is encrypted, but not everything is hidden

Contents (texts, files, file names, types, previews) are encrypted end to end with a key that the
relay does not hold: a stolen archive gives **no content**. It can therefore be stored with a third-party
service without special trust.

What remains readable in the databases and on disk, and therefore in the archive: the identifiers of spaces,
items and devices (the name of the `spaces/<id>/` folders is part of it), the names and platforms of devices,
the timestamps of emission, reading and expiry, the **size** of the encrypted blobs, the number of blobs
per item, the `key_epoch` number, the `seq` values, and the devices' FCM notification tokens
(spec/api-v1.md §1.2). This metadata reveals **when** and **how much** each person uses Tomi.
The archives are created with mode `0600`, owned by the `tomi` user; encrypt them (for example with `age`
or `gpg`) if you also want to hide this. The archive covers all spaces: if you host other
people, it is up to you to protect this copy.

Backing up does not replace the space key: it only exists on each person's devices.
Restoring an archive on a new server does not allow old items to be read again without at least one
device that holds the key.

## Before any restore: note the revocations

A restore brings the space databases back **to the state of the backup**, revocations included: a
device revoked (stolen, lost) **after** that backup is still listed there as a member. Without a
corrective step, it becomes able again to open a session with its signing key, read the space
metadata, send tombstones that the relay applies, and disrupt clients. **Each time
you revoke a device (`tomi-admin revoke`), write it down** (identifier, space, date) in your
password manager or a notebook: without this list, you will not know what to revoke again
after a restore. If you have already lost the list, compare with what each person recognizes as
their devices (`tomi-admin devices`, see "Revocations to redo" below).

## Full restore

To be done on the affected instance, with `sudo`. The relay must be stopped during the restore. The
same steps apply to an archive of the former layout: the relay migrates it at startup.

**Cut public access first**, before even starting the restored relay: a device revoked since the
backup becomes a member again at step 4. Stop Nginx (`sudo systemctl stop nginx`) or close
port 443 in the firewall (AWS security group); with Caddy, `sudo systemctl stop caddy`. Only reopen it
**after** step 5. `tomi-admin` talks to the relay through its local socket: it works
with public access cut off.

```bash
# 0. Cut public access (see above), then stop the relay.
sudo systemctl stop nginx          # or caddy, or close 443 in the firewall
sudo systemctl stop tomi-relay

# 1. Set the current state aside (delete nothing).
sudo mv /var/lib/tomi /var/lib/tomi.before-restore-$(date +%Y%m%d%H%M)

# 2. Recreate the folder and extract the chosen archive into it (routing.db, spaces/…).
sudo install -d -m 0750 -o tomi -g tomi /var/lib/tomi
sudo tar -xzf /var/backups/tomi/tomi-YYYYMMDDTHHMMSSZ.tar.gz -C /var/lib/tomi

# 3. Copy the public files (updates, assetlinks, /l/ page) back from the old folder.
sudo cp -a /var/lib/tomi.before-restore-*/public /var/lib/tomi/public
#    (or, on a new machine: sudo install -d -m 0750 -o tomi -g tomi
#     /var/lib/tomi/public /var/lib/tomi/public/updates /var/lib/tomi/public/.well-known /var/lib/tomi/public/l
#     then put the files back, see infra/README.md)
sudo chown -R tomi:tomi /var/lib/tomi
sudo chmod -R go-rwx /var/lib/tomi

# 4. Restart the relay (public access still CUT) and check.
sudo systemctl start tomi-relay
curl -fsS http://127.0.0.1:8080/healthz
sudo tomi-admin status
sudo tomi-admin spaces
sudo tomi-admin invite          # invitations restored with the archive

# 5. MANDATORY, public access still cut: redo the revocations (see "Revocations to redo
#    after a restore" below). List the devices of each space:
for ID in $(sudo tomi-admin spaces | awk '{print $1}'); do sudo tomi-admin devices --space "$ID"; done
#    then, for each device to remove: sudo tomi-admin revoke --space "$ID" <prefix>

# 6. Only once the revocations are done: reopen public access.
sudo systemctl start nginx         # or caddy, or reopen 443 in the firewall
```

**Step 5 is mandatory, with public access cut.** Devices revoked since the backup came back
at step 4: as long as 443 is closed, they cannot reach the relay. Details and commands in
"Revocations to redo after a restore" below. `tomi-admin` does not need the relay to be
stopped: it goes through the socket of the running relay.

`routing.db` comes back to the state of the backup, invitations included. If installation tokens
were issued or used since that backup, run `sudo tomi-admin invite --rotate` (replaces the
pending invitations with a single one and displays its token) so that no token already used becomes
valid again. See "And `routing.db`?" below.

If the archive comes from another machine or from S3, first copy it to `/var/backups/tomi/`. Once the
restore is verified, delete the `/var/lib/tomi.before-restore-*` folder: it takes up as much
space as the data.

## Restore ONE space without touching the others

Typical case: a space damaged or reset by mistake, while the other people have lost nothing.
Each space lives in its own folder, so only that one is replaced. The other spaces
stay in their current state.

```bash
ID=<full space_id>                                     # sudo tomi-admin spaces
A=/var/backups/tomi/tomi-YYYYMMDDTHHMMSSZ.tar.gz

# 0. Is the space in the archive?
sudo tar -tzf "$A" "spaces/$ID/relay.db"

# 1. Cut public access (Nginx/Caddy stopped, or 443 closed in the firewall), then stop the relay
#    (it keeps the databases open). Access stays cut until after step 5: the restored space
#    gets back, at step 4, its members as of the time of the backup.
sudo systemctl stop nginx          # or caddy, or close 443 in the firewall
sudo systemctl stop tomi-relay

# 2. Set the current state of the space aside, OUTSIDE spaces/ (the relay would list this folder
#    as a space). Same disk: it is a simple rename. Skip this line if the folder
#    no longer exists.
sudo mv "/var/lib/tomi/spaces/$ID" "/var/lib/tomi.before-restore-$ID-$(date +%Y%m%d%H%M)"

# 3. Extract only this space (database and blobs).
sudo tar -xzf "$A" -C /var/lib/tomi "spaces/$ID"
sudo chown -R tomi:tomi "/var/lib/tomi/spaces/$ID"
sudo chmod -R go-rwx "/var/lib/tomi/spaces/$ID"

# 4. Restart the relay (public access still CUT) and check.
sudo systemctl start tomi-relay
curl -fsS http://127.0.0.1:8080/healthz
sudo tomi-admin status --space "$ID"

# 5. MANDATORY, public access still cut: redo the revocations of this space (next
#    section), with `sudo tomi-admin revoke --space "$ID" <prefix>` for each device to remove.
sudo tomi-admin devices --space "$ID"

# 6. Only then: reopen public access.
sudo systemctl start nginx         # or caddy, or reopen 443 in the firewall
```

**Step 5 is mandatory, with public access cut**, as for the full restore: the restored space
got back its members as of the time of the backup, including a device revoked since. See
"Revocations to redo after a restore" below. The other spaces are unreachable for the
duration of the operation (a few minutes): that is the price of safety.

The set-aside folder contains no stale `-wal` file that could mix with the restored database:
it was moved as a whole. Delete it once the space is verified.

Always put the real folder back into `spaces/`, never a symbolic link to it: the relay refuses to start
if `spaces/<id>` or `spaces/<id>/relay.db` is a link (message "is a symbolic link"), so that the devices
of the space do not believe their space was deleted.

**And `routing.db`?** Do **not** restore it to recover a space: the relay discovers spaces by
reading the `spaces/*/relay.db` folders, never `routing.db`. The matching tables (device →
space, channel → space) are rebuilt at each startup from these databases. Restoring an
old `routing.db` would therefore make no space visible, and it would put back in the pending state the
invitations consumed or cancelled since the backup: a token already used would become valid again and
would allow creating a second space.

If a restored space does not appear in `sudo tomi-admin spaces` after the restart:

1. check that `/var/lib/tomi/spaces/<id>/relay.db` exists and belongs to `tomi` (`ls -l`, then
   `sudo chown -R tomi:tomi` and `chmod -R go-rwx` on the folder);
2. read the log (`sudo journalctl -u tomi-relay -n 100 --no-pager`): a folder that was skipped (without
   `relay.db`) or an identifier conflict with another space (the relay then refuses to start) is
   reported there.

What `routing.db` contains, and what the space databases cannot be used to deduce, is: the
invitations (pending and consumed, by token fingerprint), the per-space quotas
(`admin quota`) and the health state of notifications (FCM). After a **full restore**, these
items come back to the state of the backup. Check them:

```bash
sudo tomi-admin invite            # lists pending invitations (fingerprints, never a token)
sudo tomi-admin spaces            # quotas per space
```

If tokens were issued or consumed since the backup, run `sudo tomi-admin invite --rotate`:
it replaces all pending invitations with a single one and displays its token, which cancels the
resurrected tokens. Per-space quotas set since the backup must be set again (`tomi-admin quota`).

### Revocations to redo after a restore (mandatory)

Revocations are stored in each space's database: a restore, complete or of a single
space, cancels them for everything that was revoked after the backup. The scenario to avoid: a
stolen device is revoked at noon, the backup dates from the night before, the disk dies, you
restore: the stolen device is a member again with its `sign_pk`; it can open a session,
read the metadata, send tombstones applied by the relay, and disrupt the other clients.

To be done for **each restored space** (all spaces after a full restore), with the relay
running (`tomi-admin` goes through its socket, without public access) and **public access still cut**
(Nginx/Caddy stopped or 443 closed: only reopen it after the end of this section):

```bash
# a. List the devices that are members of the space (identifier, name, platform, last activity).
sudo tomi-admin devices --space "$ID"

# b. Compare with the known list (your revocation notes, plus what the person recognizes as
#    THEIR devices). Any unknown device, or one noted as revoked since the backup, is revoked
#    again (prefix of at least 8 hexadecimal characters of the identifier):
sudo tomi-admin revoke --space "$ID" <prefix-of-the-identifier>

# c. List again to check that it has disappeared.
sudo tomi-admin devices --space "$ID"
```

`revoke` cuts the device's connections and marks the space "rotation required": **a member device
must rotate the space key again** (the application offers it), otherwise the revoked device, which held
the old key, could still decrypt what was sent with it. Tell the person. `revoke` refuses to
remove the last member: in that case, the space must be reset (`tomi-admin reset`, see the optional
example in infra/forge/recipes.md §2) with a new invitation.

If a revocation was made for a theft or a loss, also handle the invitations
(`invite --rotate`, see above): a token left pending is not thereby safe.

### Effect of a restore on the devices

Items sent between the backup and the incident no longer exist on the relay (the devices that
already received them keep them). The `seq` values are never reused (`seq = max(last + 1,
relay time in ms)`): a device whose cursor is further ahead than the restored database receives
`409 cursor_ahead` and performs a full catch-up (spec/api-v1.md §7.5), without skipping any event. This
applies space by space: restoring one space has no effect on the cursors of the others.
