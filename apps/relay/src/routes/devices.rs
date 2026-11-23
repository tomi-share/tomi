//! Devices (`api-v1.md` §4.3), voluntary leave (§4.4), push registration and presence (§4.5).

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use tomi_core::{device_id_from_ed25519, DeviceCert};

use super::{empty, json};
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Gate, Rl};
use crate::input::json_body;
use crate::revoke::{revoke_device, Via};
use crate::state::{AppState, Effects, Target, WakeEvent};
use crate::store::{self, add_event, device_by_id, device_record, member_by_id, members};
use crate::wire;

/// `GET /v1/devices`: the members (revocations are read from the log).
pub async fn list(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let (cfg, now) = (st.cfg.clone(), ctx.now);
    let list = ctx
        .space()?
        .db
        .read(move |tx| {
            Ok(members(tx)?
                .iter()
                .map(|d| device_record(d, now, &cfg, true))
                .collect::<Vec<_>>())
        })
        .await?;
    Ok(json(StatusCode::OK, json!({ "devices": list })))
}

/// `POST /v1/devices`: the inviter registers the new device once the channel is `claimed`.
pub async fn register(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let channel_id = o.string("channel_id", true);
    let key_epoch = o.uint("key_epoch", true, 1, u64::from(u32::MAX));
    let name = o.name("name", true);
    let platform = o.platform("platform", true);
    let cert_bytes = o.b64("certificate", true);
    o.finish()?;
    let (Some(channel_id), Some(key_epoch), Some(name), Some(platform), Some(cert_bytes)) =
        (channel_id, key_epoch, name, platform, cert_bytes)
    else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let caller = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let (cfg, now) = (st.cfg.clone(), ctx.now);
    // A key and a `device_id` are unique across the relay: nobody else registers one between
    // the check below and the entry in the routing index.
    let registry = st.spaces.lock_registry().await;
    let spaces = st.spaces.clone();
    let (record, fx, new_device) = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`): a device revoked
            // while its request was in flight must not register another.
            store::require_caller(tx, &caller)?;
            let sp = store::require_space(tx)?;
            // Channel: unknown, expired or someone else's -> 404; not `claimed` -> 409.
            let ch: Option<(String, String, i64)> = tx
                .query_row(
                    "SELECT creator, state, expires_at FROM pairing_channels WHERE channel_id = ?1",
                    params![channel_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((creator, state, expires_at)) = ch else {
                return Err(ApiError::not_found());
            };
            if creator != caller.device_id || expires_at <= now {
                return Err(ApiError::not_found());
            }
            if state != "claimed" {
                return Err(ApiError::new(Code::ChannelClosed).detail("The channel is not in the claimed state."));
            }
            if u64::from(sp.key_epoch) != key_epoch {
                return Err(ApiError::new(Code::EpochConflict)
                    .detail("key_epoch differs from the current epoch.")
                    .with("key_epoch", sp.key_epoch));
            }
            // Certificate (decoded and verified by tomi-core): issued and signed by the caller.
            let bad = |why: &str| ApiError::field("/certificate", why);
            let cert = DeviceCert::from_bytes(&cert_bytes).map_err(|_| bad("not a device certificate"))?;
            if wire::uuid_parse(&cert.space_id) != Some(sp.space_id) {
                return Err(bad("space_id differs from the space"));
            }
            if cert.issuer_ed25519 != caller.sign_pk {
                return Err(bad("the certificate must be issued by the calling device"));
            }
            cert.verify_signature().map_err(|_| bad("invalid signature"))?;
            let subject = &cert.subject.ed25519;
            let device_id = device_id_from_ed25519(subject).map_err(|_| bad("invalid key"))?;
            // Never seen, neither as a member nor as a revoked key, in this space or in another:
            // the answer is the same, and tells only what the caller already knows (the key).
            let known: i64 = tx.query_row(
                "SELECT (SELECT count(*) FROM devices WHERE device_id = ?1 OR sign_pk = ?2)
                      + (SELECT count(*) FROM revoked_sign_pks WHERE sign_pk = ?2)",
                params![device_id, subject],
                |r| r.get(0),
            )?;
            if known > 0 || spaces.device_known(&device_id) || spaces.key_known(subject) {
                return Err(ApiError::new(Code::DeviceExists).detail("This device is already registered."));
            }
            let count = i64::try_from(members(tx)?.len()).unwrap_or(i64::MAX);
            if u64::try_from(count).unwrap_or(u64::MAX) >= cfg.max_devices {
                return Err(ApiError::new(Code::TooManyDevices)
                    .detail("The maximum number of devices is reached.")
                    .with("max_devices", cfg.max_devices));
            }
            tx.execute(
                "INSERT INTO devices (device_id, name, platform, sign_pk, kx_pk, certificate, issuer_device_id, joined_seq, registered_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    device_id,
                    name,
                    platform,
                    subject,
                    cert.subject.x25519,
                    cert_bytes,
                    caller.device_id,
                    sp.last_seq,
                    now
                ],
            )?;
            let row = device_by_id(tx, &device_id)?.ok_or_else(|| ApiError::internal("new device vanished"))?;
            let seq = add_event(
                tx,
                now,
                "device_added",
                Some(&caller.device_id),
                None,
                &json!({ "device": device_record(&row, now, &cfg, false) }),
            )?;
            tx.execute(
                "UPDATE pairing_channels SET state = 'completed' WHERE channel_id = ?1",
                params![channel_id],
            )?;
            fx.pairing.push(channel_id);
            let targets = members(tx)?
                .into_iter()
                .filter(|m| m.device_id != caller.device_id && m.device_id != device_id)
                .map(|m| Target::quiet(&m.device_id))
                .collect();
            fx.wakes.push(WakeEvent {
                seq,
                fcm: true,
                targets,
            });
            Ok((
                device_record(&row, now, &cfg, true),
                fx,
                (device_id, subject.clone()),
            ))
        })
        .await?;
    st.spaces.add_device(&new_device.0, &new_device.1, space.id);
    drop(registry);
    st.apply(&space, fx).await;
    Ok(json(StatusCode::CREATED, record))
}

/// `PATCH /v1/devices/{device_id}`: any member renames any member.
pub async fn rename(
    State(st): State<AppState>,
    Path(target): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let name = o.name("name", true);
    if !wire::is_device_id(&target) {
        o.error("device_id", "expected 32 lowercase hexadecimal characters");
    }
    o.finish()?;
    let name = name.ok_or_else(|| ApiError::internal("validated field missing"))?;
    let caller = ctx.dev()?.clone();
    let author = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let (cfg, now) = (st.cfg.clone(), ctx.now);
    let (record, fx) = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            let dev = member_by_id(tx, &target)?.ok_or_else(ApiError::not_found)?;
            if dev.name != name {
                tx.execute(
                    "UPDATE devices SET name = ?2 WHERE device_id = ?1",
                    params![target, name],
                )?;
                let seq = add_event(
                    tx,
                    now,
                    "device_updated",
                    Some(&author),
                    None,
                    &json!({ "device_id": target, "name": name }),
                )?;
                let targets = members(tx)?
                    .into_iter()
                    .filter(|m| m.device_id != author)
                    .map(|m| Target::quiet(&m.device_id))
                    .collect();
                fx.wakes.push(WakeEvent {
                    seq,
                    fcm: false,
                    targets,
                });
            }
            let fresh = member_by_id(tx, &target)?.ok_or_else(ApiError::not_found)?;
            Ok((device_record(&fresh, now, &cfg, true), fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(StatusCode::OK, record))
}

/// `DELETE /v1/me`: the device leaves the space (revocation path (b)).
pub async fn leave(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let me = ctx.dev()?.device_id.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let (seq, fx) = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            store::require_space(tx)?;
            // Read again in the transaction: a concurrent `DELETE /v1/me` of the same device may
            // have revoked it since the gate looked. It gets `403 revoked` (the proof that it
            // left, §4.4), never `401` nor `409 last_device`.
            let revoked = |tx: &rusqlite::Transaction| -> ApiResult<ApiError> {
                Ok(match device_by_id(tx, &me)? {
                    Some(d) if d.revoked_at.is_some() => ApiError::revoked(d.revoked_at),
                    _ => ApiError::unauthorized(),
                })
            };
            if !device_by_id(tx, &me)?.is_some_and(|d| d.is_member()) {
                return Err(revoked(tx)?);
            }
            if members(tx)?.len() <= 1 {
                return Err(ApiError::new(Code::LastDevice)
                    .detail("The last member cannot leave: delete the space (`DELETE /v1/space`) or use `tomi-relay admin reset`."));
            }
            let Some(seq) = revoke_device(tx, now, &me, Via::SelfLeave { by: &me }, &mut fx)?
            else {
                return Err(revoked(tx)?);
            };
            tx.execute("UPDATE space SET rotation_required = 1 WHERE id = 1", [])?;
            Ok((seq, fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(
        StatusCode::OK,
        json!({ "seq": seq, "rotation_required": true }),
    ))
}

/// `PUT /v1/me/push`.
pub async fn put_push(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let provider = o.string("provider", true);
    let token = o.string("token", true);
    let apple = matches!(
        provider.as_deref(),
        Some(crate::apns::PROVIDER | crate::apns::PROVIDER_SANDBOX)
    );
    if let Some(p) = &provider {
        if p != "fcm" && !apple {
            o.error("provider", "expected \"fcm\", \"apns\" or \"apns-sandbox\"");
        }
    }
    if let Some(t) = &token {
        if !(1..=4096).contains(&t.len()) || !t.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            o.error("token", "expected 1 to 4096 printable ASCII characters");
        } else if apple && !crate::apns::is_device_token(t) {
            // It goes in the path of the request to Apple: hexadecimal only.
            o.error(
                "token",
                "expected an APNs device token (64 to 200 hexadecimal characters)",
            );
        }
    }
    o.finish()?;
    let (Some(provider), Some(token)) = (provider, token) else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let (caller, now) = (ctx.dev()?.clone(), ctx.now);
    let me = caller.device_id.clone();
    ctx.space()?
        .db
        .write(move |tx| {
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            // The same token registered by another device is taken from it (reinstall).
            tx.execute(
                "DELETE FROM push_registrations WHERE token = ?1 AND device_id != ?2",
                params![token, me],
            )?;
            tx.execute(
                "INSERT INTO push_registrations (device_id, provider, token, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(device_id) DO UPDATE SET provider = ?2, token = ?3, updated_at = ?4",
                params![me, provider, token, now],
            )?;
            Ok(())
        })
        .await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// `DELETE /v1/me/push`: succeeds even without a registration.
pub async fn delete_push(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    ctx.space()?
        .db
        .write(move |tx| {
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            tx.execute(
                "DELETE FROM push_registrations WHERE device_id = ?1",
                params![me],
            )?;
            Ok(())
        })
        .await?;
    Ok(empty(StatusCode::NO_CONTENT))
}

/// `PUT /v1/me/presence`.
pub async fn put_presence(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DevicePresence])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let present = o.boolean("present", true);
    o.finish()?;
    let present = present.ok_or_else(|| ApiError::internal("validated field missing"))?;
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let until = present.then(|| {
        ctx.now.saturating_add(
            i64::try_from(st.cfg.presence_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX),
        )
    });
    ctx.space()?
        .db
        .write(move |tx| {
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            tx.execute(
                "UPDATE devices SET present_until = ?2 WHERE device_id = ?1",
                params![me, until],
            )?;
            Ok(())
        })
        .await?;
    Ok(empty(StatusCode::NO_CONTENT))
}
