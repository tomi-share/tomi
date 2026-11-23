//! APNs sender (`api-v1.md` §6.4): the push doorbell of the Apple devices.
//!
//! * the relay knows nothing of an item, so the notification it asks Apple to show has a
//!   **constant** content: two keys of the app's own texts ("Tomi", "New item"), the same
//!   for every message. The only variable parts are the device token and the urgency;
//! * an item that must ring is a visible alert (shown at once, even when iOS has stopped the
//!   app) that also wakes the app, which then replaces it by the notification it writes itself
//!   from the decrypted item; an item that must not ring is a background wake-up only;
//! * authentication is the provider token of Apple: a JWT signed ES256 with the `.p8` key of
//!   the developer account, renewed every 50 minutes (Apple refuses one older than an hour and
//!   a renewal more often than every 20 minutes);
//! * APNs only speaks HTTP/2: [`Http2Poster`] is the real client, the tests plug a double in
//!   the [`HttpPost`] interface shared with FCM. **Nothing here has been run against Apple**
//!   (no key in the development environment): the request follows Apple's documentation, and
//!   the message, the classification of the answers and the token are covered by unit tests.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};

use crate::clock::Clock;
use crate::fcm::{HttpPost, HttpReply};
use crate::push::{PushError, PushSender, PushTarget, Urgency};
use crate::wire::b64_encode;

/// Production endpoint (App Store, TestFlight).
pub const APNS_BASE: &str = "https://api.push.apple.com";
/// Development endpoint (an app run from Xcode: `aps-environment = development`).
pub const APNS_SANDBOX_BASE: &str = "https://api.sandbox.push.apple.com";

/// `provider` of a registration for the production endpoint.
pub const PROVIDER: &str = "apns";
/// `provider` of a registration for the development endpoint. A token is only valid on the
/// endpoint of the environment the app was signed for: the app says which one.
pub const PROVIDER_SANDBOX: &str = "apns-sandbox";

/// Keys of the app's texts shown by the generic alert (`Localizable.xcstrings`).
pub const TITLE_KEY: &str = "ios_push_title";
pub const BODY_KEY: &str = "ios_push_body";
/// Identifier of the generic alert on the device: one at most (a newer one replaces it), and
/// the app removes it once it has shown its own notifications.
pub const COLLAPSE_ID: &str = "tomi";

/// The key file or its identifiers are unusable (health `auth_error`).
#[derive(Debug, thiserror::Error)]
pub enum ApnsInitError {
    #[error("cannot read the APNs key file: {0}")]
    Read(String),
    #[error("invalid APNs configuration: {0}")]
    Invalid(String),
}

/// The signing key of the developer account and what names it.
pub struct ApnsCredentials {
    key_id: String,
    team_id: String,
    /// The bundle identifier of the app (`apns-topic`).
    pub topic: String,
    key: EcdsaKeyPair,
}

impl std::fmt::Debug for ApnsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApnsCredentials")
            .field("key_id", &self.key_id)
            .field("team_id", &self.team_id)
            .field("topic", &self.topic)
            .finish_non_exhaustive()
    }
}

impl ApnsCredentials {
    /// Reads the `.p8` file of Apple (PEM, PKCS#8, P-256) and checks the identifiers.
    pub fn load(
        path: &Path,
        key_id: &str,
        team_id: &str,
        topic: &str,
    ) -> Result<Self, ApnsInitError> {
        let pem =
            std::fs::read_to_string(path).map_err(|e| ApnsInitError::Read(e.kind().to_string()))?;
        Self::from_pem(&pem, key_id, team_id, topic)
    }

