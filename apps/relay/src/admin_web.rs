//! The web administration panel: read-only monitoring and invitations, behind a passkey.
//!
//! **Off by default** (`admin_web = false`): every path of the panel then answers exactly like any
//! unknown resource, because nothing is routed for it; the router's fallback ([`serve`]) is the
//! only entry point and it answers the unknown-resource `404` unless the panel is on *and* the
//! first path segment is the secret one.
//!
//! * **The path** is `/<segment>/`, 128 random bits in base64url, created at the first start with
//!   the panel on and kept in `routing.db`. It is defence in depth against scanners, **not** the
//!   authentication: the passkey is. It never reaches a log (the access log labels these
//!   requests with the constant [`ROUTE_LABEL`]).
//! * **The passkey** (WebAuthn, ES256, attestation `none`, user verification required; see
//!   `webauthn.rs`) is the only way in. There is one administrator and one key. The first key is
//!   enrolled with a one-time **enrolment code** made on the server (`tomi-relay admin web
//!   enroll`): 15 minutes, 5 attempts, only its SHA-256 is stored. Once a key is registered the
//!   enrolment endpoints answer `404` for good; replacing the key is `admin web reset`, local only.
//! * **The session**: 256 random bits in a `__Host-` cookie (`Secure`, `HttpOnly`,
//!   `SameSite=Strict`, `Path=/`); only its SHA-256 is kept, in memory. 30 minutes of inactivity,
//!   8 hours at most. Every `POST` must carry the `Origin` of `public_url` and the header
//!   `X-Tomi-Admin: 1`.
//! * **What it shows**: the same data, produced by the same functions, as `tomi-relay admin
//!   status`, `spaces`, `devices` and `invite`. Metadata only: the relay is blind and the panel
//!   never shows or carries an item, a file name, a key or a secret. No revoke, no reset, and no
//!   quota change *of one space* from the web (that stays `tomi-relay admin quota`). The one
//!   setting it writes besides invitations is the **default quota of a space** (`api/quota`): a
//!   number of bytes, kept in `routing.db`, applied to every space that has no quota of its own.
//! * **Added for the operator** (read-only, additive): `api/about` (version, start and uptime,
//!   server clock, end of the session, the limits of the configuration) and `api/journal`
//!   (what was done through the panel since the start: fixed words, dates and counts, in memory
//!   only, never a value from a request, an address or anything about a space).
//!
//! The panel is served from constants embedded in the binary (one page, one script, one style
//! sheet), same origin, with a strict `Content-Security-Policy`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::admin::{self, Output, WebAction};
use crate::config::{Config, Rate};
use crate::db::{lock, Db, Tx};
use crate::error::ApiError;
use crate::install;
use crate::middleware::RouteLabel;
use crate::state::AppState;
use crate::webauthn::{self, Expected, Registered};
use crate::wire::{b64_decode, b64_encode, random_bytes, sha256, sha256_parts};

/// What the access log says about every request to the panel (never the secret path).
pub const ROUTE_LABEL: &str = "admin-web";
/// The session cookie. The `__Host-` prefix makes a browser refuse it unless it is `Secure`, has
/// `Path=/` and no `Domain`.
pub const COOKIE_NAME: &str = "__Host-tomi-admin";
/// The header every `POST` must carry (a cross-site form cannot set it).
pub const CSRF_HEADER: &str = "x-tomi-admin";

/// Life of a challenge.
pub const CHALLENGE_TTL_MS: i64 = 5 * 60_000;
/// A session ends after this long without a request.
pub const IDLE_TTL_MS: i64 = 30 * 60_000;
/// A session ends this long after the sign-in, whatever the activity.
pub const ABSOLUTE_TTL_MS: i64 = 8 * 3_600_000;
/// Life of an enrolment code.
pub const ENROLL_TTL_MS: i64 = 15 * 60_000;
/// Wrong enrolment codes tolerated before the code is dropped.
pub const ENROLL_ATTEMPTS: i64 = 5;

/// How many challenges and sessions are kept at once (the oldest make room).
const MAX_PENDING: usize = 16;
const MAX_SESSIONS: usize = 16;
/// Largest body a request of the panel may have.
const MAX_BODY: usize = 16 * 1024;
/// How many lines the administration journal keeps (the oldest make room).
pub const JOURNAL_LEN: usize = 100;

/// One bucket per caller address for every sensitive request of the panel (a code, a challenge, a
/// sign-in): 10 in a burst, then 6 a minute. A person uses a handful of them a day. Same
/// mechanism as the rate limits of `gate.rs` (`ratelimit.rs`). The address is the one
/// `client_ip` derives (trusted proxy hops skipped), so someone who knows the secret path cannot
/// lock the administrator out from another address.
const SENSITIVE: Rate = Rate {
    per_minute: 6.0,
    burst: 10,
};

/// Safety net over all callers together, much wider than one person needs: it only stops a flood
/// spread over many addresses.
const ALL_CALLERS: Rate = Rate {
    per_minute: 120.0,
    burst: 200,
};

const INDEX_HTML: &str = include_str!("admin_web/index.html");
const APP_JS: &str = include_str!("admin_web/app.js");
const APP_CSS: &str = include_str!("admin_web/app.css");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

// ===== origin of public_url ===================================================================

/// The origin (`https://host[:port]`, as a browser writes it) and the WebAuthn relying party id
/// (the host) of `public_url`. `None` unless it is an https URL on a domain name: a passkey cannot
/// be bound to an IP address, and the cookie is `Secure`.
pub fn origin_and_rp_id(public_url: &str) -> Option<(String, String)> {
    let rest = public_url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') || authority.starts_with('[') {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = port {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || p.parse::<u16>().is_err() {
            return None;
        }
    }
    let host = host.to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    if !host.split('.').all(label_ok) || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let origin = match port {
        Some(p) if p != "443" => format!("https://{host}:{p}"),
        _ => format!("https://{host}"),
    };
    Some((origin, host))
}

// ===== what is kept in routing.db =============================================================

/// The registered passkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredential {
    pub credential_id: Vec<u8>,
    /// Uncompressed SEC1 point, 65 bytes.
    pub public_key: Vec<u8>,
    pub sign_count: u32,
    pub created_at: i64,
}

