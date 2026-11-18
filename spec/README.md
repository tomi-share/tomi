# Tomi format specification

This directory is the source of truth for:
- the item envelope format (`format`, see `PROJECT.md` §6),
- the test vectors frozen per version (`vectors/v1/`, `vectors/v2/`...).

Rule: a published `vectors/vN/` directory is **never modified**. A breaking change
creates `vN+1` with its own vectors; the previous one remains readable until its support expires.

Contents (see `PROJECT.md` §8):
- [x] [`format-v1.md`](format-v1.md): primitives, key hierarchy and HKDF, envelope
      (CEK encrypted by the space key), canonical AAD, STREAM attachments, device
      identities, signed rotation, tombstones, versioning, non-goals.
- [x] [`vectors/v1/`](vectors/v1/README.md): frozen vectors, verified by
      `cargo test -p tomi-core --test spec_vectors`.
- [x] [`pairing-v1.md`](pairing-v1.md): pairing channel (CPace on ristretto255, short code
      and QR, verification code, frames, sequence over the relay channel, pairing batch,
      space creation). Planned vectors in `vectors/pairing-v1/`.
      Specified, awaiting validation of the decisions in its Annex A.
- [x] [`pin-contract.md`](pin-contract.md): pin contract for the apps
      (Kotlin and Rust signatures, semantics), based on `format-v1.md` §8.8.
- [x] [`archive-contract.md`](archive-contract.md): archive contract for the apps (twin of
      the pin contract: Kotlin and Rust signatures, semantics, exclusion with pins), based on
      `format-v1.md` §8.9; vectors `vectors/v1/archive.json`.
- [x] Multi-recipient reservations, no effect in v1: `format-v1.md` §5.8 (`audience`
      field, `key_epoch` range), vectors `vectors/v1/reservations.json`; analysis in
      [`../docs/protocol-extensibility.md`](../docs/protocol-extensibility.md).
