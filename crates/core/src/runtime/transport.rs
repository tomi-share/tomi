//! Transport abstraction between the sync engine and the relay (`api-v1.md` §4).
//!
//! [`RelayTransport`] is a blocking, typed view of the REST API. Timestamps are milliseconds
//! since the Unix epoch, binary fields are raw bytes: the wire encodings (RFC 3339 with
//! milliseconds, base64url without padding) live in the HTTP implementation only, so the
//! engine can be tested against an in-memory double without any serialization.
//!
//! The session token is state of the transport ([`RelayTransport::set_session_token`]); the
//! engine obtains it with the challenge/response of §3.3 and installs it.

use serde::{Deserialize, Serialize};

/// Error of a transport call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayError {
    /// No answer (offline, DNS, TLS, timeout) or an answer that is not the API. Retry later.
    Network(String),
    /// The relay answered with an error status (`application/problem+json`, §2.5).
    /// Boxed: the problem document is much larger than the other variant.
    Api(Box<ApiProblem>),
}

/// Content of a problem document that the engine acts on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApiProblem {
    pub status: u16,
    /// Stable machine code (`stale_key_epoch`, `cursor_expired`...).
    pub code: String,
    pub detail: Option<String>,
    /// `Retry-After` / `retry_after`, seconds.
    pub retry_after_secs: Option<u64>,
    /// Extension `key_epoch` (current epoch on `stale_key_epoch`, `epoch_conflict`).
    pub key_epoch: Option<u32>,
    /// Extension `upload_offset` (`upload_offset_mismatch`, `blob_incomplete`).
    pub upload_offset: Option<u64>,
    /// Extension `head_seq` (`cursor_ahead`).
    pub head_seq: Option<u64>,
    /// Extension `update_url` (`update_required`).
    pub update_url: Option<String>,
    /// Extension `limited_by` (`format_unsupported`).
    pub limited_by: Vec<String>,
}

impl RelayError {
    /// A bare API error, for tests and the double.
    pub fn api(status: u16, code: &str) -> Self {
        RelayError::problem(ApiProblem {
            status,
            code: code.to_owned(),
            ..ApiProblem::default()
        })
    }

    /// An API error from its problem document.
    pub fn problem(p: ApiProblem) -> Self {
        RelayError::Api(Box::new(p))
    }

    /// The `code` of an API error.
    pub fn code(&self) -> Option<&str> {
        match self {
            RelayError::Api(p) => Some(&p.code),
            RelayError::Network(_) => None,
        }
    }

    /// Worth retrying later without changing the request (spec §2.5: 5xx, 429, no answer).
    pub fn is_retryable(&self) -> bool {
        match self {
            RelayError::Network(_) => true,
            RelayError::Api(p) => p.status == 429 || p.status >= 500,
        }
    }

    /// The relay was not reached at all.
    pub fn is_offline(&self) -> bool {
        matches!(self, RelayError::Network(_))
    }
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayError::Network(r) => write!(f, "network: {r}"),
            RelayError::Api(p) => write!(f, "{} {}", p.status, p.code),
        }
    }
}

impl std::error::Error for RelayError {}

impl From<RelayError> for crate::CoreError {
    fn from(e: RelayError) -> Self {
        match e {
            RelayError::Network(reason) => crate::CoreError::Network { reason },
            RelayError::Api(p) => crate::CoreError::Relay {
                status: p.status,
                code: p.code,
            },
        }
    }
}

pub type TResult<T> = std::result::Result<T, RelayError>;

/// Limits announced by the relay (`hello`, session `space.limits`). The last four are
/// additive (§4.2): `None` on an older relay, history transfer is then not offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_envelope_bytes: u64,
    pub max_blob_bytes: u64,
    pub max_item_blob_bytes: u64,
    pub max_blobs_per_item: u64,
    pub max_upload_part_bytes: u64,
    pub max_ttl_secs: u64,
    pub max_devices: u64,
    #[serde(default)]
    pub max_transfer_bytes: Option<u64>,
    #[serde(default)]
    pub max_blobs_per_transfer: Option<u64>,
    #[serde(default)]
    pub transfer_ttl_secs: Option<u64>,
    #[serde(default)]
    pub transfer_max_open: Option<u64>,
}

