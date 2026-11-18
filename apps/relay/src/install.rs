//! Installation tokens and invitations (`api-v1.md` §3.1): 160 random bits in lowercase
//! Crockford base32, one per space to create.
//!
//! An **invitation** is a permission to create a space. The relay keeps only the SHA-256 of the
//! token (in `routing.db`, with the date): the token is shown once, by `admin invite --new` (or
//! in the log of the very first start), and cannot be shown again. `POST /v1/space` consumes the
//! invitation its token designates, and the row **stays**, marked consumed, with the `space_id`
//! it created: a founder whose `201` was lost retries with the same token and must be told
//! `space_exists` (not "wrong token"), so that it can prove with its keys that the space is its
//! own (`pairing-v1.md` §10.2). `admin reset` of that space deletes the row; `invite --rotate`
//! deletes the pending invitations only.
//!
//! The older layout kept the single token in clear in `<data_dir>/install-token`; such a file
//! is imported as an invitation at start (see [`take_legacy_file`]).

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::params;
use subtle::ConstantTimeEq;

use crate::db::{Db, Tx};
use crate::error::ApiError;
use crate::wire::{random_bytes, sha256};

pub(crate) const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// 20 random bytes as 32 base32 characters in 8 groups of 4: `h7k2-m9qv-d4xr-...`.
pub fn generate() -> io::Result<String> {
    let bytes = random_bytes::<20>().map_err(|e| io::Error::other(e.to_string()))?;
    let mut chars = Vec::with_capacity(32);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(ALPHABET[((acc >> bits) & 31) as usize]);
        }
    }
    let groups: Vec<String> = chars
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    Ok(groups.join("-"))
}

/// Removes the dashes and spaces, lowercases (the comparison form of §3.1).
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Constant-time comparison of two tokens after normalisation.
pub fn matches(stored: &str, given: &str) -> bool {
    let a = normalize(stored);
    let b = normalize(given);
    // The length of the expected token is not a secret.
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// What the relay keeps of a token: the SHA-256 of its normalised form.
pub fn hash(token: &str) -> [u8; 32] {
    sha256(normalize(token).as_bytes())
}

/// What the relay knows of a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenState {
    /// No invitation has this hash: a wrong token, or one forgotten by `--rotate` or `reset`.
    Unknown,
    /// An invitation that no space has used yet.
    Pending,
    /// An invitation that created a space.
    Consumed,
}

/// A pending invitation, as listed by `admin invite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    pub token_hash: [u8; 32],
    pub created_at: i64,
}

impl Invitation {
    /// The first 8 hexadecimal characters of the hash: enough to tell invitations apart in a
    /// listing, useless to guess a token.
    pub fn short_id(&self) -> String {
        hex::encode(&self.token_hash[..4])
    }
}

/// Creates an invitation and returns its token (the only time it exists in clear).
pub fn create(routing: &Db, now_ms: i64) -> Result<String, ApiError> {
    let token = generate().map_err(ApiError::internal)?;
    insert(routing, &hash(&token), now_ms)?;
    Ok(token)
}

/// Records an invitation by hash (`INSERT OR IGNORE`).
pub fn insert(routing: &Db, token_hash: &[u8; 32], created_at: i64) -> Result<(), ApiError> {
    let h = token_hash.to_vec();
    routing.write_sync(move |tx| {
        tx.execute(
            "INSERT OR IGNORE INTO invitations (token_hash, created_at) VALUES (?1, ?2)",
            params![h, created_at],
        )?;
        Ok(Tx::Commit(()))
    })
}

/// What `routing.db` says about the token.
pub fn state(routing: &Db, token: &str) -> Result<TokenState, ApiError> {
    let h = hash(token).to_vec();
    routing.read_sync(move |tx| {
        let row: Option<Option<i64>> = tx
            .query_row(
                "SELECT consumed_at FROM invitations WHERE token_hash = ?1",
                params![h],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(match row {
            None => TokenState::Unknown,
            Some(None) => TokenState::Pending,
            Some(Some(_)) => TokenState::Consumed,
        })
    })
}

/// Whether the token designates a pending invitation. Does not consume it.
pub fn is_pending(routing: &Db, token: &str) -> Result<bool, ApiError> {
    Ok(state(routing, token)? == TokenState::Pending)
}

/// Consumes the pending invitation of `token` for the space `space_id` (text form): its
/// creation date if there was one, `None` if the token is unknown or was already consumed. The
/// row is kept (see the module documentation).
pub fn consume(
    routing: &Db,
    token: &str,
    now_ms: i64,
    space_id: &str,
) -> Result<Option<i64>, ApiError> {
    let (h, space) = (hash(token).to_vec(), space_id.to_owned());
    routing.write_sync(move |tx| {
        let at: Option<i64> = tx
            .query_row(
                "SELECT created_at FROM invitations WHERE token_hash = ?1 AND consumed_at IS NULL",
                params![h],
                |r| r.get(0),
            )
            .ok();
        if at.is_some() {
            tx.execute(
                "UPDATE invitations SET consumed_at = ?2, space_id = ?3 WHERE token_hash = ?1",
                params![h, now_ms, space],
            )?;
        }
        Ok(Tx::Commit(at))
    })
}

/// Gives a consumed invitation back (the creation of its space failed).
pub fn restore(routing: &Db, token: &str) -> Result<(), ApiError> {
    let h = hash(token).to_vec();
    routing.write_sync(move |tx| {
        tx.execute(
            "UPDATE invitations SET consumed_at = NULL, space_id = NULL WHERE token_hash = ?1",
            params![h],
        )?;
        Ok(Tx::Commit(()))
    })
}

/// Deletes the consumed invitations of a space (`admin reset`): their tokens are then wrong
/// tokens again. Runs in the transaction `tx` of the caller.
pub fn forget_space(tx: &rusqlite::Transaction, space_id: &str) -> Result<usize, ApiError> {
    Ok(tx.execute(
        "DELETE FROM invitations WHERE space_id = ?1 AND consumed_at IS NOT NULL",
        params![space_id],
    )?)
}

/// The pending invitations, oldest first.
pub fn list(routing: &Db) -> Result<Vec<Invitation>, ApiError> {
    routing.read_sync(|tx| {
        let mut st = tx.prepare(
            "SELECT token_hash, created_at FROM invitations WHERE consumed_at IS NULL
             ORDER BY created_at, rowid",
        )?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(h, created_at)| {
                <[u8; 32]>::try_from(h.as_slice())
                    .ok()
                    .map(|token_hash| Invitation {
                        token_hash,
                        created_at,
                    })
            })
            .collect())
    })
}

