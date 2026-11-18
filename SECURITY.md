# Security policy

Tomi is an end-to-end encrypted sharing tool, so security reports are taken seriously and handled privately first.

## Reporting a vulnerability

**Do not open a public issue, pull request or discussion for a vulnerability.**

Report it privately through GitHub:
<https://github.com/tomi-share/tomi/security/advisories/new> (*Security* tab, then *Report a vulnerability*).

Please include:

- the component (core, relay, Android, Windows, iOS, CLI, specification) and its version or commit;
- what an attacker needs (network position, a paired device, access to a device, a malicious relay, ...);
- steps to reproduce, or a proof of concept, and the impact you observed;
- whether you want to be credited.

Never include real secrets, real keys or real user content in a report. Use throwaway spaces and devices.

## What to expect

Tomi is maintained by a small team, so these are targets, not guarantees:

- acknowledgement within **7 days**;
- an initial assessment (accepted, needs information, or not a vulnerability) within **14 days**;
- a fix, or a mitigation plan, communicated before disclosure;
- coordinated disclosure: we ask for up to **90 days** before public details, shorter if a fix ships earlier.

Accepted reports are credited in the release notes and the advisory unless you prefer to stay anonymous.
There is no bug bounty.

## Supported versions

Tomi is pre-1.0. Only the latest release of each component (relay, Android, Windows, iOS) receives security fixes.
The `*-dev` pre-releases are development builds and are not supported.

## Scope

In scope: anything that breaks the properties described in [`AGENTS.md`](AGENTS.md) (golden rules), [`PROJECT.md`](PROJECT.md)
and [`spec/`](spec/), for example:

- the relay learning content, titles, keys or secret status of an item, or accepting forged or replayed envelopes;
- a flaw in the pairing protocol, the item format, the signatures or the key handling in `crates/core`;
- clear-text secrets at rest, in logs, in notifications or in an unprotected clipboard;
- a malicious item (HTML, Markdown, link, attachment name) that escapes its sandbox or triggers an action without
  the user's consent;
- a `tomi-share://` or App Link URL that does more than pre-fill a field;
- a vulnerability in the relay (authentication, rate limiting, administration, input handling).

Out of scope:

- the metadata the relay needs by design and that is documented in [`spec/api-v1.md`](spec/api-v1.md) §1.2 (device names,
  platforms, activity dates, sizes, receipts);
- a device that is already compromised, rooted or unlocked in the attacker's hands;
- the documented trade-off that attachments of a secret are decrypted into `files/cache/` until the app goes to the
  background or restarts (see `AGENTS.md`, golden rule 3);
- a misconfigured self-hosted relay (open ports, no TLS, weak host hardening) and volumetric denial of service;
- social engineering, and findings that need physical access plus the device PIN;
- reports from automated scanners without a demonstrated impact.

## Safe harbor

If you act in good faith, stay within this scope, avoid privacy violations and service disruption, and give us a
reasonable time to fix the issue before disclosure, we will not pursue or support any legal action against you.
Test only on your own spaces, devices and relays.
