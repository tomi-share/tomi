//! Items (`api-v1.md` §4.8): the envelope the relay stores and forwards without reading it.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tomi_core::{read_item_state_header, AttachmentTombstone, CoreError, Tombstone};

use super::json;
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Gate, Rl};
use crate::input::json_body;
use crate::purge::purge_item;
use crate::push::Urgency;
use crate::state::{AppState, Effects, Target, WakeEvent};
use crate::store::{self, add_event, check_quota, insert_event, members, next_seq};
use crate::wire::{self, b64_encode, sha256, sha256_parts, ts_format, uuid_format};

/// Hash of a set of blob ids, order independent (idempotence, §4.8).
pub fn blob_set_hash(ids: &[[u8; 16]]) -> [u8; 32] {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    let parts: Vec<&[u8]> = sorted.iter().map(<[u8; 16]>::as_slice).collect();
    sha256_parts(&parts)
}

/// The recipients of a new item and whether each one rings (§6.1): everybody rings when
/// nobody (but the sender) is present, otherwise only the present devices ring.
fn ring_decisions(tx: &Connection, me: &str, now: i64) -> ApiResult<Vec<(String, bool)>> {
    let others: Vec<_> = members(tx)?
        .into_iter()
        .filter(|m| m.device_id != me)
        .collect();
    let anyone_present = others.iter().any(|m| m.is_present(now));
    Ok(others
        .into_iter()
        .map(|m| {
            let ring = !anyone_present || m.is_present(now);
            (m.device_id, ring)
        })
        .collect())
}

