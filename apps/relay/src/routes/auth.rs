//! Challenge/response sessions (`api-v1.md` §3.3) and version negotiation (§3.5).
//!
//! The `device_id` of the body designates the space (`device_id` is unique relay-wide). A
//! `device_id` that designates none is answered like an unknown device of a one-space relay:
//! a challenge as usual (nothing is stored, it could not be used), `unauthorized` for a session.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use tomi_core::{verify_relay_auth, RELAY_CHALLENGE_LEN};

use super::json;
use crate::config::Config;
use crate::db::Tx;
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Auth, Gate, Rl, SESSION_PREFIX};
use crate::input::json_body;
use crate::middleware::update_url;
use crate::spaces::Route;
use crate::state::AppState;
use crate::store::{self, device_by_id, Budget, DeviceRow};
use crate::wire::{b64_encode, random_bytes, sha256, ts_format, uuid_format};

/// Most pending challenges per device and in total for a space (§3.3).
const MAX_CHALLENGES_PER_DEVICE: i64 = 5;
const MAX_CHALLENGES_TOTAL: i64 = 200;
/// Most valid sessions per device (§3.3).
const MAX_SESSIONS_PER_DEVICE: i64 = 5;

/// `POST /v1/auth/challenge`: the answer is the same whether the device exists or not.
pub async fn challenge(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpPublic, Rl::IpAuth],
        space: true,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, body) = gate(&st, req, g).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let device_id = o.device_id("device_id", true);
    o.finish()?;
    let device_id = device_id.ok_or_else(|| ApiError::internal("validated field missing"))?;

    let challenge = random_bytes::<RELAY_CHALLENGE_LEN>().map_err(ApiError::internal)?;
    let now = ctx.now;
    let expires = now.saturating_add(
        i64::try_from(st.cfg.challenge_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX),
    );
    // A device that no space knows gets the same answer without a trace: its challenge could
    // not be used (a session for it is `unauthorized` either way). A device of a pending space
    // gets `503` and nothing is stored (§4.4.1): never the answer of a device that is gone.
    //
    // The unknown device goes through the same work as a known one (one write transaction on
    // the single writer, the same statements), on some space of the relay, and that transaction
    // is rolled back: the answer does not come measurably faster, and nothing is stored.
    let (space, stored) = match st.spaces.route_device(&device_id) {
        Route::Live(space) => (Some(space), true),
        Route::Pending => return Err(ApiError::pending()),
        Route::Unknown => (st.spaces.list().into_iter().next(), false),
    };
    if let Some(space) = space {
        let outcome = space
            .db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO challenges (challenge, device_id, expires_at) VALUES (?1, ?2, ?3)",
                    params![challenge.to_vec(), device_id, expires],
                )?;
                // At most 5 pending per device and 200 per space: the oldest go first.
                tx.execute(
                    "DELETE FROM challenges WHERE device_id = ?1 AND rowid NOT IN
                       (SELECT rowid FROM challenges WHERE device_id = ?1 ORDER BY rowid DESC LIMIT ?2)",
                    params![device_id, MAX_CHALLENGES_PER_DEVICE],
                )?;
                tx.execute(
                    "DELETE FROM challenges WHERE rowid NOT IN
                       (SELECT rowid FROM challenges ORDER BY rowid DESC LIMIT ?1)",
                    params![MAX_CHALLENGES_TOTAL],
                )?;
                if stored {
                    Ok(())
                } else {
                    // Dropping the transaction without committing undoes all of it.
                    Err(ApiError::internal("decoy challenge, rolled back"))
                }
            })
            .await;
        if stored {
            outcome?;
        }
    }
    Ok(json(
        StatusCode::OK,
        json!({ "challenge": b64_encode(&challenge), "expires_at": ts_format(expires) }),
    ))
}

/// Outcome of the version negotiation of §3.5.
#[derive(Debug, PartialEq, Eq)]
pub enum Negotiation {
    Accepted,
    UpdateRequired {
        effective: u32,
    },
    FormatUnsupported {
        effective: u32,
        limited_by: Vec<String>,
    },
}

/// §3.5, exactly. `members` holds the members as they are *before* the update of the
/// device `d` that announces `client_min..=client_max`.
pub fn negotiate(
    members: &[DeviceRow],
    d: &DeviceRow,
    client_min: u32,
    client_max: u32,
    now: i64,
    cfg: &Config,
) -> Negotiation {
    let others: Vec<&DeviceRow> = members
        .iter()
        .filter(|m| m.device_id != d.device_id && m.is_active(now, cfg) && m.format_max.is_some())
        .collect();
    let e_others = others
        .iter()
        .filter_map(|m| m.format_max)
        .min()
        .and_then(|v| u32::try_from(v).ok());
    let effective = if d.is_active(now, cfg) {
        // The device keeps the version up: never refused for that.
        e_others.map_or(client_max, |e| e.min(client_max))
    } else {
        match e_others {
            None => client_max,
            Some(e) if client_max < e => return Negotiation::UpdateRequired { effective: e },
            Some(e) => e,
        }
    };
    if client_min > effective {
        let limited_by = others
            .iter()
            .filter(|m| m.format_max.and_then(|v| u32::try_from(v).ok()) == Some(effective))
            .map(|m| m.device_id.clone())
            .collect();
        return Negotiation::FormatUnsupported {
            effective,
            limited_by,
        };
    }
    Negotiation::Accepted
}