impl Default for Limits {
    /// Defaults of `api-v1.md` §2.8, used until the relay has announced its own.
    fn default() -> Self {
        Self {
            max_envelope_bytes: 1_114_112,
            max_blob_bytes: 26_279_936,
            max_item_blob_bytes: 105_119_744,
            max_blobs_per_item: 32,
            max_upload_part_bytes: 1_048_576,
            max_ttl_secs: crate::DEFAULT_MAX_TTL_SECS,
            max_devices: 20,
            max_transfer_bytes: Some(1_073_741_824),
            max_blobs_per_transfer: Some(10_000),
            transfer_ttl_secs: Some(604_800),
            transfer_max_open: Some(2),
        }
    }
}

/// `GET /v1/hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub server_time_ms: i64,
    pub space_initialized: bool,
    pub limits: Option<Limits>,
    /// Additive capabilities of the relay (`features`, `api-v1.md` §4.1); empty on an older relay.
    pub features: Vec<String>,
}

/// `POST /v1/auth/challenge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// 32 random bytes.
    pub challenge: Vec<u8>,
    pub expires_at_ms: i64,
}

/// `client` object of `POST /v1/auth/session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub format_min: u32,
    pub format_max: u32,
    pub app_version: String,
    pub platform: String,
}

/// `POST /v1/auth/session` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRequest {
    pub device_id: String,
    pub challenge: Vec<u8>,
    pub signature: Vec<u8>,
    pub client: ClientInfo,
}

/// SpaceState (§3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceInfo {
    pub space_id: String,
    pub effective_format: u32,
    pub key_epoch: u32,
    pub rotation_required: bool,
    pub head_seq: u64,
    pub server_time_ms: i64,
    pub quota_used_bytes: u64,
    pub quota_max_bytes: u64,
    pub limits: Limits,
    /// Additive capabilities of the relay (same list as `hello.features`); empty on an older
    /// relay. The engine reads it here because it never calls `hello`.
    pub features: Vec<String>,
}

/// Answer of `POST /v1/auth/session`.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionGrant {
    pub token: String,
    pub expires_at_ms: i64,
    pub device_id: String,
    pub space: SpaceInfo,
}

// Never prints the bearer token (AGENTS.md golden rule 3).
impl std::fmt::Debug for SessionGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGrant")
            .field("token", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("device_id", &self.device_id)
            .field("space", &self.space)
            .finish()
    }
}

/// `{blob_id, size}` pair listed by items and transfers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub blob_id: String,
    pub size: u64,
}

/// BlobStatus (§4.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobStatus {
    pub blob_id: String,
    pub size: u64,
    pub upload_offset: u64,
    pub complete: bool,
    pub attached: bool,
}

/// Reduced DeviceRecord (§4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub sign_pk: Vec<u8>,
    pub kx_pk: Vec<u8>,
    /// `DeviceCert` wire bytes.
    pub certificate: Vec<u8>,
    pub issuer_device_id: String,
    pub joined_seq: u64,
    pub registered_at_ms: i64,
    /// `last_seen_at` (§4.3), relay clock: the last session of the device, rounded down to the
    /// minute. `None` when the relay omits it, which means "never connected" if the relay
    /// reports activity at all (see [`Self::active`]).
    pub last_seen_at_ms: Option<i64>,
    /// `active` (§0.2, §4.3). Always present on a relay that follows §4.3, so `None` means an
    /// older relay that does not report the activity of devices: then `last_seen_at_ms` says
    /// nothing, not even "never connected". The core uses it only as that marker.
    pub active: Option<bool>,
}

/// One rotation of `GET /v1/keys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationRecord {
    pub key_epoch: u32,
    pub seq: u64,
    /// `KeyRotation` wire bytes.
    pub rotation: Vec<u8>,
}

