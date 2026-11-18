//! Pairing rendezvous (`api-v1.md` §4.6): an ephemeral mailbox where the inviting device
//! and the new device swap opaque messages. The relay trusts nothing of what it carries.
//!
//! The `channel_id` is unique across the whole relay and designates the space of the channel:
//! that is how the new device, which has no credential yet, reaches the space of its inviter.
//! A `Bearer` session of another space finds the channel unknown (`404`), like anyone else.

use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::time::Instant;

use super::{empty, json};
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, gate_channel, Auth, Gate, Rl, PAIRING_PREFIX};
use crate::input::{json_body, query_uint};
use crate::state::AppState;
use crate::store;
use crate::wire::{b64_encode, random_bytes, sha256, ts_format};

/// One row of `pairing_channels`.
struct Chan {
    creator: String,
    state: String,
    token_hash: Option<Vec<u8>>,
    expires_at: i64,
}

/// Which end of the channel a request speaks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Inviter,
    Joiner,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Inviter => "inviter",
            Side::Joiner => "joiner",
        }
    }
    fn other(self) -> Side {
        match self {
            Side::Inviter => Side::Joiner,
            Side::Joiner => Side::Inviter,
        }
    }
}

/// `^[1-9][0-9]{0,3}$` (§4.6).
fn valid_channel_id(s: &str) -> bool {
    (1..=4).contains(&s.len()) && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit())
}

fn check_id(id: &str) -> ApiResult<()> {
    if valid_channel_id(id) {
        Ok(())
    } else {
        Err(ApiError::field(
            "/channel_id",
            "expected a decimal number of 1 to 4 digits",
        ))
    }
}

/// The channel, unless it is unknown or past its deadline.
fn load(tx: &Connection, id: &str, now: i64) -> ApiResult<Option<Chan>> {
    let c = tx
        .query_row(
            "SELECT creator, state, token_hash, expires_at FROM pairing_channels WHERE channel_id = ?1",
            params![id],
            |r| {
                Ok(Chan {
                    creator: r.get(0)?,
                    state: r.get(1)?,
                    token_hash: r.get(2)?,
                    expires_at: r.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(c.filter(|c| c.expires_at > now))
}

fn ttl_ms(st: &AppState) -> i64 {
    i64::try_from(st.cfg.pairing_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX)
}

/// `POST /v1/pairing/channels` (inviting device).
pub async fn create(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DevicePairing])).await?;
    let o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, false).await?;
    o.finish()?;
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let (now, max_open, ttl) = (ctx.now, st.cfg.pairing_max_open, ttl_ms(&st));
    // The number is unique across the relay: nobody else takes one between the check below and
    // the entry in the routing index.
    let registry = st.spaces.lock_registry().await;
    let spaces = st.spaces.clone();
    let (id, expires) = space
        .db
        .write(move |tx| {
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            let open: i64 = tx.query_row(
                "SELECT count(*) FROM pairing_channels WHERE state IN ('open', 'claimed') AND expires_at > ?1",
                params![now],
                |r| r.get(0),
            )?;
            if u64::try_from(open).unwrap_or(u64::MAX) >= max_open {
                return Err(ApiError::new(Code::ChannelLimit).detail("Too many pairing channels are open."));
            }
            // A random number in 1..=9999 that no channel of the relay uses (no modulo bias).
            let limit = (u32::MAX / 9999) * 9999;
            for _ in 0..200 {
                let r = u32::from_be_bytes(random_bytes::<4>().map_err(ApiError::internal)?);
                if r >= limit {
                    continue;
                }
                let id = (1 + r % 9999).to_string();
                let taken: i64 = tx.query_row(
                    "SELECT count(*) FROM pairing_channels WHERE channel_id = ?1",
                    params![id],
                    |r| r.get(0),
                )?;
                if taken == 0 && !spaces.channel_taken(&id) {
                    let expires = now.saturating_add(ttl);
                    tx.execute(
                        "INSERT INTO pairing_channels (channel_id, creator, state, created_at, expires_at)
                         VALUES (?1, ?2, 'open', ?3, ?4)",
                        params![id, me, now, expires],
                    )?;
                    return Ok((id, expires));
                }
            }
            Err(ApiError::new(Code::ChannelLimit).detail("No free channel number."))
        })
        .await?;
    st.spaces.add_channel(&id, space.id);
    drop(registry);
    Ok(json(
        StatusCode::CREATED,
        json!({ "channel_id": id, "state": "open", "expires_at": ts_format(expires) }),
    ))
}

