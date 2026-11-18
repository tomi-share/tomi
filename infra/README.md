# Tomi — relay operations runbook (dedicated server: systemd + Caddy)

This document walks you through the deployment, step by step, from nothing to a relay in service at `https://relay.example.org`,
then how to operate it (updates, logs, monitoring, backup, recovery).

```
Clients (Windows, Android, iOS, CLI)
      │ HTTPS / WSS: 443 (80 → redirect + Let's Encrypt certificate)
      ▼
   Caddy  ──────────►  tomi-relay  (127.0.0.1:8080, user `tomi`, systemd)
   TLS, headers         routing.db + one SQLite database and encrypted blobs per space,
                        under /var/lib/tomi/spaces/<space_id>/
```

On a server where **nginx already holds ports 80/443** (for example one managed by a hosting panel), Caddy is replaced
by an nginx `server` block: see section 15, and skip the Caddy installation (step 4).

> **Optional example:** `infra/forge/` contains the files of a managed-hosting deployment (supervisor, `config.toml`,
> no `relay.env`). It is not required: this runbook describes the dedicated-server setup (systemd + Caddy).
> On such a setup, the `tomi-admin ...` commands are `tomi-relay --config … admin ...`.

Contents of this folder:

| File | Role |
|------|------|
| `systemd/tomi-relay.service` | relay unit (dedicated user, hardened) |
| `systemd/tomi-backup.{service,timer}` | nightly backup (see `backup.md`) |
| `caddy/Caddyfile` | reverse proxy + TLS for `relay.example.org` |
| `relay.env.example` | configuration template → `/etc/tomi/relay.env` |
| `install.sh` | idempotent installation (root, on the server) |
| `update.sh` | binary update with rollback |
| `tomi-admin.sh` | installed as `tomi-admin`: runs `tomi-relay admin ...` |
| `backup.sh`, `backup.md` | backup / restore |

Assumption: the server is **Linux x86_64 with systemd**, preferably Ubuntu or Debian (the commands
below use Ubuntu/Debian as examples). Check with `uname -m` (it must print `x86_64`).
If it prints `aarch64` (Graviton instance), see the note in step 3.

---

## 1. DNS: A record

1. On AWS, reserve an **elastic IP address** (EC2 → Network & Security → Elastic IPs) and
   associate it with the instance. Without it, the public IP changes at every stop/start and DNS breaks.
2. At your DNS provider for `example.org`, create:

   | Type | Name | Value | TTL |
   |------|------|-------|-----|
   | A | `relay` | the elastic IP | 300 |

   Do **not** add an AAAA record as long as IPv6 is not open in the security group:
   Let's Encrypt would try to reach you over IPv6 and fail.
3. Check from your workstation (propagation can take a few minutes):

   ```powershell
   Resolve-DnsName relay.example.org -Type A
   ```

## 2. AWS security group

In the instance's security group, **inbound** rules:

| Port | Protocol | Source | Why |
|------|----------|--------|-----|
| 22 | TCP | **your IP only** (`/32`) | SSH. It is also your only recovery factor (step 11) |
| 80 | TCP | 0.0.0.0/0 | Let's Encrypt challenge (HTTP-01) and redirect to HTTPS |
| 443 | TCP | 0.0.0.0/0 | the API and WebSockets |
| 443 | UDP | 0.0.0.0/0 | optional, HTTP/3 |

**Never open 8080**: the relay only listens on `127.0.0.1`. The default outbound rules (all) are
required: Let's Encrypt, Google (FCM, OAuth).

## 3. Get the `tomi-relay` binary

### Recommended path: GitHub Actions

