//! Request bodies (`api-v1.md` §2.2, §2.3): content type, size, JSON, and field-by-field
//! validation with the JSON pointer of each offending field. Unknown fields are ignored and
//! `null` equals "absent".

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use serde_json::{Map, Value};

use crate::error::{ApiError, FieldError};
use crate::wire;

/// Is the request's `Content-Type` the given media type (parameters ignored)?
pub fn has_media_type(parts: &Parts, wanted: &str) -> bool {
    parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case(wanted))
        })
}

/// Reads a body of at most `limit` bytes (`413` beyond, checked on `Content-Length` first).
pub async fn read_limited(parts: &Parts, body: Body, limit: u64) -> Result<Bytes, ApiError> {
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Err(ApiError::payload_too_large(limit));
    }
    let cap = usize::try_from(limit).unwrap_or(usize::MAX);
    match Limited::new(body, cap).collect().await {
        Ok(c) => Ok(c.to_bytes()),
        Err(e) => {
            if e.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some()
            {
                Err(ApiError::payload_too_large(limit))
            } else {
                Err(ApiError::bad_request("The request body could not be read."))
            }
        }
    }
}

/// Reads and parses a JSON object body. With `required = false` an empty body is `{}`.
pub async fn json_body(
    parts: &Parts,
    body: Body,
    limit: u64,
    required: bool,
) -> Result<Obj, ApiError> {
    let has_type = parts.headers.contains_key(CONTENT_TYPE);
    if (required || has_type) && !has_media_type(parts, "application/json") {
        // §2.2: any other Content-Type is refused (an optional body may be absent altogether).
        let declared = parts
            .headers
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        if required || declared != Some(0) {
            return Err(ApiError::new(crate::error::Code::UnsupportedMediaType)
                .detail("Content-Type must be application/json."));
        }
    }
    let bytes = read_limited(parts, body, limit).await?;
    if bytes.is_empty() {
        if required {
            return Err(ApiError::bad_request("The request body is empty."));
        }
        return Ok(Obj::new(Map::new(), String::new()));
    }
    if !required && !has_type {
        return Err(ApiError::new(crate::error::Code::UnsupportedMediaType)
            .detail("Content-Type must be application/json."));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::bad_request("The request body is not valid JSON."))?;
    match value {
        Value::Object(m) => Ok(Obj::new(m, String::new())),
        _ => Err(ApiError::field("", "expected a JSON object")),
    }
}

/// An integer query parameter (`after`, `limit`, `wait`). `Ok(None)` if absent; a malformed
/// or out-of-range value is a field error (the value itself is never echoed).
pub fn query_uint(
    parts: &Parts,
    name: &str,
    min: u64,
    max: u64,
) -> Result<Option<u64>, FieldError> {
    let Some(q) = parts.uri.query() else {
        return Ok(None);
    };
    let Some((_, v)) = form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == name) else {
        return Ok(None);
    };
    let bad = |reason: &str| FieldError {
        field: format!("/{name}"),
        reason: reason.to_owned(),
    };
    let n: u64 = v
        .parse()
        .map_err(|_| bad("expected a non-negative integer"))?;
    if n < min || n > max.min(wire::JSON_INT_MAX) {
        return Err(bad("out of range"));
    }
    Ok(Some(n))
}

/// A JSON object being validated. Errors accumulate; [`Obj::finish`] turns them into one
/// `422 validation_failed`.
pub struct Obj {
    map: Map<String, Value>,
    prefix: String,
    errors: Vec<FieldError>,
}

impl Obj {
    fn new(map: Map<String, Value>, prefix: String) -> Self {
        Self {
            map,
            prefix,
            errors: Vec::new(),
        }
    }

    /// Builds an `Obj` from a value (tests, nested bodies).
    pub fn from_map(map: Map<String, Value>) -> Self {
        Self::new(map, String::new())
    }

    fn path(&self, key: &str) -> String {
        format!("{}/{}", self.prefix, key)
    }

