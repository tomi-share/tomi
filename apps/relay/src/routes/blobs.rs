//! Resumable blobs (`api-v1.md` §4.9): encrypted files uploaded by parts straight to disk
//! (never held in memory) and downloaded with `Range`.

use std::collections::HashSet;
use std::io::SeekFrom;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE,
    TRANSFER_ENCODING,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::Response;
use futures_util::StreamExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use super::{empty, json};
use crate::blobstore;
use crate::db::lock;
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Gate, Rl};
use crate::input::{has_media_type, json_body};
use crate::state::{AppState, Effects};
use crate::store::{self, check_quota};
use crate::wire::{self, ts_format, uuid_format};

/// BlobStatus of §4.9.
fn status_json(
    blob_id: &[u8; 16],
    size: i64,
    offset: i64,
    complete: bool,
    attached: bool,
    last_activity: i64,
    orphan_ttl_secs: u64,
) -> Value {
    let mut v = json!({
        "blob_id": uuid_format(blob_id),
        "size": size,
        "upload_offset": offset,
        "complete": complete,
        "attached": attached,
    });
    if !attached {
        let ttl = i64::try_from(orphan_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
        v["expires_at"] = json!(ts_format(last_activity.saturating_add(ttl)));
    }
    v
}

fn parse_blob_id(s: &str) -> ApiResult<[u8; 16]> {
    wire::uuid_parse(s)
        .ok_or_else(|| ApiError::field("/blob_id", "expected a lowercase hyphenated UUID"))
}

/// `POST /v1/blobs`: reserves the declared size in the quota.
pub async fn create(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let blob_id = o.uuid("blob_id", true);
    let size = o.uint("size", true, 0, u64::MAX);
    if let Some(s) = size {
        if s > st.cfg.max_blob_bytes {
            return Err(ApiError::payload_too_large(st.cfg.max_blob_bytes));
        }
        if s == 0 {
            o.error("size", "must be at least 1");
        }
    }
    o.finish()?;
    let (Some(blob_id), Some(size)) = (blob_id, size) else {
        return Err(ApiError::internal("validated fields missing"));
    };
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let (cfg, now) = (st.cfg.clone(), ctx.now);
    let (status, v) = st
        .reserve(&space, move |tx, budget| {
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            store::require_space(tx)?;
            let size = i64::try_from(size).unwrap_or(i64::MAX);
            let existing = tx
                .query_row(
                    "SELECT owner, size, upload_offset, complete, item_id IS NOT NULL OR transfer_id IS NOT NULL, last_activity
                     FROM blobs WHERE blob_id = ?1",
                    params![blob_id.to_vec()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((owner, old_size, offset, complete, attached, activity)) = existing {
                if owner == me && old_size == size {
                    return Ok((
                        StatusCode::OK,
                        status_json(&blob_id, old_size, offset, complete != 0, attached != 0, activity, cfg.blob_orphan_ttl_secs),
                    ));
                }
                return Err(ApiError::new(Code::BlobIdConflict).detail("This blob_id is used by another blob."));
            }
            check_quota(tx, budget, size.unsigned_abs())?;
            tx.execute(
                "INSERT INTO blobs (blob_id, owner, size, last_activity) VALUES (?1, ?2, ?3, ?4)",
                params![blob_id.to_vec(), me, size, now],
            )?;
            Ok((
                StatusCode::CREATED,
                status_json(&blob_id, size, 0, false, false, now, cfg.blob_orphan_ttl_secs),
            ))
        })
        .await?;
    Ok(json(status, v))
}

/// `GET /v1/blobs/{blob_id}/status`: for the owner only; used to resume an upload.
pub async fn status(
    State(st): State<AppState>,
    Path(blob_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let id = parse_blob_id(&blob_id)?;
    let me = ctx.dev()?.device_id.clone();
    let orphan_ttl = st.cfg.blob_orphan_ttl_secs;
    let v = ctx
        .space()?
        .db
        .read(move |tx| {
            let row = tx
                .query_row(
                    "SELECT size, upload_offset, complete, item_id IS NOT NULL OR transfer_id IS NOT NULL, last_activity
                     FROM blobs WHERE blob_id = ?1 AND owner = ?2",
                    params![id.to_vec(), me],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()?;
            let Some((size, offset, complete, attached, activity)) = row else {
                return Err(ApiError::not_found());
            };
            Ok(status_json(&id, size, offset, complete != 0, attached != 0, activity, orphan_ttl))
        })
        .await?;
    Ok(json(StatusCode::OK, v))
}

/// Marks a blob as being written; a second part for the same blob meanwhile is a mismatch.
struct BusyGuard<'a> {
    set: &'a std::sync::Mutex<HashSet<[u8; 16]>>,
    id: [u8; 16],
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        lock(self.set).remove(&self.id);
    }
}

/// `PATCH /v1/blobs/{blob_id}`: one part, copied from the body to the file chunk by chunk.
/// The part is atomic: if the body breaks off, the file goes back to `Upload-Offset`.
pub async fn patch(
    State(st): State<AppState>,
    Path(blob_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::DeviceBlobParts])).await?;
    // (6) type and size of the body.
    if !has_media_type(&ctx.parts, "application/octet-stream") {
        return Err(ApiError::new(Code::UnsupportedMediaType)
            .detail("Content-Type must be application/octet-stream."));
    }
    let chunked = ctx
        .parts
        .headers
        .get(TRANSFER_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let length = ctx
        .parts
        .headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let (false, Some(length)) = (chunked, length) else {
        return Err(
            ApiError::new(Code::LengthRequired).detail("A Content-Length header is required.")
        );
    };
    if length > st.cfg.max_upload_part_bytes {
        return Err(ApiError::payload_too_large(st.cfg.max_upload_part_bytes));
    }
    // (7) validation.
    let id = parse_blob_id(&blob_id)?;
    let offset = ctx
        .parts
        .headers
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n <= wire::JSON_INT_MAX)
        .ok_or_else(|| {
            ApiError::field(
                "/upload_offset",
                "a non-negative Upload-Offset header is required",
            )
        })?;
    if length == 0 {
        return Err(ApiError::field(
            "/content_length",
            "a part must not be empty",
        ));
    }
    // (8) business rules.
    let caller = ctx.dev()?.clone();
    let me = caller.device_id.clone();
    let space = ctx.space()?.clone();
    let row = space
        .db
        .read(move |tx| {
            let r = tx
                .query_row(
                    "SELECT size, upload_offset, complete, item_id IS NOT NULL OR transfer_id IS NOT NULL
                     FROM blobs WHERE blob_id = ?1 AND owner = ?2",
                    params![id.to_vec(), me],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)),
                )
                .optional()?;
            Ok(r)
        })
        .await?;
    let Some((size, current, complete, attached)) = row else {
        return Err(ApiError::not_found());
    };
    if complete != 0 || attached != 0 {
        return Err(ApiError::new(Code::BlobAttached).detail("The blob is complete or attached."));
    }
    let mismatch = |at: i64| {
        ApiError::new(Code::UploadOffsetMismatch)
            .detail("Upload-Offset differs from the bytes already received.")
            .with("upload_offset", at)
    };
    if i64::try_from(offset).ok() != Some(current) {
        return Err(mismatch(current));
    }
    if offset.saturating_add(length) > u64::try_from(size).unwrap_or(0) {
        return Err(ApiError::payload_too_large(
            u64::try_from(size).unwrap_or(0),
        ));
    }
    if !lock(&st.blob_busy).insert(id) {
        return Err(mismatch(current));
    }
    let _guard = BusyGuard {
        set: &st.blob_busy,
        id,
    };

    // Stream the part to the file.
    let path = blobstore::path(&space.dir, &id);
    // A space deleted meanwhile (`admin reset`) has no directory any more: the blob is unknown.
    blobstore::ensure_parent(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found()
        } else {
            e.into()
        }
    })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .await?;
    // Whatever an earlier aborted part left behind is cut off first.
    file.set_len(offset).await?;
    file.seek(SeekFrom::Start(offset)).await?;
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    let mut failure: Option<ApiError> = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                written += bytes.len() as u64;
                if written > length {
                    failure = Some(ApiError::bad_request(
                        "The body is longer than Content-Length.",
                    ));
                    break;
                }
                if let Err(e) = file.write_all(&bytes).await {
                    failure = Some(e.into());
                    break;
                }
            }
            Err(_) => {
                failure = Some(ApiError::bad_request(
                    "The body ended before Content-Length.",
                ));
                break;
            }
        }
    }
    if failure.is_none() && written != length {
        failure = Some(ApiError::bad_request(
            "The body ended before Content-Length.",
        ));
    }
    if failure.is_none() {
        if let Err(e) = file.sync_data().await {
            failure = Some(e.into());
        }
    }
    if let Some(e) = failure {
        // Back to the offset: the progress only moves by whole parts.
        let _ = file.set_len(offset).await;
        return Err(e);
    }
    drop(file);

    let new_offset = offset + length;
    let finished = new_offset == u64::try_from(size).unwrap_or(0);
    let now = ctx.now;
    let id_db = id;
    let new_offset_i = i64::try_from(new_offset).unwrap_or(i64::MAX);
    let updated = space
        .db
        .write(move |tx| {
            // The body of a part may have taken minutes: the caller is judged again here, once
            // the bytes are written (see `store::require_caller`).
            store::require_caller(tx, &caller)?;
            let n = tx.execute(
                "UPDATE blobs SET upload_offset = ?2, complete = ?3, last_activity = ?4
                 WHERE blob_id = ?1 AND upload_offset = ?5 AND complete = 0",
                params![
                    id_db.to_vec(),
                    new_offset_i,
                    i64::from(finished),
                    now,
                    current
                ],
            )?;
            // Nothing updated: tell whether the row is still there.
            let exists = n > 0
                || tx
                    .query_row(
                        "SELECT 1 FROM blobs WHERE blob_id = ?1",
                        params![id_db.to_vec()],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
            Ok((n, exists))
        })
        .await;
    match updated {
        Ok((1, _)) => {}
        Ok((_, false)) => {
            // The row vanished while the part was being written (purge, reset): the file just
            // written is an orphan that no quota counts. Remove it.
            blobstore::remove(&space.dir, &id);
            return Err(ApiError::not_found());
        }
        Ok(_) => return Err(mismatch(current)),
        Err(e) => {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
                let _ = f.set_len(offset);
            }
            return Err(e);
        }
    }
    if finished {
        // Flush everything to disk before the blob is reported complete (§4.9).
        if let Ok(f) = tokio::fs::File::open(&path).await {
            let _ = f.sync_all().await;
        }
    }
    let mut resp = empty(StatusCode::NO_CONTENT);
    if let Ok(v) = HeaderValue::from_str(&new_offset.to_string()) {
        resp.headers_mut().insert("upload-offset", v);
    }
    Ok(resp)
}

