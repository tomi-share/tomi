//! The ordered checks every request goes through (`api-v1.md` §2.5): per-IP rate limit,
//! existence of a space, authentication, per-device rate limit. Content type, size and
//! validation (steps 6 and 7) are done by the handler afterwards, through `input`.
//!
//! **Finding the space.** A relay hosts several independent spaces behind one URL. Step 3
//! ("the space exists") is `space_not_initialized` only when the relay has no space at all. The
//! space of a request is then the one its identifier designates (`spaces.rs`): the session token
//! of a `Bearer` route, the channel of a `Pairing` route. The identifier that the gate cannot
//! read (the `device_id` of `auth/challenge` and `auth/session`, the `channel_id` of `claim`,
//! which are in the body or the path) is resolved by the handler once it has validated it. An
//! identifier that designates no space is answered like an unknown one: `unauthorized` for a
//! credential, `not_found` for a channel. Nothing tells whether it exists in another space.
//!
//! One credential bends that order, on purpose (§8.5): `Authorization: Pairing`. It carries no
//! session, so there is no device to count it against at step 5. What stands in for the two
//! limits depends on whether the token is the one of the channel named by the URL:
//!
//! * the holder of that token is counted like a device, per channel (step 5), and is **not**
//!   counted per IP: a pairing is two devices of one household behind one address, and
//!   they must not be slowed down by it. The token only comes from a `claim`, which is
//!   limited per IP and for the whole relay, and it dies with the channel;
//! * any other `Pairing` token (unknown, of another channel, channel unknown or past its
//!   deadline, not even shaped like a token) is counted per IP (step 2): without that,
//!   anyone could flood the single writer of the relay with bogus ones.
//!
//! Telling them apart takes one read of the channel, which therefore happens before the
//! per-IP limit, for this credential only. The order of what a *bogus* token is answered
//! stays the one of §2.5: `429`, then `space_not_initialized`, then what the route says.
//! The routes of the channel check the token again themselves (the channel may have changed
//! since): the gate only decides which limit applies, never what the request may do.

use std::net::IpAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use rusqlite::{params, OptionalExtension};

use crate::config::{Config, Rate};
use crate::error::{ApiError, Code};
use crate::ratelimit::ip_key;
use crate::spaces::{Route, Space};
use crate::state::AppState;
use crate::store;
use crate::wire::sha256;

/// Prefix of session tokens.
pub const SESSION_PREFIX: &str = "tms1.";
/// Prefix of pairing tokens.
pub const PAIRING_PREFIX: &str = "tmp1.";

/// The rate limits of §8.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rl {
    IpPublic,
    IpSpaceCreate,
    IpAuth,
    IpClaim,
    GlobalClaim,
    IpWs,
    IpStatic,
    Device,
    DeviceItems,
    DeviceTombstones,
    DeviceItemStates,
    DevicePresence,
    DevicePairing,
    DeviceTransfers,
    DeviceBlobParts,
    PairingPoll,
}

impl Rl {
    pub fn name(self) -> &'static str {
        match self {
            Rl::IpPublic => "rl_ip_public",
            Rl::IpSpaceCreate => "rl_ip_space_create",
            Rl::IpAuth => "rl_ip_auth",
            Rl::IpClaim => "rl_ip_claim",
            Rl::GlobalClaim => "rl_global_claim",
            Rl::IpWs => "rl_ip_ws",
            Rl::IpStatic => "rl_ip_static",
            Rl::Device => "rl_device",
            Rl::DeviceItems => "rl_device_items",
            Rl::DeviceTombstones => "rl_device_tombstones",
            Rl::DeviceItemStates => "rl_device_item_states",
            Rl::DevicePresence => "rl_device_presence",
            Rl::DevicePairing => "rl_device_pairing",
            Rl::DeviceTransfers => "rl_device_transfers",
            Rl::DeviceBlobParts => "rl_device_blob_parts",
            Rl::PairingPoll => "rl_pairing_poll",
        }
    }

    pub fn rate(self, cfg: &Config) -> Rate {
        match self {
            Rl::IpPublic => cfg.rl_ip_public,
            Rl::IpSpaceCreate => cfg.rl_ip_space_create,
            Rl::IpAuth => cfg.rl_ip_auth,
            Rl::IpClaim => cfg.rl_ip_claim,
            Rl::GlobalClaim => cfg.rl_global_claim,
            Rl::IpWs => cfg.rl_ip_ws,
            Rl::IpStatic => cfg.rl_ip_static,
            Rl::Device => cfg.rl_device,
            Rl::DeviceItems => cfg.rl_device_items,
            Rl::DeviceTombstones => cfg.rl_device_tombstones,
            Rl::DeviceItemStates => cfg.rl_device_item_states,
            Rl::DevicePresence => cfg.rl_device_presence,
            Rl::DevicePairing => cfg.rl_device_pairing,
            Rl::DeviceTransfers => cfg.rl_device_transfers,
            Rl::DeviceBlobParts => cfg.rl_device_blob_parts,
            Rl::PairingPoll => cfg.rl_pairing_poll,
        }
    }
}