/// `POST /v1/auth/session`.
pub async fn session(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpPublic, Rl::IpAuth],
        space: true,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, body) = gate(&st, req, g).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let device_id = o.device_id("device_id", true);
    let challenge = o.b64_exact("challenge", true, RELAY_CHALLENGE_LEN);
    let signature = o.b64_exact("signature", true, 64);
    let mut client = o.object("client", true);
    let (mut fmin, mut fmax, mut version, mut platform) = (None, None, None, None);
    if let Some(c) = client.as_mut() {
        fmin = c.uint("format_min", true, 1, u64::from(u32::MAX));
        fmax = c.uint("format_max", true, 1, u64::from(u32::MAX));
        version = c.app_version("app_version", true);
        platform = c.platform("platform", true);
        if let (Some(a), Some(b)) = (fmin, fmax) {
            if b < a {
                c.error("format_max", "must not be below format_min");
            }
        }
    }
    if let Some(c) = client {
        o.absorb(c);
    }
    o.finish()?;
    let (
        Some(device_id),
        Some(challenge),
        Some(signature),
        Some(fmin),
        Some(fmax),
        Some(version),
        Some(platform),
    ) = (
        device_id, challenge, signature, fmin, fmax, version, platform,
    )
    else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let fmin = u32::try_from(fmin).unwrap_or(u32::MAX);
    let fmax = u32::try_from(fmax).unwrap_or(u32::MAX);

    // The space of the device. An unknown device is `unauthorized`, as a one-space relay says;
    // a device of a pending space is `503`, never `401` (§4.4.1).
    let space = match st.spaces.route_device(&device_id) {
        Route::Live(space) => space,
        Route::Pending => return Err(ApiError::pending()),
        Route::Unknown => return Err(ApiError::unauthorized()),
    };
    let token_bytes = random_bytes::<32>().map_err(ApiError::internal)?;
    let token = format!("{SESSION_PREFIX}{}", b64_encode(&token_bytes));
    let cfg = st.cfg.clone();
    let now = ctx.now;
    let expires_at = now.saturating_add(
        i64::try_from(cfg.session_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX),
    );
    let budget = st.budget(&space).await?;
    let token_hash = sha256(token.as_bytes());
    // The update URL names the platform announced by the client (§3.5).
    let url_for_platform = update_url(&st.cfg.public_url, Some(&format!("tomi-{platform}/x")));
    let result = space
        .db
        .write_with(move |tx| {
            open_session(
                tx,
                &cfg,
                &budget,
                now,
                expires_at,
                &device_id,
                &challenge,
                &signature,
                fmin,
                fmax,
                &version,
                &platform,
                &token,
                &url_for_platform,
            )
        })
        .await?;
    // The session is in the database: the token can be routed from now on, before it is given out.
    st.spaces.add_session(token_hash, space.id, expires_at);
    Ok(json(StatusCode::OK, result))
}

