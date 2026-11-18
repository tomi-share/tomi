//! History transfers (`api-v1.md` §4.13): a sealed copy of one member's feed, dropped on the
//! relay for one other member. The relay sees blobs, sizes, the two devices and a signed
//! manifest; it holds no key and has no notion of partial inclusion.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use tomi_core::HistoryTransfer;

use super::{empty, json};
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Gate, Rl};
use crate::input::json_body;
use crate::purge::close_transfer;
use crate::routes::items::blob_set_hash;
use crate::state::{AppState, Effects, Target, WakeEvent};
use crate::store::{self, check_quota, insert_event, member_by_sign_pk, next_seq};
use crate::wire::{self, b64_encode, ts_format, uuid_format};

/// A stored transfer row: `(from_device, manifest, blob_set_hash, seq, expires_at)`.
type ExistingTransfer = (String, Vec<u8>, Vec<u8>, i64, i64);

fn parse_transfer_id(s: &str) -> ApiResult<[u8; 16]> {
    wire::uuid_parse(s)
        .ok_or_else(|| ApiError::field("/transfer_id", "expected a lowercase hyphenated UUID"))
}

/// `POST /v1/transfers`.
pub async fn post(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DeviceTransfers])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let transfer_id = o.uuid("transfer_id", true);
    let manifest = o.b64("manifest", true);
    let max_blobs = usize::try_from(st.cfg.max_blobs_per_transfer).unwrap_or(usize::MAX);
    let blob_ids = o.uuid_list("blob_ids", true, max_blobs);
    if blob_ids.as_ref().is_some_and(Vec::is_empty) {
        o.error("blob_ids", "at least one blob is required");
    }
    // Unknown members of the body (`include_secrets`...) are ignored: there is no partial transfer.
    o.finish()?;
    let (Some(transfer_id), Some(manifest), Some(blob_ids)) = (transfer_id, manifest, blob_ids)
    else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let dev = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let (cfg, now) = (st.cfg.clone(), ctx.now);
    let (status, v, fx) = st
        .reserve(&space, move |tx, budget| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &dev)?;
            let sp = store::require_space(tx)?;
            let set_hash = blob_set_hash(&blob_ids).to_vec();

            // 1. Idempotence: same caller, same manifest bytes, same set of blobs.
            // Row: (from_device, manifest, blob_set_hash, seq, expires_at).
            let existing: Option<ExistingTransfer> = tx
                .query_row(
                    "SELECT from_device, manifest, blob_set_hash, seq, expires_at FROM transfers WHERE transfer_id = ?1",
                    params![transfer_id.to_vec()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            if let Some((from, old_manifest, old_set, seq, expires_at)) = existing {
                if from == dev.device_id && old_manifest == manifest && old_set == set_hash {
                    return Ok((
                        StatusCode::OK,
                        json!({ "transfer_id": uuid_format(&transfer_id), "seq": seq, "expires_at": ts_format(expires_at) }),
                        fx,
                    ));
                }
                return Err(ApiError::new(Code::TransferIdConflict)
                    .detail("This transfer_id is used by another manifest or set of blobs."));
            }

            // 2. The manifest: decoded and verified by tomi-core.
            let bad = |why: &str| ApiError::field("/manifest", why);
            let m = HistoryTransfer::from_bytes(&manifest).map_err(|_| bad("not a history transfer manifest"))?;
            if wire::uuid_parse(&m.space_id) != Some(sp.space_id) {
                return Err(bad("space_id differs from the space"));
            }
            if wire::uuid_parse(&m.transfer_id) != Some(transfer_id) {
                return Err(bad("transfer_id differs from the request"));
            }
            if m.sender_ed25519 != dev.sign_pk {
                return Err(bad("the sender is not the calling device"));
            }
            m.verify_signature().map_err(|_| bad("invalid signature"))?;
            let recipient = member_by_sign_pk(tx, &m.recipient_ed25519)?
                .filter(|r| r.device_id != dev.device_id)
                .ok_or_else(|| bad("the recipient is not another member"))?;
            let as_of = i64::try_from(m.as_of_seq).map_err(|_| bad("as_of_seq out of range"))?;
            if as_of < recipient.joined_seq || as_of > sp.last_seq {
                return Err(bad("as_of_seq is outside [joined_seq of the recipient, head_seq]"));
            }
            for part in &m.parts {
                let id = wire::uuid_parse(&part.blob_id).ok_or_else(|| bad("invalid part id"))?;
                if !blob_ids.contains(&id) {
                    return Err(bad("a part of the manifest is missing from blob_ids"));
                }
            }

            // 3. The blobs: mine, complete, free; the total within the limit.
            let mut sizes: Vec<i64> = Vec::with_capacity(blob_ids.len());
            let mut total: u64 = 0;
            for b in &blob_ids {
                let row: Option<(String, i64, i64, i64, bool)> = tx
                    .query_row(
                        "SELECT owner, size, complete, upload_offset, item_id IS NOT NULL OR transfer_id IS NOT NULL
                         FROM blobs WHERE blob_id = ?1",
                        params![b.to_vec()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)? != 0)),
                    )
                    .optional()?;
                let Some((owner, size, complete, offset, attached)) = row else {
                    return Err(ApiError::not_found());
                };
                if owner != dev.device_id {
                    return Err(ApiError::not_found());
                }
                if complete == 0 {
                    return Err(ApiError::new(Code::BlobIncomplete)
                        .detail("A blob of the transfer is not completely uploaded.")
                        .with("upload_offset", offset));
                }
                if attached {
                    return Err(ApiError::new(Code::BlobAttached).detail("A blob of the transfer is already attached."));
                }
                total = total.saturating_add(u64::try_from(size).unwrap_or(0));
                sizes.push(size);
            }
            if total > cfg.max_transfer_bytes {
                return Err(ApiError::payload_too_large(cfg.max_transfer_bytes));
            }

            // 4. Too many transfers already open towards this recipient.
            let open: i64 = tx.query_row(
                "SELECT count(*) FROM transfers WHERE to_device = ?1 AND closed_at IS NULL AND expires_at > ?2",
                params![recipient.device_id, now],
                |r| r.get(0),
            )?;
            if u64::try_from(open).unwrap_or(u64::MAX) >= cfg.transfer_max_open {
                return Err(ApiError::new(Code::TransferLimit)
                    .detail("Too many transfers are open towards this device.")
                    .with("transfer_max_open", cfg.transfer_max_open));
            }

            // 5. The row itself must fit (the blobs have been counted since their creation).
            check_quota(tx, budget, manifest.len() as u64)?;

            let expires_at = now.saturating_add(i64::try_from(cfg.transfer_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX));
            let seq = next_seq(tx, now)?;
            tx.execute(
                "INSERT INTO transfers (transfer_id, from_device, to_device, manifest, blob_set_hash, as_of_seq, seq, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    transfer_id.to_vec(),
                    dev.device_id,
                    recipient.device_id,
                    manifest,
                    set_hash,
                    as_of,
                    seq,
                    now,
                    expires_at
                ],
            )?;
            for (pos, b) in blob_ids.iter().enumerate() {
                tx.execute(
                    "UPDATE blobs SET transfer_id = ?2, pos = ?3 WHERE blob_id = ?1",
                    params![b.to_vec(), transfer_id.to_vec(), pos as i64],
                )?;
            }
            insert_event(
                tx,
                seq,
                now,
                "history_transfer",
                Some(&dev.device_id),
                None,
                &json!({
                    "transfer_id": uuid_format(&transfer_id),
                    "from_device_id": dev.device_id,
                    "to_device_id": recipient.device_id,
                    "total_bytes": sizes.iter().map(|s| u64::try_from(*s).unwrap_or(0)).sum::<u64>(),
                    "expires_at": ts_format(expires_at),
                }),
            )?;
            // Only the recipient is rung.
            fx.wakes.push(WakeEvent {
                seq,
                fcm: true,
                targets: vec![Target::quiet(&recipient.device_id)],
            });
            Ok((
                StatusCode::CREATED,
                json!({ "transfer_id": uuid_format(&transfer_id), "seq": seq, "expires_at": ts_format(expires_at) }),
                fx,
            ))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(status, v))
}