/// `POST /v1/items`.
pub async fn post(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DeviceItems])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let item_id = o.uuid("item_id", true);
    let expires_at = o.timestamp("expires_at", true);
    let key_epoch = o.uint("key_epoch", true, 1, u64::from(u32::MAX));
    let envelope = o.b64("envelope", true);
    let blob_ids = o
        .uuid_list(
            "blob_ids",
            false,
            usize::try_from(st.cfg.max_blobs_per_item).unwrap_or(usize::MAX),
        )
        .unwrap_or_default();
    if let Some(e) = &envelope {
        if e.len() as u64 > st.cfg.max_envelope_bytes {
            return Err(ApiError::payload_too_large(st.cfg.max_envelope_bytes));
        }
        if e.is_empty() {
            o.error("envelope", "must not be empty");
        }
    }
    // The clear header of a format-1 envelope must name the item and the epoch (§4.8).
    if let (Some(e), Some(id), Some(epoch)) = (&envelope, item_id, key_epoch) {
        if e.len() < 4 {
            o.error("envelope", "shorter than 4 bytes");
        } else if e[..4] == 1u32.to_be_bytes() {
            let ok = e.len() >= 24
                && e[4..8] == u32::try_from(epoch).unwrap_or(u32::MAX).to_be_bytes()
                && e[8..24] == id;
            if !ok {
                o.error(
                    "envelope",
                    "the header does not match item_id and key_epoch",
                );
            }
        }
    }
    o.finish()?;
    let (Some(item_id), Some(expires_at), Some(key_epoch), Some(envelope)) =
        (item_id, expires_at, key_epoch, envelope)
    else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let key_epoch = u32::try_from(key_epoch).unwrap_or(u32::MAX);
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let (cfg, now) = (st.cfg.clone(), ctx.now);

    let (status, response, fx) = st
        .reserve(&space, move |tx, budget| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            let sp = store::require_space(tx)?;
            let env_hash = sha256(&envelope).to_vec();
            let set_hash = blob_set_hash(&blob_ids).to_vec();

            // Idempotence comes before every other business check.
            let existing = tx
                .query_row(
                    "SELECT seq, sender, created_at, expires_at, key_epoch, envelope_hash, blob_set_hash, purged_at
                     FROM items WHERE item_id = ?1",
                    params![item_id.to_vec()],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, Vec<u8>>(5)?,
                            r.get::<_, Vec<u8>>(6)?,
                            r.get::<_, Option<i64>>(7)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((seq, sender, created_at, old_expires, old_epoch, old_env, old_set, purged_at)) = existing {
                let same = sender == me
                    && old_expires == expires_at
                    && old_epoch == i64::from(key_epoch)
                    && old_env == env_hash
                    && old_set == set_hash;
                if !same {
                    return Err(ApiError::new(Code::ItemIdConflict).detail("This item_id is used by another item."));
                }
                let mut v = json!({
                    "item_id": uuid_format(&item_id),
                    "seq": seq,
                    "created_at": ts_format(created_at),
                    "expires_at": ts_format(old_expires),
                });
                if purged_at.is_some() {
                    v["purged"] = json!(true);
                }
                return Ok((StatusCode::OK, v, fx));
            }

            // Retention deadline (§4.8).
            if expires_at <= now {
                return Err(ApiError::new(Code::AlreadyExpired)
                    .detail("expires_at is in the past.")
                    .with("server_time", ts_format(now)));
            }
            let horizon = now.saturating_add(i64::try_from(cfg.max_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX));
            if expires_at > horizon {
                return Err(ApiError::field("/expires_at", "beyond now + max_ttl_secs"));
            }

            // The epoch is checked in the transaction that assigns the seq (§4.8): no item of
            // epoch e can get a seq above the rotation e -> e+1.
            if key_epoch != sp.key_epoch {
                return Err(ApiError::new(Code::StaleKeyEpoch)
                    .detail("The item is encrypted with an epoch that is no longer current.")
                    .with("key_epoch", sp.key_epoch));
            }

            // Blobs: mine, complete, not attached; total size within the item limit.
            let mut blobs: Vec<(String, i64)> = Vec::with_capacity(blob_ids.len());
            let mut total: u64 = 0;
            for b in &blob_ids {
                let row = tx
                    .query_row(
                        "SELECT owner, size, complete, upload_offset, item_id IS NOT NULL OR transfer_id IS NOT NULL
                         FROM blobs WHERE blob_id = ?1",
                        params![b.to_vec()],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, i64>(2)?,
                                r.get::<_, i64>(3)?,
                                r.get::<_, i64>(4)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((owner, size, complete, offset, attached)) = row else {
                    return Err(ApiError::not_found());
                };
                if owner != me {
                    return Err(ApiError::not_found());
                }
                if complete == 0 {
                    return Err(ApiError::new(Code::BlobIncomplete)
                        .detail("A referenced blob is not completely uploaded.")
                        .with("upload_offset", offset));
                }
                if attached != 0 {
                    return Err(ApiError::new(Code::BlobAttached).detail("A referenced blob is already attached."));
                }
                total = total.saturating_add(u64::try_from(size).unwrap_or(0));
                blobs.push((uuid_format(b), size));
            }
            if total > cfg.max_item_blob_bytes {
                return Err(ApiError::payload_too_large(cfg.max_item_blob_bytes));
            }
            check_quota(tx, budget, envelope.len() as u64)?;

            // Write: item, rings, blobs, event.
            let rings = ring_decisions(tx, &me, now)?;
            let seq = next_seq(tx, now)?;
            tx.execute(
                "INSERT INTO items (item_id, seq, sender, created_at, expires_at, key_epoch, envelope, envelope_size, envelope_hash, blob_set_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    item_id.to_vec(),
                    seq,
                    me,
                    now,
                    expires_at,
                    key_epoch,
                    envelope,
                    envelope.len() as i64,
                    env_hash,
                    set_hash
                ],
            )?;
            for (dev, ring) in &rings {
                tx.execute(
                    "INSERT INTO item_rings (item_id, device_id, ring) VALUES (?1, ?2, ?3)",
                    params![item_id.to_vec(), dev, i64::from(*ring)],
                )?;
            }
            for (pos, b) in blob_ids.iter().enumerate() {
                tx.execute(
                    "UPDATE blobs SET item_id = ?2, pos = ?3 WHERE blob_id = ?1",
                    params![b.to_vec(), item_id.to_vec(), pos as i64],
                )?;
            }
            let blob_list: Vec<Value> = blobs.iter().map(|(id, size)| json!({ "blob_id": id, "size": size })).collect();
            insert_event(
                tx,
                seq,
                now,
                "item",
                Some(&me),
                Some(&item_id),
                &json!({
                    "item_id": uuid_format(&item_id),
                    "sender_device_id": me,
                    "created_at": ts_format(now),
                    "expires_at": ts_format(expires_at),
                    "key_epoch": key_epoch,
                    "blobs": blob_list,
                }),
            )?;
            fx.wakes.push(WakeEvent {
                seq,
                fcm: true,
                targets: rings
                    .into_iter()
                    .map(|(device, ring)| Target {
                        device,
                        ring,
                        urgency: if ring { Urgency::High } else { Urgency::Normal },
                    })
                    .collect(),
            });
            Ok((
                StatusCode::CREATED,
                json!({
                    "item_id": uuid_format(&item_id),
                    "seq": seq,
                    "created_at": ts_format(now),
                    "expires_at": ts_format(expires_at),
                }),
                fx,
            ))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(status, response))
}