/// Steps 1 to 6 of §3.3 in one transaction. The challenge is consumed whatever happens.
#[allow(clippy::too_many_arguments)]
fn open_session(
    tx: &Connection,
    cfg: &Config,
    budget: &Budget,
    now: i64,
    expires_at: i64,
    device_id: &str,
    challenge: &[u8],
    signature: &[u8],
    fmin: u32,
    fmax: u32,
    app_version: &str,
    platform: &str,
    token: &str,
    update_url: &str,
) -> ApiResult<Tx<serde_json::Value>> {
    let sp = store::require_space(tx)?;
    let fail = |e: ApiError| Ok(Tx::CommitThenFail(e));

    // 1. The challenge: found by (device, challenge), deleted whatever the result.
    let expires: Option<i64> = tx
        .query_row(
            "SELECT expires_at FROM challenges WHERE challenge = ?1 AND device_id = ?2",
            params![challenge, device_id],
            |r| r.get(0),
        )
        .optional()?;
    tx.execute(
        "DELETE FROM challenges WHERE challenge = ?1",
        params![challenge],
    )?;
    if expires.is_none_or(|e| e <= now) {
        return fail(ApiError::unauthorized());
    }

    // 2. The device.
    let Some(dev) = device_by_id(tx, device_id)? else {
        return fail(ApiError::unauthorized());
    };
    if dev.revoked_at.is_some() {
        return fail(ApiError::revoked(dev.revoked_at));
    }
    let Some(sign_pk) = dev.sign_pk.clone() else {
        return fail(ApiError::unauthorized());
    };

    // 3. The signature, checked by tomi-core.
    if verify_relay_auth(&uuid_format(&sp.space_id), &sign_pk, challenge, signature).is_err() {
        return fail(ApiError::unauthorized());
    }

    // 4. Version negotiation: the device is not marked active when refused.
    let members = store::members(tx)?;
    match negotiate(&members, &dev, fmin, fmax, now, cfg) {
        Negotiation::Accepted => {}
        Negotiation::UpdateRequired { effective } => {
            return fail(
                ApiError::update_required(Some(effective), update_url, false).detail(format!(
                    "Space effective format is {effective}, client format_max is {fmax}."
                )),
            );
        }
        Negotiation::FormatUnsupported {
            effective,
            limited_by,
        } => {
            return fail(
                ApiError::new(Code::FormatUnsupported)
                    .detail(format!(
                        "Space effective format is {effective}, client format_min is {fmin}."
                    ))
                    .with("effective_format", effective)
                    .with("limited_by", limited_by),
            );
        }
    }

    // 5. Record what the client announces.
    tx.execute(
        "UPDATE devices SET format_min = ?1, format_max = ?2, app_version = ?3, platform = ?4, last_seen_at = ?5
         WHERE device_id = ?6",
        params![fmin, fmax, app_version, platform, now, device_id],
    )?;

    // 6. The session (hash only), at most five per device.
    tx.execute(
        "INSERT INTO sessions (token_hash, device_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![sha256(token.as_bytes()).to_vec(), device_id, now, expires_at],
    )?;
    tx.execute(
        "DELETE FROM sessions WHERE device_id = ?1 AND rowid NOT IN
           (SELECT rowid FROM sessions WHERE device_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
        params![device_id, MAX_SESSIONS_PER_DEVICE],
    )?;
    let space = store::space_state(tx, cfg, now, budget)?;
    Ok(Tx::Commit(json!({
        "session_token": token,
        "expires_at": ts_format(expires_at),
        "device_id": device_id,
        "space": space,
    })))
}

/// `DELETE /v1/auth/session`: invalidates the current token.
pub async fn delete_session(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let hash = ctx.dev()?.token_hash;
    ctx.space()?
        .db
        .write(move |tx| {
            tx.execute(
                "DELETE FROM sessions WHERE token_hash = ?1",
                params![hash.to_vec()],
            )?;
            Ok(())
        })
        .await?;
    st.spaces.remove_session(&hash);
    Ok(super::empty(StatusCode::NO_CONTENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, last_seen: Option<i64>, fmin: i64, fmax: i64) -> DeviceRow {
        DeviceRow {
            device_id: id.into(),
            name: id.into(),
            platform: "windows".into(),
            sign_pk: Some(vec![1]),
            kx_pk: Some(vec![1]),
            certificate: Some(vec![1]),
            issuer_device_id: id.into(),
            joined_seq: 0,
            registered_at: 0,
            last_seen_at: last_seen,
            present_until: None,
            format_min: last_seen.map(|_| fmin),
            format_max: last_seen.map(|_| fmax),
            app_version: None,
            cursor_hint: None,
            revoked_at: None,
            push: false,
        }
    }

    const DAY: i64 = 86_400_000;

    #[test]
    fn a_new_device_that_is_too_old_is_refused() {
        let cfg = Config::default();
        let now = 100 * DAY;
        let a = dev("a", Some(now - 1000), 1, 2);
        let newcomer = dev("n", None, 1, 1);
        let m = vec![a, newcomer.clone()];
        assert_eq!(
            negotiate(&m, &newcomer, 1, 1, now, &cfg),
            Negotiation::UpdateRequired { effective: 2 }
        );
    }

    #[test]
    fn an_active_device_is_never_refused_for_its_version() {
        let cfg = Config::default();
        let now = 100 * DAY;
        let a = dev("a", Some(now - 1000), 1, 2);
        let b = dev("b", Some(now - 1000), 1, 1);
        let m = vec![a, b.clone()];
        assert_eq!(negotiate(&m, &b, 1, 1, now, &cfg), Negotiation::Accepted);
    }

    #[test]
    fn inactive_devices_do_not_hold_the_version() {
        let cfg = Config::default();
        let now = 100 * DAY;
        let a = dev("a", Some(now - 31 * DAY), 1, 1);
        let b = dev("b", Some(now - 1000), 1, 2);
        let m = vec![a.clone(), b.clone()];
        // B alone sets the version: 2. Then A (inactive, format_max 1) is told to update.
        assert_eq!(negotiate(&m, &b, 1, 2, now, &cfg), Negotiation::Accepted);
        assert_eq!(
            negotiate(&m, &a, 1, 1, now, &cfg),
            Negotiation::UpdateRequired { effective: 2 }
        );
    }

    #[test]
    fn a_format_min_above_the_effective_format_is_refused() {
        let cfg = Config::default();
        let now = 100 * DAY;
        let a = dev("a", Some(now - 1000), 1, 1);
        let n = dev("n", None, 2, 2);
        let m = vec![a, n.clone()];
        // n.format_max (2) >= 1, but format_min 2 > effective 1.
        assert_eq!(
            negotiate(&m, &n, 2, 2, now, &cfg),
            Negotiation::FormatUnsupported {
                effective: 1,
                limited_by: vec!["a".into()]
            }
        );
    }
}