/// `GET /v1/keys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysInfo {
    pub key_epoch: u32,
    pub rotations: Vec<RotationRecord>,
}

/// An event of the log (§4.10). `data` is typed; an unknown `type` is [`EventData::Unknown`]
/// and must be skipped while advancing the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub at_ms: i64,
    pub data: EventData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventData {
    Item {
        item_id: String,
        sender_device_id: String,
        created_at_ms: i64,
        expires_at_ms: i64,
        key_epoch: u32,
        envelope: Vec<u8>,
        blobs: Vec<BlobRef>,
        ring: bool,
    },
    Ack {
        item_id: String,
        device_id: String,
        received_at_ms: i64,
    },
    Read {
        item_id: String,
        device_id: String,
        read_at_ms: i64,
    },
    Tombstone {
        item_id: String,
        device_id: String,
        deleted_at_ms: i64,
        /// `Tombstone` wire bytes, to verify.
        tombstone: Vec<u8>,
    },
    /// Removal of one attachment (`format-v1.md` §8.10). Fields other than `statement` are the
    /// relay's copy, to be checked against the signed statement; absent ones are empty.
    AttachmentTombstone {
        item_id: String,
        slot: u32,
        blob_id: String,
        device_id: String,
        deleted_at_ms: i64,
        /// `AttachmentTombstone` wire bytes, to verify.
        statement: Vec<u8>,
    },
    ItemState {
        event_id: String,
        device_id: String,
        key_epoch: u32,
        envelope: Vec<u8>,
    },
    Rotation {
        key_epoch: u32,
        device_id: String,
        revoked_device_ids: Vec<String>,
        /// `KeyRotation` wire bytes.
        rotation: Vec<u8>,
    },
    DeviceAdded {
        device: DeviceRecord,
    },
    DeviceUpdated {
        device_id: String,
        name: String,
    },
    DeviceRevoked {
        device_id: String,
        /// `rotation`, `self` or `admin`.
        via: String,
    },
    HistoryTransfer {
        transfer_id: String,
        from_device_id: String,
        to_device_id: String,
        total_bytes: u64,
        expires_at_ms: i64,
    },
    HistoryTransferClosed {
        transfer_id: String,
        outcome: String,
    },
    Unknown {
        kind: String,
    },
}

/// Page of `GET /v1/events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPage {
    pub events: Vec<Event>,
    pub head_seq: u64,
    pub has_more: bool,
    pub next_after: u64,
}

/// `POST /v1/items` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostItem {
    pub item_id: String,
    pub expires_at_ms: i64,
    pub key_epoch: u32,
    pub envelope: Vec<u8>,
    pub blob_ids: Vec<String>,
}

/// Answer of the idempotent creations (`POST /v1/items`, `/tombstones`, `/item-states`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub seq: u64,
    /// `true` when the relay answered `200` (replay of an earlier request).
    pub replayed: bool,
}

/// `POST /v1/transfers` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostTransfer {
    pub transfer_id: String,
    /// `HistoryTransfer` wire bytes.
    pub manifest: Vec<u8>,
    pub blob_ids: Vec<String>,
}

/// `GET /v1/transfers/{id}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferInfo {
    pub transfer_id: String,
    pub from_device_id: String,
    pub to_device_id: String,
    pub manifest: Vec<u8>,
    pub blobs: Vec<BlobRef>,
    pub expires_at_ms: i64,
}

/// `POST /v1/space` body (`api-v1.md` §3.2): the first device creates the space.
///
/// `install_token` is a secret (`pairing-v1.md` §10.2, §12): `Debug` hides it, nothing keeps it
/// after the call.
#[derive(Clone, PartialEq, Eq)]
pub struct NewSpace {
    pub install_token: String,
    pub space_id: String,
    pub name: String,
    pub platform: String,
    /// Self-signed `DeviceCert`, wire bytes.
    pub certificate: Vec<u8>,
}