/// Whether the panel tables exist (an older `routing.db` opened read-only may lack them).
fn tables_exist(tx: &Transaction) -> Result<bool, ApiError> {
    let n: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'admin_web_path'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn stored_segment(tx: &Transaction) -> Result<Option<String>, ApiError> {
    if !tables_exist(tx)? {
        return Ok(None);
    }
    Ok(tx
        .query_row("SELECT segment FROM admin_web_path WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()?)
}

fn stored_credential(tx: &Transaction) -> Result<Option<StoredCredential>, ApiError> {
    if !tables_exist(tx)? {
        return Ok(None);
    }
    let row = tx
        .query_row(
            "SELECT credential_id, public_key, sign_count, created_at
             FROM admin_web_credential WHERE id = 1",
            [],
            |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    Ok(row.map(
        |(credential_id, public_key, count, created_at)| StoredCredential {
            credential_id,
            public_key,
            sign_count: u32::try_from(count).unwrap_or(u32::MAX),
            created_at,
        },
    ))
}

/// Whether an enrolment code is pending (not expired, attempts left).
fn enrollment_pending(tx: &Transaction, now: i64) -> Result<bool, ApiError> {
    if !tables_exist(tx)? {
        return Ok(false);
    }
    let n: i64 = tx.query_row(
        "SELECT count(*) FROM admin_web_enrollment WHERE expires_at > ?1 AND attempts_left > 0",
        params![now],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn new_segment() -> Result<String, ApiError> {
    Ok(b64_encode(
        &random_bytes::<16>().map_err(ApiError::internal)?,
    ))
}

/// The secret path segment, created if there is none yet.
fn ensure_segment(routing: &Db, now: i64) -> Result<String, ApiError> {
    routing.write_sync(|tx| {
        if let Some(s) = stored_segment(tx)? {
            return Ok(Tx::Commit(s));
        }
        let s = new_segment()?;
        tx.execute(
            "INSERT INTO admin_web_path (id, segment, created_at) VALUES (1, ?1, ?2)",
            params![s, now],
        )?;
        Ok(Tx::Commit(s))
    })
}

/// What the relay keeps of an enrolment code: a SHA-256 over its normalised form (case, dashes
/// and spaces do not matter, like the installation tokens).
fn code_hash(code: &str) -> [u8; 32] {
    sha256_parts(&[
        b"tomi-admin-web-enroll-v1\0",
        install::normalize(code).as_bytes(),
    ])
}

/// A new enrolment code: 60 random bits as 12 Crockford base32 characters, `xxxx-xxxx-xxxx`. Short
/// enough to type, and out of reach of the 5 attempts it is allowed.
fn new_code() -> Result<String, ApiError> {
    let bits = u64::from_be_bytes(random_bytes::<8>().map_err(ApiError::internal)?);
    let chars: Vec<char> = (0..12)
        .map(|i| char::from(install::ALPHABET[((bits >> (55 - 5 * i)) & 31) as usize]))
        .collect();
    Ok(chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-"))
}

/// Records the hash of a new enrolment code, replacing any earlier one.
fn store_enrollment(tx: &Transaction, hash: &[u8; 32], now: i64) -> Result<(), ApiError> {
    tx.execute(
        "INSERT OR REPLACE INTO admin_web_enrollment (id, code_hash, expires_at, attempts_left)
         VALUES (1, ?1, ?2, ?3)",
        params![hash.to_vec(), now + ENROLL_TTL_MS, ENROLL_ATTEMPTS],
    )?;
    Ok(())
}

/// Judges a code given by a client. `true` if it is the pending, unexpired code; the code is not
/// consumed (registering the key does that). A wrong code costs one attempt, and the fifth wrong
/// one deletes the code; an expired code is deleted. The comparison is in constant time, and the
/// attempt is counted before anything is said.
fn try_enrollment_code(tx: &Transaction, given: &[u8; 32], now: i64) -> Result<bool, ApiError> {
    if !tables_exist(tx)? {
        return Ok(false);
    }
    let row: Option<(Vec<u8>, i64, i64)> = tx
        .query_row(
            "SELECT code_hash, expires_at, attempts_left FROM admin_web_enrollment WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((stored, expires_at, left)) = row else {
        return Ok(false);
    };
    if expires_at <= now || left <= 0 {
        tx.execute("DELETE FROM admin_web_enrollment WHERE id = 1", [])?;
        return Ok(false);
    }
    let right =
        stored.len() == given.len() && bool::from(stored.as_slice().ct_eq(given.as_slice()));
    if right {
        return Ok(true);
    }
    if left <= 1 {
        tx.execute("DELETE FROM admin_web_enrollment WHERE id = 1", [])?;
    } else {
        tx.execute(
            "UPDATE admin_web_enrollment SET attempts_left = attempts_left - 1 WHERE id = 1",
            [],
        )?;
    }
    Ok(false)
}

/// Stores the first passkey and closes the enrolment, in one transaction. `false` if a key is
/// already registered or the enrolment code is gone or expired.
fn register_credential(tx: &Transaction, now: i64, key: &Registered) -> Result<bool, ApiError> {
    if stored_credential(tx)?.is_some() || !enrollment_pending(tx, now)? {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO admin_web_credential (id, credential_id, public_key, sign_count, created_at)
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            key.credential_id,
            key.public_key,
            i64::from(key.sign_count),
            now
        ],
    )?;
    tx.execute("DELETE FROM admin_web_enrollment WHERE id = 1", [])?;
    Ok(true)
}

/// Moves the signature counter forward, only from the value that was read (two concurrent replays
/// of one assertion cannot both pass).
fn advance_counter(tx: &Transaction, id: &[u8], from: u32, to: u32) -> Result<bool, ApiError> {
    let n = tx.execute(
        "UPDATE admin_web_credential SET sign_count = ?1
         WHERE id = 1 AND credential_id = ?2 AND sign_count = ?3",
        params![i64::from(to), id, i64::from(from)],
    )?;
    Ok(n == 1)
}

// ===== what is kept in memory =================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ceremony {
    Register,
    Login,
}

struct Pending {
    challenge: [u8; 32],
    ceremony: Ceremony,
    expires_ms: i64,
}

struct Session {
    hash: [u8; 32],
    created_ms: i64,
    last_ms: i64,
}

/// What the administration journal records: a fixed word, never a value that came from a request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JournalKind {
    /// The administrator signed in with the passkey.
    SignedIn,
    /// A sign-in was refused (whatever the cause: the cause is not kept).
    SignInRefused,
    /// An enrolment code was refused.
    EnrollmentRefused,
    /// The administrator signed out.
    SignedOut,
    /// The passkey was registered (the enrolment is closed).
    PasskeyRegistered,
    /// An invitation was created (`new`).
    InvitationCreated,
    /// The pending invitations were replaced by one (`rotate`).
    InvitationsReplaced,
    /// The default quota of a space was set to a number of bytes.
    DefaultQuotaSet,
    /// The default quota of a space became "no limit per space".
    DefaultQuotaUnlimited,
    /// The default quota of a space went back to the configuration.
    DefaultQuotaConfig,
    /// `tomi-relay admin web --rotate-path`: every session was closed.
    PathRotated,
    /// `tomi-relay admin web reset`: the passkey was removed, every session closed.
    PasskeyReset,
}

impl JournalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SignedIn => "signed_in",
            Self::SignInRefused => "sign_in_refused",
            Self::EnrollmentRefused => "enrollment_refused",
            Self::SignedOut => "signed_out",
            Self::PasskeyRegistered => "passkey_registered",
            Self::InvitationCreated => "invitation_created",
            Self::InvitationsReplaced => "invitations_replaced",
            Self::DefaultQuotaSet => "default_quota_set",
            Self::DefaultQuotaUnlimited => "default_quota_unlimited",
            Self::DefaultQuotaConfig => "default_quota_config",
            Self::PathRotated => "path_rotated",
            Self::PasskeyReset => "passkey_reset",
        }
    }
}

/// One line of the administration journal: when, what (a fixed word), how many times in a row
/// (repeated identical lines are folded into one, so a flood of refusals cannot push the rest
/// out), and for a quota change its number of bytes. No address, no token, no code, nothing about
/// a space.
#[derive(Clone, Debug)]
struct JournalEntry {
    first_ms: i64,
    last_ms: i64,
    kind: JournalKind,
    count: u64,
    bytes: Option<u64>,
}

/// The state of the panel in this process: the secret segment, the pending challenges and the
/// sessions. All of it is lost at a restart, which only means signing in again.
pub struct AdminWeb {
    origin: String,
    rp_id: String,
    segment: RwLock<String>,
    challenges: Mutex<Vec<Pending>>,
    sessions: Mutex<Vec<Session>>,
    /// Bumped by every `clear_ephemeral`: a sign-in that began before a reset or a path change
    /// cannot open a session after it.
    epoch: AtomicU64,
    /// When this process prepared the panel (the start of the relay).
    started_ms: i64,
    /// What was done through the panel (and by `admin web` on it) since the start, in memory
    /// only: lost at a restart, like the sessions. Fixed words and dates; see [`JournalEntry`].
    journal: Mutex<VecDeque<JournalEntry>>,
}

fn session_hash(token: &str) -> Option<[u8; 32]> {
    let raw = b64_decode(token)?;
    (raw.len() == 32).then(|| sha256(&raw))
}

impl AdminWeb {
    /// Prepares the panel for a relay that may write: creates the secret segment on the first
    /// start with the panel on.
    pub fn open(cfg: &Config, routing: &Db, now: i64) -> anyhow::Result<Self> {
        let (origin, rp_id) = origin_and_rp_id(&cfg.public_url)
            .ok_or_else(|| anyhow::anyhow!("admin_web needs an https public_url"))?;
        // A configuration error above exits `1`; a write that fails in `routing.db` is the data's
        // fault (`78`: the previous binary would not do better).
        let segment = ensure_segment(routing, now).map_err(|e| {
            crate::spaces::data_refused(anyhow::anyhow!(
                "cannot prepare the web panel: {}",
                e.code.as_str()
            ))
        })?;
        Ok(Self {
            origin,
            rp_id,
            segment: RwLock::new(segment),
            challenges: Mutex::new(Vec::new()),
            sessions: Mutex::new(Vec::new()),
            epoch: AtomicU64::new(0),
            started_ms: now,
            journal: Mutex::new(VecDeque::new()),
        })
    }

    /// When this process prepared the panel (milliseconds since the epoch).
    pub fn started_ms(&self) -> i64 {
        self.started_ms
    }

    /// Records an administration action. The same kind twice in a row (without a number of
    /// bytes) folds into the previous line: a flood of refused sign-ins is one line with a count.
    pub fn record(&self, kind: JournalKind, bytes: Option<u64>, now: i64) {
        let mut list = lock(&self.journal);
        if let Some(last) = list.back_mut() {
            if last.kind == kind && bytes.is_none() && last.bytes.is_none() {
                last.count = last.count.saturating_add(1);
                last.last_ms = now;
                return;
            }
        }
        while list.len() >= JOURNAL_LEN {
            list.pop_front();
        }
        list.push_back(JournalEntry {
            first_ms: now,
            last_ms: now,
            kind,
            count: 1,
            bytes,
        });
    }

    /// The journal, newest first, as the panel shows it.
    fn journal_json(&self) -> Vec<Value> {
        lock(&self.journal)
            .iter()
            .rev()
            .map(|e| {
                let mut v = json!({
                    "at": crate::wire::ts_format(e.last_ms),
                    "first_at": crate::wire::ts_format(e.first_ms),
                    "kind": e.kind.as_str(),
                    "count": e.count,
                });
                if let Some(b) = e.bytes {
                    v["bytes"] = json!(b);
                }
                v
            })
            .collect()
    }

    /// The origin a `POST` must announce, and that a ceremony must have run on.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The WebAuthn relying party id.
    pub fn rp_id(&self) -> &str {
        &self.rp_id
    }

    /// The current secret path segment.
    pub fn segment(&self) -> String {
        self.segment
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_segment(&self, new: String) {
        *self
            .segment
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = new;
    }

    /// Is `candidate` the secret segment? In constant time (the length is public: 22 characters).
    fn segment_matches(&self, candidate: &str) -> bool {
        let current = self
            .segment
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        current.len() == candidate.len()
            && bool::from(current.as_bytes().ct_eq(candidate.as_bytes()))
    }

    /// Forgets every pending challenge and every session.
    pub fn clear_ephemeral(&self) {
        lock(&self.challenges).clear();
        let mut sessions = lock(&self.sessions);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        sessions.clear();
    }

    /// The number of live sessions (tests).
    pub fn session_count(&self, now: i64) -> usize {
        let mut list = lock(&self.sessions);
        list.retain(|s| Self::alive(s, now));
        list.len()
    }

    // ----- challenges -----

    /// A fresh 32-byte challenge, single use, valid [`CHALLENGE_TTL_MS`].
    fn issue_challenge(&self, ceremony: Ceremony, now: i64) -> Result<[u8; 32], ApiError> {
        let challenge = random_bytes::<32>().map_err(ApiError::internal)?;
        let mut list = lock(&self.challenges);
        list.retain(|p| p.expires_ms > now);
        while list.len() >= MAX_PENDING {
            list.remove(0);
        }
        list.push(Pending {
            challenge,
            ceremony,
            expires_ms: now + CHALLENGE_TTL_MS,
        });
        Ok(challenge)
    }

    /// Takes out the pending challenge that `claimed` designates, if there is one of this ceremony
    /// that has not expired. Every pending challenge is compared, in constant time, whether or not
    /// an earlier one matched; the one that matched is removed, so it works once.
    fn take_challenge(&self, ceremony: Ceremony, claimed: &[u8], now: i64) -> Option<[u8; 32]> {
        let mut list = lock(&self.challenges);
        list.retain(|p| p.expires_ms > now);
        let mut hit = None;
        for (i, p) in list.iter().enumerate() {
            let equal = claimed.len() == p.challenge.len()
                && bool::from(p.challenge.as_slice().ct_eq(claimed));
            if equal && p.ceremony == ceremony {
                hit = Some(i);
            }
        }
        hit.map(|i| list.swap_remove(i).challenge)
    }

    // ----- sessions -----

    fn alive(s: &Session, now: i64) -> bool {
        now - s.last_ms < IDLE_TTL_MS && now - s.created_ms < ABSOLUTE_TTL_MS
    }

    /// The current epoch: read it BEFORE looking at the stored key, then give it to `open_session`.
    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Opens a session and returns its token (the only time it exists in clear).
    fn open_session(&self, now: i64, epoch: u64) -> Result<String, ApiError> {
        let raw = random_bytes::<32>().map_err(ApiError::internal)?;
        let mut list = lock(&self.sessions);
        // A reset or a path change happened since this sign-in began: it must start again.
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Err(ApiError::unauthorized());
        }
        list.retain(|s| Self::alive(s, now));
        while list.len() >= MAX_SESSIONS {
            list.remove(0);
        }
        list.push(Session {
            hash: sha256(&raw),
            created_ms: now,
            last_ms: now,
        });
        Ok(b64_encode(&raw))
    }

    /// Does `token` designate a live session? If so the activity is recorded. Every session is
    /// compared in constant time.
    fn session_valid(&self, token: &str, now: i64) -> bool {
        self.session_deadlines(token, now).is_some()
    }

    /// Like [`Self::session_valid`] (the activity is recorded), and says when the session ends:
    /// `(idle, absolute)`, the end for inactivity counted from now and the end of the 8 hours.
    fn session_deadlines(&self, token: &str, now: i64) -> Option<(i64, i64)> {
        let hash = session_hash(token)?;
        let mut list = lock(&self.sessions);
        list.retain(|s| Self::alive(s, now));
        let mut found = None;
        for (i, s) in list.iter().enumerate() {
            if bool::from(s.hash.as_slice().ct_eq(hash.as_slice())) {
                found = Some(i);
            }
        }
        let s = &mut list[found?];
        s.last_ms = now;
        Some((now + IDLE_TTL_MS, s.created_ms + ABSOLUTE_TTL_MS))
    }

    fn close_session(&self, token: &str) {
        let Some(hash) = session_hash(token) else {
            return;
        };
        lock(&self.sessions).retain(|s| !bool::from(s.hash.as_slice().ct_eq(hash.as_slice())));
    }
}

// ===== HTTP ===================================================================================

/// The answer for anything that is not the panel: the unknown-resource `404` of the whole relay.
fn unknown() -> Response {
    ApiError::not_found().into_response()
}

/// The entry point, called by the fallback of the router for every request no route took.
///
/// The panel off, or a first segment that is not the secret one: the plain `404` of an unknown
/// resource, with no work done that depends on anything but the segment comparison.
pub async fn serve(st: &AppState, req: Request) -> Response {
    let Some(web) = st.admin_web.as_ref() else {
        return unknown();
    };
    let Some(rest) = req.uri().path().strip_prefix('/') else {
        return unknown();
    };
    let (segment, sub) = match rest.split_once('/') {
        Some((seg, sub)) => (seg, Some(sub.to_owned())),
        None => (rest, None),
    };
    if !web.segment_matches(segment) {
        return unknown();
    }
    let location = format!("/{segment}/");
    let resp = match handle(st, web, req, sub.as_deref(), &location).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    finish(resp)
}

/// The headers of every answer of the panel, and the constant label for the access log.
fn finish(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    resp.extensions_mut().insert(RouteLabel(ROUTE_LABEL));
    resp
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
    let mut r = Response::new(Body::from(body));
    r.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    r
}

fn json_response(status: StatusCode, v: &Value) -> Response {
    let mut r = Response::new(Body::from(serde_json::to_vec(v).unwrap_or_default()));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

fn no_content() -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::NO_CONTENT;
    r
}

async fn handle(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
    sub: Option<&str>,
    location: &str,
) -> Result<Response, ApiError> {
    let Some(sub) = sub else {
        // `/<segment>` without the slash: the relative links of the page need it.
        if req.method() == axum::http::Method::GET {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::PERMANENT_REDIRECT;
            if let Ok(v) = HeaderValue::from_str(location) {
                r.headers_mut().insert(header::LOCATION, v);
            }
            return Ok(r);
        }
        return Err(ApiError::not_found());
    };
    match (req.method().as_str(), sub) {
        ("GET", "") => Ok(asset(INDEX_HTML, "text/html; charset=utf-8")),
        ("GET", "app.js") => Ok(asset(APP_JS, "text/javascript; charset=utf-8")),
        ("GET", "app.css") => Ok(asset(APP_CSS, "text/css; charset=utf-8")),
        ("GET", "api/state") => state(st, web, req.headers()).await,
        ("POST", "api/register/options") => register_options(st, web, req).await,
        ("POST", "api/register/finish") => register_finish(st, web, req).await,
        ("POST", "api/login/options") => login_options(st, web, req).await,
        ("POST", "api/login/finish") => login_finish(st, web, req).await,
        ("POST", "api/logout") => logout(st, web, &req),
        ("GET", "api/status") => {
            authorize(st, web, req.headers())?;
            Ok(json_response(
                StatusCode::OK,
                &admin::relay_status(st).await?,
            ))
        }
        ("GET", "api/spaces") => {
            authorize(st, web, req.headers())?;
            spaces(st).await
        }
        ("GET", "api/devices") => {
            authorize(st, web, req.headers())?;
            devices(st, req.uri().query()).await
        }
        ("GET", "api/invitations") => {
            authorize(st, web, req.headers())?;
            Ok(json_response(
                StatusCode::OK,
                &json!({ "invitations": invitations(st).await? }),
            ))
        }
        ("POST", "api/invitations") => {
            check_origin(web, req.headers())?;
            authorize(st, web, req.headers())?;
            create_invitation(st, web, req).await
        }
        ("GET", "api/quota") => {
            authorize(st, web, req.headers())?;
            Ok(json_response(
                StatusCode::OK,
                &json!({ "default": admin::default_quota_json(st) }),
            ))
        }
        ("POST", "api/quota") => {
            check_origin(web, req.headers())?;
            authorize(st, web, req.headers())?;
            set_default_quota(st, web, req).await
        }
        ("GET", "api/about") => about(st, web, req.headers()),
        ("GET", "api/journal") => {
            authorize(st, web, req.headers())?;
            Ok(json_response(
                StatusCode::OK,
                &json!({
                    "since": crate::wire::ts_format(web.started_ms()),
                    "max_entries": JOURNAL_LEN,
                    "entries": web.journal_json(),
                }),
            ))
        }
        _ => Err(ApiError::not_found()),
    }
}

// ----- checks -------------------------------------------------------------------------------

/// A write request must come from the page itself: the `Origin` of `public_url` and the header
/// `X-Tomi-Admin: 1`. (`SameSite=Strict` on the cookie is the other half.)
fn check_origin(web: &AdminWeb, headers: &HeaderMap) -> Result<(), ApiError> {
    let origin_ok = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|o| o == web.origin);
    let header_ok = headers
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "1");
    if origin_ok && header_ok {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

/// The value of the session cookie, if the request carries it under its exact name.
fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_owned())
}