/// `POST /v1/pairing/channels/{channel_id}/claim` (new device, no credential): one successful
/// claim per channel, which extends the deadline once.
pub async fn claim(
    State(st): State<AppState>,
    Path(channel_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpPublic, Rl::IpClaim, Rl::GlobalClaim],
        space: true,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, body) = gate(&st, req, g).await?;
    let o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, false).await?;
    o.finish()?;
    check_id(&channel_id)?;
    let token = format!(
        "{PAIRING_PREFIX}{}",
        b64_encode(&random_bytes::<24>().map_err(ApiError::internal)?)
    );
    let hash = sha256(token.as_bytes());
    // The channel designates the space; a number no space uses is an unknown channel, one of a
    // pending space is `503` (§4.4.1).
    let space = match st.spaces.route_channel(&channel_id) {
        crate::spaces::Route::Live(space) => space,
        crate::spaces::Route::Pending => return Err(ApiError::pending()),
        crate::spaces::Route::Unknown => return Err(ApiError::not_found()),
    };
    let (now, ttl) = (ctx.now, ttl_ms(&st));
    let id = channel_id.clone();
    let expires = space
        .db
        .write(move |tx| {
            let ch = load(tx, &id, now)?.ok_or_else(ApiError::not_found)?;
            match ch.state.as_str() {
                "open" => {}
                "claimed" => {
                    return Err(ApiError::new(Code::ChannelClaimed).detail("The channel has already been joined."))
                }
                _ => return Err(ApiError::new(Code::ChannelClosed).detail("The channel is finished.")),
            }
            let expires = now.saturating_add(ttl);
            tx.execute(
                "UPDATE pairing_channels SET state = 'claimed', token_hash = ?2, expires_at = ?3 WHERE channel_id = ?1",
                params![id, hash.to_vec(), expires],
            )?;
            Ok(expires)
        })
        .await?;
    st.pairing.notify(&channel_id);
    Ok(json(
        StatusCode::OK,
        json!({ "pairing_token": token, "expires_at": ts_format(expires) }),
    ))
}

