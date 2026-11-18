//! The event log (`api-v1.md` §4.10): `GET /v1/events?after=<seq>&limit=<n>`.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use rusqlite::params;
use serde_json::{json, Value};

use super::json;
use crate::error::{ApiError, ApiResult, Code, FieldError};
use crate::gate::{gate, Gate, Rl};
use crate::input::query_uint;
use crate::state::AppState;
use crate::store;
use crate::wire::{b64_encode, ts_format};

/// A page stops before an `item` event that would bring the envelopes above this (§4.10).
pub const PAGE_ENVELOPE_BYTES: i64 = 2_097_152;

/// `GET /v1/events`.
pub async fn list(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut errs: Vec<FieldError> = Vec::new();
    let after = match query_uint(&ctx.parts, "after", 0, u64::MAX) {
        Ok(Some(n)) => Some(n),
        Ok(None) => {
            errs.push(FieldError {
                field: "/after".into(),
                reason: "required".into(),
            });
            None
        }
        Err(e) => {
            errs.push(e);
            None
        }
    };
    let limit = match query_uint(&ctx.parts, "limit", 1, 500) {
        Ok(n) => n.unwrap_or(100),
        Err(e) => {
            errs.push(e);
            100
        }
    };
    if !errs.is_empty() {
        return Err(ApiError::validation(errs));
    }
    let after = i64::try_from(after.unwrap_or(0)).unwrap_or(i64::MAX);
    let dev = ctx.dev()?.clone();
    let me = dev.device_id.clone();
    let limit = usize::try_from(limit).unwrap_or(100);

    let space = ctx.space()?.clone();
    let page = space
        .db
        .read(move |tx| {
            let sp = store::require_space(tx)?;
            if after > sp.last_seq {
                return Err(ApiError::new(Code::CursorAhead)
                    .detail("The cursor is ahead of the log (the relay was restored from a backup).")
                    .with("head_seq", sp.last_seq));
            }
            // A new device does not see what precedes its arrival (§4.10).
            let from = after.max(dev.joined_seq);
            if from < sp.retention_floor_seq {
                return Err(ApiError::new(Code::CursorExpired)
                    .detail("The cursor is older than the compaction horizon.")
                    .with("retention_floor_seq", sp.retention_floor_seq));
            }
            let mut stmt = tx.prepare(
                "SELECT e.seq, e.type, e.at, e.data, i.envelope, i.envelope_size, i.sender, r.ring
                 FROM events e
                 LEFT JOIN items i ON e.type = 'item' AND i.item_id = e.item_id
                 LEFT JOIN item_rings r ON e.type = 'item' AND r.item_id = e.item_id AND r.device_id = ?2
                 WHERE e.seq > ?1
                 ORDER BY e.seq
                 LIMIT ?3",
            )?;
            let fetch = i64::try_from(limit + 1).unwrap_or(i64::MAX);
            let rows = stmt
                .query_map(params![from, me, fetch], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<Vec<u8>>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let fetched = rows.len();
            let mut events: Vec<Value> = Vec::new();
            let mut envelope_sum = 0i64;
            for (seq, kind, at, data, envelope, size, sender, ring) in rows.into_iter().take(limit) {
                let mut data: Value = serde_json::from_str(&data).map_err(|e| ApiError::internal(format!("event data: {e}")))?;
                if kind == "item" {
                    let size = size.unwrap_or(0);
                    if !events.is_empty() && envelope_sum + size > PAGE_ENVELOPE_BYTES {
                        break;
                    }
                    envelope_sum += size;
                    data["envelope"] = json!(envelope.as_deref().map(b64_encode).unwrap_or_default());
                    data["envelope_size"] = json!(size);
                    // `ring` is the decision for the reader; always false for the sender.
                    data["ring"] = json!(ring == Some(1) && sender.as_deref() != Some(me.as_str()));
                }
                events.push(json!({ "seq": seq, "type": kind, "at": ts_format(at), "data": data }));
            }
            let has_more = events.len() < fetched;
            let next_after = if has_more {
                events.last().and_then(|e| e["seq"].as_i64()).unwrap_or(sp.last_seq)
            } else {
                sp.last_seq
            };
            Ok(json!({
                "events": events,
                "head_seq": sp.last_seq,
                "has_more": has_more,
                "next_after": next_after,
            }))
        })
        .await?;

    // The cursor is recorded as an indication for `admin devices` (never used for the purge).
    let (me, hint) = (ctx.dev()?.device_id.clone(), after);
    space
        .db
        .write(move |tx| {
            tx.execute(
                "UPDATE devices SET cursor_hint = ?2 WHERE device_id = ?1 AND (cursor_hint IS NULL OR cursor_hint != ?2)",
                params![me, hint],
            )?;
            Ok(())
        })
        .await?;
    Ok(json(StatusCode::OK, page))
}