/// The request carries a live session (and its activity is recorded).
fn authorize(st: &AppState, web: &AdminWeb, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = session_cookie(headers).ok_or_else(ApiError::unauthorized)?;
    if web.session_valid(&token, st.now()) {
        Ok(())
    } else {
        Err(ApiError::unauthorized())
    }
}

fn throttle(st: &AppState, req: &Request) -> Result<(), ApiError> {
    let ip = st.client_ip(req.headers(), req.extensions());
    st.limiter
        .check("admin-web", &crate::ratelimit::ip_key(ip), SENSITIVE)?;
    st.limiter.check("admin-web-all", "*", ALL_CALLERS)
}

async fn read_json(req: Request) -> Result<Value, ApiError> {
    let bytes = axum::body::to_bytes(req.into_body(), MAX_BODY)
        .await
        .map_err(|_| ApiError::bad_request("The body is missing or too large."))?;
    serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request("The body is not JSON."))
}

/// A base64url field of a JSON body.
fn b64_field(body: &Value, name: &str) -> Result<Vec<u8>, ApiError> {
    body.get(name)
        .and_then(Value::as_str)
        .and_then(b64_decode)
        .ok_or_else(|| ApiError::bad_request("A field is missing or not base64url."))
}

async fn current_credential(st: &AppState) -> Result<Option<StoredCredential>, ApiError> {
    st.spaces.routing().read(stored_credential).await
}

