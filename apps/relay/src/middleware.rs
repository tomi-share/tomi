//! Cross-cutting layer: request id, version of the API in the path (§3.5), problem
//! documents for `405`, and the access log (§9: route template only, never the URL, the
//! query string, a header or a body).

use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, USER_AGENT, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::{ApiError, Code, DeviceSlot, ErrorCode, REQUEST_DEVICE, REQUEST_ID};
use crate::state::AppState;
use crate::wire::is_platform;

/// Response extension that replaces the route label of the access log. The web panel sets a
/// constant one, so that its secret path segment never reaches a log line.
#[derive(Debug, Clone, Copy)]
pub struct RouteLabel(pub &'static str);

/// Highest and lowest API version served.
pub const API_VERSIONS: [u32; 1] = [1];

/// `/v<N>/...` with a canonical integer `N`.
fn path_version(path: &str) -> Option<u64> {
    let rest = path.strip_prefix("/v")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    let after = &rest[digits.len()..];
    (after.is_empty() || after.starts_with('/')).then(|| digits.parse().ok())?
}

/// `https://<host>/updates/<platform>/latest.json` from the `User-Agent`
/// (`tomi-<platform>/<version>`), else `https://<host>/updates/`.
pub fn update_url(public_url: &str, user_agent: Option<&str>) -> String {
    let base = public_url.trim_end_matches('/');
    let platform = user_agent
        .and_then(|ua| ua.strip_prefix("tomi-"))
        .and_then(|rest| rest.split('/').next())
        .filter(|p| is_platform(p));
    match platform {
        Some(p) => format!("{base}/updates/{p}/latest.json"),
        None => format!("{base}/updates/"),
    }
}

fn is_security_event(code: &str) -> bool {
    matches!(
        code,
        "unauthorized" | "session_expired" | "install_token_invalid" | "rate_limited" | "revoked"
    )
}

/// Outermost layer.
pub async fn outer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let id = uuid::Uuid::new_v4();
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "-".to_owned(), |m| m.as_str().to_owned());
    let ip = st.client_ip(req.headers(), req.extensions());
    let user_agent: String = req
        .headers()
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(64).collect())
        .unwrap_or_default();
    let req_bytes: u64 = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // The API answers carry credentials (the session token of `auth/session`) and the
    // metadata of a space: no cache, no sniffing (see below).
    let is_api = path == "/v1" || path.starts_with("/v1/");
    let public_url = st.cfg.public_url.clone();
    let ua_for_update = req
        .headers()
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let device_slot: DeviceSlot = DeviceSlot::default();
    let slot = device_slot.clone();
    let mut resp = REQUEST_ID
        .scope(
            id,
            REQUEST_DEVICE.scope(slot, async move {
                // (1) version of the API in the path.
                if let Some(n) = path_version(&path) {
                    if n < u64::from(API_VERSIONS[0]) {
                        let url = update_url(&public_url, ua_for_update.as_deref());
                        return ApiError::update_required(None, &url, true)
                            .detail("This API version is no longer served.")
                            .into_response();
                    }
                }
                let mut resp = next.run(req).await;
                // The router answers an unsupported method with an empty 405.
                if resp.status() == StatusCode::METHOD_NOT_ALLOWED
                    && !resp.headers().contains_key(CONTENT_TYPE)
                {
                    let allow = resp.headers().get(ALLOW).cloned();
                    resp = ApiError::new(Code::MethodNotAllowed).into_response();
                    if let Some(a) = allow {
                        resp.headers_mut().insert(ALLOW, a);
                    }
                }
                resp
            }),
        )
        .await;

    if let Ok(v) = HeaderValue::from_str(&id.hyphenated().to_string()) {
        resp.headers_mut().insert("x-request-id", v);
    }
    // Every answer of `/v1/*`, errors included: a cache or a proxy must never keep the token of
    // `POST /v1/auth/session` nor any other answer, and the browser must not guess a type.
    if is_api {
        let h = resp.headers_mut();
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    }
    let route = resp
        .extensions()
        .get::<RouteLabel>()
        .map_or(route, |l| l.0.to_owned());
    let status = resp.status().as_u16();
    let code = resp.extensions().get::<ErrorCode>().map(|c| c.0);
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let quiet = route == "/healthz" && status == 200;
    if !quiet {
        let device = device_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match code {
            Some(c) if is_security_event(c) => tracing::info!(
                request_id = %id, method = %method, route = %route, status, code = c,
                duration_ms = ms, req_bytes, device_id = device.as_deref(), ua = %user_agent, ip = %ip,
                "request"
            ),
            _ if status >= 500 => tracing::warn!(
                request_id = %id, method = %method, route = %route, status, code = code,
                duration_ms = ms, req_bytes, device_id = device.as_deref(), ua = %user_agent,
                "request"
            ),
            _ => tracing::info!(
                request_id = %id, method = %method, route = %route, status, code = code,
                duration_ms = ms, req_bytes, device_id = device.as_deref(), ua = %user_agent,
                "request"
            ),
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_versions() {
        assert_eq!(path_version("/v0/hello"), Some(0));
        assert_eq!(path_version("/v1/hello"), Some(1));
        assert_eq!(path_version("/v9/hello"), Some(9));
        assert_eq!(path_version("/v1"), Some(1));
        assert_eq!(path_version("/v01/hello"), None);
        assert_eq!(path_version("/vx/hello"), None);
        assert_eq!(path_version("/v1abc"), None);
        assert_eq!(path_version("/healthz"), None);
    }

    #[test]
    fn update_url_follows_the_user_agent() {
        assert_eq!(
            update_url("https://relay.example.org/", Some("tomi-android/0.1.0")),
            "https://relay.example.org/updates/android/latest.json"
        );
        assert_eq!(
            update_url("https://relay.example.org", Some("curl/8")),
            "https://relay.example.org/updates/"
        );
        assert_eq!(update_url("https://x", None), "https://x/updates/");
    }
}