    pub fn from_pem(
        pem: &str,
        key_id: &str,
        team_id: &str,
        topic: &str,
    ) -> Result<Self, ApnsInitError> {
        let ident = |name: &str, value: &str, len: std::ops::RangeInclusive<usize>| {
            let ok = len.contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric());
            if ok {
                Ok(())
            } else {
                Err(ApnsInitError::Invalid(format!(
                    "{name} is not an Apple identifier"
                )))
            }
        };
        ident("apns_key_id", key_id, 10..=10)?;
        ident("apns_team_id", team_id, 10..=10)?;
        let topic_ok = (1..=255).contains(&topic.len())
            && topic
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !topic_ok {
            return Err(ApnsInitError::Invalid(
                "apns_topic is not a bundle identifier".into(),
            ));
        }
        let der =
            pem_to_der(pem).ok_or_else(|| ApnsInitError::Invalid("the key is not PEM".into()))?;
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &SystemRandom::new())
                .map_err(|_| ApnsInitError::Invalid("the key is not a PKCS#8 P-256 key".into()))?;
        Ok(Self {
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            topic: topic.to_owned(),
            key,
        })
    }

    /// The provider token: `{"alg":"ES256","kid":<key id>}.{"iss":<team id>,"iat":<seconds>}`.
    pub fn provider_token(&self, now_ms: i64) -> Result<String, String> {
        let header = json!({ "alg": "ES256", "kid": self.key_id });
        let claims = json!({ "iss": self.team_id, "iat": now_ms / 1000 });
        let input = format!(
            "{}.{}",
            b64_encode(header.to_string().as_bytes()),
            b64_encode(claims.to_string().as_bytes())
        );
        let sig = self
            .key
            .sign(&SystemRandom::new(), input.as_bytes())
            .map_err(|_| "ECDSA signature failed".to_owned())?;
        Ok(format!("{input}.{}", b64_encode(sig.as_ref())))
    }

    /// Public key of the signing key (the tests check the token with it).
    pub fn public_key(&self) -> Vec<u8> {
        self.key.public_key().as_ref().to_vec()
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

/// The payload of §6.4. Constant for an urgency: nothing of the item, no identifier, no counter.
pub fn payload(urgency: Urgency) -> Value {
    match urgency {
        // Rings: a visible alert with the app's own fixed texts, which also wakes the app.
        Urgency::High => json!({
            "aps": {
                "alert": { "title-loc-key": TITLE_KEY, "loc-key": BODY_KEY },
                "sound": "default",
                "content-available": 1
            }
        }),
        // Does not ring: the app is only woken to fetch.
        Urgency::Normal => json!({ "aps": { "content-available": 1 } }),
    }
}

/// The headers of §6.4 (the authorization is added by the sender).
pub fn headers(topic: &str, urgency: Urgency, now_ms: i64) -> Vec<(&'static str, String)> {
    let (kind, priority) = match urgency {
        Urgency::High => ("alert", "10"),
        // A background push must be sent at priority 5.
        Urgency::Normal => ("background", "5"),
    };
    let mut headers = vec![
        ("apns-topic", topic.to_owned()),
        ("apns-push-type", kind.to_owned()),
        ("apns-priority", priority.to_owned()),
        // Kept a day for a device that is off, like the FCM `ttl`.
        ("apns-expiration", (now_ms / 1000 + 86_400).to_string()),
        ("content-type", "application/json".to_owned()),
    ];
    if urgency == Urgency::High {
        headers.push(("apns-collapse-id", COLLAPSE_ID.to_owned()));
    }
    headers
}

/// The endpoint of a registration: its provider says which environment the token is for.
pub fn base_for(provider: &str) -> Option<&'static str> {
    match provider {
        PROVIDER => Some(APNS_BASE),
        PROVIDER_SANDBOX => Some(APNS_SANDBOX_BASE),
        _ => None,
    }
}