// ----- pages and state ----------------------------------------------------------------------

async fn state(st: &AppState, web: &AdminWeb, headers: &HeaderMap) -> Result<Response, ApiError> {
    let enrolled = current_credential(st).await?.is_some();
    let authenticated = session_cookie(headers).is_some_and(|t| web.session_valid(&t, st.now()));
    Ok(json_response(
        StatusCode::OK,
        &json!({ "enrolled": enrolled, "authenticated": authenticated }),
    ))
}

// ----- enrolment ----------------------------------------------------------------------------

async fn register_options(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    throttle(st, &req)?;
    // Closed for good once a key is registered.
    if current_credential(st).await?.is_some() {
        return Err(ApiError::not_found());
    }
    let body = read_json(req).await?;
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .filter(|c| c.len() <= 64)
        .ok_or_else(|| ApiError::bad_request("The code is missing."))?;
    let (hash, now) = (code_hash(code), st.now());
    let right = st
        .spaces
        .routing()
        .write(move |tx| try_enrollment_code(tx, &hash, now))
        .await?;
    if !right {
        tracing::info!("admin web: enrolment refused");
        web.record(JournalKind::EnrollmentRefused, None, now);
        return Err(ApiError::unauthorized());
    }
    let challenge = web.issue_challenge(Ceremony::Register, now)?;
    let user_id = random_bytes::<16>().map_err(ApiError::internal)?;
    Ok(json_response(
        StatusCode::OK,
        &json!({ "publicKey": {
            "challenge": b64_encode(&challenge),
            "rp": { "id": web.rp_id, "name": "Tomi" },
            "user": {
                "id": b64_encode(&user_id),
                "name": "administrateur",
                "displayName": "Administrateur du relais Tomi",
            },
            "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
            "authenticatorSelection": {
                "residentKey": "preferred",
                "userVerification": "required",
            },
            "attestation": "none",
            "timeout": CHALLENGE_TTL_MS,
        }}),
    ))
}

