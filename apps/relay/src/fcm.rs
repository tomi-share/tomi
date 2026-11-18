//! FCM HTTP v1 sender (`api-v1.md` §6.3).
//!
//! * the message has a **constant** content (`data = {"w": "1"}`); the only variable parts
//!   are the device token and the priority;
//! * the Google OAuth token is obtained from the service-account file (RS256 assertion
//!   signed with `ring`) and renewed before it expires;
//! * the HTTP layer is the [`HttpPost`] trait: [`UreqPoster`] is the real one, the tests
//!   plug a double. **Nothing here has been run against Google** (no network, no account in
//!   the development environment): the request format follows the FCM documentation and
//!   everything around it (message, classification of the answers, OAuth assertion) is
//!   covered by unit tests with a local fake.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde_json::{json, Value};

use crate::clock::Clock;
use crate::push::{PushError, PushSender, PushTarget, Urgency};
use crate::wire::b64_encode;

/// Default FCM endpoint.
pub const FCM_BASE: &str = "https://fcm.googleapis.com";
/// OAuth scope of the messaging API.
pub const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// The service-account file is unusable (health `auth_error`).
#[derive(Debug, thiserror::Error)]
pub enum FcmInitError {
    #[error("cannot read the service account file: {0}")]
    Read(String),
    #[error("invalid service account file: {0}")]
    Invalid(String),
}

/// Credentials extracted from the service-account JSON file.
pub struct FcmCredentials {
    pub project_id: String,
    client_email: String,
    token_uri: String,
    key: RsaKeyPair,
}

impl std::fmt::Debug for FcmCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FcmCredentials")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .finish_non_exhaustive()
    }
}

impl FcmCredentials {
    /// Reads and validates the file (JSON, required members, PKCS#8 RSA key).
    pub fn load(path: &Path, project_override: Option<&str>) -> Result<Self, FcmInitError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| FcmInitError::Read(e.kind().to_string()))?;
        Self::from_json(&text, project_override)
    }

    /// Same from the file content.
    pub fn from_json(text: &str, project_override: Option<&str>) -> Result<Self, FcmInitError> {
        let v: Value =
            serde_json::from_str(text).map_err(|_| FcmInitError::Invalid("not JSON".into()))?;
        let get = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
        let pem =
            get("private_key").ok_or_else(|| FcmInitError::Invalid("no private_key".into()))?;
        let client_email = get("client_email")
            .ok_or_else(|| FcmInitError::Invalid("no client_email".into()))?
            .to_owned();
        let project_id = project_override
            .map(str::to_owned)
            .or_else(|| get("project_id").map(str::to_owned))
            .ok_or_else(|| FcmInitError::Invalid("no project_id".into()))?;
        let token_uri = get("token_uri").unwrap_or(DEFAULT_TOKEN_URI).to_owned();
        let der = pem_to_der(pem)
            .ok_or_else(|| FcmInitError::Invalid("private_key is not PEM".into()))?;
        let key = RsaKeyPair::from_pkcs8(&der)
            .map_err(|_| FcmInitError::Invalid("private_key is not a PKCS#8 RSA key".into()))?;
        Ok(Self {
            project_id,
            client_email,
            token_uri,
            key,
        })
    }

    /// The signed OAuth assertion (RS256 JWT) valid one hour from `now_ms`.
    pub fn assertion(&self, now_ms: i64) -> Result<String, String> {
        let iat = now_ms / 1000;
        let header = json!({ "alg": "RS256", "typ": "JWT" });
        let claims = json!({
            "iss": self.client_email,
            "scope": FCM_SCOPE,
            "aud": self.token_uri,
            "iat": iat,
            "exp": iat + 3600,
        });
        let input = format!(
            "{}.{}",
            b64_encode(header.to_string().as_bytes()),
            b64_encode(claims.to_string().as_bytes())
        );
        let mut sig = vec![0u8; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                input.as_bytes(),
                &mut sig,
            )
            .map_err(|_| "RSA signature failed".to_owned())?;
        Ok(format!("{input}.{}", b64_encode(&sig)))
    }

    /// Public key of the signing key (used by the tests to check the assertion).
    pub fn public_key(&self) -> Vec<u8> {
        self.key.public().as_ref().to_vec()
    }
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    if body.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// What a POST returned.
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status: u16,
    pub retry_after: Option<u64>,
    pub body: Vec<u8>,
}