fn parse_item_id(s: &str) -> ApiResult<[u8; 16]> {
    wire::uuid_parse(s)
        .ok_or_else(|| ApiError::field("/item_id", "expected a lowercase hyphenated UUID"))
}

/// `GET /v1/items/{item_id}`: visible to the sender and to members that were there before it.
pub async fn get(
    State(st): State<AppState>,
    Path(item_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let id = parse_item_id(&item_id)?;
    let dev = ctx.dev()?.clone();
    let v = ctx
        .space()?
        .db
        .read(move |tx| {
            let row = tx
                .query_row(
                    "SELECT seq, sender, created_at, expires_at, key_epoch, envelope, envelope_size
                     FROM items WHERE item_id = ?1 AND purged_at IS NULL",
                    params![id.to_vec()],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, Option<Vec<u8>>>(5)?,
                            r.get::<_, i64>(6)?,
                        ))
                    },
                )
                .optional()?;
            let Some((seq, sender, created_at, expires_at, epoch, envelope, size)) = row else {
                return Err(ApiError::not_found());
            };
            if sender != dev.device_id && dev.joined_seq >= seq {
                return Err(ApiError::not_found());
            }
            let mut st = tx.prepare("SELECT blob_id, size FROM blobs WHERE item_id = ?1 ORDER BY pos")?;
            let blobs = st
                .query_map(params![id.to_vec()], |r| {
                    Ok(json!({ "blob_id": uuid_format(&r.get::<_, Vec<u8>>(0)?), "size": r.get::<_, i64>(1)? }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut st = tx.prepare(
                "SELECT device_id, received_at, read_at FROM receipts WHERE item_id = ?1 ORDER BY received_at, device_id",
            )?;
            let receipts = st
                .query_map(params![id.to_vec()], |r| {
                    let mut m = json!({
                        "device_id": r.get::<_, String>(0)?,
                        "received_at": ts_format(r.get(1)?),
                    });
                    if let Some(t) = r.get::<_, Option<i64>>(2)? {
                        m["read_at"] = json!(ts_format(t));
                    }
                    Ok(m)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!({
                "item_id": uuid_format(&id),
                "seq": seq,
                "sender_device_id": sender,
                "created_at": ts_format(created_at),
                "expires_at": ts_format(expires_at),
                "key_epoch": epoch,
                "envelope": b64_encode(&envelope.unwrap_or_default()),
                "envelope_size": size,
                "blobs": blobs,
                "receipts": receipts,
            }))
        })
        .await?;
    Ok(json(StatusCode::OK, v))
}

/// What `ack` and `read` need to know about an item.
struct Visible {
    seq: i64,
    sender: String,
    purged: bool,
}

/// The item for `ack`/`read`: the row exists (purged or not) and the caller sent it or was a
/// member before it; otherwise `404`.
fn visible_item(tx: &Connection, id: &[u8; 16], me: &str, joined_seq: i64) -> ApiResult<Visible> {
    let row = tx
        .query_row(
            "SELECT seq, sender, purged_at FROM items WHERE item_id = ?1",
            params![id.to_vec()],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((seq, sender, purged_at)) = row else {
        return Err(ApiError::not_found());
    };
    if sender != me && joined_seq >= seq {
        return Err(ApiError::not_found());
    }
    Ok(Visible {
        seq,
        sender,
        purged: purged_at.is_some(),
    })
}

/// Wakes the sender of the item (an `ack` goes to nobody else, §4.10).
fn wake_sender(fx: &mut Effects, seq: i64, sender: &str, author: &str) {
    if sender != author {
        fx.wakes.push(WakeEvent {
            seq,
            fcm: false,
            targets: vec![Target::quiet(sender)],
        });
    }
}

/// `POST /v1/items/{item_id}/ack`.
pub async fn ack(
    State(st): State<AppState>,
    Path(item_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, false).await?;
    let release = o.boolean("release", false).unwrap_or(false);
    let id = parse_item_id(&item_id);
    o.finish()?;
    let id = id?;
    let dev = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let (v, fx) = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &dev)?;
            store::require_space(tx)?;
            let item = visible_item(tx, &id, &dev.device_id, dev.joined_seq)?;
            let receipt: Option<(i64, Option<i64>)> = tx
                .query_row(
                    "SELECT received_at, released_at FROM receipts WHERE item_id = ?1 AND device_id = ?2",
                    params![id.to_vec(), dev.device_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let mut v = json!({});
            if item.sender == dev.device_id {
                v["released"] = json!(false);
            } else if item.purged {
                // No effect, no event: report what this device had done before the purge.
                if let Some((received, released)) = receipt {
                    v["received_at"] = json!(ts_format(received));
                    v["released"] = json!(released.is_some());
                } else {
                    v["released"] = json!(false);
                }
            } else {
                let (received, released) = match receipt {
                    None => {
                        tx.execute(
                            "INSERT INTO receipts (item_id, device_id, received_at, released_at) VALUES (?1, ?2, ?3, ?4)",
                            params![id.to_vec(), dev.device_id, now, release.then_some(now)],
                        )?;
                        let seq = add_event(
                            tx,
                            now,
                            "ack",
                            Some(&dev.device_id),
                            Some(&id),
                            &json!({
                                "item_id": uuid_format(&id),
                                "device_id": dev.device_id,
                                "received_at": ts_format(now),
                            }),
                        )?;
                        wake_sender(&mut fx, seq, &item.sender, &dev.device_id);
                        (now, release.then_some(now))
                    }
                    Some((received, released)) => {
                        // A release is never undone and keeps its first date.
                        if release && released.is_none() {
                            tx.execute(
                                "UPDATE receipts SET released_at = ?3 WHERE item_id = ?1 AND device_id = ?2",
                                params![id.to_vec(), dev.device_id, now],
                            )?;
                            (received, Some(now))
                        } else {
                            (received, released)
                        }
                    }
                };
                v["received_at"] = json!(ts_format(received));
                v["released"] = json!(released.is_some());
            }
            if item.purged {
                v["purged"] = json!(true);
            }
            let _ = item.seq;
            Ok((v, fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(StatusCode::OK, v))
}

/// `POST /v1/items/{item_id}/read`.
pub async fn read(
    State(st): State<AppState>,
    Path(item_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, false).await?;
    let id = parse_item_id(&item_id);
    o.finish()?;
    let id = id?;
    let dev = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let (v, fx) = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &dev)?;
            store::require_space(tx)?;
            let item = visible_item(tx, &id, &dev.device_id, dev.joined_seq)?;
            let receipt: Option<Option<i64>> = tx
                .query_row(
                    "SELECT read_at FROM receipts WHERE item_id = ?1 AND device_id = ?2",
                    params![id.to_vec(), dev.device_id],
                    |r| r.get(0),
                )
                .optional()?;
            let mut v = json!({});
            if item.sender == dev.device_id {
                // The sender's own copy: nothing to record.
            } else if item.purged {
                if let Some(Some(t)) = receipt {
                    v["read_at"] = json!(ts_format(t));
                }
            } else if let Some(Some(t)) = receipt {
                v["read_at"] = json!(ts_format(t));
            } else {
                if receipt.is_none() {
                    tx.execute(
                        "INSERT INTO receipts (item_id, device_id, received_at, read_at) VALUES (?1, ?2, ?3, ?3)",
                        params![id.to_vec(), dev.device_id, now],
                    )?;
                    let seq = add_event(
                        tx,
                        now,
                        "ack",
                        Some(&dev.device_id),
                        Some(&id),
                        &json!({
                            "item_id": uuid_format(&id),
                            "device_id": dev.device_id,
                            "received_at": ts_format(now),
                        }),
                    )?;
                    wake_sender(&mut fx, seq, &item.sender, &dev.device_id);
                } else {
                    tx.execute(
                        "UPDATE receipts SET read_at = ?3 WHERE item_id = ?1 AND device_id = ?2",
                        params![id.to_vec(), dev.device_id, now],
                    )?;
                }
                let seq = add_event(
                    tx,
                    now,
                    "read",
                    Some(&dev.device_id),
                    Some(&id),
                    &json!({
                        "item_id": uuid_format(&id),
                        "device_id": dev.device_id,
                        "read_at": ts_format(now),
                    }),
                )?;
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
                v["read_at"] = json!(ts_format(now));
            }
            if item.purged {
                v["purged"] = json!(true);
            }
            Ok((v, fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(StatusCode::OK, v))
}

/// `POST /v1/tombstones` (§4.8): delete everywhere.
pub async fn tombstone(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DeviceTombstones])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let statement = o.b64("tombstone", true);
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
            // The statement is decoded and verified by tomi-core.
            let bad = |why: &str| ApiError::field("/tombstone", why);
            let t = Tombstone::from_bytes(&statement).map_err(|_| bad("not a tombstone"))?;
            if wire::uuid_parse(&t.space_id) != Some(sp.space_id) {
                return Err(bad("space_id differs from the space"));
            }
            if t.scope != tomi_core::DeletionScope::Everywhere {
                return Err(bad("only the scope \"everywhere\" is sent to the relay"));
            }
            if t.signer_ed25519 != dev.sign_pk {
                return Err(bad("the signer is not the calling device"));
            }
            t.verify_signature().map_err(|_| bad("invalid signature"))?;
            let deleted_at = i64::try_from(t.deleted_at_ms).ok().filter(|ms| (0..=wire::TS_MAX_MS).contains(ms));
            let deleted_at = deleted_at.ok_or_else(|| bad("deleted_at out of range"))?;
            let item_id = wire::uuid_parse(&t.item_id).ok_or_else(|| bad("invalid item id"))?;

            // Already known (from any device): the same answer, no new event.
            let known: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM tombstones WHERE item_id = ?1",
                    params![item_id.to_vec()],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(seq) = known {
                return Ok((StatusCode::OK, json!({ "seq": seq, "item_id": uuid_format(&item_id) }), fx));
            }
            // Accepted for any item id, known or not: copies may outlive the relay's.
            purge_item(tx, now, &item_id, &mut fx)?;
            let seq = next_seq(tx, now)?;
            tx.execute(
                "INSERT INTO tombstones (item_id, seq, device_id, deleted_at, statement) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![item_id.to_vec(), seq, dev.device_id, deleted_at, statement],
            )?;
            insert_event(
                tx,
                seq,
                now,
                "tombstone",
                Some(&dev.device_id),
                Some(&item_id),
                &json!({
                    "item_id": uuid_format(&item_id),
                    "device_id": dev.device_id,
                    "deleted_at": ts_format(deleted_at),
                    "tombstone": b64_encode(&statement),
                }),
            )?;
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
            Ok((StatusCode::CREATED, json!({ "seq": seq, "item_id": uuid_format(&item_id) }), fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(status, v))
}

/// `POST /v1/attachment-tombstones` (§4.8 bis): remove ONE attachment of an item, everywhere.
///
/// Same checks as [`tombstone`] (decoding, space, signer = caller, signature, caller still a
/// member). The relay stays blind: it reads `item_id`, `slot`, `blob_id` and the time from the
/// signed statement and deletes the blob only when it is the one it holds for that item at that
/// position. Accepted for an unknown or purged item, and for an item already tombstoned (the
/// clients ignore the event then): there is no special case.
pub async fn attachment_tombstone(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DeviceTombstones])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let statement = o.b64("attachment_tombstone", true);
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
            let bad = |why: &str| ApiError::field("/attachment_tombstone", why);
            let t = AttachmentTombstone::from_bytes(&statement)
                .map_err(|_| bad("not an attachment tombstone"))?;
            if wire::uuid_parse(&t.space_id) != Some(sp.space_id) {
                return Err(bad("space_id differs from the space"));
            }
            if t.signer_ed25519 != dev.sign_pk {
                return Err(bad("the signer is not the calling device"));
            }
            t.verify_signature().map_err(|_| bad("invalid signature"))?;
            let deleted_at = i64::try_from(t.deleted_at_ms)
                .ok()
                .filter(|ms| (0..=wire::TS_MAX_MS).contains(ms))
                .ok_or_else(|| bad("deleted_at out of range"))?;
            let item_id = wire::uuid_parse(&t.item_id).ok_or_else(|| bad("invalid item id"))?;
            let blob_id = wire::uuid_parse(&t.blob_id).ok_or_else(|| bad("invalid blob id"))?;
            let slot = i64::from(t.slot);
            let reply = |seq: i64| {
                json!({ "seq": seq, "item_id": uuid_format(&item_id), "slot": t.slot })
            };

            // Already known (from any device): the same answer, no new event.
            let known: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM attachment_tombstones WHERE item_id = ?1 AND slot = ?2",
                    params![item_id.to_vec(), slot],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(seq) = known {
                return Ok((StatusCode::OK, reply(seq), fx));
            }

            // The blob goes only if it is the one this item holds at this position. A blob
            // that is attached elsewhere, or another blob at this position, is a statement
            // that does not describe what the relay holds: refused, nothing changes.
            let at_slot: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT blob_id FROM blobs WHERE item_id = ?1 AND pos = ?2",
                    params![item_id.to_vec(), slot],
                    |r| r.get(0),
                )
                .optional()?;
            if at_slot.as_deref().is_some_and(|b| b != blob_id.as_slice()) {
                return Err(bad("another blob is attached at this position"));
            }
            let held: Option<(Option<Vec<u8>>, i64)> = tx
                .query_row(
                    "SELECT item_id, pos FROM blobs WHERE blob_id = ?1",
                    params![blob_id.to_vec()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match held {
                // Absent (already purged, never uploaded): nothing to delete, the event goes out.
                None => {}
                Some((Some(owner), pos)) if owner == item_id.to_vec() && pos == slot => {
                    tx.execute(
                        "DELETE FROM blobs WHERE blob_id = ?1",
                        params![blob_id.to_vec()],
                    )?;
                    fx.unlink_blobs.push(blob_id);
                }
                Some(_) => return Err(bad("the blob is attached elsewhere")),
            }

            let seq = next_seq(tx, now)?;
            tx.execute(
                "INSERT INTO attachment_tombstones (item_id, slot, blob_id, seq, device_id, deleted_at, statement)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    item_id.to_vec(),
                    slot,
                    blob_id.to_vec(),
                    seq,
                    dev.device_id,
                    deleted_at,
                    statement
                ],
            )?;
            insert_event(
                tx,
                seq,
                now,
                "attachment_tombstone",
                Some(&dev.device_id),
                Some(&item_id),
                &json!({
                    "item_id": uuid_format(&item_id),
                    "slot": t.slot,
                    "blob_id": uuid_format(&blob_id),
                    "device_id": dev.device_id,
                    "deleted_at": ts_format(deleted_at),
                    "statement": b64_encode(&statement),
                }),
            )?;
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
            Ok((StatusCode::CREATED, reply(seq), fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(status, v))
}

/// `POST /v1/item-states` (§4.8): an encrypted state event (the pin). The relay sees a random
/// `event_id`, an epoch and a constant size; never the item and never the direction.
pub async fn item_state(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device, Rl::DeviceItemStates])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let event_id = o.uuid("event_id", true);
    let key_epoch = o.uint("key_epoch", true, 1, u64::from(u32::MAX));
    let envelope = o.b64("envelope", true);
    if let Some(e) = &envelope {
        if e.len() as u64 > st.cfg.max_item_state_bytes {
            return Err(ApiError::payload_too_large(st.cfg.max_item_state_bytes));
        }
    }
    // Same length rule as the clients, through tomi-core (§4.8): a format-1 envelope of an
    // impossible size would be rejected by every client, so it never enters the log.
    if let (Some(e), Some(id), Some(epoch)) = (&envelope, event_id, key_epoch) {
        match read_item_state_header(e) {
            Ok(h) => {
                let same = h.key_epoch == u32::try_from(epoch).unwrap_or(u32::MAX)
                    && h.event_id == uuid_format(&id);
                if !same {
                    o.error(
                        "envelope",
                        "the header does not match event_id and key_epoch",
                    );
                }
            }
            Err(CoreError::UnsupportedFormat { .. }) => {}
            Err(_) => o.error("envelope", "malformed item state envelope"),
        }
    }
    o.finish()?;
    let (Some(event_id), Some(key_epoch), Some(envelope)) = (event_id, key_epoch, envelope) else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let key_epoch = u32::try_from(key_epoch).unwrap_or(u32::MAX);
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let (status, v, fx) = st
        .reserve(&space, move |tx, budget| {
            let mut fx = Effects::default();
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            let sp = store::require_space(tx)?;
            let hash = sha256(&envelope).to_vec();
            let existing: Option<(i64, String, i64, Vec<u8>)> = tx
                .query_row(
                    "SELECT seq, device_id, key_epoch, envelope_hash FROM item_states WHERE event_id = ?1",
                    params![event_id.to_vec()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if let Some((seq, device, epoch, old_hash)) = existing {
                if device == me && epoch == i64::from(key_epoch) && old_hash == hash {
                    return Ok((StatusCode::OK, json!({ "event_id": uuid_format(&event_id), "seq": seq }), fx));
                }
                return Err(ApiError::new(Code::EventIdConflict).detail("This event_id is used by another event."));
            }
            if key_epoch != sp.key_epoch {
                return Err(ApiError::new(Code::StaleKeyEpoch)
                    .detail("The event is sealed under an epoch that is no longer current.")
                    .with("key_epoch", sp.key_epoch));
            }
            check_quota(tx, budget, envelope.len() as u64)?;
            let seq = next_seq(tx, now)?;
            tx.execute(
                "INSERT INTO item_states (event_id, seq, device_id, key_epoch, envelope_size, envelope_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![event_id.to_vec(), seq, me, key_epoch, envelope.len() as i64, hash],
            )?;
            // No item id anywhere: `events.item_id` stays NULL.
            insert_event(
                tx,
                seq,
                now,
                "item_state",
                Some(&me),
                None,
                &json!({
                    "event_id": uuid_format(&event_id),
                    "device_id": me,
                    "key_epoch": key_epoch,
                    "envelope": b64_encode(&envelope),
                    "envelope_size": envelope.len(),
                }),
            )?;
            let targets = members(tx)?
                .into_iter()
                .filter(|m| m.device_id != me)
                .map(|m| Target::quiet(&m.device_id))
                .collect();
            fx.wakes.push(WakeEvent {
                seq,
                fcm: true,
                targets,
            });
            Ok((StatusCode::CREATED, json!({ "event_id": uuid_format(&event_id), "seq": seq }), fx))
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(json(status, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_set_hash_ignores_order() {
        let (a, b) = ([1u8; 16], [2u8; 16]);
        assert_eq!(blob_set_hash(&[a, b]), blob_set_hash(&[b, a]));
        assert_ne!(blob_set_hash(&[a]), blob_set_hash(&[a, b]));
        assert_eq!(blob_set_hash(&[]), sha256(&[]));
    }
}
