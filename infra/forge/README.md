# Tomi — relay on a managed server (example: Laravel Forge)

This directory is an **optional example** of running the relay on a managed server, using a
hosting panel such as **Laravel Forge** (Ubuntu server, Nginx, a supervisor daemon, a deploy
script). It is one way among others. **The reference deployment guide is `infra/README.md`**
(EC2 host + Caddy + systemd); everything in this directory is an alternative to it.

Names used throughout (replace them with your own):

| Placeholder | Meaning | Example |
|-------------|---------|---------|
| `relay.example.org` | public domain served by Nginx (`public_url` in `config.toml`) | |
| `OWNER/REPO` | GitHub repository that publishes the release (`TOMI_RELEASE_REPO`) | `tomi-share/tomi` |
| `deploy` | the Unix user that runs the relay, the deploy script and the recipes | |
| `daemon-<id>` | the supervisor program name given by the panel | |
| `<server host>` | the server's address or SSH alias | |

```
Clients (Windows, Android, Apple)
      │ HTTPS / WSS : 443  (TLS + Let's Encrypt managed by the panel / Nginx)
      ▼
   Nginx (panel)  ──────────►  tomi-relay  (127.0.0.1:8080, user `deploy`, supervisor daemon)
   reverse proxy + TLS           routing.db + one SQLite database and encrypted blobs per space,
                                 in /home/deploy/tomi-data/spaces/<space_id>/
```

Why this model rather than a PHP "site": the relay is a **standalone Rust binary** (HTTP +
WebSocket), not a PHP app. A managed-server panel handles it with two native building blocks —
a **daemon** (supervisor) and a **custom Nginx config** (reverse proxy) — without Docker or a PHP
deployment flow. It is the panel-based equivalent of the systemd + Caddy pair of `infra/`.

Paths on the server (adjust them to your layout):

| What | Path |
|------|------|
| Binary | `/home/deploy/relay.example.org/tomi-relay` |
| Deploy script | `/home/deploy/relay.example.org/deploy.sh` (copy of `infra/forge/deploy.sh`) |
| Data | `/home/deploy/tomi-data`: `routing.db`, `spaces/<space_id>/relay.db`, `spaces/<space_id>/blobs/`, `public/`, `admin.sock`, `relay.lock` |
| Relay configuration | `/home/deploy/tomi-data/config.toml` (copy of `infra/forge/config.toml`) |
| GitHub token (deployment) | `/home/deploy/.config/tomi/github-token` (0600, **outside the repository**) |
| Logs | `/home/deploy/logs/` (daemon and backup logs) |
| Backup script | `/home/deploy/.local/bin/tomi-backup` (copy of `infra/backup.sh`, see `recipes.md` §7) |
| Backup archives | `/home/deploy/tomi-backups` |

Data layout: the relay is **multi-space** (one person = one space, fewer than 5 people).
Each space has its database and blobs in `spaces/<space_id>/`; `routing.db` keeps only the
invitations (token fingerprints), the quotas per space and the FCM health. There is **no more**
`tomi.db` nor `blobs/` at the root: if the old layout is present on the first start of the
multi-space relay, it migrates it by itself (renames; nothing is copied or rewritten). There is also
no durable `install-token` file any more: an old file is imported as an invitation, then deleted
(tokens: see `recipes.md` §2).

Files in this directory (versioned copies of what lives in the panel or on the server):

| File | Role |
|------|------|
| `deploy.sh` | "pull" deployment script: downloads the latest published binary and restarts the daemon |
| `nginx-site.conf` | body of the `server {}` to paste into the site's Nginx configuration (reverse proxy) |
| `supervisor-daemon.conf` | values of the panel daemon that runs the relay |
| `config.toml` | relay configuration (no secret), including the total quota `max_relay_bytes` |
| `recipes.md` | panel recipes: multi-space administration, backups, public files, logs, troubleshooting |
| `recipes/init-relay.sh` | "Tomi relay: init" recipe (install token and guard) |
| `enable-admin-web.md` | enabling the optional web administration panel and creating its access key |

---

## 1. Delivery chain (CI → release → deployment)

```
tag relay-vX.Y.Z  ──►  GitHub Actions "Release relay"  ──►  release relay-vX.Y.Z
   (git push)              build musl + windows, checks        + release relay-latest
                           the digest, publishes                (moving pointer)
                                                                          │
   panel deploy action  ◄── or the "Trigger deploy hook (optional)" step ─┘
      │                     (POST to FORGE_DEPLOY_URL)
      ▼
   The panel runs deploy.sh on the server:
   downloads relay-latest (GitHub API + token) ─► checks SHA-256 + ELF x86_64
   ─► replaces the binary (atomic, old one kept) ─► restarts the daemon ─► /healthz
                                                                 └─ automatic rollback if KO
```