async fn register_finish(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    throttle(st, &req)?;
    if current_credential(st).await?.is_some() {
        return Err(ApiError::not_found());
    }
    let body = read_json(req).await?;
    let client_data = b64_field(&body, "clientDataJSON")?;
    let attestation = b64_field(&body, "attestationObject")?;
    let now = st.now();
    let refuse = |why: &str| {
        tracing::info!("admin web: registration refused ({why})");
        ApiError::unauthorized()
    };
    let challenge = webauthn::claimed_challenge(&client_data)
        .and_then(|c| web.take_challenge(Ceremony::Register, &c, now))
        .ok_or_else(|| refuse("challenge"))?;
    let expected = Expected {
        origin: web.origin(),
        rp_id: web.rp_id(),
        challenge: &challenge,
    };
    let key = webauthn::verify_registration(&expected, &client_data, &attestation)
        .map_err(|why| refuse(why.as_str()))?;
    let stored = st
        .spaces
        .routing()
        .write(move |tx| register_credential(tx, now, &key))
        .await?;
    if !stored {
        return Err(refuse("enrolment closed or expired"));
    }
    tracing::warn!("admin web: a passkey was registered, the enrolment is closed");
    web.record(JournalKind::PasskeyRegistered, None, now);
    Ok(no_content())
}

