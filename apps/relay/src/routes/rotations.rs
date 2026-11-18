//! Rotation of the space key and revocation (`api-v1.md` §3.4, §4.7). The relay cannot open
//! the sealed keys: it decodes the signed `KeyRotation` with `tomi-core`, checks who signed
//! it and that it covers exactly the members, and records it.

use std::collections::BTreeSet;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use tomi_core::{device_id_from_ed25519, KeyRotation};

use super::json;
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Gate, Rl};
use crate::input::json_body;
use crate::revoke::{revoke_device, Via};
use crate::state::{AppState, Effects, Target, WakeEvent};
use crate::store::{self, insert_event, members, next_seq};
use crate::wire::{self, b64_encode, ts_format};

/// `POST /v1/rotations`.
pub async fn post(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let statement = o.b64("rotation", true);
    o.finish()?;
    let statement = statement.ok_or_else(|| ApiError::internal("validated field missing"))?;
    let dev = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let (status, v, fx) = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &dev)?;
            let sp = store::require_space(tx)?;
            let bad = |why: &str| ApiError::field("/rotation", why);

            // Decoded by tomi-core; the space must be this one.
            let r = KeyRotation::from_bytes(&statement).map_err(|_| bad("not a key rotation"))?;
            if wire::uuid_parse(&r.space_id) != Some(sp.space_id) {
                return Err(bad("space_id differs from the space"));
            }

            // The same bytes accepted before: the same answer.
            let known: Option<(i64, String)> = tx
                .query_row(
                    "SELECT seq, revoked_ids FROM rotations WHERE key_epoch = ?1 AND statement = ?2",
                    params![r.new_epoch, statement],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((seq, revoked_ids)) = known {
                let ids: Value = serde_json::from_str(&revoked_ids).unwrap_or_else(|_| json!([]));
                return Ok((
                    StatusCode::OK,
                    json!({ "seq": seq, "key_epoch": r.new_epoch, "revoked_device_ids": ids }),
                    fx,
                ));
            }

            if r.new_epoch != sp.key_epoch.saturating_add(1) {
                return Err(ApiError::new(Code::EpochConflict)
                    .detail("new_epoch is not the current epoch + 1.")
                    .with("key_epoch", sp.key_epoch));
            }
            if r.issuer_ed25519 != dev.sign_pk {
                return Err(bad("the rotation must be issued by the calling device"));
            }
            r.verify_signature().map_err(|_| bad("invalid signature"))?;
            let created_at = i64::try_from(r.issued_at_ms)
                .ok()
                .filter(|ms| (0..=wire::TS_MAX_MS).contains(ms))
                .ok_or_else(|| bad("issued_at out of range"))?;

            // Revoked keys: known devices (members or already revoked), never the caller.
            let current = members(tx)?;
            let member_keys: BTreeSet<Vec<u8>> = current.iter().filter_map(|m| m.sign_pk.clone()).collect();
            let mut revoked_keys: BTreeSet<Vec<u8>> = BTreeSet::new();
            for k in &r.revoked {
                let was_revoked: i64 = tx.query_row(
                    "SELECT count(*) FROM revoked_sign_pks WHERE sign_pk = ?1",
                    params![k],
                    |row| row.get(0),
                )?;
                if *k == dev.sign_pk {
                    return Err(bad("a device cannot revoke itself"));
                }
                if !member_keys.contains(k) && was_revoked == 0 {
                    return Err(bad("revoked contains an unknown key"));
                }
                revoked_keys.insert(k.clone());
            }

            // The recipients are exactly the members that stay.
            let expected: BTreeSet<Vec<u8>> = member_keys.difference(&revoked_keys).cloned().collect();
            let given: BTreeSet<Vec<u8>> = r.recipients.iter().map(|x| x.ed25519.clone()).collect();
            if expected != given {
                let mut ids: Vec<String> = expected
                    .iter()
                    .filter_map(|k| device_id_from_ed25519(k).ok())
                    .collect();
                ids.sort();
                return Err(ApiError::new(Code::DeviceSetMismatch)
                    .detail("The recipients are not exactly the members that remain.")
                    .with("expected_device_ids", ids));
            }

            // Apply: epoch, statement, `rotation` event, then one `device_revoked` per device.
            let newly: Vec<String> = current
                .iter()
                .filter(|m| m.sign_pk.as_ref().is_some_and(|k| revoked_keys.contains(k)))
                .map(|m| m.device_id.clone())
                .collect();
            let seq = next_seq(tx, now)?;
            tx.execute(
                "UPDATE space SET key_epoch = ?1, rotation_required = 0 WHERE id = 1",
                params![r.new_epoch],
            )?;
            tx.execute(
                "INSERT INTO rotations (key_epoch, device_id, created_at, seq, statement, revoked_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    r.new_epoch,
                    dev.device_id,
                    created_at,
                    seq,
                    statement,
                    serde_json::to_string(&newly).unwrap_or_else(|_| "[]".into())
                ],
            )?;
            insert_event(
                tx,
                seq,
                now,
                "rotation",
                Some(&dev.device_id),
                None,
                &json!({
                    "key_epoch": r.new_epoch,
                    "device_id": dev.device_id,
                    "created_at": ts_format(created_at),
                    "revoked_device_ids": newly,
                    "rotation": b64_encode(&statement),
                }),
            )?;
            for id in &newly {
                revoke_device(
                    tx,
                    now,
                    id,
                    Via::Rotation {
                        by: &dev.device_id,
                        rotation_seq: seq,
                    },
                    &mut fx,
                )?;
            }
            let targets = members(tx)?
                .into_iter()
                .filter(|m| m.device_id != dev.device_id)
                .map(|m| Target::quiet(&m.device_id))
                .collect();
            fx.wakes.push(WakeEvent {
                seq,
                fcm: true,
                targets,
            });
            Ok((
                StatusCode::CREATED,
                json!({ "seq": seq, "key_epoch": r.new_epoch, "revoked_device_ids": newly }),
                fx,
            ))
        })
        .await?;
    // The sockets of the revoked devices are cut before the revoker gets its answer (§3.4).
    st.apply(&space, fx).await;
    Ok(json(status, v))
}

/// `GET /v1/keys`: every rotation of the space, by increasing epoch.
pub async fn keys(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let v = ctx
        .space()?
        .db
        .read(|tx| {
            let sp = store::require_space(tx)?;
            let mut stmt =
                tx.prepare("SELECT key_epoch, seq, statement FROM rotations ORDER BY key_epoch")?;
            let rotations = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "key_epoch": r.get::<_, i64>(0)?,
                        "seq": r.get::<_, i64>(1)?,
                        "rotation": b64_encode(&r.get::<_, Vec<u8>>(2)?),
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({ "key_epoch": sp.key_epoch, "rotations": rotations }))
        })
        .await?;
    Ok(json(StatusCode::OK, v))
}
