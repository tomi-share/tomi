//! Error model of `api-v1.md` §2.5: RFC 9457 `application/problem+json` documents with a
//! stable machine `code`.

use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};

tokio::task_local! {
    /// `X-Request-Id` of the request being served, repeated in every error document and log.
    pub static REQUEST_ID: uuid::Uuid;
}

/// Device authenticated by the current request, for the access log only.
pub type DeviceSlot = std::sync::Arc<std::sync::Mutex<Option<String>>>;

tokio::task_local! {
    pub static REQUEST_DEVICE: DeviceSlot;
}

/// Remembers the authenticated device of the current request for the access log.
pub fn log_device(device_id: &str) {
    let _ = REQUEST_DEVICE.try_with(|slot| {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(device_id.to_owned());
    });
}

/// Response extension carrying the `code` of an error response (read by the access log).
#[derive(Debug, Clone, Copy)]
pub struct ErrorCode(pub &'static str);

/// Stable codes of the table of §2.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    BadRequest,
    ValidationFailed,
    Unauthorized,
    SessionExpired,
    InstallTokenInvalid,
    Revoked,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    SpaceExists,
    SpaceNotInitialized,
    UpdateRequired,
    FormatUnsupported,
    ItemIdConflict,
    EventIdConflict,
    BlobIdConflict,
    BlobIncomplete,
    BlobAttached,
    TransferIdConflict,
    TransferLimit,
    UploadOffsetMismatch,
    StaleKeyEpoch,
    EpochConflict,
    DeviceSetMismatch,
    DeviceExists,
    LastDevice,
    NotLastDevice,
    TooManyDevices,
    ChannelClaimed,
    ChannelClosed,
    ChannelLimit,
    CursorExpired,
    CursorAhead,
    AlreadyExpired,
    LengthRequired,
    PayloadTooLarge,
    UnsupportedMediaType,
    RangeNotSatisfiable,
    RateLimited,
    Internal,
    Unavailable,
    RelayFull,
}