impl std::fmt::Debug for NewSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewSpace")
            .field("install_token", &"<redacted>")
            .field("space_id", &self.space_id)
            .field("name", &self.name)
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

/// Answer of `POST /v1/space` (`201`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSpace {
    pub space_id: String,
    pub created_at_ms: i64,
    pub device_id: String,
    pub key_epoch: u32,
    pub head_seq: u64,
}

/// `POST /v1/devices` body (`api-v1.md` §4.3): the inviter registers the new device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDevice {
    pub channel_id: String,
    pub key_epoch: u32,
    pub name: String,
    pub platform: String,
    /// `DeviceCert` of the new device, issued and signed by the caller, wire bytes.
    pub certificate: Vec<u8>,
}

/// State of a pairing channel (`api-v1.md` §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    Open,
    Claimed,
    Completed,
    Closed,
}

impl ChannelState {
    /// Parses the wire name; `None` for a state this build does not know.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => ChannelState::Open,
            "claimed" => ChannelState::Claimed,
            "completed" => ChannelState::Completed,
            "closed" => ChannelState::Closed,
            _ => return None,
        })
    }
}

/// Answer of `POST /v1/pairing/channels`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingChannel {
    /// 1 to 4 decimal digits, no leading zero.
    pub channel_id: String,
    pub state: ChannelState,
    pub expires_at_ms: i64,
}

/// Answer of `POST /v1/pairing/channels/{id}/claim`. The token authenticates the new device
/// on this channel only (`Authorization: Pairing`); it lives in memory, never in a log.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingClaim {
    pub pairing_token: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for PairingClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingClaim")
            .field("pairing_token", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// Who calls a pairing route (`api-v1.md` §2.4, §4.6): the side is deduced from the
/// authentication.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PairingAuth<'a> {
    /// The inviter: `Authorization: Bearer <session>`, the session installed in the transport.
    Inviter,
    /// The new device: `Authorization: Pairing <token>`.
    Joiner(&'a str),
}

impl std::fmt::Debug for PairingAuth<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairingAuth::Inviter => f.write_str("PairingAuth::Inviter"),
            PairingAuth::Joiner(_) => f.write_str("PairingAuth::Joiner(<redacted>)"),
        }
    }
}

/// Side of a pairing channel that wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingSide {
    Inviter,
    Joiner,
}

/// A message of a pairing channel: opaque to the relay (`api-v1.md` §4.6).
#[derive(Clone, PartialEq, Eq)]
pub struct PairingMessage {
    /// Position in the channel, from 1, common to both sides.
    pub index: u64,
    pub side: PairingSide,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for PairingMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingMessage")
            .field("index", &self.index)
            .field("side", &self.side)
            .field("len", &self.body.len())
            .finish()
    }
}

/// Answer of `GET /v1/pairing/channels/{id}/messages`: the messages of the other side after
/// the given index, and the state of the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingMessages {
    pub state: ChannelState,
    pub expires_at_ms: i64,
    pub messages: Vec<PairingMessage>,
}

/// Message of the default implementation of the pairing methods.
const NO_PAIRING: &str = "pairing is not supported by this transport";

/// The relay as seen by the engine. Every method maps to one endpoint of `api-v1.md`.
/// Implementations must be thread-safe; calls block.
pub trait RelayTransport: Send + Sync {
    /// Installs (or clears) the `Bearer` session token used by authenticated calls.
    fn set_session_token(&self, token: Option<String>);

    // --- unauthenticated ---
    fn hello(&self) -> TResult<Hello>;
    fn auth_challenge(&self, device_id: &str) -> TResult<Challenge>;
    fn auth_session(&self, request: &SessionRequest) -> TResult<SessionGrant>;