/// How a route authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// No credential.
    None,
    /// `Authorization: Bearer <session token>` of a member device.
    Bearer,
    /// Bearer, or `Authorization: Pairing <token>`. The gate only tells whether the token is
    /// the channel's (see the module documentation, call [`gate_channel`]); the handler
    /// checks it against the channel for good.
    BearerOrPairing,
}

/// Policy of a route.
#[derive(Debug, Clone, Copy)]
pub struct Gate {
    /// Per-IP limits (step 2).
    pub ip: &'static [Rl],
    /// The relay must host a space (step 3).
    pub space: bool,
    pub auth: Auth,
    /// Per-device limits (step 5).
    pub dev: &'static [Rl],
}

impl Gate {
    /// A route with a device session and the general per-device limit.
    pub const fn bearer(dev: &'static [Rl]) -> Self {
        Self {
            ip: &[],
            space: true,
            auth: Auth::Bearer,
            dev,
        }
    }
}

/// The device behind a valid session.
#[derive(Debug, Clone)]
pub struct Dev {
    pub device_id: String,
    pub sign_pk: Vec<u8>,
    pub joined_seq: i64,
    pub token_hash: [u8; 32],
}

/// What a handler receives from the gate.
pub struct Ctx {
    pub parts: Parts,
    pub ip: IpAddr,
    pub now: i64,
    pub dev: Option<Dev>,
    /// SHA-256 of the `Pairing` token presented, if any.
    pub pairing_hash: Option<[u8; 32]>,
    /// The space of the request: the one of the session (`Bearer`), or the one of the channel
    /// (`Pairing`). `None` when no identifier designated one.
    pub space: Option<Arc<Space>>,
}

impl Ctx {
    /// The authenticated device (routes with `Auth::Bearer`).
    pub fn dev(&self) -> Result<&Dev, ApiError> {
        self.dev.as_ref().ok_or_else(ApiError::unauthorized)
    }

    /// The space of the authenticated device (routes with `Auth::Bearer`).
    pub fn space(&self) -> Result<&Arc<Space>, ApiError> {
        self.space.as_ref().ok_or_else(ApiError::unauthorized)
    }
}

/// `(scheme, credential)` of the `Authorization` header.
fn credential(parts: &Parts) -> Option<(String, String)> {
    let v = parts.headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, cred) = v.split_once(' ')?;
    let cred = cred.trim();
    (!cred.is_empty() && !cred.contains(' '))
        .then(|| (scheme.to_ascii_lowercase(), cred.to_owned()))
}

/// What one read of the database says about a `Pairing` credential.
struct Seen {
    /// Step 3: `space_not_initialized` if the relay hosts no space.
    space: Result<(), ApiError>,
    /// Step 4: the token is the one the channel of the URL was claimed with.
    valid: bool,
    /// The space of the channel of the URL, if there is one.
    owner: Option<Arc<Space>>,
    /// The channel of the URL belongs to a pending space (`503`).
    pending: bool,
}