/// The one HTTP operation the sender needs.
#[async_trait]
pub trait HttpPost: Send + Sync {
    /// Network or TLS failure is `Err` (the text never contains request data).
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<HttpReply, String>;
}

/// Real HTTPS client: `ureq` on the blocking pool (same TLS stack as `tomi-core`'s client).
pub struct UreqPoster {
    agent: ureq::Agent,
}

impl Default for UreqPoster {
    fn default() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent("tomi-relay")
            .build()
            .into();
        Self { agent }
    }
}

#[async_trait]
impl HttpPost for UreqPoster {
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<HttpReply, String> {
        let agent = self.agent.clone();
        let url = url.to_owned();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        tokio::task::spawn_blocking(move || {
            let mut req = agent.post(&url);
            for (k, v) in &headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let mut resp = req.send(&body[..]).map_err(|e| format!("transport: {e}"))?;
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let body = resp
                .body_mut()
                .read_to_vec()
                .map_err(|e| format!("transport: {e}"))?;
            Ok(HttpReply {
                status,
                retry_after,
                body,
            })
        })
        .await
        .map_err(|e| format!("task: {e}"))?
    }
}

/// The message of §6.3. Only `token` and `priority` vary.
pub fn message_body(token: &str, urgency: Urgency) -> Value {
    json!({
        "message": {
            "token": token,
            "data": { "w": "1" },
            "android": {
                "priority": match urgency { Urgency::High => "HIGH", Urgency::Normal => "NORMAL" },
                "collapse_key": "tomi",
                "ttl": "86400s"
            }
        }
    })
}

/// Maps an FCM answer to the relay's reaction (§6.3).
pub fn classify(reply: &HttpReply) -> Result<(), PushError> {
    let retry_after = reply.retry_after.map(Duration::from_secs);
    match reply.status {
        200..=299 => Ok(()),
        404 => Err(PushError::Unregistered),
        400 => {
            let v: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
            let about_token = v["error"]["details"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d["fieldViolations"].as_array())
                .flatten()
                .any(|f| f["field"].as_str() == Some("message.token"));
            if about_token {
                Err(PushError::Unregistered)
            } else {
                Err(PushError::InvalidArgument)
            }
        }
        401 | 403 => Err(PushError::AuthFailure),
        429 | 500..=599 => Err(PushError::Transient { retry_after }),
        _ => Err(PushError::InvalidArgument),
    }
}

struct CachedToken {
    value: String,
    expires_at_ms: i64,
}

/// The FCM sender.
pub struct FcmSender {
    creds: FcmCredentials,
    http: Arc<dyn HttpPost>,
    clock: Arc<dyn Clock>,
    base: String,
    token: tokio::sync::Mutex<Option<CachedToken>>,
}

impl FcmSender {
    pub fn new(
        creds: FcmCredentials,
        http: Arc<dyn HttpPost>,
        clock: Arc<dyn Clock>,
        base: Option<String>,
    ) -> Self {
        Self {
            creds,
            http,
            clock,
            base: base.unwrap_or_else(|| FCM_BASE.to_owned()),
            token: tokio::sync::Mutex::new(None),
        }
    }

    /// A valid OAuth access token (cached, renewed five minutes before it expires).
    async fn access_token(&self) -> Result<String, PushError> {
        let mut cached = self.token.lock().await;
        let now = self.clock.now_ms();
        if let Some(t) = cached.as_ref() {
            if t.expires_at_ms - now > 300_000 {
                return Ok(t.value.clone());
            }
        }
        let assertion = self
            .creds
            .assertion(now)
            .map_err(|_| PushError::AuthFailure)?;
        let form = form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer")
            .append_pair("assertion", &assertion)
            .finish();
        let reply = self
            .http
            .post(
                &self.creds.token_uri,
                &[(
                    "Content-Type",
                    "application/x-www-form-urlencoded".to_owned(),
                )],
                form.into_bytes(),
            )
            .await
            .map_err(|_| PushError::Transient { retry_after: None })?;
        match reply.status {
            200..=299 => {
                let v: Value =
                    serde_json::from_slice(&reply.body).map_err(|_| PushError::AuthFailure)?;
                let value = v["access_token"]
                    .as_str()
                    .ok_or(PushError::AuthFailure)?
                    .to_owned();
                let ttl = v["expires_in"].as_i64().unwrap_or(3600);
                *cached = Some(CachedToken {
                    value: value.clone(),
                    expires_at_ms: now + ttl.saturating_mul(1000),
                });
                Ok(value)
            }
            429 | 500..=599 => Err(PushError::Transient {
                retry_after: reply.retry_after.map(Duration::from_secs),
            }),
            _ => Err(PushError::AuthFailure),
        }
    }
}