// ----- sign-in ------------------------------------------------------------------------------

async fn login_options(st: &AppState, web: &AdminWeb, req: Request) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    throttle(st, &req)?;
    let credential = current_credential(st)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let challenge = web.issue_challenge(Ceremony::Login, st.now())?;
    Ok(json_response(
        StatusCode::OK,
        &json!({ "publicKey": {
            "challenge": b64_encode(&challenge),
            "rpId": web.rp_id,
            "allowCredentials": [{ "type": "public-key", "id": b64_encode(&credential.credential_id) }],
            "userVerification": "required",
            "timeout": CHALLENGE_TTL_MS,
        }}),
    ))
}

async fn login_finish(st: &AppState, web: &AdminWeb, req: Request) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    throttle(st, &req)?;
    let epoch = web.epoch();
    let credential = current_credential(st)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let body = read_json(req).await?;
    let id = b64_field(&body, "id")?;
    let client_data = b64_field(&body, "clientDataJSON")?;
    let auth_data = b64_field(&body, "authenticatorData")?;
    let signature = b64_field(&body, "signature")?;
    let now = st.now();
    let refuse = |why: &str| {
        tracing::info!("admin web: sign-in refused ({why})");
        web.record(JournalKind::SignInRefused, None, now);
        ApiError::unauthorized()
    };
    let challenge = webauthn::claimed_challenge(&client_data)
        .and_then(|c| web.take_challenge(Ceremony::Login, &c, now))
        .ok_or_else(|| refuse("challenge"))?;
    if id.len() != credential.credential_id.len()
        || !bool::from(id.as_slice().ct_eq(credential.credential_id.as_slice()))
    {
        return Err(refuse("credential"));
    }
    let expected = Expected {
        origin: web.origin(),
        rp_id: web.rp_id(),
        challenge: &challenge,
    };
    let new_count = webauthn::verify_assertion(
        &expected,
        &credential.public_key,
        credential.sign_count,
        &client_data,
        &auth_data,
        &signature,
    )
    .map_err(|why| refuse(why.as_str()))?;
    let (cred_id, from) = (credential.credential_id.clone(), credential.sign_count);
    let advanced = st
        .spaces
        .routing()
        .write(move |tx| advance_counter(tx, &cred_id, from, new_count))
        .await?;
    if !advanced {
        return Err(refuse("counter"));
    }
    let token = web.open_session(now, epoch)?;
    tracing::info!("admin web: the administrator signed in");
    web.record(JournalKind::SignedIn, None, now);
    let mut resp = no_content();
    let cookie = format!(
        "{COOKIE_NAME}={token}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
        ABSOLUTE_TTL_MS / 1000
    );
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    Ok(resp)
}

fn logout(st: &AppState, web: &AdminWeb, req: &Request) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    if let Some(token) = session_cookie(req.headers()) {
        // Only a live session that ends here makes a line of the journal.
        let now = st.now();
        if web.session_valid(&token, now) {
            web.record(JournalKind::SignedOut, None, now);
        }
        web.close_session(&token);
    }
    let mut resp = no_content();
    let gone = format!("{COOKIE_NAME}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0");
    if let Ok(v) = HeaderValue::from_str(&gone) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    Ok(resp)
}

// ----- the dashboard ------------------------------------------------------------------------

/// Runs a blocking closure on the blocking pool.
async fn blocking<T, F>(st: &AppState, f: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, ApiError> + Send + 'static,
{
    let st = st.clone();
    tokio::task::spawn_blocking(move || f(&st))
        .await
        .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
}

/// The first 8 characters of a space id: the only form of it the panel shows.
fn short(space_id: &str) -> &str {
    &space_id[..space_id.len().min(8)]
}

/// `GET api/about`: what the panel needs to say "since when", "until when" and "within which
/// limits": the version, the start of the relay and its uptime, the clock of the server (the page
/// counts its "3 days ago" from it, not from the clock of the browser), when this session ends,
/// and the limits of the configuration that shape what a space may hold and for how long. Numbers
/// and dates only: no path, no URL, no file name, no FCM project, nothing about a space.
fn about(st: &AppState, web: &AdminWeb, headers: &HeaderMap) -> Result<Response, ApiError> {
    let now = st.now();
    let (idle, absolute) = session_cookie(headers)
        .and_then(|t| web.session_deadlines(&t, now))
        .ok_or_else(ApiError::unauthorized)?;
    let c = &st.cfg;
    let fmt = crate::wire::ts_format;
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "relay_version": env!("CARGO_PKG_VERSION"),
            "server_time": fmt(now),
            "started_at": fmt(web.started_ms()),
            "uptime_secs": (now - web.started_ms()).max(0) / 1000,
            "session": {
                "idle_ends_at": fmt(idle),
                "ends_at": fmt(idle.min(absolute)),
                "absolute_ends_at": fmt(absolute),
                "idle_ttl_secs": IDLE_TTL_MS / 1000,
                "absolute_ttl_secs": ABSOLUTE_TTL_MS / 1000,
            },
            "push_configured": c.fcm_service_account_file.is_some(),
            "limits": {
                "max_relay_bytes": c.max_relay_bytes,
                "max_space_bytes": c.configured_space_quota(),
                "max_devices": c.max_devices,
                "max_envelope_bytes": c.max_envelope_bytes,
                "max_blob_bytes": c.max_blob_bytes,
                "max_item_blob_bytes": c.max_item_blob_bytes,
                "max_ttl_secs": c.max_ttl_secs,
                "event_retention_days": c.event_retention_days,
                "inactive_after_days": c.inactive_after_days,
                "purge_interval_secs": c.purge_interval_secs,
                "blob_orphan_ttl_secs": c.blob_orphan_ttl_secs,
                "session_ttl_secs": c.session_ttl_secs,
                "min_free_disk_bytes": c.min_free_disk_bytes,
            },
        }),
    ))
}