/// `GET /v1/transfers/{transfer_id}`: the sender and the recipient of an open transfer.
pub async fn get(
    State(st): State<AppState>,
    Path(transfer_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let id = parse_transfer_id(&transfer_id)?;
    let me = ctx.dev()?.device_id.clone();
    let now = ctx.now;
    let v = ctx
        .space()?
        .db
        .read(move |tx| {
            let row = tx
                .query_row(
                    "SELECT from_device, to_device, manifest, created_at, expires_at, seq
                     FROM transfers WHERE transfer_id = ?1 AND closed_at IS NULL AND expires_at > ?2",
                    params![id.to_vec(), now],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((from, to, manifest, created, expires, seq)) = row else {
                return Err(ApiError::not_found());
            };
            if me != from && me != to {
                return Err(ApiError::not_found());
            }
            let mut st = tx.prepare("SELECT blob_id, size FROM blobs WHERE transfer_id = ?1 ORDER BY pos")?;
            let blobs = st
                .query_map(params![id.to_vec()], |r| {
                    Ok(json!({ "blob_id": uuid_format(&r.get::<_, Vec<u8>>(0)?), "size": r.get::<_, i64>(1)? }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({
                "transfer_id": uuid_format(&id),
                "from_device_id": from,
                "to_device_id": to,
                "manifest": b64_encode(&manifest),
                "blobs": blobs,
                "created_at": ts_format(created),
                "expires_at": ts_format(expires),
                "seq": seq,
            }))
        })
        .await?;
    Ok(json(StatusCode::OK, v))
}

/// `DELETE /v1/transfers/{transfer_id}`: the sender cancels, the recipient is done. Idempotent.
pub async fn delete(
    State(st): State<AppState>,
    Path(transfer_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, false).await?;
    let outcome = o.string("outcome", false);
    if let Some(s) = &outcome {
        if !(1..=32).contains(&s.len()) || !s.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            o.error("outcome", "expected 1 to 32 printable ASCII characters");
        }
    }
    let id = parse_transfer_id(&transfer_id);
    o.finish()?;
    let id = id?;
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let fx = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            store::require_space(tx)?;
            let row: Option<(String, String, Option<i64>, i64)> = tx
                .query_row(
                    "SELECT from_device, to_device, closed_at, expires_at FROM transfers WHERE transfer_id = ?1",
                    params![id.to_vec()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((from, to, closed_at, expires_at)) = row else {
                return Err(ApiError::not_found());
            };
            if me != from && me != to {
                return Err(ApiError::not_found());
            }
            // Idempotent: a transfer already closed, or expired and awaiting the purge, returns
            // 204 with no event (§4.13). Do not attribute a close to the caller in that case; the
            // purge closes expired transfers itself (by = relay, outcome "expired").
            if closed_at.is_none() && expires_at > now {
                // The sender cancels by default, the recipient reports the import.
                let outcome = outcome
                    .unwrap_or_else(|| if me == from { "cancelled" } else { "imported" }.to_owned());
                close_transfer(tx, now, &id, Some(&me), &outcome, &mut fx)?;
            }
            Ok(fx)
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(empty(StatusCode::NO_CONTENT))
}