    // --- space ---
    fn delete_session(&self) -> TResult<()>;
    /// `DELETE /v1/me` (`api-v1.md` §4.4): this device leaves the space. The body of the
    /// answer (`seq`, `rotation_required`) is not needed by the engine. The default is a
    /// final refusal (`404`, nothing is erased, no marker is kept), so that a transport written
    /// before leaving existed still compiles.
    fn leave_space(&self) -> TResult<()> {
        Err(RelayError::api(404, "not_found"))
    }
    /// `DELETE /v1/space` with `{"deletion_id": b64}` (`api-v1.md` §4.4.1): the last device
    /// deletes the space. `Ok(())` only on `200 {"space_deleted": true}` (strict boolean); a
    /// `2xx` without it is a network error. The default answers like a relay without the route
    /// (`404`: "relay too old", nothing is erased), so that a transport written before this
    /// route existed still compiles.
    fn delete_space(&self, deletion_id: &[u8; 32]) -> TResult<()> {
        let _ = deletion_id;
        Err(RelayError::api(404, "not_found"))
    }
    /// `POST /v1/space/receipt` (`api-v1.md` §4.4.2), without session. `Ok(())` only on
    /// `200 {"space_deleted": true}`; no receipt is `Err(Api 404)`. Same default as
    /// [`delete_space`](Self::delete_space).
    fn space_deletion_receipt(&self, deletion_id: &[u8; 32]) -> TResult<()> {
        let _ = deletion_id;
        Err(RelayError::api(404, "not_found"))
    }
    fn space(&self) -> TResult<SpaceInfo>;
    fn keys(&self) -> TResult<KeysInfo>;
    fn devices(&self) -> TResult<Vec<DeviceRecord>>;
    fn rename_device(&self, device_id: &str, name: &str) -> TResult<()>;
    /// `POST /v1/rotations`; returns the `seq` of the `rotation` event.
    fn post_rotation(&self, rotation: &[u8]) -> TResult<u64>;

    // --- log ---
    fn events(&self, after: u64, limit: u32) -> TResult<EventPage>;

    // --- items ---
    fn post_item(&self, item: &PostItem) -> TResult<Accepted>;
    fn ack_item(&self, item_id: &str, release: bool) -> TResult<()>;
    fn read_item(&self, item_id: &str) -> TResult<()>;
    fn post_tombstone(&self, tombstone: &[u8]) -> TResult<Accepted>;
    /// `POST /v1/attachment-tombstones` (`api-v1.md` §4.8 bis). The default answers like a
    /// relay without the route (`404`): the operation stays queued, never dropped.
    fn post_attachment_tombstone(&self, statement: &[u8]) -> TResult<Accepted> {
        let _ = statement;
        Err(RelayError::api(404, "not_found"))
    }
    fn post_item_state(&self, event_id: &str, key_epoch: u32, envelope: &[u8])
        -> TResult<Accepted>;

    // --- blobs ---
    fn create_blob(&self, blob_id: &str, size: u64) -> TResult<BlobStatus>;
    fn blob_status(&self, blob_id: &str) -> TResult<BlobStatus>;
    /// `PATCH` of one part at `offset`; returns the new `Upload-Offset`.
    fn put_blob_part(&self, blob_id: &str, offset: u64, data: &[u8]) -> TResult<u64>;
    /// `GET` of `len` bytes from `offset` (a `Range` request).
    fn get_blob_range(&self, blob_id: &str, offset: u64, len: u64) -> TResult<Vec<u8>>;
    fn delete_blob(&self, blob_id: &str) -> TResult<()>;

    // --- doorbell ---
    /// Opens the doorbell connection (WebSocket, `api-v1.md` §5), authenticated with the
    /// current session token. A transport without a doorbell leaves the default.
    fn open_wake(&self) -> TResult<Box<dyn WakeSource>> {
        Err(RelayError::Network("no doorbell on this transport".into()))
    }

    // --- history transfers ---
    fn post_transfer(&self, transfer: &PostTransfer) -> TResult<Accepted>;
    fn get_transfer(&self, transfer_id: &str) -> TResult<TransferInfo>;
    /// `DELETE /v1/transfers/{id}` with an `outcome`.
    fn delete_transfer(&self, transfer_id: &str, outcome: &str) -> TResult<()>;

