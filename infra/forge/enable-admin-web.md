# Enable the web administration panel and create the access key

Step by step, over SSH on the server (see `infra/forge/README.md`). The panel is
**disabled by default**; it shows the state of the relay and offers a few actions (remove devices, delete a
space into a 7-day trash, invitations, quotas). It is protected by access keys (passkeys, WebAuthn, up to 5), and
what destroys or creates a right asks for a key **at each action**. The relay stays blind: the panel never shows content,
keys or secrets. Full description of the panel: `apps/relay/README.md`, section
"Web administration panel".

The binary is **not** in the `PATH`: call it with its full path, with `--config`.

Prerequisite: `public_url` in `/home/deploy/tomi-data/config.toml` is an **https** URL on a domain
name (for example `https://relay.example.org`). Otherwise the relay refuses to start with the
panel enabled.

> **Warning: a line added by hand can disappear.**
> `admin_web = true` is written here in `/home/deploy/tomi-data/config.toml`, **outside** the
> versioned configuration `infra/forge/config.toml`. A redeployment that rewrites `config.toml` from
> that copy would erase the line: the panel would become a 404 with no other sign. Two options: add
> the line to `infra/forge/config.toml` as well, or provide an activation recipe (to come, see
> `infra/forge/recipes.md`). Until then, run command 3 again after each redeployment.

## 1. Enable the panel (once)

```bash
ssh deploy@<server host>

# 1. enable the panel: add the line to the relay configuration
printf '\nadmin_web = true\n' >> /home/deploy/tomi-data/config.toml

# 2. restart the supervisor daemon (use the name shown by: sudo supervisorctl status)
sudo supervisorctl restart daemon-<id>:

# 3. check
curl -fsS http://127.0.0.1:8080/healthz && echo
```

Before command 1, check that the line does not already exist
(`grep -n admin_web /home/deploy/tomi-data/config.toml`): a duplicate key is a configuration error.
The configuration is flat: the line goes at the root level, not under a `[...]` table.

At command 3, `curl -f` returns without error if the relay answers 200. Otherwise: `sudo supervisorctl status`,
then the daemon's log.

## 2. Show the panel's address

```bash
# 4. show the panel's URL (secret path included)
/home/deploy/relay.example.org/tomi-relay --config /home/deploy/tomi-data/config.toml admin web
```

The command prints the full URL (`public_url` + path) and the state of the key (registered or not).
The path is a random 128-bit segment created at the first start with the panel enabled. It is **not**
a password (the access key is), but do not paste it into a message, a screenshot or a ticket: it is
defence in depth against scanners.

## 3. Create the enrolment code

```bash
# 5. create an enrolment code (shown ONCE, valid for 15 minutes, 5 attempts)
/home/deploy/relay.example.org/tomi-relay --config /home/deploy/tomi-data/config.toml admin web enroll
```

The code has the form `xxxx-xxxx-xxxx`. It is shown only once (the relay keeps only its fingerprint),
is valid for 15 minutes, and the fifth failed attempt invalidates it. If you lose it or it expires,
run the command again.

This command writes: it goes through the socket of the running relay (`admin.sock`), so run it
**after** the restart (command 2), as the `deploy` user.

## 4. Create the access key, from the phone

1. Open the URL shown by command 4 in the phone's (or computer's) browser: the page offers
   "Create my access key".
2. Paste the enrolment code.
3. Confirm the creation of the key with the device (fingerprint, face or device passcode).
4. The page then asks you to sign in: "Sign in with my access key", and confirm again.

The browser must support access keys (WebAuthn); the relay requires user verification (fingerprint,
face or passcode), not just the presence of the key.

**As soon as a key is registered, enrolment is closed** for good: its addresses answer 404, and any
remaining code is useless. There is only one key and one administrator per relay.

## 5. Manage the path and the account: over SSH only

None of this is done from the panel or from the network. Same command prefix as above
(`BIN` = `/home/deploy/relay.example.org/tomi-relay`, `CFG` = `/home/deploy/tomi-data/config.toml`):

```bash
BIN=/home/deploy/relay.example.org/tomi-relay
CFG=/home/deploy/tomi-data/config.toml

# Show the URL and the state of the key.
"$BIN" --config "$CFG" admin web

# New path: the old one and all open sessions stop working.
"$BIN" --config "$CFG" admin web --rotate-path

# Delete the key and the sessions, and reopen enrolment (then repeat the "Create the enrolment code" section).
# Without --confirm, the command refuses (code 2) and shows the 8 characters to retype.
"$BIN" --config "$CFG" admin web reset
"$BIN" --config "$CFG" admin web reset --confirm <first 8 characters of the path>
```

- `--rotate-path`: use it if the URL has leaked, or as a precaution. Then open the new URL (shown by
  `admin web`); the access key stays valid.
- Several keys: run `admin web enroll` again for each one (5 at most), ideally one **bound to its device** (hardware
  key). `admin web keys` lists them, `admin web keys remove <N>` removes one and closes its sessions (never the last).
  `admin web policy --space-deletion device-key` then restricts the deletion of a space to the device-bound keys (off
  by default; refused while none is enrolled).
- The trash: a space deleted from the panel waits 7 days. `admin trash` lists it, `admin trash restore --space <id>`
  brings it back (then open Tomi on each device of the space), `admin trash purge --space <id>` deletes it for good.
- `reset`: use it to change device or after losing every key. It deletes the registered keys and the
  sessions, and **reopens enrolment**: create a new code (`admin web enroll`) and register a new key.
  It requires `--confirm` with the first 8 characters of the panel's path.
- Disable the panel: remove `admin_web = true` from the configuration and restart the daemon. All the
  panel's paths then answer exactly like any unknown resource.

## See also

- `infra/forge/recipes.md`: the relay's panel recipes (state, administration, backups), and §2 for
  command-line administration.
- `apps/relay/README.md`, section "Web administration panel": path, enrolment, WebAuthn, limits and
  security headers.
- `infra/forge/README.md`: paths and daemon of the example deployment.