async fn spaces(st: &AppState) -> Result<Response, ApiError> {
    let rows = admin::space_rows(st).await?;
    let spaces: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": short(r["space_id"].as_str().unwrap_or("")),
                "created_at": r["created_at"],
                "members": r["members"],
                "active": r["active"],
                "used_bytes": r["used_bytes"],
                "max_bytes": r["max_bytes"],
                "quota_source": r["quota_source"],
                // Added later (additive): counts, dates and a flag, as `admin spaces --json`.
                "present": r["present"],
                "last_activity_at": r["last_activity_at"],
                "items": r["items"],
                "blobs": r["blobs"],
                "events": r["events"],
                "head_seq": r["head_seq"],
                "retention_floor_seq": r["retention_floor_seq"],
                "rotation_required": r["rotation_required"],
            })
        })
        .collect();
    Ok(json_response(StatusCode::OK, &json!({ "spaces": spaces })))
}

async fn devices(st: &AppState, query: Option<&str>) -> Result<Response, ApiError> {
    let wanted = query.and_then(|q| {
        form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "space")
            .map(|(_, v)| v.into_owned())
    });
    let Some(wanted) = wanted else {
        return Err(ApiError::bad_request("Say which space."));
    };
    let space = admin::pick_space(st, Some(&wanted)).map_err(|out| {
        if out.exit == 2 {
            ApiError::bad_request("Give the id of a space, as listed.")
        } else {
            ApiError::not_found()
        }
    })?;
    // Only what the page shows: the full rows of the CLI also carry the device id, the cursor and
    // the format range, which the panel has no use for.
    const SHOWN: [&str; 9] = [
        "name",
        "platform",
        "app_version",
        "last_seen_at",
        "registered_at",
        "active",
        "present",
        "push",
        "fingerprint",
    ];
    let rows: Vec<Value> = admin::device_rows(st, &space)
        .await?
        .into_iter()
        .map(|row| {
            let kept: serde_json::Map<String, Value> = SHOWN
                .iter()
                .filter_map(|k| row.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                .collect();
            Value::Object(kept)
        })
        .collect();
    Ok(json_response(
        StatusCode::OK,
        &json!({ "space": space.short_id(), "devices": rows }),
    ))
}

async fn invitations(st: &AppState) -> Result<Vec<Value>, ApiError> {
    blocking(st, |st| {
        Ok(install::list(st.spaces.routing())?
            .iter()
            .map(|i| json!({ "id": i.short_id(), "created_at": crate::wire::ts_format(i.created_at) }))
            .collect())
    })
    .await
}

async fn create_invitation(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    let body = read_json(req).await?;
    let rotate = match body.get("mode").and_then(Value::as_str) {
        Some("new") => false,
        Some("rotate") => true,
        _ => return Err(ApiError::bad_request("The mode is `new` or `rotate`.")),
    };
    let token = blocking(st, move |st| admin::issue_invitation(st, rotate)).await?;
    let kind = if rotate {
        JournalKind::InvitationsReplaced
    } else {
        JournalKind::InvitationCreated
    };
    web.record(kind, None, st.now());
    let list = invitations(st).await?;
    // The token (and the link that carries it) is in this answer and nowhere else, ever.
    let link = creation_link(web.origin(), &token);
    Ok(json_response(
        StatusCode::OK,
        &json!({ "token": token, "link": link, "invitations": list }),
    ))
}

/// The link that creates a space: `<origin of public_url>/create#<token>`. The token is in the
/// fragment, which a browser never sends to a server. The host comes from the configuration
/// (`public_url`, already the origin the panel enforces), never from the `Host` of the request.
pub fn creation_link(origin: &str, token: &str) -> String {
    format!("{origin}/create#{token}")
}

/// `POST api/quota`: replaces the default quota of a space (the one of every space that has no
/// quota of its own from `tomi-relay admin quota`). Same guards as every write of the panel (the
/// `Origin`, `X-Tomi-Admin`, a live session, checked by the caller). Applies at once, no restart;
/// nothing stored is deleted: a space already above the new quota can only shrink (its writes are
/// refused with `relay_full` until it is back under).
async fn set_default_quota(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    let body = read_json(req).await?;
    let value = admin::parse_default_quota(&body)?;
    blocking(st, move |st| admin::set_default_quota(st, value)).await?;
    let (kind, bytes) = match value {
        None => (JournalKind::DefaultQuotaConfig, None),
        Some(None) => (JournalKind::DefaultQuotaUnlimited, None),
        Some(Some(b)) => (JournalKind::DefaultQuotaSet, Some(b)),
    };
    web.record(kind, bytes, st.now());
    Ok(json_response(
        StatusCode::OK,
        &json!({ "default": admin::default_quota_json(st) }),
    ))
}

// ===== the `tomi-relay admin web` commands ====================================================

fn panel_url(cfg: &Config, segment: &str) -> String {
    format!("{}/{segment}/", cfg.public_url.trim().trim_end_matches('/'))
}