    /// Records an error on `key`.
    pub fn error(&mut self, key: &str, reason: &str) {
        let field = self.path(key);
        self.errors.push(FieldError {
            field,
            reason: reason.to_owned(),
        });
    }

    fn value(&self, key: &str) -> Option<&Value> {
        self.map.get(key).filter(|v| !v.is_null())
    }

    /// Whether `key` is present (and not null).
    pub fn has(&self, key: &str) -> bool {
        self.value(key).is_some()
    }

    fn required_missing(&mut self, key: &str, required: bool) {
        if required {
            self.error(key, "required");
        }
    }

    pub fn string(&mut self, key: &str, required: bool) -> Option<String> {
        match self.value(key) {
            None => {
                self.required_missing(key, required);
                None
            }
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                self.error(key, "expected a string");
                None
            }
        }
    }

    pub fn boolean(&mut self, key: &str, required: bool) -> Option<bool> {
        match self.value(key) {
            None => {
                self.required_missing(key, required);
                None
            }
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => {
                self.error(key, "expected a boolean");
                None
            }
        }
    }

    /// An integer in `min..=max` (and never above 2^53 - 1, §2.2).
    pub fn uint(&mut self, key: &str, required: bool, min: u64, max: u64) -> Option<u64> {
        match self.value(key) {
            None => {
                self.required_missing(key, required);
                None
            }
            Some(v) => match v.as_u64() {
                Some(n) if n >= min && n <= max.min(wire::JSON_INT_MAX) => Some(n),
                Some(_) => {
                    self.error(key, "out of range");
                    None
                }
                None => {
                    self.error(key, "expected a non-negative integer");
                    None
                }
            },
        }
    }

    pub fn uuid(&mut self, key: &str, required: bool) -> Option<[u8; 16]> {
        let s = self.string(key, required)?;
        let r = wire::uuid_parse(&s);
        if r.is_none() {
            self.error(key, "expected a lowercase hyphenated UUID");
        }
        r
    }

    /// base64url, canonical, without padding.
    pub fn b64(&mut self, key: &str, required: bool) -> Option<Vec<u8>> {
        let s = self.string(key, required)?;
        let r = wire::b64_decode(&s);
        if r.is_none() {
            self.error(key, "expected base64url without padding");
        }
        r
    }

    /// base64url that must decode to exactly `len` bytes.
    pub fn b64_exact(&mut self, key: &str, required: bool, len: usize) -> Option<Vec<u8>> {
        let v = self.b64(key, required)?;
        if v.len() == len {
            Some(v)
        } else {
            self.error(key, &format!("expected {len} bytes"));
            None
        }
    }

    pub fn timestamp(&mut self, key: &str, required: bool) -> Option<i64> {
        let s = self.string(key, required)?;
        let r = wire::ts_parse(&s);
        if r.is_none() {
            self.error(key, "expected YYYY-MM-DDTHH:MM:SS.mmmZ");
        }
        r
    }

    pub fn device_id(&mut self, key: &str, required: bool) -> Option<String> {
        let s = self.string(key, required)?;
        if wire::is_device_id(&s) {
            Some(s)
        } else {
            self.error(key, "expected 32 lowercase hexadecimal characters");
            None
        }
    }

    pub fn name(&mut self, key: &str, required: bool) -> Option<String> {
        let s = self.string(key, required)?;
        let r = wire::clean_name(&s);
        if r.is_none() {
            self.error(
                key,
                "expected 1 to 64 characters without control characters",
            );
        }
        r
    }

    pub fn platform(&mut self, key: &str, required: bool) -> Option<String> {
        let s = self.string(key, required)?;
        if wire::is_platform(&s) {
            Some(s)
        } else {
            self.error(key, "expected [a-z0-9-]{1,32}");
            None
        }
    }

    pub fn app_version(&mut self, key: &str, required: bool) -> Option<String> {
        let s = self.string(key, required)?;
        if wire::is_app_version(&s) {
            Some(s)
        } else {
            self.error(key, "expected 1 to 32 printable ASCII characters");
            None
        }
    }

    /// A list of canonical UUIDs without duplicates.
    pub fn uuid_list(&mut self, key: &str, required: bool, max: usize) -> Option<Vec<[u8; 16]>> {
        let arr = match self.value(key) {
            None => {
                self.required_missing(key, required);
                return None;
            }
            Some(Value::Array(a)) => a.clone(),
            Some(_) => {
                self.error(key, "expected an array");
                return None;
            }
        };
        if arr.len() > max {
            self.error(key, &format!("at most {max} elements"));
            return None;
        }
        let mut out: Vec<[u8; 16]> = Vec::with_capacity(arr.len());
        for v in &arr {
            match v.as_str().and_then(wire::uuid_parse) {
                Some(u) if !out.contains(&u) => out.push(u),
                Some(_) => {
                    self.error(key, "duplicate element");
                    return None;
                }
                None => {
                    self.error(key, "expected lowercase hyphenated UUIDs");
                    return None;
                }
            }
        }
        Some(out)
    }

    /// A nested object (its errors carry the parent path).
    pub fn object(&mut self, key: &str, required: bool) -> Option<Obj> {
        match self.value(key) {
            None => {
                self.required_missing(key, required);
                None
            }
            Some(Value::Object(m)) => Some(Obj::new(m.clone(), self.path(key))),
            Some(_) => {
                self.error(key, "expected an object");
                None
            }
        }
    }

    /// Merges the errors of a nested object into this one.
    pub fn absorb(&mut self, nested: Obj) {
        self.errors.extend(nested.errors);
    }

    /// `422 validation_failed` if any field was refused.
    pub fn finish(self) -> Result<(), ApiError> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(ApiError::validation(self.errors))
        }
    }

    /// Whether a field was refused so far.
    pub fn failed(&self) -> bool {
        !self.errors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Obj {
        match v {
            Value::Object(m) => Obj::from_map(m),
            _ => unreachable!(),
        }
    }

    #[test]
    fn collects_every_error_with_its_pointer() {
        let mut o = obj(json!({
            "item_id": "NOT-A-UUID",
            "key_epoch": 1.5,
            "envelope": "AQ==",
            "client": { "format_min": 0 },
            "unknown": 3
        }));
        assert!(o.uuid("item_id", true).is_none());
        assert!(o.uint("key_epoch", true, 1, u64::from(u32::MAX)).is_none());
        assert!(o.b64("envelope", true).is_none());
        let mut c = o.object("client", true).unwrap();
        assert!(c.uint("format_min", true, 1, 100).is_none());
        o.absorb(c);
        assert!(o.string("missing", true).is_none());
        let e = o.finish().unwrap_err();
        let fields: Vec<&str> = e.errors.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "/item_id",
                "/key_epoch",
                "/envelope",
                "/client/format_min",
                "/missing"
            ]
        );
        // The reasons never echo the value received.
        assert!(e.errors.iter().all(|f| !f.reason.contains("NOT-A-UUID")));
    }

    #[test]
    fn null_equals_absent_and_integers_are_bounded() {
        let mut o = obj(json!({ "a": null, "b": 9007199254740992u64, "c": 5 }));
        assert!(o.string("a", false).is_none());
        assert!(!o.failed());
        assert!(o.uint("b", false, 0, u64::MAX).is_none());
        assert_eq!(o.uint("c", true, 0, 10), Some(5));
        assert!(o.failed());
    }

    #[test]
    fn uuid_lists_refuse_duplicates() {
        let u = "308339b6-1358-4bed-8328-23cc68b7f02b";
        let mut o = obj(json!({ "ok": [u], "dup": [u, u], "many": [u] }));
        assert_eq!(o.uuid_list("ok", false, 2).unwrap().len(), 1);
        assert!(o.uuid_list("dup", false, 2).is_none());
        assert!(o.uuid_list("missing", false, 2).is_none());
        assert!(o.finish().is_err());
    }
}