/// What a blob row says about who may read it.
struct BlobAccess {
    owner: String,
    size: i64,
    offset: i64,
    complete: bool,
    attached: bool,
    item_seq: Option<i64>,
    item_live: bool,
    transfer_to: Option<String>,
    transfer_open: bool,
}

fn blob_access(tx: &Connection, id: &[u8; 16], now: i64) -> ApiResult<Option<BlobAccess>> {
    let r = tx
        .query_row(
            "SELECT b.owner, b.size, b.upload_offset, b.complete, b.item_id, b.transfer_id,
                    i.seq, i.purged_at, t.to_device, t.closed_at, t.expires_at
             FROM blobs b
             LEFT JOIN items i ON i.item_id = b.item_id
             LEFT JOIN transfers t ON t.transfer_id = b.transfer_id
             WHERE b.blob_id = ?1",
            params![id.to_vec()],
            |r| {
                let item_id: Option<Vec<u8>> = r.get(4)?;
                let transfer_id: Option<Vec<u8>> = r.get(5)?;
                let closed: Option<i64> = r.get(9)?;
                let expires: Option<i64> = r.get(10)?;
                Ok(BlobAccess {
                    owner: r.get(0)?,
                    size: r.get(1)?,
                    offset: r.get(2)?,
                    complete: r.get::<_, i64>(3)? != 0,
                    attached: item_id.is_some() || transfer_id.is_some(),
                    item_seq: r.get(6)?,
                    item_live: r.get::<_, Option<i64>>(7)?.is_none(),
                    transfer_to: r.get(8)?,
                    transfer_open: closed.is_none() && expires.is_some_and(|e| e > now),
                })
            },
        )
        .optional()?;
    Ok(r)
}