/// Looks at a `Pairing` credential: the existence of a space (step 3), the space of the channel
/// (from the index) and, in that space's database, the token (step 4). The token is never valid
/// without a channel to judge it against (a route that is not one of a channel, or a channel
/// that no space has), without a space, or if it is not even shaped like a pairing token
/// (`hash` is `None`).
async fn look_at_token(
    st: &AppState,
    channel_id: Option<&str>,
    hash: Option<[u8; 32]>,
    now: i64,
) -> Result<Seen, ApiError> {
    let space = if st.spaces.is_empty() {
        Err(ApiError::space_not_initialized())
    } else {
        Ok(())
    };
    let route = match (&space, channel_id) {
        (Ok(()), Some(id)) => st.spaces.route_channel(id),
        _ => Route::Unknown,
    };
    let pending = matches!(route, Route::Pending);
    let owner = match route {
        Route::Live(sp) => Some(sp),
        Route::Pending | Route::Unknown => None,
    };
    let valid = match (&owner, channel_id, hash) {
        (Some(sp), Some(id), Some(h)) => {
            let id = id.to_owned();
            sp.db
                .read(move |tx| store::pairing_token_matches(tx, &id, &h, now))
                .await?
        }
        _ => false,
    };
    Ok(Seen {
        space,
        valid,
        owner,
        pending,
    })
}
/// Runs steps 2 to 5 of the order of checks and returns the context and the body.
pub async fn gate(st: &AppState, req: Request, g: Gate) -> Result<(Ctx, Body), ApiError> {
    gate_in(st, req, g, None).await
}

/// [`gate`] for a route of one pairing channel (`Auth::BearerOrPairing`). `channel_id` is the
/// one of the URL: a `Pairing` token is judged against that channel.
pub async fn gate_channel(
    st: &AppState,
    req: Request,
    g: Gate,
    channel_id: &str,
) -> Result<(Ctx, Body), ApiError> {
    gate_in(st, req, g, Some(channel_id)).await
}

async fn gate_in(
    st: &AppState,
    req: Request,
    g: Gate,
    channel_id: Option<&str>,
) -> Result<(Ctx, Body), ApiError> {
    let (parts, body) = req.into_parts();
    let ip = st.client_ip(&parts.headers, &parts.extensions);
    let now = st.now();

    // (2) per-IP rate limits.
    for rl in g.ip {
        let key = if *rl == Rl::GlobalClaim {
            "*".to_owned()
        } else {
            ip_key(ip)
        };
        st.limiter.check(rl.name(), &key, rl.rate(&st.cfg))?;
    }

    let mut ctx = Ctx {
        parts,
        ip,
        now,
        dev: None,
        pairing_hash: None,
        space: None,
    };

    // (3) the relay hosts a space, (4) authentication.
    let cred = credential(&ctx.parts);
    // The key under which the holder of a channel's pairing token is counted at step 5.
    let mut token_holder: Option<String> = None;
    match g.auth {
        Auth::None => {
            if g.space && st.spaces.is_empty() {
                return Err(ApiError::space_not_initialized());
            }
        }
        Auth::Bearer => {
            let (space, dev) = bearer(st, cred, now).await?;
            ctx.space = Some(space);
            ctx.dev = Some(dev);
        }
        Auth::BearerOrPairing => match cred {
            Some((scheme, token)) if scheme == "pairing" => {
                let hash = token
                    .starts_with(PAIRING_PREFIX)
                    .then(|| sha256(token.as_bytes()));
                let seen = look_at_token(st, channel_id, hash, now).await?;
                // (2) A token that is not the channel's is counted per IP, whatever the way it
                // is not. Without this limit it would have none at all (it has no session, so
                // step 5 cannot apply): anyone could flood the single SQLite writer with
                // `Authorization: Pairing …` on the pairing routes. The token of the channel
                // is not counted here (module documentation): it is counted at step 5.
                if !seen.valid {
                    st.limiter.check(
                        Rl::IpPublic.name(),
                        &ip_key(ctx.ip),
                        Rl::IpPublic.rate(&st.cfg),
                    )?;
                }
                // (3) the relay hosts a space.
                seen.space?;
                // The channel is one of a pending space: try again later, never "unknown".
                if seen.pending {
                    return Err(ApiError::pending());
                }
                // (4) the credential is shaped like a pairing token; the route decides for good.
                let Some(hash) = hash else {
                    return Err(ApiError::unauthorized());
                };
                ctx.pairing_hash = Some(hash);
                ctx.space = seen.owner;
                if seen.valid {
                    token_holder = channel_id.map(|id| format!("pairing:{id}"));
                }
            }
            other => {
                let (space, dev) = bearer(st, other, now).await?;
                ctx.space = Some(space);
                ctx.dev = Some(dev);
            }
        },
    }

    // (5) per-device rate limits. The holder of a channel's pairing token has no device: it is
    // counted, per channel, as if it were one.
    let counted = ctx
        .dev
        .as_ref()
        .map(|d| d.device_id.as_str())
        .or(token_holder.as_deref());
    if let Some(key) = counted {
        for rl in g.dev {
            st.limiter.check(rl.name(), key, rl.rate(&st.cfg))?;
        }
    }
    Ok((ctx, body))
}

