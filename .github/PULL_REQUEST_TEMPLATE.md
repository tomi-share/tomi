## What and why

<!-- What does this change, and why? Link the issue if there is one. Keep behavior changes and refactors separate. -->

## Platforms affected

- [ ] Core / spec
- [ ] Relay
- [ ] Android
- [ ] Windows
- [ ] iOS
- [ ] CLI / docs only

## How it was tested

<!-- Commands run and their result. Say explicitly what you could not verify (device, simulator, network, signing). -->

## Checklist

- [ ] [`docs/FEATURES.md`](../docs/FEATURES.md) updated in this PR if the behavior changed (and any platform difference listed)
- [ ] Rust: `cargo check`, `cargo test`, `cargo fmt --check`, `cargo clippy -D warnings` pass (if the core or relay changed)
- [ ] Frozen vectors unchanged (`spec/vectors/**` regenerated with no diff, never edited by hand)
- [ ] Platform checks from [`CONTRIBUTING.md`](../CONTRIBUTING.md) run for the apps touched
- [ ] No secret, personal data or real hostname added (placeholders: `relay.example.org`, `user@example.org`)
- [ ] Golden rules in [`AGENTS.md`](../AGENTS.md) respected (blind relay, secrets, additive format changes)