/// `POST /v1/pairing/channels/{channel_id}/messages`.
pub async fn post_message(
    State(st): State<AppState>,
    Path(channel_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let g = Gate {
        ip: &[],
        space: true,
        auth: Auth::BearerOrPairing,
        dev: &[Rl::Device],
    };
    let (ctx, body) = gate_channel(&st, req, g, &channel_id).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let data = o.b64("body", true);
    if let Some(d) = &data {
        if d.is_empty() {
            o.error("body", "must not be empty");
        } else if d.len() as u64 > st.cfg.pairing_max_message_bytes {
            return Err(ApiError::payload_too_large(
                st.cfg.pairing_max_message_bytes,
            ));
        }
    }
    if !valid_channel_id(&channel_id) {
        o.error("channel_id", "expected a decimal number of 1 to 4 digits");
    }
    o.finish()?;
    let data = data.ok_or_else(|| ApiError::internal("validated field missing"))?;
    // No space for this channel number: an unknown channel.
    let space = ctx.space.clone().ok_or_else(ApiError::not_found)?;
    let (now, max_messages) = (ctx.now, st.cfg.pairing_max_messages);
    let (id, dev, pairing) = (channel_id.clone(), ctx.dev.clone(), ctx.pairing_hash);
    let index = space
        .db
        .write(move |tx| {
            // A device caller is judged again here (see `store::require_caller`); the holder of
            // the pairing token has no session, the channel is what judges it.
            if let Some(d) = &dev {
                store::require_caller(tx, d)?;
            }
            let ch = load(tx, &id, now)?.ok_or_else(ApiError::not_found)?;
            let side = side_of_parts(dev.as_ref().map(|d| d.device_id.as_str()), pairing, &ch)?;
            if matches!(ch.state.as_str(), "completed" | "closed") {
                return Err(ApiError::new(Code::ChannelClosed).detail("The channel is finished."));
            }
            let mine: i64 = tx.query_row(
                "SELECT count(*) FROM pairing_messages WHERE channel_id = ?1 AND side = ?2",
                params![id, side.name()],
                |r| r.get(0),
            )?;
            if u64::try_from(mine).unwrap_or(u64::MAX) >= max_messages {
                return Err(ApiError::new(Code::ChannelLimit).detail("Too many messages on this channel."));
            }
            let next: i64 = tx.query_row(
                "SELECT COALESCE(MAX(idx), 0) + 1 FROM pairing_messages WHERE channel_id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO pairing_messages (channel_id, idx, side, body) VALUES (?1, ?2, ?3, ?4)",
                params![id, next, side.name(), data],
            )?;
            Ok(next)
        })
        .await?;
    st.pairing.notify(&channel_id);
    Ok(json(StatusCode::CREATED, json!({ "index": index })))
}

/// Who speaks: the creator's session (404 for any other device) or the holder of the
/// channel's pairing token (401 for another token).
fn side_of_parts(device: Option<&str>, pairing: Option<[u8; 32]>, ch: &Chan) -> ApiResult<Side> {
    if let Some(d) = device {
        return if d == ch.creator {
            Ok(Side::Inviter)
        } else {
            Err(ApiError::not_found())
        };
    }
    match (pairing, &ch.token_hash) {
        (Some(given), Some(stored)) if bool::from(given.as_slice().ct_eq(stored.as_slice())) => {
            Ok(Side::Joiner)
        }
        _ => Err(ApiError::unauthorized()),
    }
}

/// What a read of the channel returns.
struct Snapshot {
    side: Side,
    state: String,
    expires_at: i64,
    messages: Vec<(i64, &'static str, Vec<u8>)>,
}

fn snapshot(
    tx: &Connection,
    id: &str,
    now: i64,
    who: (Option<&str>, Option<[u8; 32]>),
    after: i64,
) -> ApiResult<Snapshot> {
    let ch = load(tx, id, now)?.ok_or_else(ApiError::not_found)?;
    let side = side_of_parts(who.0, who.1, &ch)?;
    let from = side.other();
    let mut st = tx.prepare(
        "SELECT idx, body FROM pairing_messages WHERE channel_id = ?1 AND side = ?2 AND idx > ?3 ORDER BY idx",
    )?;
    let messages = st
        .query_map(params![id, from.name(), after], |r| {
            Ok((r.get(0)?, from.name(), r.get(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Snapshot {
        side,
        state: ch.state,
        expires_at: ch.expires_at,
        messages,
    })
}

/// `GET /v1/pairing/channels/{channel_id}/messages?after=&wait=`: the other side's messages,
/// with an optional long poll. The wait ends at the first message, at a change of state, or
/// at once if the channel is already finished (`completed` or `closed`): the reader gets the
/// messages that are left and the state, and nothing more can come.
pub async fn get_messages(
    State(st): State<AppState>,
    Path(channel_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let g = Gate {
        ip: &[],
        space: true,
        auth: Auth::BearerOrPairing,
        dev: &[Rl::Device],
    };
    let (ctx, _body) = gate_channel(&st, req, g, &channel_id).await?;
    let mut errs = Vec::new();
    let after = query_uint(&ctx.parts, "after", 0, u64::MAX).unwrap_or_else(|e| {
        errs.push(e);
        None
    });
    let wait = query_uint(&ctx.parts, "wait", 0, 25).unwrap_or_else(|e| {
        errs.push(e);
        None
    });
    if !valid_channel_id(&channel_id) {
        errs.push(crate::error::FieldError {
            field: "/channel_id".into(),
            reason: "expected a decimal number of 1 to 4 digits".into(),
        });
    }
    if !errs.is_empty() {
        return Err(ApiError::validation(errs));
    }
    // No space for this channel number: an unknown channel.
    let space = ctx.space.clone().ok_or_else(ApiError::not_found)?;
    let after = i64::try_from(after.unwrap_or(0)).unwrap_or(i64::MAX);
    let wait = Duration::from_secs(wait.unwrap_or(0)).min(st.timing.max_long_poll);
    let deadline = Instant::now() + wait;
    let who_dev = ctx.dev.as_ref().map(|d| d.device_id.clone());
    let who_pairing = ctx.pairing_hash;
    let mut rx = st.pairing.subscribe(&channel_id);
    let mut first_state: Option<String> = None;
    let mut limited = false;
    loop {
        rx.borrow_and_update();
        let (id, dev) = (channel_id.clone(), who_dev.clone());
        let now = st.now();
        let snap = space
            .db
            .read(move |tx| snapshot(tx, &id, now, (dev.as_deref(), who_pairing), after))
            .await?;
        if !limited {
            // One token per poll request, per channel and side.
            let key = format!("{channel_id}:{}", snap.side.name());
            st.limiter
                .check(Rl::PairingPoll.name(), &key, Rl::PairingPoll.rate(&st.cfg))?;
            limited = true;
        }
        let changed = first_state.as_ref().is_some_and(|s| *s != snap.state);
        first_state.get_or_insert_with(|| snap.state.clone());
        let remaining = deadline.saturating_duration_since(Instant::now());
        // Nothing can arrive on a finished channel (`completed` takes no message, `closed` has
        // none): waiting would only keep the reader there until `wait` is over.
        let finished = matches!(snap.state.as_str(), "completed" | "closed");
        if !snap.messages.is_empty() || changed || finished || remaining.is_zero() {
            let messages: Vec<_> = snap
                .messages
                .iter()
                .map(
                    |(i, side, body)| json!({ "index": i, "side": side, "body": b64_encode(body) }),
                )
                .collect();
            return Ok(json(
                StatusCode::OK,
                json!({ "state": snap.state, "expires_at": ts_format(snap.expires_at), "messages": messages }),
            ));
        }
        let _ = tokio::time::timeout(remaining, rx.changed()).await;
    }
}

/// `DELETE /v1/pairing/channels/{channel_id}`: closes the channel and erases its messages.
pub async fn close(
    State(st): State<AppState>,
    Path(channel_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let g = Gate {
        ip: &[],
        space: true,
        auth: Auth::BearerOrPairing,
        dev: &[Rl::Device],
    };
    let (ctx, _body) = gate_channel(&st, req, g, &channel_id).await?;
    check_id(&channel_id)?;
    // No space for this channel number: an unknown channel.
    let space = ctx.space.clone().ok_or_else(ApiError::not_found)?;
    let (now, id, dev, pairing) = (
        ctx.now,
        channel_id.clone(),
        ctx.dev.clone(),
        ctx.pairing_hash,
    );
    space
        .db
        .write(move |tx| {
            // A device caller is judged again here (see `store::require_caller`).
            if let Some(d) = &dev {
                store::require_caller(tx, d)?;
            }
            let ch = load(tx, &id, now)?.ok_or_else(ApiError::not_found)?;
            side_of_parts(dev.as_ref().map(|d| d.device_id.as_str()), pairing, &ch)?;
            tx.execute(
                "UPDATE pairing_channels SET state = 'closed' WHERE channel_id = ?1",
                params![id],
            )?;
            tx.execute(
                "DELETE FROM pairing_messages WHERE channel_id = ?1",
                params![id],
            )?;
            Ok(())
        })
        .await?;
    st.pairing.notify(&channel_id);
    Ok(empty(StatusCode::NO_CONTENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_ids_are_one_to_four_digits_without_leading_zero() {
        for ok in ["1", "42", "4821", "9999"] {
            assert!(valid_channel_id(ok), "{ok}");
        }
        for bad in ["", "0", "01", "12345", "a1", "-1", "1 "] {
            assert!(!valid_channel_id(bad), "{bad:?}");
        }
    }
}