impl Code {
    /// The `code` string of the document.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::BadRequest => "bad_request",
            Code::ValidationFailed => "validation_failed",
            Code::Unauthorized => "unauthorized",
            Code::SessionExpired => "session_expired",
            Code::InstallTokenInvalid => "install_token_invalid",
            Code::Revoked => "revoked",
            Code::Forbidden => "forbidden",
            Code::NotFound => "not_found",
            Code::MethodNotAllowed => "method_not_allowed",
            Code::SpaceExists => "space_exists",
            Code::SpaceNotInitialized => "space_not_initialized",
            Code::UpdateRequired => "update_required",
            Code::FormatUnsupported => "format_unsupported",
            Code::ItemIdConflict => "item_id_conflict",
            Code::EventIdConflict => "event_id_conflict",
            Code::BlobIdConflict => "blob_id_conflict",
            Code::BlobIncomplete => "blob_incomplete",
            Code::BlobAttached => "blob_attached",
            Code::TransferIdConflict => "transfer_id_conflict",
            Code::TransferLimit => "transfer_limit",
            Code::UploadOffsetMismatch => "upload_offset_mismatch",
            Code::StaleKeyEpoch => "stale_key_epoch",
            Code::EpochConflict => "epoch_conflict",
            Code::DeviceSetMismatch => "device_set_mismatch",
            Code::DeviceExists => "device_exists",
            Code::LastDevice => "last_device",
            Code::NotLastDevice => "not_last_device",
            Code::TooManyDevices => "too_many_devices",
            Code::ChannelClaimed => "channel_claimed",
            Code::ChannelClosed => "channel_closed",
            Code::ChannelLimit => "channel_limit",
            Code::CursorExpired => "cursor_expired",
            Code::CursorAhead => "cursor_ahead",
            Code::AlreadyExpired => "already_expired",
            Code::LengthRequired => "length_required",
            Code::PayloadTooLarge => "payload_too_large",
            Code::UnsupportedMediaType => "unsupported_media_type",
            Code::RangeNotSatisfiable => "range_not_satisfiable",
            Code::RateLimited => "rate_limited",
            Code::Internal => "internal",
            Code::Unavailable => "unavailable",
            Code::RelayFull => "relay_full",
        }
    }

    /// Default HTTP status of the code.
    pub fn status(self) -> StatusCode {
        match self {
            Code::BadRequest => StatusCode::BAD_REQUEST,
            Code::ValidationFailed | Code::AlreadyExpired => StatusCode::UNPROCESSABLE_ENTITY,
            Code::Unauthorized | Code::SessionExpired => StatusCode::UNAUTHORIZED,
            Code::InstallTokenInvalid | Code::Revoked | Code::Forbidden => StatusCode::FORBIDDEN,
            Code::NotFound => StatusCode::NOT_FOUND,
            Code::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Code::CursorExpired => StatusCode::GONE,
            Code::LengthRequired => StatusCode::LENGTH_REQUIRED,
            Code::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Code::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Code::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            Code::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Code::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Code::RelayFull => StatusCode::INSUFFICIENT_STORAGE,
            _ => StatusCode::CONFLICT,
        }
    }

    /// Short English label, stable per code.
    pub fn title(self) -> &'static str {
        match self {
            Code::BadRequest => "Bad request",
            Code::ValidationFailed => "Validation failed",
            Code::Unauthorized => "Unauthorized",
            Code::SessionExpired => "Session expired",
            Code::InstallTokenInvalid => "Install token invalid",
            Code::Revoked => "Device revoked",
            Code::Forbidden => "Forbidden",
            Code::NotFound => "Not found",
            Code::MethodNotAllowed => "Method not allowed",
            Code::SpaceExists => "Space exists",
            Code::SpaceNotInitialized => "Space not initialized",
            Code::UpdateRequired => "Update required",
            Code::FormatUnsupported => "Format unsupported",
            Code::ItemIdConflict => "Item id conflict",
            Code::EventIdConflict => "Event id conflict",
            Code::BlobIdConflict => "Blob id conflict",
            Code::BlobIncomplete => "Blob incomplete",
            Code::BlobAttached => "Blob attached",
            Code::TransferIdConflict => "Transfer id conflict",
            Code::TransferLimit => "Transfer limit reached",
            Code::UploadOffsetMismatch => "Upload offset mismatch",
            Code::StaleKeyEpoch => "Stale key epoch",
            Code::EpochConflict => "Epoch conflict",
            Code::DeviceSetMismatch => "Device set mismatch",
            Code::DeviceExists => "Device exists",
            Code::LastDevice => "Last device",
            Code::NotLastDevice => "Other devices remain",
            Code::TooManyDevices => "Too many devices",
            Code::ChannelClaimed => "Channel already claimed",
            Code::ChannelClosed => "Channel closed",
            Code::ChannelLimit => "Channel limit reached",
            Code::CursorExpired => "Cursor expired",
            Code::CursorAhead => "Cursor ahead of the log",
            Code::AlreadyExpired => "Already expired",
            Code::LengthRequired => "Length required",
            Code::PayloadTooLarge => "Payload too large",
            Code::UnsupportedMediaType => "Unsupported media type",
            Code::RangeNotSatisfiable => "Range not satisfiable",
            Code::RateLimited => "Rate limited",
            Code::Internal => "Internal error",
            Code::Unavailable => "Service unavailable",
            Code::RelayFull => "Relay full",
        }
    }
}

/// One entry of the `errors` array of a `validation_failed` document.
#[derive(Debug, Clone)]
pub struct FieldError {
    /// JSON pointer of the offending field (`/certificate`).
    pub field: String,
    /// English reason. Never contains the value received.
    pub reason: String,
}

/// An API error, rendered as `application/problem+json`.
#[derive(Debug)]
pub struct ApiError {
    pub code: Code,
    pub status: StatusCode,
    pub detail: Option<String>,
    pub errors: Vec<FieldError>,
    pub ext: Map<String, Value>,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    /// Internal explanation, written to the logs only (never sent).
    pub internal: Option<String>,
}