/// Deletes every **pending** invitation (`admin invite --rotate`). Returns how many there were.
/// The consumed ones stay: they are what lets a founder recover after a lost `201`.
pub fn clear(routing: &Db) -> Result<usize, ApiError> {
    routing.write_sync(|tx| {
        Ok(Tx::Commit(tx.execute(
            "DELETE FROM invitations WHERE consumed_at IS NULL",
            [],
        )?))
    })
}

/// `<data_dir>/install-token` of the layout that preceded the multi-space relay.
pub fn legacy_path(data_dir: &Path) -> PathBuf {
    data_dir.join("install-token")
}

/// Reads and deletes the legacy token file, if there is one: the caller turns the token into an
/// invitation (or drops it if a space exists).
pub fn take_legacy_file(data_dir: &Path) -> Option<String> {
    let path = legacy_path(data_dir);
    let token = std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|t| !t.is_empty());
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("cannot remove the legacy install token file: {}", e.kind()),
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_has_the_documented_shape() {
        let t = generate().unwrap();
        assert_eq!(t.len(), 8 * 4 + 7);
        let groups: Vec<&str> = t.split('-').collect();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().all(|g| g.len() == 4));
        assert!(t
            .bytes()
            .filter(|b| *b != b'-')
            .all(|b| ALPHABET.contains(&b)));
        assert_ne!(generate().unwrap(), t);
    }

    #[test]
    fn comparison_ignores_dashes_spaces_and_case() {
        assert!(matches("h7k2-m9qv-d4xr-8tbn", "H7K2 M9QV d4xr8tbn"));
        assert!(!matches("h7k2-m9qv-d4xr-8tbn", "h7k2-m9qv-d4xr-8tbm"));
        assert!(!matches("h7k2", "h7k2-m9qv"));
        assert_eq!(hash("h7k2-m9qv-d4xr-8tbn"), hash("H7K2 M9QV d4xr8tbn"));
        assert_ne!(hash("h7k2-m9qv-d4xr-8tbn"), hash("h7k2-m9qv-d4xr-8tbm"));
    }

    #[test]
    fn invitation_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let routing = Db::open_routing(&dir.path().join("routing.db")).unwrap();
        assert!(list(&routing).unwrap().is_empty());
        let a = create(&routing, 10).unwrap();
        let b = create(&routing, 20).unwrap();
        assert_ne!(a, b);
        let pending = list(&routing).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].created_at, 10);
        assert_eq!(pending[0].short_id().len(), 8);
        assert!(is_pending(&routing, &a).unwrap());
        assert!(!is_pending(&routing, "0000-0000").unwrap());
        assert_eq!(state(&routing, &a).unwrap(), TokenState::Pending);
        assert_eq!(state(&routing, "0000-0000").unwrap(), TokenState::Unknown);
        // Consumed once, in any spelling of the token; the row stays.
        assert_eq!(
            consume(&routing, &a.to_uppercase(), 99, "space-a").unwrap(),
            Some(10)
        );
        assert_eq!(consume(&routing, &a, 100, "space-b").unwrap(), None);
        assert_eq!(state(&routing, &a).unwrap(), TokenState::Consumed);
        assert!(!is_pending(&routing, &a).unwrap());
        assert_eq!(list(&routing).unwrap().len(), 1, "only b is pending");
        // `--rotate` drops the pending ones and keeps the consumed ones.
        assert_eq!(clear(&routing).unwrap(), 1);
        assert!(list(&routing).unwrap().is_empty());
        assert_eq!(state(&routing, &a).unwrap(), TokenState::Consumed);
        assert_eq!(state(&routing, &b).unwrap(), TokenState::Unknown);
        // A consumed invitation can be given back; forgetting a space drops its row.
        restore(&routing, &a).unwrap();
        assert_eq!(state(&routing, &a).unwrap(), TokenState::Pending);
        consume(&routing, &a, 101, "space-a").unwrap();
        let n = routing
            .write_sync(|tx| Ok(Tx::Commit(forget_space(tx, "space-x")?)))
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(state(&routing, &a).unwrap(), TokenState::Consumed);
        let n = routing
            .write_sync(|tx| Ok(Tx::Commit(forget_space(tx, "space-a")?)))
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(state(&routing, &a).unwrap(), TokenState::Unknown);
    }

    #[test]
    fn the_legacy_file_is_read_once_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(take_legacy_file(dir.path()), None);
        std::fs::write(legacy_path(dir.path()), "h7k2-m9qv\n").unwrap();
        assert_eq!(take_legacy_file(dir.path()).as_deref(), Some("h7k2-m9qv"));
        assert!(!legacy_path(dir.path()).exists());
        assert_eq!(take_legacy_file(dir.path()), None);
    }
}
