//! Real transport (feature `http`): `api-v1.md` over HTTPS with `ureq` (rustls, `ring`
//! provider, bundled root certificates), and the WebSocket doorbell over `tungstenite`.
//!
//! This layer only converts between the wire format (JSON, RFC 3339 timestamps with
//! milliseconds, base64url without padding) and the typed [`RelayTransport`] view. It holds no
//! logic of its own. It cannot be tested end to end until a relay exists; the unit tests
//! below run it against a tiny local TCP server that returns canned answers.

use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::error::Result;
use crate::relay_url::check_base_url;
use crate::runtime::transport::*;

const USER_AGENT: &str = concat!("tomi-core/", env!("CARGO_PKG_VERSION"));

/// The relay a transport talks to: its base URL, and its authority in the form the pairing PAKE
/// binds to (`relay_authority`).
#[derive(Clone)]
struct Target {
    base: String,
    authority: String,
}

/// Blocking HTTPS client for the relay API.
pub struct HttpTransport {
    agent: ureq::Agent,
    /// `None` until a relay is configured: every call then fails with a network error and nothing
    /// is sent. Changed only by [`RelayTransport::set_relay`] (a device that is not provisioned).
    target: RwLock<Option<Target>>,
    token: Mutex<Option<String>>,
}

fn target_of(base_url: &str) -> Result<Target> {
    let parsed = check_base_url(base_url)?;
    Ok(Target {
        authority: parsed.relay_authority(),
        base: parsed.url,
    })
}

fn not_configured() -> RelayError {
    RelayError::Network("no relay configured".into())
}

/// Which `Authorization` header a request carries (`api-v1.md` §2.4).
#[derive(Clone, Copy)]
enum Auth<'a> {
    /// `Bearer <session>` when a session is installed, nothing otherwise.
    Session,
    /// No `Authorization` header, even if a session is installed.
    None,
    /// `Pairing <token>`.
    Pairing(&'a str),
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("base", &self.base_url())
            .finish_non_exhaustive()
    }
}

/// Connection set-up limit (TCP), for the API and the doorbell.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Limits of the three calls of a departure (`DELETE /v1/me`, `DELETE /v1/space`,
/// `POST /v1/space/receipt`, `api-v1.md` §7.9.3): connection, then the whole call.
const DEPARTURE_TIMEOUTS: (Duration, Duration) = (Duration::from_secs(10), Duration::from_secs(20));

/// Largest response body read from the relay; a bigger one is refused, not truncated.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

impl HttpTransport {
    /// A transport that talks to `base_url` (`https://host[:port][/prefix]`).
    pub fn new(base_url: &str) -> Result<Self> {
        Self::with_relay(Some(base_url))
    }

    /// A transport with no relay yet (`None`), or the one given. Nothing is sent while there is
    /// none; [`RelayTransport::set_relay`] gives it one.
    pub fn with_relay(base_url: Option<&str>) -> Result<Self> {
        let target = base_url.map(target_of).transpose()?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // No redirect: a 3xx could lead to plain HTTP or to another host (with the
            // token); it is reported as a network error instead (captive portal...).
            .max_redirects(0)
            .https_only(!cfg!(debug_assertions))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(Duration::from_secs(60)))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Ok(Self {
            agent,
            target: RwLock::new(target),
            token: Mutex::new(None),
        })
    }

    fn target(&self) -> Option<Target> {
        self.target
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn base_url(&self) -> Option<String> {
        self.target().map(|t| t.base)
    }

    fn url(&self, path: &str) -> TResult<String> {
        let target = self.target().ok_or_else(not_configured)?;
        Ok(format!("{}{path}", target.base))
    }

    fn bearer(&self) -> Option<String> {
        self.token
            .lock()
            .ok()
            .and_then(|t| t.clone())
            .map(|t| format!("Bearer {t}"))
    }

    /// The `Authorization` header value for `auth`, if any.
    fn authorization(&self, auth: Auth<'_>) -> TResult<Option<String>> {
        Ok(match auth {
            Auth::Session => self.bearer(),
            Auth::None => None,
            Auth::Pairing(token) => {
                // A token comes from the relay: keep anything but visible ASCII out of a header.
                if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
                    return Err(RelayError::Network("invalid pairing token".into()));
                }
                Some(format!("Pairing {token}"))
            }
        })
    }

    /// Sends a request with the session (when installed) and returns the status, the
    /// `Upload-Offset` header and the body.
    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> TResult<Reply> {
        self.send_as(Auth::Session, method, path, headers, body)
    }

    fn send_as(
        &self,
        auth: Auth<'_>,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> TResult<Reply> {
        self.send_with(auth, method, path, headers, body, None)
    }

    /// Runs `req`, with the agent's limits or with `timeouts` (connection, whole call) for
    /// this request only.
    fn run<S: ureq::AsSendBody>(
        &self,
        req: http::Request<S>,
        timeouts: Option<(Duration, Duration)>,
    ) -> std::result::Result<http::Response<ureq::Body>, ureq::Error> {
        match timeouts {
            Some((connect, total)) => {
                let req = self
                    .agent
                    .configure_request(req)
                    .timeout_connect(Some(connect))
                    .timeout_global(Some(total))
                    .build();
                self.agent.run(req)
            }
            None => self.agent.run(req),
        }
    }

    fn send_with(
        &self,
        auth: Auth<'_>,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
        timeouts: Option<(Duration, Duration)>,
    ) -> TResult<Reply> {
        let url = self.url(path)?;
        let auth = self.authorization(auth)?;
        let mut builder = http::Request::builder().method(method).uri(url);
        if let Some(a) = auth {
            builder = builder.header("Authorization", a);
        }
        for (k, v) in headers {
            builder = builder.header(*k, v.as_str());
        }
        let result = match body {
            Some(b) => {
                let req = builder
                    .body(b)
                    .map_err(|e| RelayError::Network(e.to_string()))?;
                self.run(req, timeouts)
            }
            None => {
                let req = builder
                    .body(())
                    .map_err(|e| RelayError::Network(e.to_string()))?;
                self.run(req, timeouts)
            }
        };
        let mut resp = result.map_err(|e| RelayError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            return Err(RelayError::Network(format!(
                "the relay answered a redirect ({status}), not followed"
            )));
        }
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let upload_offset = header("upload-offset").and_then(|v| v.parse().ok());
        let retry_after = header("retry-after").and_then(|v| v.parse().ok());
        let data = read_body_limited(resp.body_mut().as_reader(), MAX_RESPONSE_BYTES)?;
        let reply = Reply {
            status,
            upload_offset,
            data,
        };
        if status >= 400 {
            return Err(RelayError::problem(problem(&reply, retry_after)));
        }
        Ok(reply)
    }

    fn json_call(&self, method: &str, path: &str, body: Option<Value>) -> TResult<(u16, Value)> {
        self.json_call_as(Auth::Session, method, path, body)
    }

    fn json_call_as(
        &self,
        auth: Auth<'_>,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> TResult<(u16, Value)> {
        self.json_call_with(auth, method, path, body, None)
    }

    fn json_call_with(
        &self,
        auth: Auth<'_>,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeouts: Option<(Duration, Duration)>,
    ) -> TResult<(u16, Value)> {
        let (headers, bytes) = match body {
            Some(v) => (
                vec![("Content-Type", "application/json".to_owned())],
                Some(serde_json::to_vec(&v).map_err(|e| RelayError::Network(e.to_string()))?),
            ),
            None => (Vec::new(), None),
        };
        let r = self.send_with(auth, method, path, &headers, bytes, timeouts)?;
        if r.data.is_empty() {
            return Ok((r.status, Value::Null));
        }
        let v: Value = serde_json::from_slice(&r.data).map_err(|_| {
            RelayError::Network("the relay answered something that is not JSON".into())
        })?;
        Ok((r.status, v))
    }
}

/// Reads a body of at most `limit` bytes. One byte more is read to tell "exactly at the
/// limit" from "longer": a longer body is refused, never silently truncated. The error is a
/// network one (retryable, never a definitive refusal): a relay `413` (`payload_too_large`,
/// which fails a transfer for good) is about a request, not about this response.
fn read_body_limited(reader: impl Read, limit: u64) -> TResult<Vec<u8>> {
    let mut data = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| RelayError::Network(e.to_string()))?;
    if data.len() as u64 > limit {
        return Err(RelayError::Network(format!(
            "the relay answered more than {limit} bytes"
        )));
    }
    Ok(data)
}

/// Checks the answer to a `Range: bytes=offset-(offset+len-1)` request (`api-v1.md` §4.9).
/// A relay or proxy that ignores `Range` answers `200` with the whole blob: written at
/// `offset` it would corrupt the download, and the blob would then fail to decrypt and be
/// fetched again at every sync. `206` is the answer; `200` is only accepted when the range
/// starts at 0 and the body fits in it (the range then covers the whole blob). Either way,
/// the relay never sends more than was asked.
fn check_range_reply(status: u16, offset: u64, len: u64, received: usize) -> TResult<()> {
    let ok_status = status == 206 || (status == 200 && offset == 0);
    if !ok_status {
        return Err(RelayError::Network(format!(
            "unexpected status {status} for a range request (the relay or a proxy ignores Range)"
        )));
    }
    if received as u64 > len {
        return Err(RelayError::Network(format!(
            "the answer to a range request holds {received} bytes, {len} asked (the relay or a proxy ignores Range)"
        )));
    }
    Ok(())
}

struct Reply {
    status: u16,
    upload_offset: Option<u64>,
    data: Vec<u8>,
}