/// `tomi-relay admin web ...`: the URL, the enrolment code, the new path, the reset. Runs inside
/// the relay (through the administration socket) or on the stopped relay's databases.
pub async fn command(st: &AppState, action: WebAction) -> Output {
    if !st.cfg.admin_web {
        return Output::fail(
            1,
            "the web panel is off: set `admin_web = true` (or TOMI_RELAY_ADMIN_WEB=true) and restart the relay",
        );
    }
    let routing = st.spaces.routing().clone();
    let now = st.now();
    let fail = |e: ApiError| Output::fail(1, format!("error: {}", e.code.as_str()));

    // What the databases say, for every action.
    let seen = routing
        .read(move |tx| {
            Ok((
                stored_segment(tx)?,
                stored_credential(tx)?.is_some(),
                enrollment_pending(tx, now)?,
            ))
        })
        .await;
    let (segment, enrolled, pending) = match seen {
        Ok(v) => v,
        Err(e) => return fail(e),
    };

    match action {
        WebAction::Show => {
            let Some(segment) = segment else {
                return Output::fail(
                    1,
                    "no path yet: it is created when the relay starts with `admin_web = true`",
                );
            };
            let key = if enrolled {
                "passkey: registered".to_owned()
            } else if pending {
                "passkey: not registered; an enrolment code is pending".to_owned()
            } else {
                "passkey: not registered; `tomi-relay admin web enroll` makes an enrolment code"
                    .to_owned()
            };
            Output::ok(format!("{}\n{key}\n", panel_url(&st.cfg, &segment)))
        }
        WebAction::RotatePath => {
            let Some(web) = st.admin_web.as_ref() else {
                return Output::fail(1, "the panel is not running in this process");
            };
            let new = match new_segment() {
                Ok(s) => s,
                Err(e) => return fail(e),
            };
            let stored = new.clone();
            let done = routing
                .write(move |tx| {
                    tx.execute(
                        "INSERT OR REPLACE INTO admin_web_path (id, segment, created_at)
                         VALUES (1, ?1, ?2)",
                        params![stored, now],
                    )?;
                    Ok(())
                })
                .await;
            if let Err(e) = done {
                return fail(e);
            }
            web.set_segment(new.clone());
            web.clear_ephemeral();
            web.record(JournalKind::PathRotated, None, now);
            tracing::warn!("admin web: the secret path was replaced, every session is closed");
            Output::ok(format!("{}\n", panel_url(&st.cfg, &new)))
        }
        WebAction::Enroll => {
            let Some(segment) = segment else {
                return Output::fail(1, "no path yet: start the relay with `admin_web = true`");
            };
            if enrolled {
                return Output::fail(
                    2,
                    "a passkey is already registered: `tomi-relay admin web reset` removes it first",
                );
            }
            let code = match new_code() {
                Ok(c) => c,
                Err(e) => return fail(e),
            };
            let hash = code_hash(&code);
            if let Err(e) = routing
                .write(move |tx| store_enrollment(tx, &hash, now))
                .await
            {
                return fail(e);
            }
            let mut out = Output::ok(format!("{code}\n"));
            out.stderr = format!(
                "enrolment code above: shown only now, valid 15 minutes, {ENROLL_ATTEMPTS} attempts.\nOpen {} and paste it.",
                panel_url(&st.cfg, &segment)
            );
            out
        }
        WebAction::Reset { confirm } => {
            let Some(segment) = segment else {
                return Output::fail(1, "nothing to reset: no path yet");
            };
            let expected = &segment[..segment.len().min(8)];
            if confirm.as_deref().map(str::trim) != Some(expected) {
                return Output::fail(
                    2,
                    format!(
                        "confirmation needed: retype the first 8 characters of the panel path \
                         ({expected}); this removes the passkey and closes every session"
                    ),
                );
            }
            let done = routing
                .write(|tx| {
                    if tables_exist(tx)? {
                        tx.execute("DELETE FROM admin_web_credential", [])?;
                        tx.execute("DELETE FROM admin_web_enrollment", [])?;
                    }
                    Ok(())
                })
                .await;
            if let Err(e) = done {
                return fail(e);
            }
            if let Some(web) = st.admin_web.as_ref() {
                web.clear_ephemeral();
                web.record(JournalKind::PasskeyReset, None, now);
            }
            tracing::warn!("admin web: the passkey was removed by the administrator");
            Output::ok(
                "passkey removed, sessions closed; `tomi-relay admin web enroll` makes a new enrolment code\n",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_and_rp_id_follow_public_url() {
        let ok = |u: &str| origin_and_rp_id(u);
        assert_eq!(
            ok("https://relay.example.org"),
            Some((
                "https://relay.example.org".into(),
                "relay.example.org".into()
            ))
        );
        assert_eq!(
            ok("https://Relay.Example.org/"),
            Some((
                "https://relay.example.org".into(),
                "relay.example.org".into()
            ))
        );
        assert_eq!(
            ok("https://relay.example.org:443/x"),
            Some((
                "https://relay.example.org".into(),
                "relay.example.org".into()
            ))
        );
        assert_eq!(
            ok("https://tomi.example:8443"),
            Some(("https://tomi.example:8443".into(), "tomi.example".into()))
        );
        assert_eq!(
            ok("https://localhost"),
            Some(("https://localhost".into(), "localhost".into()))
        );
        for bad in [
            "http://relay.example.org",
            "relay.example.org",
            "https://",
            "https://10.0.0.1",
            "https://[::1]:8443",
            "https://user@tomi.example",
            "https://tomi.example:",
            "https://tomi.example:99999",
            "https://tomi..example",
            "https://-tomi.example",
            "https://tomi_example",
        ] {
            assert_eq!(ok(bad), None, "{bad}");
        }
    }

    #[test]
    fn enrolment_codes_have_the_documented_shape() {
        let code = new_code().unwrap();
        assert_eq!(code.len(), 14);
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| g.len() == 4));
        assert!(code
            .bytes()
            .filter(|b| *b != b'-')
            .all(|b| install::ALPHABET.contains(&b)));
        assert_ne!(new_code().unwrap(), code);
        // Case, dashes and spaces do not matter; the hash is not the installation-token hash.
        assert_eq!(
            code_hash(&code),
            code_hash(&code.to_uppercase().replace('-', " "))
        );
        assert_ne!(code_hash(&code), install::hash(&code));
    }
}