#[async_trait]
impl PushSender for FcmSender {
    async fn wake(&self, target: &PushTarget, urgency: Urgency) -> Result<(), PushError> {
        let access = self.access_token().await?;
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.base, self.creds.project_id
        );
        let body = serde_json::to_vec(&message_body(&target.token, urgency))
            .map_err(|_| PushError::InvalidArgument)?;
        let reply = self
            .http
            .post(
                &url,
                &[
                    ("Authorization", format!("Bearer {access}")),
                    ("Content-Type", "application/json; charset=UTF-8".to_owned()),
                ],
                body,
            )
            .await
            .map_err(|_| PushError::Transient { retry_after: None })?;
        let result = classify(&reply);
        if result == Err(PushError::AuthFailure) {
            *self.token.lock().await = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn message_has_a_constant_content() {
        let a = message_body("tok-A", Urgency::High);
        let b = message_body("tok-B", Urgency::Normal);
        assert_eq!(a["message"]["data"], json!({ "w": "1" }));
        assert_eq!(a["message"]["android"]["priority"], "HIGH");
        assert_eq!(b["message"]["android"]["priority"], "NORMAL");
        assert_eq!(a["message"]["android"]["collapse_key"], "tomi");
        assert!(a["message"].get("notification").is_none());
        // Same message once the two variable parts are neutralised.
        let mut a2 = a.clone();
        a2["message"]["token"] = json!("x");
        a2["message"]["android"]["priority"] = json!("x");
        let mut b2 = b.clone();
        b2["message"]["token"] = json!("x");
        b2["message"]["android"]["priority"] = json!("x");
        assert_eq!(a2, b2);
    }

    #[test]
    fn answers_are_classified_as_the_spec_says() {
        assert_eq!(classify(&reply(200, "{}")), Ok(()));
        assert_eq!(classify(&reply(404, "{}")), Err(PushError::Unregistered));
        let token_violation = r#"{"error":{"status":"INVALID_ARGUMENT","details":[{"fieldViolations":[{"field":"message.token"}]}]}}"#;
        assert_eq!(
            classify(&reply(400, token_violation)),
            Err(PushError::Unregistered)
        );
        let other = r#"{"error":{"status":"INVALID_ARGUMENT","details":[{"fieldViolations":[{"field":"message.data"}]}]}}"#;
        assert_eq!(
            classify(&reply(400, other)),
            Err(PushError::InvalidArgument)
        );
        assert_eq!(classify(&reply(401, "{}")), Err(PushError::AuthFailure));
        assert_eq!(classify(&reply(403, "{}")), Err(PushError::AuthFailure));
        assert!(matches!(
            classify(&reply(503, "{}")),
            Err(PushError::Transient { .. })
        ));
        let mut r = reply(429, "{}");
        r.retry_after = Some(7);
        assert_eq!(
            classify(&r),
            Err(PushError::Transient {
                retry_after: Some(Duration::from_secs(7))
            })
        );
    }

    #[test]
    fn invalid_service_account_files_are_refused() {
        assert!(matches!(
            FcmCredentials::from_json("not json", None),
            Err(FcmInitError::Invalid(_))
        ));
        assert!(FcmCredentials::from_json("{}", None).is_err());
        let bad_key = r#"{"project_id":"p","client_email":"a@b","private_key":"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"}"#;
        assert!(FcmCredentials::from_json(bad_key, None).is_err());
        assert!(FcmCredentials::load(Path::new("/definitely/not/here.json"), None).is_err());
    }
}