impl ApiError {
    pub fn new(code: Code) -> Self {
        Self {
            status: code.status(),
            code,
            detail: None,
            errors: Vec::new(),
            ext: Map::new(),
            headers: Vec::new(),
            internal: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Adds an extension member (`key_epoch`, `upload_offset`...).
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.ext.insert(key.to_owned(), value.into());
        self
    }

    pub fn header(mut self, name: HeaderName, value: &str) -> Self {
        if let Ok(v) = HeaderValue::from_str(value) {
            self.headers.push((name, v));
        }
        self
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(Code::BadRequest).detail(detail)
    }

    /// `422 validation_failed` for one field.
    pub fn field(field: &str, reason: &str) -> Self {
        Self::validation(vec![FieldError {
            field: field.to_owned(),
            reason: reason.to_owned(),
        }])
    }

    pub fn validation(errors: Vec<FieldError>) -> Self {
        let mut e = Self::new(Code::ValidationFailed).detail("One or more fields are invalid.");
        e.errors = errors;
        e
    }

    pub fn unauthorized() -> Self {
        Self::new(Code::Unauthorized).detail("Missing, unknown or invalid credentials.")
    }

    pub fn not_found() -> Self {
        Self::new(Code::NotFound).detail("Unknown resource.")
    }

    pub fn forbidden() -> Self {
        Self::new(Code::Forbidden)
    }

    pub fn revoked(revoked_at_ms: Option<i64>) -> Self {
        let mut e = Self::new(Code::Revoked).detail("This device has been revoked.");
        if let Some(ms) = revoked_at_ms {
            e = e.with("revoked_at", crate::wire::ts_format(ms));
        }
        e
    }

    pub fn payload_too_large(limit: u64) -> Self {
        Self::new(Code::PayloadTooLarge)
            .detail("The payload exceeds the limit.")
            .with("limit", limit)
    }

    pub fn rate_limited(retry_after_secs: u64) -> Self {
        let secs = retry_after_secs.max(1);
        Self::new(Code::RateLimited)
            .detail("Rate limit reached.")
            .with("retry_after", secs)
            .header(header::RETRY_AFTER, &secs.to_string())
    }

    pub fn relay_full(used_bytes: u64, max_bytes: u64) -> Self {
        Self::new(Code::RelayFull)
            .detail("The relay has no room left for this request.")
            .with("used_bytes", used_bytes)
            .with("max_bytes", max_bytes)
    }

    pub fn unavailable() -> Self {
        Self::new(Code::Unavailable).detail("The relay is temporarily unavailable.")
    }

    /// `503 unavailable` with `Retry-After: 5`, for an identifier of a space that is "pending"
    /// (its deletion is under way, or its state is unknown until the next start, `api-v1.md`
    /// §4.4.1). Neither "deleted" nor "unknown": the client tries again later.
    pub fn pending() -> Self {
        Self::unavailable().header(header::RETRY_AFTER, "5")
    }

    /// `409 space_not_initialized`: only when the relay hosts no space at all. An identifier that
    /// designates no space of a non-empty relay gets the "unknown" answer of its route instead.
    pub fn space_not_initialized() -> Self {
        Self::new(Code::SpaceNotInitialized).detail("No space exists on this relay.")
    }

    /// `500 internal`: the explanation goes to the logs, not to the client.
    pub fn internal(why: impl std::fmt::Display) -> Self {
        let mut e = Self::new(Code::Internal);
        e.internal = Some(why.to_string());
        e
    }

    /// `409 update_required` (retired API versions answer `410`).
    pub fn update_required(effective_format: Option<u32>, update_url: &str, retired: bool) -> Self {
        let mut e = Self::new(Code::UpdateRequired).with("update_url", update_url);
        if let Some(f) = effective_format {
            e = e.with("effective_format", f);
        }
        if retired {
            e.status = StatusCode::GONE;
        }
        e
    }

    /// Renders the document for a given request id.
    pub fn body(&self, request_id: uuid::Uuid) -> Value {
        let mut m = Map::new();
        m.insert(
            "type".into(),
            json!(format!("urn:tomi:problem:{}", self.code.as_str())),
        );
        m.insert("title".into(), json!(self.code.title()));
        m.insert("status".into(), json!(self.status.as_u16()));
        m.insert("code".into(), json!(self.code.as_str()));
        if let Some(d) = &self.detail {
            m.insert("detail".into(), json!(d));
        }
        m.insert(
            "request_id".into(),
            json!(request_id.hyphenated().to_string()),
        );
        if !self.errors.is_empty() {
            let list: Vec<Value> = self
                .errors
                .iter()
                .map(|e| json!({ "field": e.field, "reason": e.reason }))
                .collect();
            m.insert("errors".into(), Value::Array(list));
        }
        for (k, v) in &self.ext {
            m.insert(k.clone(), v.clone());
        }
        Value::Object(m)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.code.as_str())
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_id = REQUEST_ID
            .try_with(|id| *id)
            .unwrap_or_else(|_| uuid::Uuid::nil());
        if let Some(why) = &self.internal {
            tracing::error!(request_id = %request_id, code = self.code.as_str(), "internal error: {why}");
        }
        let body = serde_json::to_vec(&self.body(request_id)).unwrap_or_default();
        let mut resp = Response::new(axum::body::Body::from(body));
        *resp.status_mut() = self.status;
        let h = resp.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        for (k, v) in self.headers {
            h.insert(k, v);
        }
        resp.extensions_mut().insert(ErrorCode(self.code.as_str()));
        resp
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(f, _) = &e {
            match f.code {
                rusqlite::ErrorCode::DiskFull => {
                    let mut err = ApiError::new(Code::RelayFull).detail("Disk full.");
                    err.internal = Some(e.to_string());
                    return err;
                }
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                    let mut err = ApiError::unavailable();
                    err.internal = Some(e.to_string());
                    return err;
                }
                _ => {}
            }
        }
        ApiError::internal(format!("database: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::StorageFull {
            let mut err = ApiError::new(Code::RelayFull).detail("Disk full.");
            err.internal = Some(e.to_string());
            return err;
        }
        ApiError::internal(format!("io: {e}"))
    }
}