impl BlobAccess {
    /// §4.9: the owner; any member that was there before the (live) item; the recipient of
    /// the (open) history transfer, and nobody else.
    fn visible_to(&self, me: &str, joined_seq: i64) -> bool {
        if self.owner == me {
            return true;
        }
        if let Some(seq) = self.item_seq {
            return self.item_live && joined_seq < seq;
        }
        self.transfer_open && self.transfer_to.as_deref() == Some(me)
    }
}

/// A byte range of a download.
#[derive(Debug, PartialEq, Eq)]
pub enum Window {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}

/// `Range` of §4.9: one `bytes=a-b`, `bytes=a-` or `bytes=-n`; anything else is `416`.
pub fn parse_range(header: &str, size: u64) -> Window {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Window::Unsatisfiable;
    };
    if spec.contains(',') || size == 0 {
        return Window::Unsatisfiable;
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Window::Unsatisfiable;
    };
    let (a, b) = (a.trim(), b.trim());
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    match (a.is_empty(), b.is_empty()) {
        (false, false) if digits(a) && digits(b) => match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(start), Ok(end)) if start <= end && start < size => {
                Window::Partial(start, end.min(size - 1))
            }
            _ => Window::Unsatisfiable,
        },
        (false, true) if digits(a) => match a.parse::<u64>() {
            Ok(start) if start < size => Window::Partial(start, size - 1),
            _ => Window::Unsatisfiable,
        },
        (true, false) if digits(b) => match b.parse::<u64>() {
            Ok(n) if n > 0 => Window::Partial(size - n.min(size), size - 1),
            _ => Window::Unsatisfiable,
        },
        _ => Window::Unsatisfiable,
    }
}