    // --- pairing (`pairing-v1.md`). The defaults refuse, so that a transport written
    // before pairing existed still compiles; the HTTP transport and the double implement them. ---

    /// Authority of the relay this transport talks to now (`host`, and `:port` unless it is the
    /// default of the scheme), lowercase ASCII; `None` when no relay is configured. It enters the
    /// PAKE channel identifier, so it must be the address this transport really talks to
    /// (`pairing-v1.md` §3.3).
    fn relay_authority(&self) -> Option<String> {
        None
    }

    /// Base URL of the relay this transport talks to now (`https://host[:port][/prefix]`,
    /// normalised); `None` when no relay is configured.
    fn relay_url(&self) -> Option<String> {
        None
    }

    /// Points the transport at another relay (`None`: no relay, nothing is sent any more) and
    /// drops the session token. Only a device that is not provisioned changes relay
    /// (`pairing-v1.md` §4.4): the engine checks that, and checks the URL. The default refuses,
    /// so that a transport with a fixed relay still compiles.
    fn set_relay(&self, url: Option<&str>) -> TResult<()> {
        let _ = url;
        Err(RelayError::Network(
            "this transport has a fixed relay".into(),
        ))
    }

    /// `POST /v1/space`, without session (`api-v1.md` §3.2).
    fn post_space(&self, space: &NewSpace) -> TResult<CreatedSpace> {
        let _ = space;
        Err(RelayError::Network(NO_PAIRING.into()))
    }

    /// `POST /v1/devices`, session of the inviter (`api-v1.md` §4.3).
    fn post_device(&self, device: &NewDevice) -> TResult<DeviceRecord> {
        let _ = device;
        Err(RelayError::Network(NO_PAIRING.into()))
    }

    /// `POST /v1/pairing/channels`, session of the inviter (`api-v1.md` §4.6).
    fn create_pairing_channel(&self) -> TResult<PairingChannel> {
        Err(RelayError::Network(NO_PAIRING.into()))
    }

    /// `POST /v1/pairing/channels/{id}/claim`, without authentication.
    fn claim_pairing_channel(&self, channel_id: &str) -> TResult<PairingClaim> {
        let _ = channel_id;
        Err(RelayError::Network(NO_PAIRING.into()))
    }

    /// `POST /v1/pairing/channels/{id}/messages`; returns the index of the message.
    fn post_pairing_message(
        &self,
        channel_id: &str,
        auth: PairingAuth<'_>,
        body: &[u8],
    ) -> TResult<u64> {
        let _ = (channel_id, auth, body);
        Err(RelayError::Network(NO_PAIRING.into()))
    }

    /// `GET /v1/pairing/channels/{id}/messages?after=<index>&wait=<secs>`: the messages of the
    /// other side with an index above `after`, waiting up to `wait_secs` (0 to 25) if none.
    fn pairing_messages(
        &self,
        channel_id: &str,
        auth: PairingAuth<'_>,
        after: u64,
        wait_secs: u32,
    ) -> TResult<PairingMessages> {
        let _ = (channel_id, auth, after, wait_secs);
        Err(RelayError::Network(NO_PAIRING.into()))
    }

    /// `DELETE /v1/pairing/channels/{id}`.
    fn close_pairing_channel(&self, channel_id: &str, auth: PairingAuth<'_>) -> TResult<()> {
        let _ = (channel_id, auth);
        Err(RelayError::Network(NO_PAIRING.into()))
    }
}

/// A source of "doorbell" wake-ups (`api-v1.md` §5): the WebSocket carries no data, only the
/// `seq` up to which the log has grown. The engine reacts with a [`sync`](crate::runtime::Engine::sync).
pub trait WakeSource: Send {
    /// Blocks until the next message. `Ok(Some(wake))` for a `wake`, `Ok(None)` for a
    /// message to ignore, `Err` when the connection ended.
    fn next(&mut self) -> Result<Option<Wake>, WakeClosed>;
}

/// `{"type":"wake","seq":44,"ring":true}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wake {
    pub seq: u64,
    pub ring: bool,
}