/// Existence of a space, then the session, then the `last_seen_at` refresh.
async fn bearer(
    st: &AppState,
    cred: Option<(String, String)>,
    now: i64,
) -> Result<(Arc<Space>, Dev), ApiError> {
    // The relay must host a space before any credential is looked at.
    if st.spaces.is_empty() {
        return Err(ApiError::space_not_initialized());
    }
    let Some((scheme, token)) = cred else {
        return Err(ApiError::unauthorized());
    };
    if scheme != "bearer" {
        return Err(ApiError::unauthorized());
    }
    let (space, dev, _expires, last_seen) = lookup_session(st, &token, now).await?;
    // `last_seen_at` is refreshed at most once a minute (§3.3).
    if last_seen.is_none_or(|t| now - t >= 60_000) {
        let id = dev.device_id.clone();
        space
            .db
            .write(move |tx| {
                tx.execute(
                    "UPDATE devices SET last_seen_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
                    params![now, id],
                )?;
                Ok(())
            })
            .await?;
    }
    crate::error::log_device(&dev.device_id);
    Ok((space, dev))
}

/// Looks a session token up: its space, the device, the end of the session and the last
/// activity of the device. `unauthorized` (unknown or malformed, in any space), `session_expired`,
/// `revoked`, `unavailable` (the space of the token is pending, §4.4.1; with `Retry-After`), or
/// `space_not_initialized` (the relay hosts no space); also used by the WebSocket
/// authentication (§5.1, where `unavailable` closes with `1013`).
///
/// The index names the space of the token; that space's database then judges it. A token the
/// index does not know and a token the database does not know are answered the same.
pub async fn lookup_session(
    st: &AppState,
    token: &str,
    now: i64,
) -> Result<(Arc<Space>, Dev, i64, Option<i64>), ApiError> {
    if st.spaces.is_empty() {
        return Err(ApiError::space_not_initialized());
    }
    if !token.starts_with(SESSION_PREFIX) {
        return Err(ApiError::unauthorized());
    }
    let hash = sha256(token.as_bytes());
    // One look (§1.4): a token of a pending space (deletion under way, or state unknown until
    // the next start) is neither "gone" nor "unknown", and is never answered like them.
    let space = match st.spaces.route_token(&hash) {
        Route::Live(space) => space,
        Route::Pending => return Err(ApiError::pending()),
        Route::Unknown => return Err(ApiError::unauthorized()),
    };
    let found = space
        .db
        .read(move |tx| {
            let row = tx
                .query_row(
                    "SELECT s.device_id, s.expires_at, s.revoked, d.revoked_at, d.sign_pk, d.joined_seq, d.last_seen_at
                     FROM sessions s JOIN devices d ON d.device_id = s.device_id
                     WHERE s.token_hash = ?1",
                    params![hash.to_vec()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, Option<i64>>(3)?,
                            r.get::<_, Option<Vec<u8>>>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, Option<i64>>(6)?,
                        ))
                    },
                )
                .optional()?;
            Ok(row)
        })
        .await?;
    let Some((device_id, expires_at, session_revoked, revoked_at, sign_pk, joined_seq, last_seen)) =
        found
    else {
        return Err(ApiError::unauthorized());
    };
    if session_revoked != 0 || revoked_at.is_some() {
        return Err(ApiError::revoked(revoked_at));
    }
    if expires_at <= now {
        return Err(ApiError::new(Code::SessionExpired).detail("The session token has expired."));
    }
    let Some(sign_pk) = sign_pk else {
        return Err(ApiError::unauthorized());
    };
    Ok((
        space,
        Dev {
            device_id,
            sign_pk,
            joined_seq,
            token_hash: hash,
        },
        expires_at,
        last_seen,
    ))
}
