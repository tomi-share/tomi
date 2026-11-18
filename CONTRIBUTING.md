# Contributing to Tomi

Thanks for your interest in Tomi. This project is licensed under the [Apache License 2.0](LICENSE); by submitting a
contribution you agree that it is released under the same license.

## Before you start

- Read [AGENTS.md](AGENTS.md) (architecture, golden rules, build commands) and the relevant part of `spec/`.
- For anything that touches the protocol, crypto, wire format or security properties, open an issue first to discuss the
  design. The golden rules in `AGENTS.md` (blind relay, secrets never in clear at rest, additive format changes) are not
  negotiable without a protocol-level discussion.
- Security vulnerabilities must not be reported in public issues; contact the maintainers privately (see the repository
  security policy once published).

## Workflow

1. Fork the repository and create a branch from `main`.
2. Make small, coherent commits with a clear message (imperative mood, what and why).
3. Keep behavior changes and refactors separate.
4. Update [docs/FEATURES.md](docs/FEATURES.md) (feature matrix per platform) in the same commit as any
   behavior change, so that Android, iOS and Windows stay aligned; list unavoidable differences there.
5. Open a pull request describing the change, how you tested it, and which platforms are affected.

## Checks to run

- Rust: `cargo check --workspace`, `cargo test --workspace`, `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`.
- Frozen vectors must remain "unchanged" when regenerated:
  `cargo run -p tomi-core --example gen_spec_vectors -- spec/vectors/v1` and
  `cargo run -p tomi-core --example gen_pairing_vectors -- spec/vectors/pairing-v1`. Never edit `spec/vectors/**` by hand.
- Android: `cd apps/android && ./gradlew assembleDebug testDebugUnitTest lint`.
- Windows: `cargo check` / `cargo test` in `apps/windows/src-tauri`, `npm run check` in `apps/windows`.
- iOS: generate the project with XcodeGen (`apps/apple/project.yml`) and run the tests in Xcode or with the `just` recipes (see `apps/apple/README.md`); describe what you could and could not verify.

## Conventions

- Code, comments, documentation and commit messages are in English. User-facing strings live in the localization files
  (French is the default locale; English can be added as a separate locale).
- Do not add personal data, real hostnames or credentials to code, tests, docs or fixtures; use `relay.example.org`,
  `user@example.org` and similar placeholders.
- Do not commit generated artifacts or secrets.

## Code of conduct

Be respectful and constructive. Harassment and personal attacks are not tolerated.