/// End of a doorbell connection, with the WebSocket close code when known (§5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeClosed {
    pub code: Option<u16>,
}

/// What to do after a WebSocket close (`api-v1.md` §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconnect {
    /// Do not reconnect (`4003` revoked, `4008` replaced).
    Stop,
    /// `4004` (`space_reset`): not a proof of anything (`api-v1.md` §5.4, §7.9). Open a fresh
    /// session first: still a member, reconnect with the normal delay; otherwise stop.
    Probe,
    /// Renew the session, then reconnect at once (`4001`, `4002`).
    RenewSession,
    /// Reconnect after a uniform delay in `[1 s, 15 s]` (`1001`, `1012`).
    AfterRestart,
    /// Reconnect after full jitter `uniform(0, min(60 s, 1 s · 2^attempt))`.
    Backoff,
}

impl Reconnect {
    pub fn for_close(code: Option<u16>) -> Self {
        match code {
            Some(4003 | 4008) => Reconnect::Stop,
            Some(4004) => Reconnect::Probe,
            Some(4001 | 4002) => Reconnect::RenewSession,
            Some(1001 | 1012) => Reconnect::AfterRestart,
            _ => Reconnect::Backoff,
        }
    }
}

/// Full-jitter ceiling for attempt `n` (`api-v1.md` §5.5): `min(60 s, 1 s · 2^n)`, in ms.
pub fn reconnect_ceiling_ms(attempt: u32) -> u64 {
    (1000u64.saturating_mul(1u64 << attempt.min(16))).min(60_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_policy_follows_the_spec_table() {
        assert_eq!(Reconnect::for_close(Some(4003)), Reconnect::Stop);
        assert_eq!(Reconnect::for_close(Some(4004)), Reconnect::Probe);
        // `try_again_later`: the space is pending, the normal delay applies.
        assert_eq!(Reconnect::for_close(Some(1013)), Reconnect::Backoff);
        assert_eq!(Reconnect::for_close(Some(4008)), Reconnect::Stop);
        assert_eq!(Reconnect::for_close(Some(4001)), Reconnect::RenewSession);
        assert_eq!(Reconnect::for_close(Some(4002)), Reconnect::RenewSession);
        assert_eq!(Reconnect::for_close(Some(1012)), Reconnect::AfterRestart);
        assert_eq!(Reconnect::for_close(Some(4000)), Reconnect::Backoff);
        assert_eq!(Reconnect::for_close(None), Reconnect::Backoff);
        assert_eq!(reconnect_ceiling_ms(0), 1000);
        assert_eq!(reconnect_ceiling_ms(3), 8000);
        assert_eq!(reconnect_ceiling_ms(30), 60_000);
    }

    #[test]
    fn debug_of_a_session_grant_never_prints_the_token() {
        let g = SessionGrant {
            token: "SECRET-BEARER".into(),
            expires_at_ms: 5,
            device_id: "dev-1".into(),
            space: SpaceInfo {
                space_id: "sp-1".into(),
                effective_format: 1,
                key_epoch: 1,
                rotation_required: false,
                head_seq: 0,
                server_time_ms: 0,
                quota_used_bytes: 0,
                quota_max_bytes: 0,
                limits: Limits::default(),
                features: Vec::new(),
            },
        };
        for dbg in [format!("{g:?}"), format!("{g:#?}")] {
            assert!(!dbg.contains("SECRET-BEARER"), "{dbg}");
            assert!(dbg.contains("dev-1") && dbg.contains("<redacted>"), "{dbg}");
        }
    }

    #[test]
    fn error_classes() {
        assert!(RelayError::Network("x".into()).is_retryable());
        assert!(RelayError::api(429, "rate_limited").is_retryable());
        assert!(RelayError::api(503, "unavailable").is_retryable());
        assert!(!RelayError::api(409, "item_id_conflict").is_retryable());
        assert_eq!(RelayError::api(409, "x").code(), Some("x"));
    }
}