The server **fetches the release already built**: it never compiles (the host has little RAM and no
Rust toolchain; a Rust build would run out of memory there). The binary therefore always comes from CI.

## 2. Setting up the panel (once)

1. **Daemon** (Server → Daemons → New Daemon): enter the values of `supervisor-daemon.conf`
   (command, directory `/home/deploy/tomi-data/`, user `deploy`). The panel writes them to
   `/etc/supervisor/conf.d/daemon-<id>.conf` and manages the daemon.
2. **Nginx** (site → Edit Files → Edit Nginx Configuration): paste the body of `nginx-site.conf`
   into the `server {}`, keeping the panel's own "FORGE CONFIG / FORGE SSL" blocks (the name is the
   panel's; keep it as the panel writes it).
3. **TLS** (site → SSL): Let's Encrypt for `relay.example.org`.
4. **Directories and configuration**: create the closed data directory, then put `config.toml` in it:
   ```bash
   install -d -m 0700 /home/deploy/tomi-data /home/deploy/logs
   install -m 0600 config.toml /home/deploy/tomi-data/config.toml
   chmod -R go-rwx /home/deploy/tomi-data
   ```
   (The daemon has `umask=077`: everything the relay creates afterwards is born closed. The
   "Tomi relay: init" recipe reruns the `chmod -R`; see §8.)
5. **Environment** (site → Environment): set `TOMI_RELEASE_REPO=OWNER/REPO` (the repository that
   publishes the release). The deploy script refuses to run without it.
6. **Deploy script** (site → Deploy Script): have `deploy.sh` run. Two ways:
   - copy `deploy.sh` to `/home/deploy/relay.example.org/deploy.sh` and set the panel's deploy script
     to `bash /home/deploy/relay.example.org/deploy.sh`;
   - or paste the content of `deploy.sh` directly into the Deploy Script field.
   Keep the repository copy (`infra/forge/deploy.sh`) as the source of truth.

## 3. The GitHub token (the "key", set by hand, outside the repository)

`deploy.sh` downloads the asset of the `relay-latest` release through the GitHub API. The
repository that publishes it is private in the original deployment, so a read-only token is needed.
An SSH deploy key is **not enough**: it grants access to the git repository, not to release assets.
(For a public repository, release assets can be downloaded without a token; the script as written
still expects one. See §8.)

1. GitHub → Settings → Developer settings → **Fine-grained token**: *Repository access* =
   `OWNER/REPO` only; *Permissions* = **Contents: Read-only**; short lifetime, renewable. (Nothing else.)
2. Put it on the server, readable by `deploy` only:
   ```bash
   install -d -m 0700 /home/deploy/.config/tomi
   printf '%s' 'github_pat_xxx' > /home/deploy/.config/tomi/github-token
   chmod 0600 /home/deploy/.config/tomi/github-token
   ```
   `deploy.sh` never logs its value. It also accepts the token in the site's `.env` file
   (`TOMI_GITHUB_TOKEN=`) or in the environment; see the comments in `deploy.sh`.

## 4. Deploying

- **Normal**: publish a release (`git push origin relay-vX.Y.Z`), then trigger the panel's deploy
  action — from a workstation, the workflow "Trigger deploy hook (optional)" (automatic after a release
  when `FORGE_DEPLOY_URL` is set), or the Deploy button of the panel's UI.
- `deploy.sh` is **idempotent**: if the binary in service is already the latest published one, it does
  nothing. Otherwise it replaces it, restarts, checks `/healthz`, and **rolls back by itself** to the
  previous binary (kept as `tomi-relay.prev`) if the relay does not come back healthy.
- **Exception: data migration.** The first deployment of a multi-space relay migrates `tomi.db` and
  `blobs/` to `spaces/<id>/` by itself. A binary from before this migration would not find its data:
  it would start empty (`space_not_initialized`) and the clients would record their space as deleted.
  If spaces (`spaces/*/relay.db`) appear during a deployment that had none, `deploy.sh` therefore
  **does not roll back** to the old binary: it stops with an error and leaves the new binary in place
  (procedure: `recipes.md` §3). As long as no space has been created (an empty relay), there is nothing
  to lose and the automatic rollback stays active.
- Before the first multi-space deployment **with data**, take a backup (`recipes.md` §7).
- Pinned deployment (a precise version instead of the latest): run `deploy.sh` with
  `TOMI_RELEASE_TAG=relay-vX.Y.Z`.

## 5. CI

One workflow, **`.github/workflows/release-relay.yml`**, on a `relay-v*` tag:

1. builds (musl + Windows x64), checks the SHA-256 digests;
2. publishes the release `relay-vX.Y.Z`;
3. updates `relay-latest` (same assets);
4. **optional deploy hook**: if the secret `FORGE_DEPLOY_URL` is set, `POST`s to the site's deployment
   URL → the panel runs `deploy.sh` on the server, which downloads `relay-latest` and restarts the relay.
   Without the secret, the release is still published and the deployment can be triggered by hand
   (the panel's deploy action).

Secret to define (Settings → Secrets and variables → Actions): **`FORGE_DEPLOY_URL`**, the site's
deployment URL copied from the panel's UI. This webhook belongs to the site and gives access to nothing
else; no broad panel token is stored in CI.

## 6. Operation

See `recipes.md`: health, install tokens and spaces (`spaces`, `invite --new|--rotate`, `quota`,
`reset --space`), devices, **backups** (§7), public files (assetlinks, `/l/pair` page, APK), FCM,
logs. The panel's CLI can show the logs from a workstation.

## 7. Differences from the EC2/Caddy deployment (`infra/`)

| | EC2 + Caddy (`infra/`) | Managed server (this directory) |
|--|------------------------|---------------------------------|
| TLS / reverse proxy | Caddy (`infra/caddy/Caddyfile`) | Nginx managed by the panel (`nginx-site.conf`) |
| Service | hardened systemd (`infra/systemd/tomi-relay.service`) | the panel's supervisor daemon |
| User | `tomi` (dedicated, nologin) | `deploy` |
| Data | `/var/lib/tomi` | `/home/deploy/tomi-data` |
| Configuration | `TOMI_RELAY_*` environment (`relay.env`) | `config.toml` |
| Binary | manual `scp` + `install.sh` / `update.sh` | `deploy.sh` (pulls the release, run by the panel) |
| Backup | `tomi-backup.timer` (systemd, user `tomi`) | panel cron (user `deploy`), `recipes.md` §7 |
| Admin commands | `sudo tomi-admin ...` | `tomi-relay --config … admin ...` (`recipes.md` §2) |
| Hardening | `Protect*`, `SystemCallFilter`… | none (supervisor); `umask=077` + data in 0700; see §8 |

## 8. Isolation, token and release: residual risks

**Shared user `deploy`.** On a managed server, the relay runs as `deploy`, the same user as the panel,
the recipes and the other sites on the server. Mitigations in place: `umask=077` in
`supervisor-daemon.conf` (files created as 0600, folders as 0700) and `chmod -R go-rwx` on
`/home/deploy/tomi-data` (§2 step 4, init recipe). They close the data to *other* accounts on the
server, not to `deploy`. Residual risk:

- any code that runs as `deploy` (a recipe, another site, a compromised deploy script) can read the
  databases, the encrypted blobs, `config.toml` and the GitHub token;
- the binary (`/home/deploy/relay.example.org/tomi-relay`) can be modified by the user that runs it: a
  compromised relay can rewrite itself, and the replacement persists across restarts.

The blobs and the contents stay end-to-end encrypted (the relay is blind): the risk lies in the metadata,
the relay's databases and availability, not in the content of the spaces.
**Recommended: a dedicated user** (`tomi`, system account, no shell) that owns `/home/deploy/tomi-data`
(or better, `/var/lib/tomi`), with `user=tomi` in the daemon; the binary stays owned by `deploy` (or root)
with mode 0755, so `tomi` cannot modify it.

**GitHub token of `deploy.sh`.** It must be a **fine-grained, read-only** token (*Contents: Read-only*,
repository `OWNER/REPO` only, short lifetime). `deploy.sh` no longer puts it on the `curl` command line
(it sends the `Authorization` header on `curl`'s standard input, read as configuration with `-K -`: it
therefore does not appear in `ps` and is written to no temporary file).

**Integrity, not authenticity.** The release's `.sha256` guarantees that the downloaded binary is the one
that was published (integrity of the transfer), not that it comes from us: whoever can write to the GitHub
release can replace both the binary *and* its digest. Next step: sign the release (minisign, or GitHub's
build provenance attestation `gh attestation`) and have `deploy.sh` verify the signature before installing.
Also deploy a pinned tag (`TOMI_RELEASE_TAG=relay-vX.Y.Z`) rather than `relay-latest`, which is rewritten at
every publication. Details: the "To do" section of `infra/README.md`.

**Daemon log.** `/home/deploy/logs/tomi-relay.log` is created by supervisord with its own umask, not the
relay's, and it contains the `install token:` line of the first start (an invitation that never expires).
The init recipe tries to set it to 0600, but only if the file is owned by `deploy`: supervisord runs as root
and opens this log itself, so the file is probably owned by root, in which case the `chmod` fails (a warning,
without stopping the recipe). Check on the server: `ls -l /home/deploy/logs/tomi-relay.log*`. If the log
remains readable, close the folder (`sudo chmod 700 /home/deploy/logs`) or run this block of the recipe as
root. The real safety net is the rotation of init mode (`admin invite --rotate`), which cancels the logged
token: run it anyway after creating the first space.