/// Reads an `application/problem+json` document (`api-v1.md` §2.5).
fn problem(r: &Reply, retry_after: Option<u64>) -> ApiProblem {
    let v: Value = serde_json::from_slice(&r.data).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    ApiProblem {
        status: r.status,
        code: s("code").unwrap_or_else(|| format!("http_{}", r.status)),
        detail: s("detail"),
        retry_after_secs: retry_after.or_else(|| n("retry_after")),
        key_epoch: n("key_epoch").and_then(|x| u32::try_from(x).ok()),
        upload_offset: n("upload_offset"),
        head_seq: n("head_seq"),
        update_url: s("update_url"),
        limited_by: v
            .get("limited_by")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Wire conversions
// ---------------------------------------------------------------------------

fn proto<T>(what: &str) -> TResult<T> {
    Err(RelayError::Network(format!("unexpected answer: {what}")))
}

/// `AAAA-MM-JJTHH:MM:SS.mmmZ` from milliseconds since the epoch.
pub(crate) fn ts_format(ms: i64) -> String {
    let nanos = i128::from(ms) * 1_000_000;
    match time::OffsetDateTime::from_unix_timestamp_nanos(nanos) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
            t.millisecond()
        ),
        Err(_) => "1970-01-01T00:00:00.000Z".to_owned(),
    }
}

pub(crate) fn ts_parse(s: &str) -> Option<i64> {
    let t = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok()
}

fn get_str<'a>(v: &'a Value, k: &str) -> TResult<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .map_or_else(|| proto(k), Ok)
}

fn get_u64(v: &Value, k: &str) -> TResult<u64> {
    v.get(k)
        .and_then(Value::as_u64)
        .map_or_else(|| proto(k), Ok)
}

fn get_ts(v: &Value, k: &str) -> TResult<i64> {
    v.get(k)
        .and_then(Value::as_str)
        .and_then(ts_parse)
        .map_or_else(|| proto(k), Ok)
}

fn get_b64(v: &Value, k: &str) -> TResult<Vec<u8>> {
    B64.decode(get_str(v, k)?)
        .map_err(|_| RelayError::Network(format!("unexpected answer: {k} is not base64url")))
}

fn limits(v: &Value) -> Limits {
    let d = Limits::default();
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    Limits {
        max_envelope_bytes: n("max_envelope_bytes").unwrap_or(d.max_envelope_bytes),
        max_blob_bytes: n("max_blob_bytes").unwrap_or(d.max_blob_bytes),
        max_item_blob_bytes: n("max_item_blob_bytes").unwrap_or(d.max_item_blob_bytes),
        max_blobs_per_item: n("max_blobs_per_item").unwrap_or(d.max_blobs_per_item),
        max_upload_part_bytes: n("max_upload_part_bytes").unwrap_or(d.max_upload_part_bytes),
        max_ttl_secs: n("max_ttl_secs").unwrap_or(d.max_ttl_secs),
        max_devices: n("max_devices").unwrap_or(d.max_devices),
        // Additive limits: absent on an older relay, the transfer is then not offered.
        max_transfer_bytes: n("max_transfer_bytes"),
        max_blobs_per_transfer: n("max_blobs_per_transfer"),
        transfer_ttl_secs: n("transfer_ttl_secs"),
        transfer_max_open: n("transfer_max_open"),
    }
}