/// Result alias used by the handlers.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_has_the_fields_of_the_spec() {
        let id = uuid::Uuid::nil();
        let e = ApiError::field("/certificate", "bad signature");
        let b = e.body(id);
        assert_eq!(b["type"], "urn:tomi:problem:validation_failed");
        assert_eq!(b["status"], 422);
        assert_eq!(b["code"], "validation_failed");
        assert_eq!(b["errors"][0]["field"], "/certificate");
        assert_eq!(b["request_id"], id.hyphenated().to_string());
    }

    #[test]
    fn statuses_follow_the_table() {
        assert_eq!(Code::SessionExpired.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(Code::CursorAhead.status(), StatusCode::CONFLICT);
        assert_eq!(Code::CursorExpired.status(), StatusCode::GONE);
        assert_eq!(Code::RelayFull.status(), StatusCode::INSUFFICIENT_STORAGE);
        assert_eq!(
            Code::AlreadyExpired.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let e = ApiError::update_required(Some(2), "https://x/updates/", true);
        assert_eq!(e.status, StatusCode::GONE);
        let e = ApiError::update_required(Some(2), "https://x/updates/", false);
        assert_eq!(e.status, StatusCode::CONFLICT);
    }

    #[test]
    fn internal_errors_never_expose_their_cause() {
        let e = ApiError::internal("secret path /var/lib/x");
        let b = e.body(uuid::Uuid::nil());
        assert!(b.get("detail").is_none());
        assert!(!b.to_string().contains("secret path"));
    }
}