The workflow `.github/workflows/release-relay.yml` builds a **static** Linux binary (target
`x86_64-unknown-linux-musl`, so independent of the server's glibc version) and publishes it in a
release of the repository.

1. Check that the version in `apps/relay/Cargo.toml` (inherited from the `Cargo.toml` at the root) is `X.Y.Z`,
   that CI is green on the commit, then push the tag:

   ```powershell
   git tag relay-vX.Y.Z
   git push origin relay-vX.Y.Z
   gh run watch          # follows the build (a few minutes)
   ```

   The workflow refuses a tag whose number does not match the crate version.
2. Download the release (`gh` uses your GitHub login; **no GitHub token is stored on the server**):

   ```powershell
   mkdir $env:USERPROFILE\tomi-deploy ; cd $env:USERPROFILE\tomi-deploy
   gh release download relay-vX.Y.Z -R tomi-share/tomi -p "tomi-relay-x86_64-unknown-linux-musl*"
   Get-FileHash .\tomi-relay-x86_64-unknown-linux-musl -Algorithm SHA256
   Get-Content .\tomi-relay-x86_64-unknown-linux-musl.sha256   # both hashes must be identical
   ```

### Why not cross-compile from Windows?

Verified: the current relay code (pure Rust) cross-compiles from Windows to
`x86_64-unknown-linux-musl` with `rustup target add x86_64-unknown-linux-musl` and the `rust-lld`
linker shipped with Rust (variable `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld`). But as soon as the relay
embeds C code (SQLite via SQLx, `ring`...), a cross C compiler is also needed (zig/`cargo-zigbuild`, WSL or Docker:
none of them is installed on the build machine).
The GitHub Actions build does not have this problem and guarantees a reproducible binary. Keep it as a
fallback only.

Another fallback: build on the server itself (`rustup`, `cargo build --release -p tomi-relay`). It is
slow and memory-hungry on a small instance (2 GB or more recommended).

> **Graviton instance (`aarch64`)?** In `release-relay.yml`, replace `TARGET` and `ASSET` with
> `aarch64-unknown-linux-musl` and install a cross compiler (or use an `ubuntu-24.04-arm` runner).
> `install.sh` and `update.sh` refuse a binary built for another architecture than the machine.

## 4. Prepare the server

Connect (with the `.pem` key of your AWS key pair; the user is `ubuntu` on Ubuntu, `admin` on
Debian, `ec2-user` on Amazon Linux):

```powershell
ssh -i C:\path\to\your-key.pem ubuntu@relay.example.org
```

Required packages:

```bash
sudo apt-get update
sudo apt-get install -y curl sqlite3          # sqlite3: backups. Amazon Linux: sudo dnf install -y curl sqlite
```

### Install Caddy

> **Is nginx already on ports 80/443?** (for example, on a server managed by a hosting panel.) Do not install Caddy: it
> could not listen. Skip this step, run `install.sh` with `--no-caddy` (step 6) and configure
> nginx as described in section 15.

On Ubuntu/Debian (official commands from [caddyserver.com/docs/install](https://caddyserver.com/docs/install),
re-read them on that page before copying, they may change):

```bash
sudo apt install --yes debian-keyring debian-archive-keyring apt-transport-https curl
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg
sudo chmod o+r /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install caddy
```

For another distribution, follow the same page. The package creates the `caddy` user and the
`caddy.service` service.

## 5. Copy the files to the server

From the repository folder on your workstation (`infra/`) and the folder where you downloaded the binary:

```powershell
ssh -i C:\path\to\your-key.pem ubuntu@relay.example.org "mkdir -p ~/tomi-deploy"
scp -i C:\path\to\your-key.pem -r <repo>\infra ubuntu@relay.example.org:~/tomi-deploy/
scp -i C:\path\to\your-key.pem $env:USERPROFILE\tomi-deploy\tomi-relay-x86_64-unknown-linux-musl ubuntu@relay.example.org:~/tomi-deploy/
```

The scripts must have LF line endings (the repository's `.gitattributes` takes care of this). If you see
`bad interpreter: /bin/bash^M`, convert them: `sed -i 's/\r$//' ~/tomi-deploy/infra/*.sh`.

## 6. Install

On the server:

```bash
cd ~/tomi-deploy
sudo bash infra/install.sh ./tomi-relay-x86_64-unknown-linux-musl
# server already served by nginx (section 15): sudo bash infra/install.sh ./tomi-relay-x86_64-unknown-linux-musl --no-caddy
```

The script can be **re-run** safely. It creates the system user `tomi` (no shell), the folders
(`/var/lib/tomi`, `/etc/tomi`, `/var/backups/tomi`), installs the binary in
`/usr/local/bin/tomi-relay`, creates `/etc/tomi/relay.env` **only if it does not exist**, installs the
systemd units and the `Caddyfile`, then enables and starts the services. It never overwrites the configuration,
the existing data, or a customized `Caddyfile` (in that case it writes `Caddyfile.tomi` next to it,
to be merged by hand).

Checks:

```bash
systemctl status tomi-relay --no-pager
curl -fsS http://127.0.0.1:8080/healthz          # {"status":"ok"}
sudo journalctl -u caddy -n 30 --no-pager        # should show the certificate being obtained
curl -fsS https://relay.example.org/healthz      # from anywhere
```

If the certificate does not arrive: DNS not yet propagated, or ports 80/443 closed (steps 1 and 2).

Review `/etc/tomi/relay.env` (`sudo nano /etc/tomi/relay.env`): the default values are suitable.
After a change: `sudo systemctl restart tomi-relay`.

## 7. Get the installation token

The relay is **multi-space** (fewer than 5 people, one URL): each person has their own space, created
with its own single-use installation token (an "invitation"). The relay only keeps the
fingerprint (SHA-256) of a token, so a token is visible **only once**, at the moment it is created.
Three ways to get one:

1. **First start of a new relay** (no space, no invitation): the relay creates an invitation
   and writes its token to its log, once (a line `install token: ...`):

   ```bash
   sudo journalctl -u tomi-relay --no-pager | grep "install token"
   ```

   This token **never expires** (until it is used or replaced) and stays readable in journald,
   so by `root` and by members of the `adm` and `systemd-journal` groups. **Once the first
   space is created** (or right away, if you do not use it), run `sudo tomi-admin invite --rotate`: it
   replaces this token with another one that you note down or discard, which makes the log line harmless. A
   lifetime for invitations would require a change in the relay (see "To do" at the end of this file).
2. **`sudo tomi-admin invite --rotate`**: replaces all pending invitations with a single one and
   displays its token. The token logged at the first start stops working. This is the safe way
   when the token was lost or when you want only one token to be valid.
3. **`sudo tomi-admin invite --new`**: adds an invitation and displays its token, without touching the others
   (for a new person, see section 14).

`sudo tomi-admin invite` **without options** only lists the pending invitations (short fingerprints and
dates): it never displays a token. Note a token as soon as it is displayed; if it is lost, create another one
with `--new` or `--rotate`. There is no `install-token` file to read any more: an old
`install-token` file (layout from before multi-space) is imported as an invitation at the first start
and then deleted.

Paste the token into the **Créer mon espace** (*Create my space*) screen on the first client (Windows). Once the space
is created, the token is consumed and cannot be used to create another space.

`tomi-admin` is a small script that runs `tomi-relay admin ...` as the service user,
with the same configuration. It passes all its arguments through unchanged:

```bash
sudo tomi-admin status      # global state: spaces, total quota, disk, FCM health
sudo tomi-admin status --space <id>   # state of one space
sudo tomi-admin spaces      # list of spaces
sudo tomi-admin devices --space <id>  # devices of a space
sudo tomi-admin revoke --space <id> <start-of-device-identifier>
sudo tomi-admin --help      # exact list of commands of the installed binary
```

## 8. Public files: updates and App Links

The relay serves, from `/var/lib/tomi/public/` (created by `install.sh`):

```
public/
  .well-known/assetlinks.json    Android App Links (fingerprint of the APK signing certificate)
  l/index.html                   "Installer Tomi" (Install Tomi) page of the pairing links
  updates/
    windows/latest.json          Tauri updater manifest, installers and .sig
    android/latest.json          Android manifest, and tomi-<version>.apk
    cli/latest.json
```

To upload a file (example: an APK), from the workstation then to the server:

```powershell
scp -i C:\path\to\your-key.pem .\tomi-0.1.0.apk ubuntu@relay.example.org:~/
```

```bash
sudo install -o tomi -g tomi -m 0640 ~/tomi-0.1.0.apk /var/lib/tomi/public/updates/android/
```

(Create the subfolder first: `sudo install -d -o tomi -g tomi -m 0750 /var/lib/tomi/public/updates/android`.)
These files are public by design: the application is useless without a space. Publish `latest.json` **last**,
once the installer is in place. Never put a signing key in this folder.

## 9. Update the relay

Same chain as in step 3 (new tag `relay-vX.Y.Z`, download, `scp` of the binary), then on the server:

```bash
cd ~/tomi-deploy
sudo bash infra/update.sh ./tomi-relay-x86_64-unknown-linux-musl
```

The script backs up the data, installs the new binary by atomic rename, **keeps the old one**
in `/usr/local/bin/tomi-relay.prev`, restarts, and checks `/healthz`. If the service does not come back
healthy, it rolls back to the old binary by itself. Manual rollback (can be replayed to go forward again):

```bash
sudo bash infra/update.sh --rollback
```

Warning, data migration: the first multi-space version moves the old layout (`tomi.db` and `blobs/` at the root)
to `spaces/<id>/` by itself at startup. A binary from before this migration can no longer find its data: it
would start **empty** (`space_not_initialized`), and clients would record their space as deleted. If spaces appear during an update
when there were none before, `update.sh` therefore **disables the automatic rollback** (it stops with an error, with the new
binary in place), and `--rollback` requires `--force`. To go back to the old version in that case:
`sudo systemctl stop tomi-relay`, restore the backup taken just before the update (see
`backup.md`), then `sudo bash infra/update.sh --rollback --force`. Between two versions that already understand the
multi-space layout, automatic rollback stays active.

Rolling back to a binary from before the "unmounted volume" safeguard: once `TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=true`
is written in `/etc/tomi/relay.env`, a binary that does not know this key refuses to start (unknown key).
Before any rollback (automatic or `--rollback`), `update.sh` detects this case by running the target binary
ALONE: empty environment, throwaway empty data directory, key set to `false` (`admin status --json`; a
binary that does not know the key fails). This test never goes through the running relay (its admin socket
would answer with its own version) and does not read `relay.env`; `install.sh` and the optional managed-hosting
example in `infra/forge/` run the same test. If the binary does not know the key, the line is commented out with a message, once
the binary swap has succeeded (if the swap or the rewrite of `relay.env` fails, the swap is undone and nothing
changes). The safeguard is then **inactive** while this version runs; run `install.sh` again once
you are back on a version that knows it, and it restores the line. For the managed-hosting example, the equivalent
setting (`require_existing_data_dir` in `config.toml`) must be removed by hand before redeploying an old binary.

"Unmounted volume" safeguard after an update: once the update is **healthy** (service active, `/healthz`
OK, old process replaced), `update.sh` arms `TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=true` by itself in
`/etc/tomi/relay.env` if the line is not there (a value already present, `true` or `false`, is respected), with the
same test as `install.sh`: a binary that knows the key (run alone, see above) and a `routing.db` in the
data directory. `relay.env` is rewritten through a temporary file, verified (non-empty, old content intact,
last line = the key), then renamed: never an empty or truncated file. Arming only takes effect at the next
start, and a failure does not cancel the update (a warning is shown). **The systemd unit** (`RequiresMountsFor=`) is
only installed by `install.sh`: `update.sh` never replaces it; if it finds it missing, it says so, and you should then
run `sudo bash infra/install.sh`. In addition, `--rollback` (without `--force`) is refused if `/var/lib/tomi` contains
neither `routing.db`, nor `spaces/`, nor `tomi.db` (volume not mounted, wrong path), whether the safeguard is armed or not.

Refusal to start because of the data (exit code **78**, `spec/api-v1.md` §10.1): volume not mounted, symbolic link on
`spaces/<id>` or `relay.db`, folder or database unreadable (permissions), database belonging to another space, `relay.lock`
impossible to open, two layouts at once (`tomi.db` and `spaces/`), or an interrupted migration
(`spaces.migrating/`). A binary from before multi-space would ignore these spaces, start empty and answer
`409 space_not_initialized` / `401` for a live space: **`update.sh` therefore does NOT roll back** in
this case, neither automatically nor with `--rollback` (with or without `--force`). Before any rollback, it reads the last
exit code of the service (`systemctl show -p ExecMainStatus`) and runs the installed binary (`admin status --json`,
as the `tomi` user and with the `TOMI_RELAY_*` variables of `relay.env`); with 78, it stops, changes nothing,
and tells you what to check: volume mounted, `tomi` permissions on `/var/lib/tomi`, `spaces/` and each `spaces/<id>/`,
no symbolic links. Note: an unmounted volume only gives **78 if the safeguard is armed** (or if `spaces/`
is a link whose target is missing); without the safeguard, the empty folder looks like a new relay and the relay starts
"empty" without refusing, which is why `--rollback` refuses an empty directory above. Fix the problem, then
`sudo systemctl reset-failed tomi-relay ; sudo systemctl restart tomi-relay`. An intact old layout
(`tomi.db` alone, not yet migrated) is **not** a refusal: a binary from before the migration reads it, `admin status`
exits 1 without migrating it, and the rollback is not blocked. A check that **could not conclude** (`runuser`
missing, timeout, exit code 124 / 137 / anything other than 0 and 78) is reported: on a manual `--rollback` the script stops
(`--force` overrides this block, **never** an established 78; by hand: `sudo -u tomi /usr/local/bin/tomi-relay admin
status ; echo $?`), on the automatic rollback it warns and continues (`tomi-relay.prev` has just been copied from the
binary in service). The optional managed-hosting example (`infra/forge/`) has its own deploy script, which runs a
similar check, but without `runuser` or reading `ExecMainStatus`, and it **skips** the check, with a warning, if `config.toml` is missing.

`spaces/` must be a **sub-folder** of the data volume, never the mount point itself nor a link to the root of the mount point: with the volume unmounted,
an empty folder would remain in its place, the relay would see "no space" there and would answer like a new relay. Mount the volume on `/var/lib/tomi` (or above) and leave
`spaces/` inside it.

If `systemctl restart` is refused but `/healthz` answers, `update.sh` does **not** declare the update successful
(it is the old process that is running): it exits with an error and this explanation.

Restarts cut WebSockets: clients reconnect by themselves (increasing random delay) and
catch up using their cursor, no item is lost.

## 10. Day-to-day operations

### Logs and rotation

Everything goes through journald: the relay (JSON on stdout) and Caddy (access log on stderr).

```bash
sudo journalctl -u tomi-relay -f                # follow the relay
sudo journalctl -u tomi-relay --since "1 hour ago" --no-pager
sudo journalctl -u caddy --since today --no-pager   # certificates, access
journalctl --disk-usage
```

There is no log file to rotate: journald rotates by itself. By default it can take up to 10% of the disk
(4 GB at most). To cap it (recommended on a small disk):

```bash
sudo install -d /etc/systemd/journald.conf.d
printf '[Journal]\nSystemMaxUse=500M\nMaxRetentionSec=30day\n' | sudo tee /etc/systemd/journald.conf.d/50-tomi.conf
sudo systemctl restart systemd-journald
```

Privacy: the relay logs no bodies, no headers, no query strings, and no tokens (except the line
`install token:`, written once at the first start of a new relay, see step 7). Caddy's access log is also filtered: it removes
all request and response headers, the query string of URLs and the TLS details. It keeps the method,
the path, the status, the duration and the client IP. Calls to `/healthz` do not appear in it.

### Monitoring

| What | How |
|------|-----|
| The process | `Restart=always`: systemd restarts the relay within 2 s. Beyond 10 failures per minute the unit enters `failed` (`systemctl --failed`) |
| Health seen from outside | `GET https://relay.example.org/healthz` → `200 {"status":"ok"}`, `503` if one of the databases (routing or space) does not respond. Plug in a free probe (UptimeRobot, Healthchecks.io, or a Route 53 health check) that notifies you on failure |
| Functional state | `sudo tomi-admin status`: spaces, active devices, total quota used, **free disk**, **FCM health** (`--space <id>` for one space) |
| The disk | a CloudWatch alarm on disk usage (or a simple look at `df -h /var/lib/tomi`). The relay refuses uploads ("relay full") beyond the total quota (`TOMI_RELAY_MAX_RELAY_BYTES`, 1 GiB by default) or below 1 GiB of free disk |
| The certificate | Caddy renews it by itself; if there is a problem: `journalctl -u caddy`. Behind nginx: renewal is handled by your hosting setup (section 15) |

Before moving from 0.1.x to 0.2.0: the per-space cap of 2 GiB is replaced by this total. Check `sudo du -sh /var/lib/tomi` and, if usage exceeds the total, raise `TOMI_RELAY_MAX_RELAY_BYTES` in `/etc/tomi/relay.env` before the update, otherwise uploads will be refused (`507`).

The total quota (1 GiB by default, all spaces combined) is sized for a small shared disk:
a local backup takes up to 4 times that volume at its peak (see `backup.md`). If you raise it,
recalculate this peak or enlarge the EBS volume.

### Backup

Automatic every night, local copy (2 archives kept). **Set up the off-site copy
(S3, instance IAM role) as soon as the service goes live**: see `backup.md`, which also explains the disk peak,
the safeguard and the restore, complete or of a single space. On your side, keep in your password manager:
`/etc/tomi/relay.env` and the FCM JSON file.

## 11. Losing all devices (or reinstalling the only PC)

If no device holds the space key any more, the old contents are permanently unreadable
(that is the principle of end-to-end encryption). Only the relay operator can then start over
for that space.
**Your SSH access to the instance is the only recovery factor.**

```bash
sudo tomi-admin spaces                  # find the space; note the first 8 characters of its space_id
sudo tomi-admin reset --space <id>      # asks you to retype them to confirm
```

`reset --space <id>` cuts the WebSockets of this space, **deletes all the data of this space only**
(database and blobs; neither the other spaces nor `public/`) and forgets the tokens already consumed for this space.
If it was the **last** space of the relay, the command displays a new installation token. Otherwise
(other spaces exist), it creates none: run `sudo tomi-admin invite --new` for the person
concerned (section 14). The person pastes this token into **Créer mon espace** on their first device, then adds
their other devices by pairing as usual. Take a backup first if in doubt
(`sudo systemctl start tomi-backup.service`); `backup.md` explains how to restore a single space.

**Warn the person beforehand**: on a relay that hosts other spaces, their applications only receive an
authentication refusal (`401`) and will show "device not recognized", without offering to
start over. They must reset (or reinstall) the application, then use the new token.

To remove only a lost device, while another one still works: revoke it from the application, or
`sudo tomi-admin devices --space <id>` then `sudo tomi-admin revoke --space <id> <prefix>`.

Protect SSH access accordingly: `.pem` key backed up outside the PC (password manager), port 22
restricted to your IP in the security group, MFA on the AWS account. If you lose the SSH key, AWS lets
you regain access (EC2 Instance Connect, SSM Session Manager, or mounting the volume on another instance),
provided you have access to the AWS account.

## 12. Android notifications (FCM)

Setup:

1. Firebase console → your project → Project settings → Service accounts → "Generate new private key":
   you get a JSON file. It is a **secret**: never commit it.
2. Put it on the server:

   ```bash
   sudo install -o root -g tomi -m 0640 ~/fcm-service-account.json /etc/tomi/fcm-service-account.json
   shred -u ~/fcm-service-account.json
   ```
3. In `/etc/tomi/relay.env`, uncomment `TOMI_RELAY_FCM_SERVICE_ACCOUNT_FILE=...`, then
   `sudo systemctl restart tomi-relay` and check `sudo tomi-admin status` (FCM health: `ok` after the first
   send, `unknown` before).

**If the FCM credentials expire or are revoked** (key deleted or disabled in the Google Cloud console,
service account deleted, organization policy limiting key lifetime): Android notifications stop
**silently** on the phone. The relay itself keeps working (REST, WebSocket, Windows and CLI clients unchanged).
It logs the error, and `sudo tomi-admin status` shows the FCM health as `auth_error` with the date of the last error.
Nothing is lost: the Android app catches up on items at the next launch thanks to its cursor, just without the
background ping. To repair: generate a new key (step 1), replace the file (step 2), `sudo systemctl restart tomi-relay`,
then check that `status` returns to `ok`. Service-account keys do not expire on their own by default.

## 13. Quick troubleshooting

| Symptom | Lead |
|---------|------|
| `install.sh`: "bad interpreter" | CRLF line endings: `sed -i 's/\r$//' infra/*.sh` |
| `install.sh`: wrong architecture | x86_64 binary on an `aarch64` instance (or the reverse), see step 3 |
| The service restarts in a loop | `journalctl -u tomi-relay -n 100 --no-pager`: invalid configuration (an unknown key makes startup fail), permissions on `/var/lib/tomi` |
| `status=226/NAMESPACE` | a folder listed in `ReadWritePaths` is missing: run `install.sh` again |
| "data_dir looks empty (volume not mounted?)" | safeguard `TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR=true` (added by `install.sh` after the first start, or by `update.sh` after a healthy update): mount the volume on `/var/lib/tomi` (the unit carries `RequiresMountsFor=`) or fix `TOMI_RELAY_DATA_DIR`; set it to `false` only for a new installation |
| `tomi-admin status`: `pending spaces: N` (JSON: `pending_spaces`) with N > 0 | **pending** space: a deletion could not be completed, or the relay could not re-register the space (its devices get `503`, never `401`). Check that the volume of `/var/lib/tomi` is mounted and that `spaces/<id>/relay.db` is there, then **restart the relay**: the space comes back whole, or stays deleted if the deletion had already completed on disk |
| The service refuses to start: "is a symbolic link" | `spaces/<id>` or `spaces/<id>/relay.db` is a symbolic link, which the relay neither loads nor ignores (its devices would believe their space was deleted): replace the link with the real folder (or the real file). A link on `spaces/` or on `/var/lib/tomi` itself is allowed **as long as its target exists** (otherwise, unmounted volume: the service refuses to start, "the spaces folder cannot be read", instead of believing it is empty); in that case keep them on the same filesystem, otherwise a space deletion is done in place (rename `EXDEV` impossible) |
| The service refuses to start: "cannot look at the database of space …" / "cannot read … the spaces folder" (`Permission denied`, I/O error) | the relay could not look at `spaces/` or `spaces/<id>/relay.db`: it does NOT interpret this as "no space" (its devices would receive `401`, or `409` and an installation token if alone). Typical cause: backup restored as root without `chown -R tomi:tomi /var/lib/tomi`, or a failing disk. Fix the permissions (`chown -R tomi:tomi`, folders `0700`), then restart |
| The service stops with exit code 78 (`systemctl status tomi-relay`: `status=78`), `update.sh` refuses the rollback | refusal to start **because of the data** (volume not mounted, symbolic link, folder or database unreadable, database of another space): do NOT go back to an old binary (it would answer `409`/`401` for a live space). Read `journalctl -u tomi-relay -n 30`, check the volume mount, `ls -ld /var/lib/tomi /var/lib/tomi/spaces`, `find /var/lib/tomi/spaces -maxdepth 2 -type l`, the permissions of `tomi`, then `sudo systemctl reset-failed tomi-relay ; sudo systemctl restart tomi-relay`. `spaces/` is a sub-folder of the volume, never the mount point itself |
| Certificate not obtained | DNS (step 1), ports 80/443 (step 2), `journalctl -u caddy` (behind nginx: the site certificate in your hosting panel, section 15) |
| 502 from Caddy or nginx | the relay is stopped or crashed: `systemctl status tomi-relay` |
| 413 on an upload | body larger than 4 MiB refused by Caddy or nginx (`client_max_body_size`); the relay already has much lower limits |
| WebSockets (`/v1/ws`) drop or fail behind nginx | `location = /v1/ws` block missing or without `Upgrade`/`Connection`, or `proxy_read_timeout` too short (section 15) |
| `relay_full` although the disk has room | total quota (`TOMI_RELAY_MAX_RELAY_BYTES`) or space quota reached: `sudo tomi-admin status` / `spaces` |
| `tomi-backup`: exit code 3, "not enough disk space" | the safeguard refused the backup without writing anything: free up space or lower `TOMI_BACKUP_KEEP` (see `backup.md`) |
| `tomi-admin`: permission denied | run it with `sudo` (it switches to the `tomi` user by itself) |

## 14. Several people on the relay

One relay and one URL (`relay.example.org`) serve several people (fewer than 5). Each person
has **their own space**: their key, their devices, their SQLite database and their blobs, in `spaces/<space_id>/`. Nothing is
shared between spaces, and the relay cannot read any content. As the operator, you only see the
metadata (`backup.md`, "Everything is encrypted, but not everything is hidden").

The commands below are the ones provided by the relay's admin interface. `sudo tomi-admin --help` gives the
actual list.

**Create the space of a new person**

```bash
sudo tomi-admin invite --new
```

The command adds an invitation and displays its installation token (only once: the relay keeps only its
fingerprint). Pass it on **out of band** (in person, or through an encrypted messenger),
never in a ticket or a plain email: it is a single-use secret. The person pastes it into
**Créer mon espace**; the space is created and the invitation is consumed. Then check with `spaces` that
the space appears. `sudo tomi-admin invite` alone lists the pending invitations; `invite --rotate` replaces
them all with a single one.

**List and inspect**

```bash
sudo tomi-admin spaces                  # all spaces: state, devices, usage
sudo tomi-admin status                  # global: number of spaces, total quota, disk, FCM
sudo tomi-admin status --space <id>     # one space (the identifier or its beginning)
sudo tomi-admin devices --space <id>
sudo tomi-admin revoke --space <id> <device-prefix>
```

**Quotas**

Two caps, plus the free-disk floor (1 GiB):

| Setting | Scope | Default |
|---------|-------|---------|
| `TOMI_RELAY_MAX_RELAY_BYTES` (`/etc/tomi/relay.env`) | sum of all spaces | 1 GiB |
| `tomi-admin quota --space <id> <octets>` or `none` | one space | no limit |
| `TOMI_RELAY_MAX_SPACE_BYTES` (optional) | identical cap for each space | no limit |

```bash
sudo tomi-admin quota --space <id> 268435456   # 256 MiB for this space
sudo tomi-admin quota --space <id> none        # removes the limit of this space
```

The total quota protects the server's disk; the sum of the per-space quotas may exceed it. A
person who reaches a cap receives `relay_full`; they only see the usage of their own space. With
fewer than 5 people, start without per-space quotas and look at `spaces` from time to time. If you change
`TOMI_RELAY_MAX_RELAY_BYTES`, restart the relay (`sudo systemctl restart tomi-relay`), and redo the calculation of the
backup peak (`backup.md`: about 4 times the data with 2 local archives).

**Reset a space** (the person has lost all their devices): see step 11, with `--space <id>`:

```bash
sudo tomi-admin reset --space <id>
```

Only this space is erased. Take a backup first (`sudo systemctl start tomi-backup.service`). To
roll back a single space without touching the others, `backup.md` describes restoring one space.

As long as other spaces remain, `reset --space` creates **no** token: for the person to start again, then run
`sudo tomi-admin invite --new` and pass the token on to them. Warn them: their applications will show
"device not recognized" (see step 11).

## 15. Server already served by nginx

Typical case: a server with nginx on ports 80 and 443 (for example one managed by a hosting panel, with MySQL and
PHP-FPM alongside). Caddy cannot listen there at the same time, and moving the existing sites behind Caddy is not
worth it: nginx does the same job. The relay itself listens on `127.0.0.1:8080` and does not change.

What changes compared with the rest of the runbook:

- step 4: do not install Caddy. Step 6: `install.sh ... --no-caddy`;
- the security group already has 80 and 443 open for nginx; the DNS `A relay` record must point
  to the IP of this server;
- TLS: use the Let's Encrypt certificate that your hosting panel (or certbot) obtains and renews for the site. Create the
  site `relay.example.org` in the panel, request the Let's Encrypt certificate, then edit the nginx configuration of the
  site (in the panel, or the site file under `/etc/nginx/`). Keep **as they are** the `listen`, `server_name` and
  `ssl_certificate`/`ssl_certificate_key` lines written by the panel (paths it chose), the port 80 redirect and the
  ACME challenge. Replace the rest with the block below. Where the file lives and how the includes are organized
  depends on your setup: check it first.

File `/etc/nginx/snippets/tomi-headers.conf` (security headers, the same as in the `Caddyfile`).
An `add_header` set in a `location` cancels those of the level above, hence this file is included in
both places:

```nginx
add_header Strict-Transport-Security "max-age=31536000" always;
add_header X-Content-Type-Options "nosniff" always;
add_header X-Frame-Options "DENY" always;
add_header Referrer-Policy "no-referrer" always;
add_header Permissions-Policy "camera=(), microphone=(), geolocation=(), payment=()" always;
add_header Cross-Origin-Opener-Policy "same-origin" always;
```

`server` block of the site (HTTPS):

```nginx
server {
    listen 443 ssl;
    listen [::]:443 ssl;
    server_name relay.example.org;

    # --- Lines written by the hosting panel or certbot: keep them as they are ---
    ssl_certificate     /etc/nginx/ssl/relay.example.org/…/server.crt;
    ssl_certificate_key /etc/nginx/ssl/relay.example.org/…/server.key;
    # (and the rest of your TLS configuration)

    # A JSON body of 2 MiB at most or a blob part of 1 MiB: 4 MiB, as in the Caddyfile. The relay
    # applies its own, stricter limits.
    client_max_body_size 4m;
    client_header_timeout 10s;

    # No access log: the default format ("combined") writes the full URI, query string included,
    # where a token could appear. The error log only keeps serious failures, for the same reason
    # (an upstream error copies the request line into it). The relay's log (journald)
    # remains the diagnostic source. Variant with an access log without query string: see below.
    access_log off;
    error_log /var/log/nginx/relay.example.org-error.log crit;

    include snippets/tomi-headers.conf;

    # WebSocket: Upgrade/Connection, and a long read timeout. An idle connection stays open
    # (Caddy does not apply a read timeout either).
    location = /v1/ws {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 1d;
        proxy_send_timeout 1d;
        proxy_buffering off;
    }

    # API: never cached by intermediaries. No buffering to disk (the disk is small): uploads
    # in 1 MiB parts and blobs are streamed. The Authorization header is passed as is to the relay,
    # which checks it.
    location /v1/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_request_buffering off;
        proxy_buffering off;
        proxy_read_timeout 60s;

        proxy_hide_header Cache-Control;
        add_header Cache-Control "no-store" always;
        add_header Content-Security-Policy "default-src 'none'; frame-ancestors 'none'" always;
        include snippets/tomi-headers.conf;
    }

    # The rest (/healthz, /updates/, /.well-known/, /l/): public files of the relay, which sets
    # their Cache-Control itself, and the CSP of /l/ (spec/api-v1.md).
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 60s;
    }
}
```

Known differences from the `Caddyfile`: nginx cannot set a header "only if it is missing" (Caddy's `?` prefix). The
`Cache-Control` and the CSP of the API are therefore enforced on `/v1/`, and left to the relay
elsewhere. Caddy's compression (`encode zstd gzip`) is not reproduced, and `/healthz` needs no exclusion from the
log since access is not logged.

Variant: an access log without query string or `Authorization` header. `log_format` can only be declared
in the `http` context; put it in `/etc/nginx/conf.d/tomi-log.conf` (provided that
`/etc/nginx/nginx.conf` contains `include /etc/nginx/conf.d/*.conf;`, to check with
`grep -n conf.d /etc/nginx/nginx.conf`):

```nginx
# The space creation link may carry the installation token in the path (`/create/<token>`,
# the form accepted by the apps): this segment is never logged.
map $uri $tomi_logged_uri {
    default $uri;
    ~^/create/ /create/-;
}
log_format tomi '$remote_addr [$time_local] "$request_method $tomi_logged_uri" $status $body_bytes_sent';
```

then replace `access_log off;` in the `server` block with
`access_log /var/log/nginx/relay.example.org-access.log tomi;`. `$uri` has no query string, and the `map`
above hides the segment that follows `/create/`. Never add `$http_authorization`, `$request` or
`$request_uri` to this format (they would log the token of a `/create/<token>` link).

Check and reload:

```bash
sudo nginx -t && sudo systemctl reload nginx
curl -fsS https://relay.example.org/healthz                 # {"status":"ok"}
curl -si -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
     -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' https://relay.example.org/v1/ws | head -n 1
#   a response from the relay (for example 401 without authentication) proves the Upgrade gets through; a 400/502 comes from nginx
```

Server resources: on a small server shared with other services, memory is tight and `/var/log` may already be large.
Cap journald as in step 10 (`SystemMaxUse=500M`), and watch the relay's memory with
`systemctl status tomi-relay` after the first few days. If needed, `sudo systemctl edit
tomi-relay` lets you add `MemoryMax=256M` (indicative value, adjust it to your measurements).

## To do

Known points, **not implemented**:

- **Unsigned relay releases, deployment on a floating tag** (the optional managed-hosting example in `infra/forge/`
  deploys from `TOMI_RELEASE_TAG`, default `relay-latest`). The `.sha256` comes from the same release as the binary: it
  guarantees the integrity of the transfer, not its authenticity. Whoever can write to the release replaces both the binary and
  its fingerprint, and `relay-latest` is rewritten at each publication. Options:
  1. sign the binary with **minisign** (private key outside CI or in a protected secret, public key versioned
     in the repository and copied to the server) and have `minisign -V` checked by `deploy.sh` / `update.sh`
     before installation;
  2. or publish a **GitHub provenance attestation** (`actions/attest-build-provenance` in CI) and verify it
     with `gh attestation verify <binary> --repo tomi-share/tomi` (requires `gh` on the server);
  3. in both cases, deploy a **frozen tag** (`relay-vX.Y.Z`, never rewritten) passed explicitly
     (`TOMI_RELEASE_TAG=relay-vX.Y.Z`) rather than the floating tag, so that the deployment is
     reproducible and a rollback points to a precise version.
- **Lifetime of invitations**: an invitation never expires. The token written to the log at the first
  start of a new relay therefore remains valid indefinitely; in the meantime, it is made harmless with
  `tomi-admin invite --rotate` (step 7). An expiry requires a code change in `apps/relay`
  (and a field in `routing.db`), outside the scope of the infrastructure.
- **Token never logged**: the relay writes the line `install token:` to its log (`apps/relay`, first
  start). The infrastructure can only restrict the log (for example mode 0600); removing it or sending it only to
  standard output of a console also requires a change in the relay.

## Assumptions about the relay (to be respected in `apps/relay`)

These points come from `spec/api-v1.md`; the infrastructure relies on them.

- Without arguments, `tomi-relay` starts the server; `tomi-relay admin <subcommand>` administers it.
- Configuration through `TOMI_RELAY_*` variables only (`LISTEN`, `DATA_DIR`, `PUBLIC_URL`,
  `FCM_SERVICE_ACCOUNT_FILE`): the absence of the default TOML file (`/etc/tomi-relay/config.toml`) must
  not prevent startup. This deployment sets `data_dir` to `/var/lib/tomi` (the default in the spec is
  `/var/lib/tomi-relay`).
- The relay writes only under `data_dir`: `routing.db`, `spaces/<space_id>/relay.db` and
  `spaces/<space_id>/blobs/<2 hex>/<blob_id>`, `public/`, `admin.sock`, `relay.lock`. There is no
  `install-token` file any more: a file inherited from the old layout is imported as an invitation and then
  deleted, and the following tokens are never written to disk (only their fingerprints, in
  `routing.db`). It reads `public/updates/`, `public/.well-known/assetlinks.json` and `public/l/index.html`.
  The old layout (`tomi.db` and `blobs/` at the root) is migrated by itself at startup.
- Quotas: `max_relay_bytes` (total, 1 GiB by default) and `max_space_bytes` (optional; absent = no
  limit, `0` = no limit; this default can also be set from the relay's web panel); per-space quota
  with `tomi-relay admin quota --space <id> <octets|none|default>`. `admin quota default` returns the space to the
  relay's default: the web panel first, then `max_space_bytes`, then no limit. Admin commands:
  `spaces`, `quota`, `invite --new`, and `--space <id>` on `status`, `devices`, `revoke`, `reset`.
- `GET /healthz` answers `200` if **all** the databases (`routing.db` and each `spaces/<id>/relay.db`)
  respond within two seconds, `503` otherwise.
- Backup: `tomi-backup` reads `routing.db` and `spaces/*/relay.db` (`.backup` copy). `TOMI_BACKUP_DB` is only
  used with the old layout.
- No Linux capability, privileged port or persistent `/tmp` is needed; outbound calls are HTTPS
  to Google (FCM). DNS lookups may go through a Unix socket.