/// `features` of `hello` and of `SpaceState`: strings only, anything else ignored.
fn features(v: &Value) -> Vec<String> {
    v.get("features")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| f.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn space_info(v: &Value) -> TResult<SpaceInfo> {
    let q = v.get("quota").cloned().unwrap_or(Value::Null);
    Ok(SpaceInfo {
        space_id: get_str(v, "space_id")?.to_owned(),
        effective_format: u32::try_from(get_u64(v, "effective_format")?).unwrap_or(u32::MAX),
        key_epoch: u32::try_from(get_u64(v, "key_epoch")?).unwrap_or(u32::MAX),
        rotation_required: v
            .get("rotation_required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        head_seq: get_u64(v, "head_seq")?,
        server_time_ms: get_ts(v, "server_time")?,
        quota_used_bytes: q.get("used_bytes").and_then(Value::as_u64).unwrap_or(0),
        quota_max_bytes: q.get("max_bytes").and_then(Value::as_u64).unwrap_or(0),
        limits: v.get("limits").map(limits).unwrap_or_default(),
        features: features(v),
    })
}

fn device_record(v: &Value) -> TResult<DeviceRecord> {
    Ok(DeviceRecord {
        device_id: get_str(v, "device_id")?.to_owned(),
        name: get_str(v, "name")?.to_owned(),
        platform: get_str(v, "platform")?.to_owned(),
        sign_pk: get_b64(v, "sign_pk")?,
        kx_pk: get_b64(v, "kx_pk")?,
        certificate: get_b64(v, "certificate")?,
        issuer_device_id: get_str(v, "issuer_device_id")?.to_owned(),
        joined_seq: get_u64(v, "joined_seq")?,
        registered_at_ms: get_ts(v, "registered_at")?,
        // Both are tolerated absent (an older relay) or malformed: they only inform.
        last_seen_at_ms: v
            .get("last_seen_at")
            .and_then(Value::as_str)
            .and_then(ts_parse),
        active: v.get("active").and_then(Value::as_bool),
    })
}

/// A pairing `channel_id`: 1 to 4 decimal digits, no leading zero (`api-v1.md` §4.6). It is
/// written into a URL path, so anything else is refused before a request is built, and an
/// answer of the relay that carries another form is a protocol error.
fn is_channel_id(s: &str) -> bool {
    (1..=4).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) && !s.starts_with('0')
}

fn channel_path(channel_id: &str, tail: &str) -> TResult<String> {
    if !is_channel_id(channel_id) {
        return Err(RelayError::Network("invalid pairing channel id".into()));
    }
    Ok(format!("/v1/pairing/channels/{channel_id}{tail}"))
}

fn channel_state(v: &Value) -> TResult<ChannelState> {
    ChannelState::parse(get_str(v, "state")?).map_or_else(|| proto("state"), Ok)
}

/// The `Auth` of a pairing route: the inviter uses its session.
fn pairing_auth(auth: PairingAuth<'_>) -> Auth<'_> {
    match auth {
        PairingAuth::Inviter => Auth::Session,
        PairingAuth::Joiner(token) => Auth::Pairing(token),
    }
}

fn pairing_messages_from(v: &Value) -> TResult<PairingMessages> {
    let mut messages = Vec::new();
    for m in v
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // A side this build does not know is not a message for it.
        let side = match m.get("side").and_then(Value::as_str) {
            Some("inviter") => PairingSide::Inviter,
            Some("joiner") => PairingSide::Joiner,
            _ => continue,
        };
        messages.push(PairingMessage {
            index: get_u64(m, "index")?,
            side,
            body: get_b64(m, "body")?,
        });
    }
    Ok(PairingMessages {
        state: channel_state(v)?,
        expires_at_ms: get_ts(v, "expires_at")?,
        messages,
    })
}

fn blob_refs(v: &Value) -> Vec<BlobRef> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    Some(BlobRef {
                        blob_id: b.get("blob_id")?.as_str()?.to_owned(),
                        size: b.get("size")?.as_u64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn blob_status(v: &Value) -> TResult<BlobStatus> {
    Ok(BlobStatus {
        blob_id: get_str(v, "blob_id")?.to_owned(),
        size: get_u64(v, "size")?,
        upload_offset: get_u64(v, "upload_offset")?,
        complete: v.get("complete").and_then(Value::as_bool).unwrap_or(false),
        attached: v.get("attached").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Typed event from its JSON form (`api-v1.md` §4.10). An unknown `type` becomes
/// [`EventData::Unknown`]; a known type with a malformed `data` is a protocol error.
pub(crate) fn event_from_json(v: &Value) -> TResult<Event> {
    let seq = get_u64(v, "seq")?;
    let at_ms = get_ts(v, "at")?;
    let kind = get_str(v, "type")?;
    let d = v.get("data").cloned().unwrap_or(Value::Null);
    let data = match kind {
        "item" => EventData::Item {
            item_id: get_str(&d, "item_id")?.to_owned(),
            sender_device_id: get_str(&d, "sender_device_id")?.to_owned(),
            created_at_ms: get_ts(&d, "created_at")?,
            expires_at_ms: get_ts(&d, "expires_at")?,
            key_epoch: u32::try_from(get_u64(&d, "key_epoch")?).unwrap_or(u32::MAX),
            envelope: get_b64(&d, "envelope")?,
            blobs: d.get("blobs").map(blob_refs).unwrap_or_default(),
            ring: d.get("ring").and_then(Value::as_bool).unwrap_or(false),
        },
        "ack" => EventData::Ack {
            item_id: get_str(&d, "item_id")?.to_owned(),
            device_id: get_str(&d, "device_id")?.to_owned(),
            received_at_ms: get_ts(&d, "received_at")?,
        },
        "read" => EventData::Read {
            item_id: get_str(&d, "item_id")?.to_owned(),
            device_id: get_str(&d, "device_id")?.to_owned(),
            read_at_ms: get_ts(&d, "read_at")?,
        },
        "tombstone" => EventData::Tombstone {
            item_id: get_str(&d, "item_id")?.to_owned(),
            device_id: get_str(&d, "device_id")?.to_owned(),
            deleted_at_ms: get_ts(&d, "deleted_at")?,
            tombstone: get_b64(&d, "tombstone")?,
        },
        "attachment_tombstone" => EventData::AttachmentTombstone {
            // Lenient: a malformed event must reach the engine (diagnostic, cursor advances)
            // rather than fail the whole page. Only the signed `statement` is authoritative.
            item_id: d
                .get("item_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            slot: d
                .get("slot")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(u32::MAX),
            blob_id: d
                .get("blob_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            device_id: d
                .get("device_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            deleted_at_ms: get_ts(&d, "deleted_at").unwrap_or(0),
            statement: get_b64(&d, "statement").unwrap_or_default(),
        },
        "item_state" => EventData::ItemState {
            event_id: get_str(&d, "event_id")?.to_owned(),
            device_id: get_str(&d, "device_id")?.to_owned(),
            key_epoch: u32::try_from(get_u64(&d, "key_epoch")?).unwrap_or(u32::MAX),
            envelope: get_b64(&d, "envelope")?,
        },
        "rotation" => EventData::Rotation {
            key_epoch: u32::try_from(get_u64(&d, "key_epoch")?).unwrap_or(u32::MAX),
            device_id: get_str(&d, "device_id")?.to_owned(),
            revoked_device_ids: d
                .get("revoked_device_ids")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            rotation: get_b64(&d, "rotation")?,
        },
        "device_added" => EventData::DeviceAdded {
            device: device_record(d.get("device").unwrap_or(&Value::Null))?,
        },
        "device_updated" => EventData::DeviceUpdated {
            device_id: get_str(&d, "device_id")?.to_owned(),
            name: get_str(&d, "name")?.to_owned(),
        },
        "device_revoked" => EventData::DeviceRevoked {
            device_id: get_str(&d, "device_id")?.to_owned(),
            via: get_str(&d, "via")?.to_owned(),
        },
        "history_transfer" => EventData::HistoryTransfer {
            transfer_id: get_str(&d, "transfer_id")?.to_owned(),
            from_device_id: get_str(&d, "from_device_id")?.to_owned(),
            to_device_id: get_str(&d, "to_device_id")?.to_owned(),
            total_bytes: get_u64(&d, "total_bytes")?,
            expires_at_ms: get_ts(&d, "expires_at")?,
        },
        "history_transfer_closed" => EventData::HistoryTransferClosed {
            transfer_id: get_str(&d, "transfer_id")?.to_owned(),
            outcome: get_str(&d, "outcome")?.to_owned(),
        },
        other => EventData::Unknown {
            kind: other.to_owned(),
        },
    };
    Ok(Event { seq, at_ms, data })
}

impl RelayTransport for HttpTransport {
    fn set_session_token(&self, token: Option<String>) {
        if let Ok(mut t) = self.token.lock() {
            *t = token;
        }
    }

    fn hello(&self) -> TResult<Hello> {
        let (_, v) = self.json_call("GET", "/v1/hello", None)?;
        Ok(Hello {
            server_time_ms: get_ts(&v, "server_time")?,
            space_initialized: v
                .get("space_initialized")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            limits: v.get("limits").map(limits),
            features: features(&v),
        })
    }

    fn auth_challenge(&self, device_id: &str) -> TResult<Challenge> {
        let (_, v) = self.json_call(
            "POST",
            "/v1/auth/challenge",
            Some(json!({ "device_id": device_id })),
        )?;
        Ok(Challenge {
            challenge: get_b64(&v, "challenge")?,
            expires_at_ms: get_ts(&v, "expires_at")?,
        })
    }

    fn auth_session(&self, r: &SessionRequest) -> TResult<SessionGrant> {
        let body = json!({
            "device_id": r.device_id,
            "challenge": B64.encode(&r.challenge),
            "signature": B64.encode(&r.signature),
            "client": {
                "format_min": r.client.format_min,
                "format_max": r.client.format_max,
                "app_version": r.client.app_version,
                "platform": r.client.platform,
            },
        });
        let (_, v) = self.json_call("POST", "/v1/auth/session", Some(body))?;
        Ok(SessionGrant {
            token: get_str(&v, "session_token")?.to_owned(),
            expires_at_ms: get_ts(&v, "expires_at")?,
            device_id: get_str(&v, "device_id")?.to_owned(),
            space: space_info(v.get("space").unwrap_or(&Value::Null))?,
        })
    }

    fn delete_session(&self) -> TResult<()> {
        self.json_call("DELETE", "/v1/auth/session", None)
            .map(|_| ())
    }

    fn leave_space(&self) -> TResult<()> {
        // The departure is proven by the `seq` of the `device_revoked` event it logged
        // (`api-v1.md` §4.4): a `2xx` without it (a proxy, a captive portal) proves nothing,
        // and the device would then be erased while still a member.
        let (_, v) = self.json_call_with(
            Auth::Session,
            "DELETE",
            "/v1/me",
            None,
            Some(DEPARTURE_TIMEOUTS),
        )?;
        match v.get("seq").and_then(Value::as_u64) {
            Some(_) => Ok(()),
            None => proto("leave without seq"),
        }
    }

    fn delete_space(&self, deletion_id: &[u8; 32]) -> TResult<()> {
        // Only the boolean `true` proves the deletion (`api-v1.md` §4.4.1, §7.9.1). The
        // identifier goes in the body, never in the URL or in an error message.
        let body = json!({ "deletion_id": B64.encode(deletion_id) });
        let (_, v) = self.json_call_with(
            Auth::Session,
            "DELETE",
            "/v1/space",
            Some(body),
            Some(DEPARTURE_TIMEOUTS),
        )?;
        match v.get("space_deleted") {
            Some(Value::Bool(true)) => Ok(()),
            _ => proto("delete without space_deleted"),
        }
    }

    fn space_deletion_receipt(&self, deletion_id: &[u8; 32]) -> TResult<()> {
        // Without session: the session went with the space (`api-v1.md` §4.4.2).
        let body = json!({ "deletion_id": B64.encode(deletion_id) });
        let (_, v) = self.json_call_with(
            Auth::None,
            "POST",
            "/v1/space/receipt",
            Some(body),
            Some(DEPARTURE_TIMEOUTS),
        )?;
        match v.get("space_deleted") {
            Some(Value::Bool(true)) => Ok(()),
            _ => proto("receipt without space_deleted"),
        }
    }

    fn space(&self) -> TResult<SpaceInfo> {
        space_info(&self.json_call("GET", "/v1/space", None)?.1)
    }

    fn keys(&self) -> TResult<KeysInfo> {
        let (_, v) = self.json_call("GET", "/v1/keys", None)?;
        let mut rotations = Vec::new();
        for r in v
            .get("rotations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            rotations.push(RotationRecord {
                key_epoch: u32::try_from(get_u64(r, "key_epoch")?).unwrap_or(u32::MAX),
                seq: get_u64(r, "seq")?,
                rotation: get_b64(r, "rotation")?,
            });
        }
        Ok(KeysInfo {
            key_epoch: u32::try_from(get_u64(&v, "key_epoch")?).unwrap_or(u32::MAX),
            rotations,
        })
    }

    fn devices(&self) -> TResult<Vec<DeviceRecord>> {
        let (_, v) = self.json_call("GET", "/v1/devices", None)?;
        v.get("devices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(device_record)
            .collect()
    }

    fn rename_device(&self, device_id: &str, name: &str) -> TResult<()> {
        self.json_call(
            "PATCH",
            &format!("/v1/devices/{device_id}"),
            Some(json!({ "name": name })),
        )
        .map(|_| ())
    }

    fn post_rotation(&self, rotation: &[u8]) -> TResult<u64> {
        let (_, v) = self.json_call(
            "POST",
            "/v1/rotations",
            Some(json!({ "rotation": B64.encode(rotation) })),
        )?;
        get_u64(&v, "seq")
    }

    fn events(&self, after: u64, limit: u32) -> TResult<EventPage> {
        let (_, v) = self.json_call(
            "GET",
            &format!("/v1/events?after={after}&limit={limit}"),
            None,
        )?;
        let events = v
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(event_from_json)
            .collect::<TResult<Vec<_>>>()?;
        Ok(EventPage {
            events,
            head_seq: get_u64(&v, "head_seq")?,
            has_more: v.get("has_more").and_then(Value::as_bool).unwrap_or(false),
            next_after: get_u64(&v, "next_after")?,
        })
    }

    fn post_item(&self, item: &PostItem) -> TResult<Accepted> {
        let body = json!({
            "item_id": item.item_id,
            "expires_at": ts_format(item.expires_at_ms),
            "key_epoch": item.key_epoch,
            "envelope": B64.encode(&item.envelope),
            "blob_ids": item.blob_ids,
        });
        let (status, v) = self.json_call("POST", "/v1/items", Some(body))?;
        Ok(Accepted {
            seq: get_u64(&v, "seq")?,
            replayed: status == 200,
        })
    }

    fn ack_item(&self, item_id: &str, release: bool) -> TResult<()> {
        self.json_call(
            "POST",
            &format!("/v1/items/{item_id}/ack"),
            Some(json!({ "release": release })),
        )
        .map(|_| ())
    }

    fn read_item(&self, item_id: &str) -> TResult<()> {
        self.json_call(
            "POST",
            &format!("/v1/items/{item_id}/read"),
            Some(json!({})),
        )
        .map(|_| ())
    }

    fn post_tombstone(&self, tombstone: &[u8]) -> TResult<Accepted> {
        let (status, v) = self.json_call(
            "POST",
            "/v1/tombstones",
            Some(json!({ "tombstone": B64.encode(tombstone) })),
        )?;
        Ok(Accepted {
            seq: get_u64(&v, "seq")?,
            replayed: status == 200,
        })
    }

    fn post_attachment_tombstone(&self, statement: &[u8]) -> TResult<Accepted> {
        let (status, v) = self.json_call(
            "POST",
            "/v1/attachment-tombstones",
            Some(json!({ "attachment_tombstone": B64.encode(statement) })),
        )?;
        Ok(Accepted {
            seq: get_u64(&v, "seq")?,
            replayed: status == 200,
        })
    }

    fn post_item_state(
        &self,
        event_id: &str,
        key_epoch: u32,
        envelope: &[u8],
    ) -> TResult<Accepted> {
        let body = json!({ "event_id": event_id, "key_epoch": key_epoch, "envelope": B64.encode(envelope) });
        let (status, v) = self.json_call("POST", "/v1/item-states", Some(body))?;
        Ok(Accepted {
            seq: get_u64(&v, "seq")?,
            replayed: status == 200,
        })
    }

    fn create_blob(&self, blob_id: &str, size: u64) -> TResult<BlobStatus> {
        let (_, v) = self.json_call(
            "POST",
            "/v1/blobs",
            Some(json!({ "blob_id": blob_id, "size": size })),
        )?;
        blob_status(&v)
    }

    fn blob_status(&self, blob_id: &str) -> TResult<BlobStatus> {
        blob_status(
            &self
                .json_call("GET", &format!("/v1/blobs/{blob_id}/status"), None)?
                .1,
        )
    }

    fn put_blob_part(&self, blob_id: &str, offset: u64, data: &[u8]) -> TResult<u64> {
        let headers = [
            ("Content-Type", "application/octet-stream".to_owned()),
            ("Upload-Offset", offset.to_string()),
        ];
        let r = self.send(
            "PATCH",
            &format!("/v1/blobs/{blob_id}"),
            &headers,
            Some(data.to_vec()),
        )?;
        Ok(r.upload_offset.unwrap_or(offset + data.len() as u64))
    }

    fn get_blob_range(&self, blob_id: &str, offset: u64, len: u64) -> TResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let range = format!("bytes={}-{}", offset, offset + len - 1);
        let r = self.send(
            "GET",
            &format!("/v1/blobs/{blob_id}"),
            &[("Range", range)],
            None,
        )?;
        check_range_reply(r.status, offset, len, r.data.len())?;
        Ok(r.data)
    }

    fn delete_blob(&self, blob_id: &str) -> TResult<()> {
        self.send("DELETE", &format!("/v1/blobs/{blob_id}"), &[], None)
            .map(|_| ())
    }

    fn open_wake(&self) -> TResult<Box<dyn WakeSource>> {
        let token = self.token.lock().ok().and_then(|t| t.clone());
        let Some(token) = token else {
            return Err(RelayError::api(401, "unauthorized"));
        };
        let base = self.base_url().ok_or_else(not_configured)?;
        Ok(Box::new(WsWake::connect(&base, &token)?))
    }

    fn post_transfer(&self, t: &PostTransfer) -> TResult<Accepted> {
        let body = json!({
            "transfer_id": t.transfer_id,
            "manifest": B64.encode(&t.manifest),
            "blob_ids": t.blob_ids,
        });
        let (status, v) = self.json_call("POST", "/v1/transfers", Some(body))?;
        Ok(Accepted {
            seq: v.get("seq").and_then(Value::as_u64).unwrap_or(0),
            replayed: status == 200,
        })
    }

    fn get_transfer(&self, transfer_id: &str) -> TResult<TransferInfo> {
        let (_, v) = self.json_call("GET", &format!("/v1/transfers/{transfer_id}"), None)?;
        Ok(TransferInfo {
            transfer_id: get_str(&v, "transfer_id")?.to_owned(),
            from_device_id: get_str(&v, "from_device_id")?.to_owned(),
            to_device_id: get_str(&v, "to_device_id")?.to_owned(),
            manifest: get_b64(&v, "manifest")?,
            blobs: v.get("blobs").map(blob_refs).unwrap_or_default(),
            expires_at_ms: get_ts(&v, "expires_at")?,
        })
    }

    fn delete_transfer(&self, transfer_id: &str, outcome: &str) -> TResult<()> {
        let body = serde_json::to_vec(&json!({ "outcome": outcome })).unwrap_or_default();
        self.send(
            "DELETE",
            &format!("/v1/transfers/{transfer_id}"),
            &[("Content-Type", "application/json".to_owned())],
            Some(body),
        )
        .map(|_| ())
    }

    fn relay_authority(&self) -> Option<String> {
        self.target().map(|t| t.authority)
    }

    fn relay_url(&self) -> Option<String> {
        self.base_url()
    }

    fn set_relay(&self, url: Option<&str>) -> TResult<()> {
        let target = url
            .map(target_of)
            .transpose()
            .map_err(|e| RelayError::Network(e.to_string()))?;
        // A session belongs to one relay: it goes **first**. A request reads the target, then the
        // session; with this order none can pair the new relay with the old session (the old
        // target with no session at worst).
        self.set_session_token(None);
        *self.target.write().unwrap_or_else(PoisonError::into_inner) = target;
        Ok(())
    }

    fn post_space(&self, s: &NewSpace) -> TResult<CreatedSpace> {
        let body = json!({
            "install_token": s.install_token,
            "space_id": s.space_id,
            "name": s.name,
            "platform": s.platform,
            "certificate": B64.encode(&s.certificate),
        });
        // Without session: the install token authorises the call.
        let (_, v) = self.json_call_as(Auth::None, "POST", "/v1/space", Some(body))?;
        Ok(CreatedSpace {
            space_id: get_str(&v, "space_id")?.to_owned(),
            created_at_ms: get_ts(&v, "created_at")?,
            device_id: get_str(&v, "device_id")?.to_owned(),
            key_epoch: u32::try_from(get_u64(&v, "key_epoch")?).unwrap_or(u32::MAX),
            head_seq: get_u64(&v, "head_seq")?,
        })
    }

    fn post_device(&self, d: &NewDevice) -> TResult<DeviceRecord> {
        let body = json!({
            "channel_id": d.channel_id,
            "key_epoch": d.key_epoch,
            "name": d.name,
            "platform": d.platform,
            "certificate": B64.encode(&d.certificate),
        });
        let (_, v) = self.json_call("POST", "/v1/devices", Some(body))?;
        device_record(&v)
    }

    fn create_pairing_channel(&self) -> TResult<PairingChannel> {
        let (_, v) = self.json_call("POST", "/v1/pairing/channels", Some(json!({})))?;
        let channel_id = get_str(&v, "channel_id")?;
        if !is_channel_id(channel_id) {
            return proto("channel_id");
        }
        Ok(PairingChannel {
            channel_id: channel_id.to_owned(),
            state: channel_state(&v)?,
            expires_at_ms: get_ts(&v, "expires_at")?,
        })
    }

    fn claim_pairing_channel(&self, channel_id: &str) -> TResult<PairingClaim> {
        let path = channel_path(channel_id, "/claim")?;
        // No `Authorization`: a new device has no session, and must not send one by accident.
        let (_, v) = self.json_call_as(Auth::None, "POST", &path, Some(json!({})))?;
        Ok(PairingClaim {
            pairing_token: get_str(&v, "pairing_token")?.to_owned(),
            expires_at_ms: get_ts(&v, "expires_at")?,
        })
    }

    fn post_pairing_message(
        &self,
        channel_id: &str,
        auth: PairingAuth<'_>,
        body: &[u8],
    ) -> TResult<u64> {
        let path = channel_path(channel_id, "/messages")?;
        let (_, v) = self.json_call_as(
            pairing_auth(auth),
            "POST",
            &path,
            Some(json!({ "body": B64.encode(body) })),
        )?;
        get_u64(&v, "index")
    }

    fn pairing_messages(
        &self,
        channel_id: &str,
        auth: PairingAuth<'_>,
        after: u64,
        wait_secs: u32,
    ) -> TResult<PairingMessages> {
        // `wait` is 0 to 25 seconds (`api-v1.md` §4.6). Nothing secret travels in the query.
        let path = channel_path(
            channel_id,
            &format!("/messages?after={after}&wait={}", wait_secs.min(25)),
        )?;
        let (_, v) = self.json_call_as(pairing_auth(auth), "GET", &path, None)?;
        pairing_messages_from(&v)
    }

    fn close_pairing_channel(&self, channel_id: &str, auth: PairingAuth<'_>) -> TResult<()> {
        let path = channel_path(channel_id, "")?;
        self.json_call_as(pairing_auth(auth), "DELETE", &path, None)
            .map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// WebSocket doorbell (api-v1 §5)
// ---------------------------------------------------------------------------

type Stream = rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>;

enum Socket {
    Tls(Box<tungstenite::WebSocket<Stream>>),
    Plain(Box<tungstenite::WebSocket<std::net::TcpStream>>),
}

/// Silence after which the connection is considered dead (`api-v1.md` §5.3).
const WS_SILENCE: Duration = Duration::from_secs(60);

/// One doorbell connection. After `connect` it has sent the `auth` message and received
/// `ready`; [`WakeSource::next`] then yields `wake` messages. The caller reconnects with
/// [`Reconnect::for_close`](super::transport::Reconnect::for_close).
pub struct WsWake {
    socket: Socket,
    last_frame: std::time::Instant,
    /// `head_seq` of the `ready` message: run a catch-up sync now (§5.1, step 4).
    pub head_seq: u64,
}

impl std::fmt::Debug for WsWake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsWake")
            .field("head_seq", &self.head_seq)
            .finish_non_exhaustive()
    }
}

fn closed(code: Option<u16>) -> WakeClosed {
    WakeClosed { code }
}

impl WsWake {
    /// Opens `wss://host/v1/ws` (no token in the URL), authenticates with `token`.
    pub fn connect(base_url: &str, token: &str) -> TResult<Self> {
        use std::net::ToSocketAddrs;
        let net = |e: &dyn std::fmt::Display| RelayError::Network(e.to_string());
        let base = check_base_url(base_url).map_err(|e| net(&e))?;
        let tls = base.tls;
        let host = base.host.clone();
        // Every resolved address in turn (IPv6 and IPv4), each with a bounded set-up time.
        let mut tcp = None;
        let mut last_err = String::from("the relay host has no address");
        for addr in (host.as_str(), base.port)
            .to_socket_addrs()
            .map_err(|e| net(&e))?
        {
            match std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        let tcp = tcp.ok_or(RelayError::Network(last_err))?;
        // Short reads so that the caller can stop the loop; a silent connection is declared
        // dead after 60 s (`api-v1.md` section 5.3), see `WsWake::next`.
        tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
        tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
        tcp.set_nodelay(true).ok();
        let url = base.ws_url();
        let mut socket = if tls {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| net(&e))?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let name =
                rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| net(&e))?;
            let conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| net(&e))?;
            let stream = rustls::StreamOwned::new(conn, tcp);
            let (ws, _) = tungstenite::client(url.as_str(), stream).map_err(|e| net(&e))?;
            Socket::Tls(Box::new(ws))
        } else {
            let (ws, _) = tungstenite::client(url.as_str(), tcp).map_err(|e| net(&e))?;
            Socket::Plain(Box::new(ws))
        };
        let auth = json!({ "type": "auth", "token": token }).to_string();
        socket.send_text(auth).map_err(|e| net(&e))?;
        // The first message must be `ready` (or a close with a code of §5.4).
        let mut last_frame = std::time::Instant::now();
        match socket.read_text(&mut last_frame) {
            Ok(Some(text)) => {
                let v: Value = serde_json::from_str(&text).map_err(|e| net(&e))?;
                if v.get("type").and_then(Value::as_str) != Some("ready") {
                    return Err(RelayError::Network("the doorbell did not say ready".into()));
                }
                Ok(Self {
                    head_seq: v.get("head_seq").and_then(Value::as_u64).unwrap_or(0),
                    socket,
                    last_frame,
                })
            }
            Ok(None) => Err(RelayError::Network("no ready message".into())),
            Err(c) => Err(RelayError::Network(format!(
                "closed before ready: {:?}",
                c.code
            ))),
        }
    }
}

impl Socket {
    fn send_text(&mut self, text: String) -> std::result::Result<(), tungstenite::Error> {
        match self {
            Socket::Tls(ws) => ws.send(tungstenite::Message::text(text)),
            Socket::Plain(ws) => ws.send(tungstenite::Message::text(text)),
        }
    }

    /// Next text message; `Ok(None)` for control frames. `Err` when the connection ended.
    fn read_text(
        &mut self,
        activity: &mut std::time::Instant,
    ) -> std::result::Result<Option<String>, WakeClosed> {
        let msg = match self {
            Socket::Tls(ws) => ws.read(),
            Socket::Plain(ws) => ws.read(),
        };
        if msg.is_ok() {
            // Any frame (ping included) proves the connection is alive.
            *activity = std::time::Instant::now();
        }
        match msg {
            Ok(tungstenite::Message::Text(t)) => Ok(Some(t.as_str().to_owned())),
            Ok(tungstenite::Message::Close(frame)) => Err(closed(frame.map(|f| u16::from(f.code)))),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                Err(closed(None))
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(_) => Err(closed(None)),
        }
    }
}

impl WakeSource for WsWake {
    fn next(&mut self) -> std::result::Result<Option<Wake>, WakeClosed> {
        match self.socket.read_text(&mut self.last_frame)? {
            Some(text) => Ok(parse_wake(&text)),
            None if self.last_frame.elapsed() > WS_SILENCE => Err(closed(None)),
            None => Ok(None),
        }
    }
}

/// `{"type":"wake","seq":44,"ring":true}`; any other message is ignored (§5.2).
pub(crate) fn parse_wake(text: &str) -> Option<Wake> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("type")?.as_str()? != "wake" {
        return None;
    }
    Some(Wake {
        seq: v.get("seq")?.as_u64()?,
        ring: v.get("ring").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves `answers` one per connection and returns the raw requests received.
    fn serve(answers: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for a in answers {
                let (mut s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len = text[..i]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                seen.push(String::from_utf8_lossy(&buf).to_string());
                s.write_all(a.as_bytes()).unwrap();
            }
            seen
        });
        (url, h)
    }

    fn reply(status: &str, extra: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn a_body_over_the_limit_is_refused_not_truncated() {
        let body = vec![7u8; 10];
        assert_eq!(read_body_limited(&body[..], 10).unwrap(), body);
        assert_eq!(read_body_limited(&body[..], 11).unwrap(), body);
        let e = read_body_limited(&body[..], 9).unwrap_err();
        // Not a relay 413: callers must not take it for a definitive refusal of a request.
        assert_eq!(e.code(), None);
        assert!(e.is_retryable() && e.is_offline());
    }

    #[test]
    fn a_range_answer_must_be_206_and_no_longer_than_asked() {
        // 206, exactly or less than asked (the end of the blob): fine.
        assert!(check_range_reply(206, 100, 50, 50).is_ok());
        assert!(check_range_reply(206, 100, 50, 10).is_ok());
        // 200 only when the range starts at 0 and the body fits in it.
        assert!(check_range_reply(200, 0, 50, 50).is_ok());
        assert!(check_range_reply(200, 0, 50, 10).is_ok());
        // A proxy that ignores Range: whole blob in a 200.
        assert!(check_range_reply(200, 100, 50, 50).is_err());
        assert!(check_range_reply(200, 0, 50, 500).is_err());
        assert!(check_range_reply(206, 0, 50, 51).is_err());
        // Any other success status is not a range answer.
        assert!(check_range_reply(204, 0, 50, 0).is_err());
        // Refused as a network error: retried later, never a decryption failure.
        let e = check_range_reply(200, 100, 50, 500).unwrap_err();
        assert!(matches!(e, RelayError::Network(_)) && e.is_retryable());
    }

    #[test]
    fn get_blob_range_over_http_refuses_a_200_that_ignores_range() {
        if !cfg!(debug_assertions) {
            return; // the local test server speaks plain HTTP, refused in release builds
        }
        let ok = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 10\r\nContent-Range: bytes 20-29/100\r\nConnection: close\r\n\r\n{}",
            "x".repeat(10)
        );
        let whole = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}",
            "y".repeat(100)
        );
        let (url, h) = serve(vec![ok, whole]);
        let t = HttpTransport::new(&url).unwrap();
        assert_eq!(
            t.get_blob_range("aa29c935-8ce3-4ab6-a443-d13cc3c73e5f", 20, 10)
                .unwrap(),
            b"xxxxxxxxxx"
        );
        let e = t
            .get_blob_range("aa29c935-8ce3-4ab6-a443-d13cc3c73e5f", 20, 10)
            .unwrap_err();
        assert!(matches!(e, RelayError::Network(_)), "{e:?}");
        let seen = h.join().unwrap();
        assert!(seen[0].to_ascii_lowercase().contains("range: bytes=20-29"));
    }

    #[test]
    fn timestamps_use_the_exact_wire_format() {
        assert_eq!(ts_format(1_793_836_800_123), "2026-11-05T00:00:00.123Z");
        assert_eq!(
            ts_parse("2026-11-05T00:00:00.123Z"),
            Some(1_793_836_800_123)
        );
        assert_eq!(ts_parse("not a date"), None);
        assert_eq!(ts_parse(&ts_format(0)), Some(0));
    }

    #[test]
    fn only_https_is_accepted_outside_the_local_debug_hosts() {
        assert!(HttpTransport::new("https://relay.example.org").is_ok());
        assert!(HttpTransport::new("http://relay.example.org").is_err());
        assert!(HttpTransport::new("ftp://x").is_err());
        assert!(HttpTransport::new("http://127.0.0.1:8080").is_ok() == cfg!(debug_assertions));
        assert!(HttpTransport::new("http://[::1]:8080").is_ok() == cfg!(debug_assertions));
        // User information is refused everywhere, debug builds included: the request would
        // go to the host after the `@`.
        for url in [
            "http://localhost:1@evil.com",
            "http://localhost@evil.com",
            "https://user:pw@relay.example.org",
            "http://127.0.0.1@evil.com/x",
        ] {
            assert!(HttpTransport::new(url).is_err(), "{url}");
        }
        for url in [
            "https://relay.example.org?x=1",
            "https://relay.example.org#f",
            "https://relay example.org",
            "https://:443",
            "https://relay.example.org:0",
            "https://relay.example.org:99999",
            "https://[::1",
            "https://[not-ipv6]:443",
            "https://[::1]x",
        ] {
            assert!(HttpTransport::new(url).is_err(), "{url}");
        }
    }

    #[test]
    fn base_url_with_a_path_or_ipv6_gives_the_right_doorbell_target() {
        let b = check_base_url("https://relay.example/tomi/").unwrap();
        assert_eq!(b.url, "https://relay.example/tomi");
        assert_eq!((b.host.as_str(), b.port), ("relay.example", 443));
        assert_eq!(b.ws_url(), "wss://relay.example/tomi/v1/ws");
        let b = check_base_url("https://[2001:db8::1]:8443").unwrap();
        assert_eq!((b.host.as_str(), b.port), ("2001:db8::1", 8443));
        assert_eq!(b.ws_url(), "wss://[2001:db8::1]:8443/v1/ws");
        let b = check_base_url("https://relay.example.org:8443/a/b").unwrap();
        assert_eq!((b.host.as_str(), b.port), ("relay.example.org", 8443));
        assert_eq!(b.ws_url(), "wss://relay.example.org:8443/a/b/v1/ws");
        if cfg!(debug_assertions) {
            let b = check_base_url("http://10.0.2.2:8080").unwrap();
            assert_eq!(b.ws_url(), "ws://10.0.2.2:8080/v1/ws");
            assert_eq!(b.port, 8080);
        }
    }

    #[test]
    fn redirects_are_not_followed() {
        if !cfg!(debug_assertions) {
            return; // the local test server speaks plain HTTP, refused in release builds
        }
        let (url, h) = serve(vec![reply(
            "302 Found",
            "Location: http://127.0.0.1:1/v1/hello\r\n",
            "",
        )]);
        let t = HttpTransport::new(&url).unwrap();
        let e = t.hello().unwrap_err();
        assert!(e.is_offline(), "{e:?}");
        assert_eq!(h.join().unwrap().len(), 1, "a single request, to the relay");
    }

    #[test]
    fn challenge_session_and_events_round_trip_over_http() {
        if !cfg!(debug_assertions) {
            return; // the local test server speaks plain HTTP, refused in release builds
        }
        let ch = B64.encode([5u8; 32]);
        let tok = "nvs1.abc";
        let space = r#"{"space_id":"6b5167fa-6c43-43da-a770-bb43fad022e2","created_at":"2026-11-04T09:00:01.250Z","effective_format":1,"key_epoch":1,"rotation_required":false,"head_seq":3,"server_time":"2026-11-04T09:00:02.000Z","quota":{"used_bytes":0,"max_bytes":2147483648},"limits":{"max_envelope_bytes":1114112,"max_blob_bytes":26279936,"max_item_blob_bytes":105119744,"max_blobs_per_item":32,"max_upload_part_bytes":1048576,"max_ttl_secs":2592000,"max_devices":20}}"#;
        let env = B64.encode([1u8, 2, 3]);
        let events = format!(
            r#"{{"events":[{{"seq":2,"type":"item","at":"2026-11-04T09:05:00.120Z","data":{{"item_id":"308339b6-1358-4bed-8328-23cc68b7f02b","sender_device_id":"2e2ad3e872079a9cbce60917f275457f","created_at":"2026-11-04T09:05:00.120Z","expires_at":"2026-11-11T09:05:00.000Z","key_epoch":1,"envelope":"{env}","envelope_size":3,"blobs":[{{"blob_id":"aa29c935-8ce3-4ab6-a443-d13cc3c73e5f","size":10}}],"ring":true}}}},{{"seq":3,"type":"from_the_future","at":"2026-11-04T09:05:01.000Z","data":{{}}}}],"head_seq":3,"has_more":false,"next_after":3}}"#
        );
        let (url, h) = serve(vec![
            reply(
                "200 OK",
                "",
                &format!(r#"{{"challenge":"{ch}","expires_at":"2026-11-04T09:01:01.250Z"}}"#),
            ),
            reply(
                "200 OK",
                "",
                &format!(
                    r#"{{"session_token":"{tok}","expires_at":"2026-11-04T10:00:02.000Z","device_id":"2e2ad3e872079a9cbce60917f275457f","space":{space}}}"#
                ),
            ),
            reply("200 OK", "", &events),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        let c = t
            .auth_challenge("2e2ad3e872079a9cbce60917f275457f")
            .unwrap();
        assert_eq!(c.challenge, vec![5u8; 32]);
        let g = t
            .auth_session(&SessionRequest {
                device_id: "2e2ad3e872079a9cbce60917f275457f".into(),
                challenge: c.challenge,
                signature: vec![9u8; 64],
                client: ClientInfo {
                    format_min: 1,
                    format_max: 1,
                    app_version: "0.1.0".into(),
                    platform: "android".into(),
                },
            })
            .unwrap();
        assert_eq!(g.token, tok);
        assert_eq!(g.space.head_seq, 3);
        assert_eq!(g.space.limits.max_upload_part_bytes, 1_048_576);
        t.set_session_token(Some(g.token));
        let page = t.events(1, 200).unwrap();
        assert_eq!(page.events.len(), 2);
        assert!(
            matches!(&page.events[0].data, EventData::Item { ring: true, blobs, envelope, .. } if blobs.len() == 1 && envelope == &[1, 2, 3])
        );
        assert!(
            matches!(&page.events[1].data, EventData::Unknown { kind } if kind == "from_the_future")
        );
        assert_eq!(page.next_after, 3);

        let seen = h.join().unwrap();
        // Requests: JSON bodies, snake_case, base64url without padding, Bearer token only after the session.
        assert!(seen[0].starts_with("POST /v1/auth/challenge "));
        assert!(seen[0].contains(r#""device_id":"2e2ad3e872079a9cbce60917f275457f""#));
        assert!(!seen[0].to_ascii_lowercase().contains("authorization"));
        assert!(seen[1].starts_with("POST /v1/auth/session "));
        assert!(seen[1].contains(&format!(r#""challenge":"{ch}""#)));
        assert!(seen[1].contains(r#""format_max":1"#));
        assert!(seen[2].starts_with("GET /v1/events?after=1&limit=200 "));
        assert!(seen[2]
            .to_ascii_lowercase()
            .contains("authorization: bearer nvs1.abc"));
    }

    #[test]
    fn problem_json_becomes_a_typed_error_with_extensions() {
        if !cfg!(debug_assertions) {
            return; // the local test server speaks plain HTTP, refused in release builds
        }
        let (url, h) = serve(vec![
            reply(
                "409 Conflict",
                "",
                r#"{"type":"urn:tomi:problem:stale_key_epoch","title":"Stale key epoch","status":409,"code":"stale_key_epoch","request_id":"358e7b4a-ca97-49a1-a89d-85a69096e034","key_epoch":2}"#,
            ),
            reply(
                "429 Too Many Requests",
                "Retry-After: 7\r\n",
                r#"{"status":429,"code":"rate_limited","request_id":"358e7b4a-ca97-49a1-a89d-85a69096e034","retry_after":7}"#,
            ),
            reply("503 Service Unavailable", "", "not json at all"),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        let item = PostItem {
            item_id: "308339b6-1358-4bed-8328-23cc68b7f02b".into(),
            expires_at_ms: 1_793_836_800_000,
            key_epoch: 1,
            envelope: vec![1, 2, 3],
            blob_ids: vec![],
        };
        let RelayError::Api(p) = t.post_item(&item).unwrap_err() else {
            panic!()
        };
        assert_eq!(
            (p.status, p.code.as_str(), p.key_epoch),
            (409, "stale_key_epoch", Some(2))
        );
        let RelayError::Api(p) = t
            .ack_item("308339b6-1358-4bed-8328-23cc68b7f02b", false)
            .unwrap_err()
        else {
            panic!()
        };
        assert_eq!((p.status, p.retry_after_secs), (429, Some(7)));
        let e = t
            .read_item("308339b6-1358-4bed-8328-23cc68b7f02b")
            .unwrap_err();
        assert!(e.is_retryable());
        let seen = h.join().unwrap();
        assert!(seen[0].contains(r#""expires_at":"2026-11-05T00:00:00.000Z""#));
        assert!(seen[0].contains(r#""envelope":"AQID""#));
    }

    #[test]
    fn unreachable_relay_is_a_network_error() {
        if !cfg!(debug_assertions) {
            return; // plain HTTP is refused in release builds
        }
        let t = HttpTransport::new("http://127.0.0.1:1").unwrap();
        assert!(t.hello().unwrap_err().is_offline());
    }

    #[test]
    fn changing_the_relay_drops_the_session_so_the_new_relay_never_sees_it() {
        if !cfg!(debug_assertions) {
            return; // the local test server speaks plain HTTP, refused in release builds
        }
        let (old, h_old) = serve(vec![reply("200 OK", "", "{}")]);
        let (new, h_new) = serve(vec![reply("200 OK", "", "{}")]);
        let t = HttpTransport::new(&old).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        // A relay that is refused changes nothing: not the relay, not the session.
        assert!(t.set_relay(Some("ftp://elsewhere.example.org")).is_err());
        assert_eq!(t.base_url().as_deref(), Some(old.as_str()));
        assert_eq!(t.bearer().as_deref(), Some("Bearer tms1.session"));
        let _ = t.delete_blob("b1");
        let seen = h_old.join().unwrap();
        assert!(seen[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer tms1.session"));
        // Another relay: the session is gone with the old target, whatever the order of reads.
        t.set_relay(Some(&new)).unwrap();
        assert_eq!(t.bearer(), None);
        let _ = t.delete_blob("b2");
        let seen = h_new.join().unwrap();
        assert!(
            !seen[0].to_ascii_lowercase().contains("authorization"),
            "the session of the old relay was sent to the new one: {}",
            seen[0]
        );
    }

    #[test]
    fn the_relay_authority_is_the_lowercase_host_without_a_default_port() {
        let authority = |url: &str| check_base_url(url).unwrap().relay_authority();
        assert_eq!(authority("https://relay.example.org"), "relay.example.org");
        assert_eq!(
            authority("https://Relay.Example.ORG:443/"),
            "relay.example.org"
        );
        assert_eq!(
            authority("https://relay.example.org:8443/a"),
            "relay.example.org:8443"
        );
        assert_eq!(authority("https://[2001:db8::1]"), "[2001:db8::1]");
        assert_eq!(
            authority("https://[2001:DB8::1]:8443"),
            "[2001:db8::1]:8443"
        );
        if cfg!(debug_assertions) {
            assert_eq!(authority("http://localhost:8080"), "localhost:8080");
            assert_eq!(authority("http://localhost"), "localhost");
            assert_eq!(authority("http://10.0.2.2:8080"), "10.0.2.2:8080");
            let t = HttpTransport::new("http://127.0.0.1:8080").unwrap();
            assert_eq!(t.relay_authority().as_deref(), Some("127.0.0.1:8080"));
        }
        // What the PAKE binds to is accepted as such by the machines.
        for url in [
            "https://relay.example.org",
            "https://relay.example.org:8443",
            "https://[2001:db8::1]:8443",
        ] {
            crate::pairing::Joiner::new(crate::pairing::JoinerParams {
                relay_authority: authority(url),
                code: crate::pairing::PairingCode::parse("4821-crayon-renard").unwrap(),
                device_name: "Pixel".into(),
                platform: "android".into(),
            })
            .unwrap();
        }
    }

    #[test]
    fn pairing_routes_carry_the_right_authorization_and_no_secret_in_the_url() {
        if !cfg!(debug_assertions) {
            return; // the local test server speaks plain HTTP, refused in release builds
        }
        let at = "2026-11-04T09:12:00.000Z";
        let token = "tmp1.uFJcfepRayMOiriiRhh-eL7yHvoUg3Ej";
        let list = format!(
            r#"{{"state":"claimed","expires_at":"{at}","messages":[{{"index":1,"side":"inviter","body":"{}"}},{{"index":2,"side":"from_the_future","body":"AAAA"}},{{"index":4,"side":"inviter","body":"{}"}}]}}"#,
            B64.encode([1u8, 2, 3]),
            B64.encode([9u8; 40])
        );
        let (url, h) = serve(vec![
            reply(
                "201 Created",
                "",
                &format!(r#"{{"channel_id":"4821","state":"open","expires_at":"{at}"}}"#),
            ),
            reply(
                "200 OK",
                "",
                &format!(r#"{{"pairing_token":"{token}","expires_at":"{at}"}}"#),
            ),
            reply("201 Created", "", r#"{"index":3}"#),
            reply("200 OK", "", &list),
            reply("201 Created", "", r#"{"index":5}"#),
            reply("204 No Content", "", ""),
            reply("204 No Content", "", ""),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        // A session is installed: the inviter's calls use it, the claim must not.
        t.set_session_token(Some("tms1.session".into()));

        let c = t.create_pairing_channel().unwrap();
        assert_eq!(
            (c.channel_id.as_str(), c.state),
            ("4821", ChannelState::Open)
        );
        let claim = t.claim_pairing_channel("4821").unwrap();
        assert_eq!(claim.pairing_token, token);
        assert!(!format!("{claim:?}").contains(token));
        let joiner = PairingAuth::Joiner(&claim.pairing_token);
        assert_eq!(
            t.post_pairing_message("4821", joiner, &[1, 2, 3]).unwrap(),
            3
        );
        let got = t.pairing_messages("4821", joiner, 0, 40).unwrap();
        assert_eq!(got.state, ChannelState::Claimed);
        // The side this build does not know is not a message for it.
        let seen: Vec<(u64, PairingSide, usize)> = got
            .messages
            .iter()
            .map(|m| (m.index, m.side, m.body.len()))
            .collect();
        assert_eq!(
            seen,
            vec![(1, PairingSide::Inviter, 3), (4, PairingSide::Inviter, 40)]
        );
        assert_eq!(
            t.post_pairing_message("4821", PairingAuth::Inviter, &[7])
                .unwrap(),
            5
        );
        t.close_pairing_channel("4821", PairingAuth::Inviter)
            .unwrap();
        t.close_pairing_channel("4821", joiner).unwrap();

        let seen = h.join().unwrap();
        let lower: Vec<String> = seen.iter().map(|r| r.to_ascii_lowercase()).collect();
        assert!(seen[0].starts_with("POST /v1/pairing/channels "));
        assert!(lower[0].contains("authorization: bearer tms1.session"));
        assert!(seen[1].starts_with("POST /v1/pairing/channels/4821/claim "));
        assert!(
            !lower[1].contains("authorization"),
            "a claim is made without authentication"
        );
        assert!(seen[2].starts_with("POST /v1/pairing/channels/4821/messages "));
        assert!(lower[2].contains(&format!(
            "authorization: pairing {}",
            token.to_ascii_lowercase()
        )));
        assert!(!lower[2].contains("bearer"));
        assert!(seen[2].contains(r#""body":"AQID""#));
        // The wait is capped at 25 s, and the query holds the index and the wait only.
        assert!(seen[3].starts_with("GET /v1/pairing/channels/4821/messages?after=0&wait=25 "));
        assert!(lower[3].contains("authorization: pairing"));
        assert!(seen[4].starts_with("POST /v1/pairing/channels/4821/messages "));
        assert!(lower[4].contains("authorization: bearer tms1.session"));
        assert!(seen[5].starts_with("DELETE /v1/pairing/channels/4821 "));
        assert!(lower[5].contains("authorization: bearer tms1.session"));
        assert!(seen[6].starts_with("DELETE /v1/pairing/channels/4821 "));
        assert!(lower[6].contains("authorization: pairing"));
        for r in &seen {
            let line = r.lines().next().unwrap_or_default();
            assert!(
                !line.contains("tmp1.") && !line.contains("tms1."),
                "no token in a URL: {line}"
            );
        }
    }

    #[test]
    fn a_channel_id_is_checked_before_it_is_written_into_a_path() {
        if !cfg!(debug_assertions) {
            return;
        }
        let (url, h) = serve(vec![reply(
            "201 Created",
            "",
            r#"{"channel_id":"../v1/me","state":"open","expires_at":"2026-11-04T09:12:00.000Z"}"#,
        )]);
        let t = HttpTransport::new(&url).unwrap();
        for bad in [
            "",
            "0",
            "01",
            "12345",
            "../me",
            "4821/claim",
            "48 1",
            "4821?x=1",
        ] {
            let e = t.claim_pairing_channel(bad).unwrap_err();
            assert!(e.is_offline(), "{bad:?}: {e:?}");
            assert!(t.pairing_messages(bad, PairingAuth::Inviter, 0, 0).is_err());
            assert!(t.close_pairing_channel(bad, PairingAuth::Inviter).is_err());
        }
        // And an id the relay invents is a protocol error, not something to build a path from.
        assert!(t.create_pairing_channel().is_err());
        // A token that could not be a header value is refused before any request.
        assert!(t
            .pairing_messages("4821", PairingAuth::Joiner("tmp1.a b"), 0, 0)
            .is_err());
        assert!(t
            .pairing_messages("4821", PairingAuth::Joiner(""), 0, 0)
            .is_err());
        assert_eq!(
            h.join().unwrap().len(),
            1,
            "only the creation reached the server"
        );
    }

    #[test]
    fn space_and_device_registration_use_the_documented_bodies_and_authentication() {
        if !cfg!(debug_assertions) {
            return;
        }
        let record = |id: &str| {
            format!(
                r#"{{"device_id":"{id}","name":"Pixel","platform":"android","sign_pk":"{k}","kx_pk":"{k}","certificate":"AQID","issuer_device_id":"2e2ad3e872079a9cbce60917f275457f","joined_seq":7,"registered_at":"2026-11-04T09:00:01.250Z","active":true,"present":false,"push":false}}"#,
                k = B64.encode([5u8; 32])
            )
        };
        let (url, h) = serve(vec![
            reply(
                "201 Created",
                "",
                r#"{"space_id":"6b5167fa-6c43-43da-a770-bb43fad022e2","created_at":"2026-11-04T09:00:01.250Z","device_id":"2e2ad3e872079a9cbce60917f275457f","key_epoch":1,"head_seq":1}"#,
            ),
            reply(
                "201 Created",
                "",
                &record("3f9a0c6e1b7d4e2a8c5f0b9d7e6a1c34"),
            ),
            reply(
                "409 Conflict",
                "",
                r#"{"status":409,"code":"epoch_conflict","key_epoch":2,"request_id":"358e7b4a-ca97-49a1-a89d-85a69096e034"}"#,
            ),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        let space = NewSpace {
            install_token: "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns".into(),
            space_id: "6b5167fa-6c43-43da-a770-bb43fad022e2".into(),
            name: "PC-Bureau".into(),
            platform: "windows".into(),
            certificate: vec![1, 2, 3],
        };
        let created = t.post_space(&space).unwrap();
        assert_eq!(created.device_id, "2e2ad3e872079a9cbce60917f275457f");
        assert_eq!((created.key_epoch, created.head_seq), (1, 1));
        assert!(!format!("{space:?}").contains("h7k2"));

        let device = NewDevice {
            channel_id: "4821".into(),
            key_epoch: 1,
            name: "Pixel".into(),
            platform: "android".into(),
            certificate: vec![1, 2, 3],
        };
        let rec = t.post_device(&device).unwrap();
        assert_eq!((rec.name.as_str(), rec.joined_seq), ("Pixel", 7));
        assert_eq!(rec.certificate, vec![1, 2, 3]);
        let RelayError::Api(p) = t.post_device(&device).unwrap_err() else {
            panic!()
        };
        assert_eq!(
            (p.status, p.code.as_str(), p.key_epoch),
            (409, "epoch_conflict", Some(2))
        );

        let seen = h.join().unwrap();
        let lower: Vec<String> = seen.iter().map(|r| r.to_ascii_lowercase()).collect();
        assert!(seen[0].starts_with("POST /v1/space "));
        assert!(
            !lower[0].contains("authorization"),
            "no session to create a space"
        );
        for field in [
            r#""install_token":"h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns""#,
            r#""space_id":"6b5167fa-6c43-43da-a770-bb43fad022e2""#,
            r#""name":"PC-Bureau""#,
            r#""platform":"windows""#,
            r#""certificate":"AQID""#,
        ] {
            assert!(seen[0].contains(field), "{field}");
        }
        assert!(seen[1].starts_with("POST /v1/devices "));
        assert!(lower[1].contains("authorization: bearer tms1.session"));
        for field in [
            r#""channel_id":"4821""#,
            r#""key_epoch":1"#,
            r#""name":"Pixel""#,
            r#""platform":"android""#,
            r#""certificate":"AQID""#,
        ] {
            assert!(seen[1].contains(field), "{field}");
        }
    }

    #[test]
    fn leaving_is_a_delete_of_v1_me_with_the_session() {
        if !cfg!(debug_assertions) {
            return;
        }
        let (url, h) = serve(vec![
            reply("200 OK", "", r#"{"seq":12,"rotation_required":true}"#),
            reply(
                "409 Conflict",
                "",
                r#"{"status":409,"code":"last_device","request_id":"358e7b4a-ca97-49a1-a89d-85a69096e034"}"#,
            ),
            // A `2xx` that does not carry the departure's `seq` proves nothing.
            reply("200 OK", "", "{}"),
            reply("200 OK", "", ""),
            reply("200 OK", "", r#"{"seq":"12"}"#),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        t.leave_space().unwrap();
        let RelayError::Api(p) = t.leave_space().unwrap_err() else {
            panic!("an API refusal was expected")
        };
        assert_eq!((p.status, p.code.as_str()), (409, "last_device"));
        for _ in 0..3 {
            assert!(
                matches!(t.leave_space(), Err(RelayError::Network(_))),
                "an answer without `seq` is not a departure"
            );
        }
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("DELETE /v1/me "), "{}", seen[0]);
        assert!(seen[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer tms1.session"));
        assert!(seen[1].starts_with("DELETE /v1/me "));
    }

    #[test]
    fn a_departure_answer_that_is_cut_unreadable_or_not_an_integer_proves_nothing() {
        if !cfg!(debug_assertions) {
            return;
        }
        let (url, h) = serve(vec![
            // Cut in the middle of the body: fewer bytes than announced, then the connection
            // closes.
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 40\r\nConnection: close\r\n\r\n{\"seq\":1".to_owned(),
            reply("200 OK", "", "pas du json"),
            // A floating point `seq` is not the integer sequence number of the event.
            reply("200 OK", "", r#"{"seq":12.5}"#),
            reply("200 OK", "", r#"{"seq":12.0}"#),
            reply("200 OK", "", r#"{"seq":-3}"#),
            reply("200 OK", "", r#"{"seq":null}"#),
            reply("200 OK", "", r#"{"seq":true}"#),
            reply("200 OK", "", "[12]"),
            // The control: an integer.
            reply("200 OK", "", r#"{"seq":12}"#),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        for i in 0..8 {
            assert!(
                matches!(t.leave_space(), Err(RelayError::Network(_))),
                "answer {i} is not a departure"
            );
        }
        t.leave_space().unwrap();
        h.join().unwrap();
    }

    #[test]
    fn a_403_that_is_cut_or_unreadable_is_never_a_revocation() {
        if !cfg!(debug_assertions) {
            return;
        }
        let (url, h) = serve(vec![
            // The control: a complete problem document is a typed `403 revoked`.
            reply("403 Forbidden", "", r#"{"status":403,"code":"revoked"}"#),
            // Cut in the middle: the code is not known, so it is a network error.
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/problem+json\r\nContent-Length: 80\r\nConnection: close\r\n\r\n{\"status\":403,\"code\":\"rev".to_owned(),
            // A proxy or a captive portal in front of the relay.
            reply("403 Forbidden", "", "<html>forbidden</html>"),
            reply("403 Forbidden", "", ""),
        ]);
        let t = HttpTransport::new(&url).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        let RelayError::Api(p) = t.leave_space().unwrap_err() else {
            panic!("a complete `403 revoked` is typed")
        };
        assert_eq!((p.status, p.code.as_str()), (403, "revoked"));
        assert!(
            matches!(t.leave_space(), Err(RelayError::Network(_))),
            "a cut 403 is not a revocation"
        );
        for _ in 0..2 {
            let RelayError::Api(p) = t.leave_space().unwrap_err() else {
                panic!("an unreadable 403 is an API refusal without a known code")
            };
            assert_eq!(p.status, 403);
            assert_ne!(
                p.code, "revoked",
                "only the relay's own code is a revocation"
            );
        }
        h.join().unwrap();
    }

    /// Canned answers shared by the two routes that prove a deletion: only the first one is a
    /// proof (`api-v1.md` §7.9.1).
    fn deletion_answers() -> Vec<String> {
        vec![
            reply("200 OK", "", r#"{"space_deleted":true}"#),
            reply("200 OK", "", r#"{"space_deleted":"true"}"#),
            reply("200 OK", "", r#"{"space_deleted":1}"#),
            reply("200 OK", "", r#"{"space_deleted":null}"#),
            reply("200 OK", "", "{}"),
            reply("200 OK", "", ""),
            reply("200 OK", "", "pas du json"),
            // Cut in the middle: fewer bytes than announced, then the connection closes.
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 40\r\nConnection: close\r\n\r\n{\"space_deleted\":tr".to_owned(),
            reply(
                "409 Conflict",
                "",
                r#"{"status":409,"code":"not_last_device"}"#,
            ),
            reply("404 Not Found", "", r#"{"status":404,"code":"not_found"}"#),
        ]
    }

    /// Runs `call` against [`deletion_answers`] and checks the verdict of each answer.
    fn check_deletion_answers(call: impl Fn() -> TResult<()>) {
        call().unwrap();
        for i in 0..7 {
            assert!(
                matches!(call(), Err(RelayError::Network(_))),
                "answer {i} is not a proof"
            );
        }
        let RelayError::Api(p) = call().unwrap_err() else {
            panic!("an API refusal was expected")
        };
        assert_eq!((p.status, p.code.as_str()), (409, "not_last_device"));
        let RelayError::Api(p) = call().unwrap_err() else {
            panic!("an API refusal was expected")
        };
        assert_eq!(p.status, 404);
    }

    #[test]
    fn deleting_the_space_is_proven_only_by_the_boolean_true() {
        if !cfg!(debug_assertions) {
            return;
        }
        let id = [7u8; 32];
        let (url, h) = serve(deletion_answers());
        let t = HttpTransport::new(&url).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        check_deletion_answers(|| t.delete_space(&id));
        let seen = h.join().unwrap();
        let encoded = B64.encode(id);
        for req in &seen {
            assert!(req.starts_with("DELETE /v1/space "), "{req}");
            let lower = req.to_ascii_lowercase();
            assert!(lower.contains("content-type: application/json"));
            assert!(lower.contains("authorization: bearer tms1.session"));
            assert!(
                req.contains(&format!(r#""deletion_id":"{encoded}""#)),
                "{req}"
            );
        }
    }

    #[test]
    fn the_receipt_is_asked_without_session_and_proven_only_by_the_boolean_true() {
        if !cfg!(debug_assertions) {
            return;
        }
        let id = [8u8; 32];
        let (url, h) = serve(deletion_answers());
        let t = HttpTransport::new(&url).unwrap();
        t.set_session_token(Some("tms1.session".into()));
        check_deletion_answers(|| t.space_deletion_receipt(&id));
        let seen = h.join().unwrap();
        let encoded = B64.encode(id);
        for req in &seen {
            assert!(req.starts_with("POST /v1/space/receipt "), "{req}");
            let lower = req.to_ascii_lowercase();
            assert!(!lower.contains("authorization:"), "no session: {req}");
            assert!(lower.contains("content-type: application/json"));
            assert!(
                req.contains(&format!(r#""deletion_id":"{encoded}""#)),
                "{req}"
            );
            assert!(
                !req.split("\r\n").next().unwrap().contains(&encoded),
                "not in the URL"
            );
        }
    }

    #[test]
    fn wake_messages_are_parsed_and_others_ignored() {
        assert_eq!(
            parse_wake(r#"{"type":"wake","seq":44,"ring":true}"#),
            Some(Wake {
                seq: 44,
                ring: true
            })
        );
        assert_eq!(parse_wake(r#"{"type":"ready","head_seq":1}"#), None);
        assert_eq!(parse_wake("garbage"), None);
    }

    fn device_json(extra: Value) -> Value {
        let mut v = json!({
            "device_id": "d",
            "name": "Pixel",
            "platform": "android",
            "sign_pk": B64.encode(b"s"),
            "kx_pk": B64.encode(b"k"),
            "certificate": B64.encode(b"c"),
            "issuer_device_id": "d",
            "joined_seq": 3,
            "registered_at": "2026-11-01T10:00:00Z",
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    }

    #[test]
    fn a_device_record_carries_the_activity_the_relay_reports() {
        let d = device_record(&device_json(json!({
            "last_seen_at": "2026-11-08T09:30:00Z",
            "active": true,
        })))
        .unwrap();
        assert_eq!(d.last_seen_at_ms, ts_parse("2026-11-08T09:30:00Z"));
        assert!(d.last_seen_at_ms.is_some());
        assert_eq!(d.active, Some(true));
    }

    #[test]
    fn a_device_record_without_last_seen_means_never_connected() {
        let d = device_record(&device_json(json!({ "active": false }))).unwrap();
        assert_eq!(d.last_seen_at_ms, None);
        assert_eq!(d.active, Some(false));
    }

    #[test]
    fn a_device_record_of_an_older_relay_is_tolerated() {
        let d = device_record(&device_json(json!({}))).unwrap();
        assert_eq!((d.last_seen_at_ms, d.active), (None, None));
        // A malformed value informs nothing and breaks nothing.
        let d = device_record(&device_json(json!({ "last_seen_at": 7, "active": "yes" }))).unwrap();
        assert_eq!((d.last_seen_at_ms, d.active), (None, None));
    }
}
