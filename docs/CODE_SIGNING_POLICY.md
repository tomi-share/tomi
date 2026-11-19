# Code signing policy

This page describes how Tomi's Windows installer is code-signed, who is allowed to sign, and what the signed program does with
your data. It follows the conditions of the [SignPath Foundation](https://signpath.org/) for open source projects.

**Status:** the project is applying for free code signing from the SignPath Foundation and has not been accepted yet. Until it
is, the Windows installer published on the [releases page](https://github.com/tomi-share/tomi/releases?q=windows-v) is **not
code-signed** and Windows SmartScreen may warn. Every release states which of the two situations applies, and you can always
verify what you download as described in [`docs/RELEASES.md`](RELEASES.md#verifying-a-download).

<!--
Maintainers: when the SignPath Foundation accepts the project, add this exact line here, on the repository home page (README.md)
and in the notes of every Windows release (the release workflow does the latter once signing is configured), then change the
status above:

    Free code signing provided by SignPath.io, certificate by SignPath Foundation
-->

## What is signed

- Only the **Windows installer** of Tomi (`tomi-windows-X.Y.Z-setup.exe`, NSIS), and only the files built by the project's own
  GitHub Actions workflows from the source code of this repository.
- The Android app is **not** covered by this policy: it is signed with the project's own release key, which is documented in
  [`docs/RELEASES.md`](RELEASES.md#android-release-signing). The relay and the command-line client are not signed.
- Everything in the installer is built from this repository. The build scripts and the CI configuration are part of the source.
  Third-party libraries are used as published by their authors (Rust crates and npm packages, pinned by lock files and audited
  in CI: see [`security.yml`](../.github/workflows/security.yml)).

## How a release is signed

1. A maintainer pushes a release tag (`windows-vX.Y.Z`) on `main`.
2. The [`release-apps.yml`](../.github/workflows/release-apps.yml) workflow checks that the version files match the tag, builds
   the installer on a **GitHub-hosted** runner and uploads the unsigned file as a workflow artifact.
3. The workflow submits that artifact to SignPath. SignPath verifies where it comes from (this repository, the commit and the
   workflow run) and holds the request until an **approver** approves it by hand.
4. The signed installer is downloaded by the workflow, given a SHA-256 checksum and a build provenance attestation, and published.

Nothing is signed from a developer's computer, and a signing request is never approved automatically.

## Roles

| Role | Who | Responsibility |
|---|---|---|
| Authors (committers) | [@shaffe-fr](https://github.com/shaffe-fr) | Write and merge the code that is released |
| Reviewers | [@shaffe-fr](https://github.com/shaffe-fr), through [`CODEOWNERS`](../.github/CODEOWNERS) | Review every change that does not come from a committer before it is merged |
| Approvers | [@shaffe-fr](https://github.com/shaffe-fr) | Approve each signing request after checking the version, the commit and the workflow run |

The project is maintained by one person today, so one account holds the three roles. Everyone who can sign or approve must use
multi-factor authentication on GitHub and on SignPath. The organization requires it for all its members.

## Privacy

Tomi is end-to-end encrypted: what you share is encrypted on your devices before it leaves them, and the relay only routes
opaque data (see [`docs/threat-model.md`](threat-model.md) for what the project does and does not protect against). In addition:

- **This program will not transfer any information to other networked systems unless specifically requested by the user or the
  person installing or operating it.** The only network destination Tomi contacts is the relay that you set up, enter, or scan
  from a pairing link, and only once you create or join a space.
- Tomi has no telemetry, no analytics and no crash reporting.
- The installer does not change your system configuration beyond what a per-user installation does. It installs for the current
  user, does not need administrator rights, and adds: the program files under your user profile, a Start menu shortcut, an
  entry in *Settings > Apps*, and a *Send to > Tomi* shortcut (`%APPDATA%\Microsoft\Windows\SendTo\Tomi.lnk`) so that you can
  send a file to Tomi from Explorer. The uninstaller removes all of them.
- **Third-party component:** the installer relies on the Microsoft Edge WebView2 runtime. If it is missing from your PC, the
  installer downloads Microsoft's WebView2 bootstrapper from Microsoft. That download is governed by
  [Microsoft's privacy statement](https://privacy.microsoft.com/privacystatement).

## Uninstalling

Tomi uninstalls from *Settings > Apps > Installed apps*, like any Windows application. Uninstalling removes the program; your
encrypted local data stays in `%LOCALAPPDATA%\eu.shaffe.tomi\` until you delete that folder (see
[`apps/windows/README.md`](../apps/windows/README.md)).

## Reporting a problem

If you believe a signed Tomi binary is malicious, or that the signing certificate was misused, report it privately as described
in [`SECURITY.md`](../SECURITY.md). The maintainer will help the SignPath Foundation to verify, investigate and find the root
cause of any report concerning code signed for this project.