/// A device token of APNs: hexadecimal, 64 characters today, longer ones announced by Apple.
pub fn is_device_token(token: &str) -> bool {
    (64..=200).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Maps an APNs answer to the relay's reaction (§6.4).
pub fn classify(reply: &HttpReply) -> Result<(), PushError> {
    let retry_after = reply.retry_after.map(Duration::from_secs);
    let reason = serde_json::from_slice::<Value>(&reply.body)
        .ok()
        .and_then(|v| v["reason"].as_str().map(str::to_owned))
        .unwrap_or_default();
    match reply.status {
        200..=299 => Ok(()),
        // The token is no longer valid for this app.
        410 => Err(PushError::Unregistered),
        400 if matches!(
            reason.as_str(),
            "BadDeviceToken" | "DeviceTokenNotForTopic" | "Unregistered"
        ) =>
        {
            Err(PushError::Unregistered)
        }
        // The provider token is refused (wrong key, key id or team; token too old or too recent).
        403 => Err(PushError::AuthFailure),
        429 | 500..=599 => Err(PushError::Transient { retry_after }),
        _ => Err(PushError::InvalidArgument),
    }
}

struct CachedToken {
    value: String,
    made_at_ms: i64,
}

/// How long a provider token is used before a new one is signed.
const TOKEN_LIFETIME_MS: i64 = 50 * 60 * 1000;
/// Apple refuses a new provider token more often than this.
const TOKEN_MIN_RENEWAL_MS: i64 = 20 * 60 * 1000;

/// The APNs sender.
pub struct ApnsSender {
    creds: ApnsCredentials,
    http: Arc<dyn HttpPost>,
    clock: Arc<dyn Clock>,
    /// Replaces both endpoints (tests).
    base: Option<String>,
    token: tokio::sync::Mutex<Option<CachedToken>>,
}

impl ApnsSender {
    pub fn new(
        creds: ApnsCredentials,
        http: Arc<dyn HttpPost>,
        clock: Arc<dyn Clock>,
        base: Option<String>,
    ) -> Self {
        Self {
            creds,
            http,
            clock,
            base,
            token: tokio::sync::Mutex::new(None),
        }
    }

    async fn provider_token(&self) -> Result<String, PushError> {
        let mut cached = self.token.lock().await;
        let now = self.clock.now_ms();
        if let Some(t) = cached.as_ref() {
            if (0..TOKEN_LIFETIME_MS).contains(&(now - t.made_at_ms)) {
                return Ok(t.value.clone());
            }
        }
        let value = self
            .creds
            .provider_token(now)
            .map_err(|_| PushError::AuthFailure)?;
        *cached = Some(CachedToken {
            value: value.clone(),
            made_at_ms: now,
        });
        Ok(value)
    }

    /// Apple refused the provider token: a new one is signed at the next send, unless this one is
    /// younger than 20 minutes (Apple refuses renewals more frequent than that, and a token that
    /// young is not refused for its age: the key or the identifiers are wrong).
    async fn forget_refused_token(&self) {
        let mut cached = self.token.lock().await;
        let now = self.clock.now_ms();
        if cached
            .as_ref()
            .is_some_and(|t| now - t.made_at_ms >= TOKEN_MIN_RENEWAL_MS)
        {
            *cached = None;
        }
    }
}

#[async_trait]
impl PushSender for ApnsSender {
    async fn wake(&self, target: &PushTarget, urgency: Urgency) -> Result<(), PushError> {
        let Some(default_base) = base_for(&target.provider) else {
            return Err(PushError::Unsupported);
        };
        // A token that is not one would be put in the path of the request.
        if !is_device_token(&target.token) {
            return Err(PushError::Unregistered);
        }
        let base = self.base.as_deref().unwrap_or(default_base);
        let url = format!("{base}/3/device/{}", target.token);
        let body = payload(urgency).to_string().into_bytes();
        let jwt = self.provider_token().await?;
        let mut headers = headers(&self.creds.topic, urgency, self.clock.now_ms());
        headers.push(("authorization", format!("bearer {jwt}")));
        let reply = self
            .http
            .post(&url, &headers, body)
            .await
            .map_err(|_| PushError::Transient { retry_after: None })?;
        let outcome = classify(&reply);
        if outcome == Err(PushError::AuthFailure) {
            self.forget_refused_token().await;
        }
        outcome
    }
}

/// Real HTTPS client for APNs: HTTP/2 only (`hyper`), same TLS stack and root certificates as
/// the rest of the relay (`rustls` with `ring`, `webpki-roots`). One connection is kept open and
/// reused, as Apple asks.
pub struct Http2Poster {
    client: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        http_body_util::Full<bytes::Bytes>,
    >,
}

impl Http2Poster {
    /// Fails only if the TLS provider refuses its own default protocol versions.
    pub fn new() -> Result<Self, String> {
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| "TLS: no usable protocol version".to_owned())?
        .with_root_certificates(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
        .with_no_client_auth();
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_only()
            .enable_http2()
            .build();
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .http2_only(true)
                // A connection that died without a word is found by a ping, not by a send that hangs.
                .timer(hyper_util::rt::TokioTimer::new())
                .pool_timer(hyper_util::rt::TokioTimer::new())
                .http2_keep_alive_interval(Some(Duration::from_secs(60)))
                .http2_keep_alive_timeout(Duration::from_secs(10))
                .http2_keep_alive_while_idle(true)
                .pool_idle_timeout(Duration::from_secs(300))
                .build(connector);
        Ok(Self { client })
    }
}

#[async_trait]
impl HttpPost for Http2Poster {
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<HttpReply, String> {
        use http_body_util::BodyExt as _;
        let mut request = hyper::Request::builder().method("POST").uri(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let request = request
            .body(http_body_util::Full::new(bytes::Bytes::from(body)))
            .map_err(|_| "request: invalid".to_owned())?;
        let call = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|_| "transport".to_owned())?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let body = response
                .into_body()
                .collect()
                .await
                .map_err(|_| "transport".to_owned())?
                .to_bytes()
                .to_vec();
            Ok(HttpReply {
                status,
                retry_after,
                body,
            })
        };
        tokio::time::timeout(Duration::from_secs(20), call)
            .await
            .map_err(|_| "transport: timeout".to_owned())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
    use std::sync::Mutex;

    /// A P-256 key made for the tests (never a real one).
    fn test_pem() -> String {
        use base64::Engine as _;
        let doc =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(doc.as_ref());
        format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n")
    }

    fn creds() -> ApnsCredentials {
        ApnsCredentials::from_pem(&test_pem(), "ABC123DEFG", "TEAM123456", "eu.shaffe.tomi")
            .unwrap()
    }

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    struct FixedClock(Mutex<i64>);
    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            *self.0.lock().unwrap()
        }
    }

    type Seen = Arc<Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>;
    struct FakeHttp {
        seen: Seen,
        answer: HttpReply,
    }
    #[async_trait]
    impl HttpPost for FakeHttp {
        async fn post(
            &self,
            url: &str,
            headers: &[(&str, String)],
            body: Vec<u8>,
        ) -> Result<HttpReply, String> {
            let headers = headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect();
            self.seen
                .lock()
                .unwrap()
                .push((url.to_owned(), headers, body));
            Ok(self.answer.clone())
        }
    }

    fn sender(answer: HttpReply, now: i64) -> (ApnsSender, Seen, Arc<FixedClock>) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let clock = Arc::new(FixedClock(Mutex::new(now)));
        let http = Arc::new(FakeHttp {
            seen: seen.clone(),
            answer,
        });
        (
            ApnsSender::new(creds(), http, clock.clone(), None),
            seen,
            clock,
        )
    }

    fn target(provider: &str) -> PushTarget {
        PushTarget {
            provider: provider.to_owned(),
            token: "a1".repeat(32),
        }
    }

    #[test]
    fn the_payload_is_constant_and_says_nothing_of_the_item() {
        let ring = payload(Urgency::High);
        assert_eq!(
            ring,
            json!({"aps": {"alert": {"title-loc-key": "ios_push_title", "loc-key": "ios_push_body"},
                           "sound": "default", "content-available": 1}})
        );
        assert_eq!(payload(Urgency::High), ring, "the same for every message");
        assert_eq!(
            payload(Urgency::Normal),
            json!({"aps": {"content-available": 1}})
        );
    }

    #[test]
    fn an_item_that_rings_is_an_alert_and_the_others_a_background_wake() {
        let h = headers("eu.shaffe.tomi", Urgency::High, 1_000_000);
        let get = |h: &Vec<(&str, String)>, k: &str| {
            h.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
        };
        assert_eq!(get(&h, "apns-push-type").as_deref(), Some("alert"));
        assert_eq!(get(&h, "apns-priority").as_deref(), Some("10"));
        assert_eq!(get(&h, "apns-collapse-id").as_deref(), Some("tomi"));
        assert_eq!(get(&h, "apns-topic").as_deref(), Some("eu.shaffe.tomi"));
        assert_eq!(get(&h, "apns-expiration").as_deref(), Some("87400"));
        let quiet = headers("eu.shaffe.tomi", Urgency::Normal, 0);
        assert_eq!(get(&quiet, "apns-push-type").as_deref(), Some("background"));
        assert_eq!(get(&quiet, "apns-priority").as_deref(), Some("5"));
        assert_eq!(get(&quiet, "apns-collapse-id"), None);
    }

    #[test]
    fn answers_are_classified_as_the_spec_says() {
        assert_eq!(classify(&reply(200, "")), Ok(()));
        assert_eq!(
            classify(&reply(410, r#"{"reason":"Unregistered"}"#)),
            Err(PushError::Unregistered)
        );
        assert_eq!(
            classify(&reply(400, r#"{"reason":"BadDeviceToken"}"#)),
            Err(PushError::Unregistered)
        );
        assert_eq!(
            classify(&reply(400, r#"{"reason":"DeviceTokenNotForTopic"}"#)),
            Err(PushError::Unregistered)
        );
        assert_eq!(
            classify(&reply(400, r#"{"reason":"BadTopic"}"#)),
            Err(PushError::InvalidArgument)
        );
        assert_eq!(
            classify(&reply(400, "not json")),
            Err(PushError::InvalidArgument)
        );
        assert_eq!(
            classify(&reply(403, r#"{"reason":"InvalidProviderToken"}"#)),
            Err(PushError::AuthFailure)
        );
        assert_eq!(
            classify(&reply(403, r#"{"reason":"ExpiredProviderToken"}"#)),
            Err(PushError::AuthFailure)
        );
        assert_eq!(
            classify(&reply(429, "")),
            Err(PushError::Transient { retry_after: None })
        );
        assert_eq!(
            classify(&reply(503, "")),
            Err(PushError::Transient { retry_after: None })
        );
        let mut later = reply(503, "");
        later.retry_after = Some(7);
        assert_eq!(
            classify(&later),
            Err(PushError::Transient {
                retry_after: Some(Duration::from_secs(7))
            })
        );
    }

    #[test]
    fn the_provider_token_is_signed_by_the_key_and_names_it() {
        use base64::Engine as _;
        let c = creds();
        let jwt = c.provider_token(1_700_000_123_456).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let decode = |s: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s)
                .unwrap()
        };
        let header: Value = serde_json::from_slice(&decode(parts[0])).unwrap();
        let claims: Value = serde_json::from_slice(&decode(parts[1])).unwrap();
        assert_eq!(header, json!({"alg": "ES256", "kid": "ABC123DEFG"}));
        assert_eq!(claims, json!({"iss": "TEAM123456", "iat": 1_700_000_123}));
        let signed = format!("{}.{}", parts[0], parts[1]);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, c.public_key())
            .verify(signed.as_bytes(), &decode(parts[2]))
            .expect("the signature is the key's");
    }

    #[test]
    fn unusable_keys_and_identifiers_are_refused() {
        let pem = test_pem();
        assert!(
            ApnsCredentials::from_pem(&pem, "ABC123DEFG", "TEAM123456", "eu.shaffe.tomi").is_ok()
        );
        assert!(ApnsCredentials::from_pem(&pem, "short", "TEAM123456", "eu.shaffe.tomi").is_err());
        assert!(
            ApnsCredentials::from_pem(&pem, "ABC123DEFG", "TEAM 23456", "eu.shaffe.tomi").is_err()
        );
        assert!(ApnsCredentials::from_pem(&pem, "ABC123DEFG", "TEAM123456", "eu shaffe").is_err());
        assert!(
            ApnsCredentials::from_pem("", "ABC123DEFG", "TEAM123456", "eu.shaffe.tomi").is_err()
        );
        assert!(ApnsCredentials::from_pem(
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
            "ABC123DEFG",
            "TEAM123456",
            "eu.shaffe.tomi"
        )
        .is_err());
        // Never the key in a debug print.
        let shown = format!("{:?}", creds());
        assert!(
            shown.contains("ABC123DEFG") && !shown.contains("PRIVATE") && !shown.contains("key:")
        );
    }

    #[tokio::test]
    async fn a_wake_goes_to_the_endpoint_of_its_environment_with_the_token_in_the_path() {
        let (s, seen, _) = sender(reply(200, ""), 1_000);
        s.wake(&target(PROVIDER), Urgency::High).await.unwrap();
        s.wake(&target(PROVIDER_SANDBOX), Urgency::Normal)
            .await
            .unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen[0].0,
            format!("https://api.push.apple.com/3/device/{}", "a1".repeat(32))
        );
        assert_eq!(
            seen[1].0,
            format!(
                "https://api.sandbox.push.apple.com/3/device/{}",
                "a1".repeat(32)
            )
        );
        let auth = |i: usize| {
            seen[i]
                .1
                .iter()
                .find(|(k, _)| k == "authorization")
                .unwrap()
                .1
                .clone()
        };
        assert!(auth(0).starts_with("bearer "));
        assert_eq!(
            auth(0),
            auth(1),
            "one provider token serves both, within its lifetime"
        );
        assert_eq!(seen[0].2, payload(Urgency::High).to_string().into_bytes());
    }

    #[tokio::test]
    async fn the_provider_token_is_renewed_after_fifty_minutes() {
        let (s, seen, clock) = sender(reply(200, ""), 0);
        s.wake(&target(PROVIDER), Urgency::High).await.unwrap();
        *clock.0.lock().unwrap() = 49 * 60 * 1000;
        s.wake(&target(PROVIDER), Urgency::High).await.unwrap();
        *clock.0.lock().unwrap() = 51 * 60 * 1000;
        s.wake(&target(PROVIDER), Urgency::High).await.unwrap();
        let seen = seen.lock().unwrap();
        let auth = |i: usize| {
            seen[i]
                .1
                .iter()
                .find(|(k, _)| k == "authorization")
                .unwrap()
                .1
                .clone()
        };
        assert_eq!(auth(0), auth(1));
        assert_ne!(auth(1), auth(2));
    }

    #[tokio::test]
    async fn a_refused_provider_token_is_signed_again_but_not_within_twenty_minutes() {
        let (s, seen, clock) = sender(reply(403, r#"{"reason":"ExpiredProviderToken"}"#), 0);
        let auth = |i: usize| {
            let seen = seen.lock().unwrap();
            let found = seen[i].1.iter().find(|(k, _)| k == "authorization");
            found.unwrap().1.clone()
        };
        assert_eq!(
            s.wake(&target(PROVIDER), Urgency::High).await,
            Err(PushError::AuthFailure)
        );
        *clock.0.lock().unwrap() = 5 * 60 * 1000;
        let _ = s.wake(&target(PROVIDER), Urgency::High).await;
        assert_eq!(
            auth(0),
            auth(1),
            "too young to be renewed: Apple would refuse the renewal"
        );
        *clock.0.lock().unwrap() = 21 * 60 * 1000;
        let _ = s.wake(&target(PROVIDER), Urgency::High).await;
        assert_eq!(
            auth(1),
            auth(2),
            "the refusal at 21 minutes is what makes the next one new"
        );
        *clock.0.lock().unwrap() = 22 * 60 * 1000;
        let _ = s.wake(&target(PROVIDER), Urgency::High).await;
        assert_ne!(auth(2), auth(3));
    }

    #[tokio::test]
    async fn what_is_not_an_apns_registration_is_not_sent() {
        let (s, seen, _) = sender(reply(200, ""), 0);
        assert_eq!(
            s.wake(&target("fcm"), Urgency::High).await,
            Err(PushError::Unsupported)
        );
        let odd = PushTarget {
            provider: PROVIDER.to_owned(),
            token: "../../3/device/x".to_owned(),
        };
        assert_eq!(
            s.wake(&odd, Urgency::High).await,
            Err(PushError::Unregistered),
            "never put in a path"
        );
        assert!(seen.lock().unwrap().is_empty());
        assert!(is_device_token(&"0f".repeat(32)) && is_device_token(&"AB".repeat(50)));
        assert!(!is_device_token("abc") && !is_device_token(&"zz".repeat(32)));
    }
}