/// `GET` and `HEAD /v1/blobs/{blob_id}`.
pub async fn download(
    State(st): State<AppState>,
    Path(blob_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::DeviceBlobParts])).await?;
    let id = parse_blob_id(&blob_id)?;
    let dev = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let access = space
        .db
        .read(move |tx| match blob_access(tx, &id, now)? {
            Some(a) if a.visible_to(&dev.device_id, dev.joined_seq) => Ok(a),
            _ => Err(ApiError::not_found()),
        })
        .await?;
    if !access.complete {
        return Err(ApiError::new(Code::BlobIncomplete)
            .detail("The blob is not completely uploaded.")
            .with("upload_offset", access.offset));
    }
    let size = u64::try_from(access.size).unwrap_or(0);
    let etag = format!("\"{}\"", uuid_format(&id));
    let header = |name: &str| {
        ctx.parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    // `If-Range` with another validator: the whole blob is sent.
    let honour_range = header("if-range").is_none_or(|v| v.trim() == etag);
    let window = match header(RANGE.as_str()) {
        Some(r) if honour_range => parse_range(&r, size),
        _ => Window::Full,
    };
    let (status, start, end) = match window {
        Window::Full => (StatusCode::OK, 0, size.saturating_sub(1)),
        Window::Partial(a, b) => (StatusCode::PARTIAL_CONTENT, a, b),
        Window::Unsatisfiable => {
            let mut e = ApiError::new(Code::RangeNotSatisfiable)
                .detail("The Range header cannot be satisfied.");
            e = e.header(CONTENT_RANGE, &format!("bytes */{size}"));
            return Err(e);
        }
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    let body = if ctx.parts.method == Method::HEAD {
        Body::empty()
    } else {
        let mut file = tokio::fs::File::open(blobstore::path(&space.dir, &id)).await?;
        file.seek(SeekFrom::Start(start)).await?;
        Body::from_stream(ReaderStream::with_capacity(file.take(len), 64 * 1024))
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(ETAG, v);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")) {
            h.insert(CONTENT_RANGE, v);
        }
    }
    Ok(resp)
}

/// `DELETE /v1/blobs/{blob_id}`: abandons an upload. Attached blobs go with their item or
/// transfer.
pub async fn delete(
    State(st): State<AppState>,
    Path(blob_id): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let id = parse_blob_id(&blob_id)?;
    let dev = ctx.dev()?.clone();
    let space = ctx.space()?.clone();
    let now = ctx.now;
    let fx = space
        .db
        .write(move |tx| {
            // Still a member, still this session (see `store::require_caller`).
            store::require_caller(tx, &dev)?;
            store::require_space(tx)?;
            let a = blob_access(tx, &id, now)?.ok_or_else(ApiError::not_found)?;
            if !a.visible_to(&dev.device_id, dev.joined_seq) {
                return Err(ApiError::not_found());
            }
            if a.owner != dev.device_id {
                return Err(ApiError::forbidden().detail("Only the owner can delete a blob."));
            }
            if a.attached {
                return Err(ApiError::new(Code::BlobAttached)
                    .detail("An attached blob goes with its item or transfer."));
            }
            tx.execute("DELETE FROM blobs WHERE blob_id = ?1", params![id.to_vec()])?;
            Ok(Effects {
                unlink_blobs: vec![id],
                ..Effects::default()
            })
        })
        .await?;
    st.apply(&space, fx).await;
    Ok(empty(StatusCode::NO_CONTENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_forms() {
        assert_eq!(
            parse_range("bytes=100-199", 1000),
            Window::Partial(100, 199)
        );
        assert_eq!(parse_range("bytes=900-", 1000), Window::Partial(900, 999));
        assert_eq!(parse_range("bytes=-100", 1000), Window::Partial(900, 999));
        assert_eq!(parse_range("bytes=-5000", 1000), Window::Partial(0, 999));
        assert_eq!(
            parse_range("bytes=990-5000", 1000),
            Window::Partial(990, 999)
        );
    }

    #[test]
    fn invalid_or_multiple_ranges_are_unsatisfiable() {
        for r in [
            "bytes=0-1,5-6",
            "bytes=1000-",
            "bytes=5-2",
            "bytes=-0",
            "bytes=abc",
            "items=0-1",
            "bytes=",
            "bytes=-",
            "bytes=1-x",
        ] {
            assert_eq!(parse_range(r, 1000), Window::Unsatisfiable, "{r}");
        }
        assert_eq!(parse_range("bytes=0-1", 0), Window::Unsatisfiable);
    }
}
