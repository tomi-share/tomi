//! UniFFI bindings (feature `ffi`): the interface the apps call, as Rust types and proc-macros.
//!
//! [`RustTomiCore`] is the real implementation of the `TomiCore` contract of the apps
//! (Kotlin `apps/android/.../core/TomiCore.kt`, Rust `apps/windows/.../core_api.rs`,
//! `spec/pin-contract.md`) on top of the [`Engine`](crate::runtime::Engine): SQLite store,
//! HTTPS relay transport, outbox, history transfer.
//!
//! # Threading and `Flow`
//!
//! Every method **blocks** (SQLite and network). Kotlin calls them from `Dispatchers.IO`;
//! the Windows app from a blocking task. UniFFI has no `Flow`: the feed is exposed as
//! [`RustTomiCore::items`] (a snapshot, newest first) plus [`CoreListener::on_changed`]
//! (fired after every change visible in the feed, the devices, the transfers or the
//! space state). The Kotlin adapter turns that pair into a `Flow` (`callbackFlow` that
//! re-reads `items()` on each notification, and once at collection start).
//!
//! # Pairing and space creation
//!
//! `spec/pairing-v1.md`. The whole orchestration runs in the core (code, PAKE, verification
//! code, batch, registration at the relay, provisioning): the device keys, the space key and the
//! secrets of a session never cross this boundary. The app only gets handles and what it has to
//! show. [`RustTomiCore::provision`] stays for tests and tools: an app never feeds a pairing
//! batch to it (`pairing-v1.md` §8.4).
//!
//! | who | entry point | handle |
//! |-----|-------------|--------|
//! | first device of a space | [`RustTomiCore::create_space`] | none |
//! | existing device that adds one ("A") | [`RustTomiCore::start_invitation`] | [`FfiInviteSession`] |
//! | new device ("N") | [`RustTomiCore::join_space`] | [`FfiJoinSession`] |
//! | N, at the next start after a stop | [`RustTomiCore::resume_pairing`] | none |
//!
//! ```text
//! A: start_invitation -> show code() and link() (QR) -> wait_for_peer -> show sas + peer
//!      -> [user: "Add"] confirm -> wait_done -> sync                  [user: "Cancel": cancel]
//! N: join_space(code or link) -> wait_for_inviter -> show sas + inviter (+ the history question)
//!      -> [user: "Join"] confirm(import_history) -> wait_done          [user: "Cancel": cancel]
//! ```
//!
//! ## Driving a session
//!
//! The core never spawns a thread: **the app drives each session from a background thread**
//! (`Dispatchers.IO`), like every other call of this file. `poll(wait_secs)` is the single step
//! (at most one read of the channel, and the registration of the new device when it is due;
//! `0` never waits for a frame); `wait_for_peer`, `wait_for_inviter` and `wait_done` are loops
//! of it that block (long polls of 25 s, 600 s at most) until there is something to show or
//! the pairing is over; `phase`, `prompt`, `outcome`, `code` and `link` are snapshots that
//! answer at once. One driving call (`poll`, `wait_*`, `confirm`) at a time
//! per session, otherwise the second fails with `InvalidInput` and changes nothing; `cancel`
//! and the snapshots can be called from any thread at any moment, even while a wait is blocked
//! (the blocked call then returns `pairing_cancelled`). Do not drive a session from
//! [`CoreListener::on_changed`].
//!
//! A session ends in `Done`, `Cancelled` or `Failed` (read `phase()`). An error of a call ends
//! it too, except `Network`, `Storage`, and `Relay` with status 429 or 500 and above: those
//! leave the session as it was, call again. `InvalidInput` from `confirm` or `wait_done`
//! ("not the moment") also leaves it alive. Dropping a handle closes nothing at the relay: call
//! `cancel` when the screen closes before the end. Release the handle too (Kotlin `close()`)
//! once the session is over: the verified batch that a live [`FfiJoinSession`] owns is not
//! settled by [`RustTomiCore::resume_pairing`] until the handle is released.
//!
//! ## Safety properties kept by the core, not by this file
//!
//! * the space key leaves A only after **both** people confirmed: nothing in this API sends the
//!   batch before A's `confirm` **and** N's `confirm` (neither `poll` nor `wait_*` confirms
//!   for the user, and `wait_done` fails at once if the user of that side did not confirm);
//! * the verified batch of N is sealed in the store before its acknowledgement leaves, and
//!   [`RustTomiCore::resume_pairing`] settles it after a stop.
//!
//! ## Secrets
//!
//! The code, the link, the verification code (SAS) and the install token are secrets: they are
//! exposed to be **shown**, never to be logged, copied or typed back. On this side their `Debug`
//! output is redacted, and the inputs of `create_space` and `join_space` are wiped when the call
//! ends. The Kotlin records generated from `FfiVerifyPrompt` and `FfiJoinPrompt` are data classes
//! whose `toString()` prints every field: the adapter must not print or log them.
//!
//! # Leaving the space
//!
//! `spec/api-v1.md` §7.9. Two gestures: [`RustTomiCore::leave_space`] ("Sign out",
//! `DELETE /v1/me`) and, after `Refused { LastDevice }` and an irreversible confirmation,
//! [`RustTomiCore::delete_space`] ("Delete space", `DELETE /v1/space`). Each writes a
//! departure marker first, then makes one attempt and returns an [`FfiDepartureOutcome`]:
//! every answer of the relay is there, `Err` is only for `InvalidInput` and `Storage`.
//!
//! The core erases everything it holds (every row of the database, compacted; the files; the
//! keys; the session) **only on a positive proof** of the relay: `200` with `seq` for
//! `DELETE /v1/me`, `403 revoked`, or `200 {"space_deleted": true}`. Nothing else, not a `401`,
//! not `409 space_not_initialized` (an unmounted relay volume answers exactly that), not any
//! state kept from earlier. Otherwise the departure stays pending: [`RustTomiCore::departure`]
//! says so (show it at all times, with "Retry" = [`RustTomiCore::retry_departure`] and
//! "Annuler" = [`RustTomiCore::cancel_departure`]). At start-up, before `resume_pairing`, and on
//! a back-off or when the network comes back, call [`RustTomiCore::continue_departure`]: it
//! sends the request again only within 15 minutes of the last gesture, and only verifies later.
//!
//! **Duration.** These calls block for the whole attempt, which chains several requests with
//! their own time limits: on a network that answers nothing in time, up to 520 s (about 9
//! minutes) for `leave_space` / `retry_departure`, up to 1060 s (about 18 minutes) for
//! `delete_space`, up to 280 s for `continue_departure` outside the 15-minute window; plus the
//! wait for an attempt already running. Never "about a minute". Call them from a background
//! thread, keep showing `departure()` (written before the first request), and let "Annuler"
//! work meanwhile: `cancel_departure` does not wait.
//!
//! "Erase anyway" (D4) is [`RustTomiCore::erase_without_proof`]: the same erasure
//! without any call to the relay, allowed only while [`RustTomiCore::can_erase_without_proof`]
//! is true (the relay said "not found" or "too old" less than 10 minutes ago). The core cannot
//! tell who calls it: **the app must ask for strong biometrics first, every time**, and an
//! honest irreversible confirmation. After an
//! erasure `is_paired()` is false and the doorbell loop is told to stop. What the app keeps
//! outside the core (notifications, clipboard, copies of files to send, screens) is the app's
//! to forget. The key rotation that the remaining members owe after a departure is done by the
//! core itself.
//!
//! # Generating the Kotlin / Swift bindings
//!
//! ```text
//! cargo build -p tomi-core --release --features ffi            # per Android ABI with cargo-ndk
//! cargo run  -p tomi-core --features ffi-bindgen --bin uniffi-bindgen -- \
//!     generate --library target/release/libtomi_core.so --language kotlin --out-dir <dir>
//! ```
//!
//! The generated `uniffi/tomi_core/tomi_core.kt` and the `libtomi_core.so` of each ABI are
//! then added to the Android app (JNA `net.java.dev.jna:jna` is required by UniFFI). The
//! Swift bindings and the XCFramework of the Apple apps are built by
//! `apps/apple/tools/build-core.sh`.
//!
//! # Demo mode
//!
//! [`RustTomiCore::open_demo`] (feature `demo`) gives the same object, backed by the real
//! engine and an in-memory demo space (`runtime::demo`): the apps use it instead of a fake
//! core of their own, so every client gets the same demo behaviour from one implementation.
//! Pairing, archive and [`RustTomiCore::simulate_incoming`] are part of the contract in both
//! modes; where the real implementation does not exist yet (pairing) the real core answers
//! `Unsupported`.
//!
//! # Unified core
//!
//! A core opened by [`RustTomiCore::open`] can also *enter* the demo
//! ([`RustTomiCore::enter_demo`]) and leave it ([`RustTomiCore::leave_demo`]) without closing
//! its real engine (`runtime::unified`): for an app, entering is "this device just joined a
//! space" and leaving is "this device left it". Only while the real device is in no space.
//! [`RustTomiCore::supports_demo_entry`] says whether the build has it. This is the core the
//! released apps run on; `open_demo` stays for development builds that start in the demo.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "demo")]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use crate::item::{ReadPolicy, SourceFormat, Thumbnail};
use crate::pairing::{HistoryAnnouncement, HistoryOffer};
use crate::runtime::http::HttpTransport;
use crate::runtime::store::{DeviceState, ItemOrigin, Store};
use crate::runtime::{
    self, AwaitedHistory, Blocked, DispatchOutcome, DraftAttachment, DraftText, Engine,
    EngineConfig, InviteOutcome, InvitePhase, InviteSession, ItemContentView, JoinOutcome,
    JoinPhase, JoinPrompt, JoinSession, OutgoingDraft, PairingPeer, Provision, ResumeOutcome,
    SpaceStatus, SystemClock, UnifiedCore, VerifyPrompt,
};
use crate::{CoreError, DeviceKeys, SpaceKey, TrustStore, UnreadableReason};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error of every call (`TomiCoreException` on Android). Never carries content or keys.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TomiError {
    /// The local database or files failed.
    #[error("storage: {reason}")]
    Storage { reason: String },
    /// The relay could not be reached. Retry later.
    #[error("network: {reason}")]
    Network { reason: String },
    /// The relay refused the request (`code` is the stable code of `api-v1.md` §2.5).
    #[error("relay {status}: {code}")]
    Relay { status: u16, code: String },
    /// The device cannot use the relay until the user acts (revoked, update required...).
    #[error("blocked: {reason}")]
    Blocked { reason: String },
    /// Invalid argument or state (not provisioned, unknown device...).
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    /// Cryptographic, format or pairing failure (`code` is `CoreError::code`).
    ///
    /// The pairing codes are stable (`pairing-v1.md` §11.5): `pairing_failed` (wrong code,
    /// unexpected frame, refused batch: deliberately indistinguishable), `pairing_expired`,
    /// `pairing_taken` (another device already used the code), `pairing_cancelled` (the other
    /// device or the user cancelled) and `pairing_unsupported` (update Tomi on both devices),
    /// and `relay_not_configured` (no relay: the build has no default and neither a link nor the
    /// "Relais" field gave one; nothing was sent).
    /// The other pairing refusals keep their own variant: `Network` (offline, retry),
    /// `Relay { 409, "too_many_devices" | "channel_limit" | "epoch_conflict" | "space_exists" }`,
    /// `Relay { 403, "install_token_invalid" }`, `Blocked`, and `InvalidInput` for a code or a
    /// link that cannot be read (nothing was consumed), an invalid device name, a device that is
    /// already provisioned, or a call that is not valid at that moment.
    #[error("crypto: {code}")]
    Crypto { code: String },
    #[error("internal: {reason}")]
    Internal { reason: String },
}

impl From<CoreError> for TomiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::Storage { reason } => TomiError::Storage { reason },
            CoreError::Network { reason } => TomiError::Network { reason },
            CoreError::Relay { status, code } => TomiError::Relay { status, code },
            CoreError::Blocked { reason } => TomiError::Blocked { reason },
            CoreError::InvalidInput { reason } => TomiError::InvalidInput { reason },
            CoreError::Internal { reason } => TomiError::Internal { reason },
            other => TomiError::Crypto {
                code: other.code().to_owned(),
            },
        }
    }
}

type FfiResult<T> = std::result::Result<T, TomiError>;

// ---------------------------------------------------------------------------
// Records and enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TextFormat {
    Plain,
    Markdown,
    Html,
}

// The Debug impls of the content-carrying records below never print content (title, text,
// url, file names, paths, thumbnails): a `{:?}` in a host log must not leak a secret
// (AGENTS.md golden rule 3; same policy as `Item` in item.rs).

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct TextContent {
    /// Always present: the safe fallback.
    pub plain: String,
    pub html: Option<String>,
    pub markdown: Option<String>,
    pub source_format: TextFormat,
}

impl std::fmt::Debug for TextContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextContent")
            .field("source_format", &self.source_format)
            .field("has_html", &self.html.is_some())
            .field("has_markdown", &self.markdown.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReadPolicyKind {
    Normal,
    AfterFirstRead,
}

/// Why an item cannot be shown. The item is never dropped because of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum UnreadableKind {
    /// Unknown content version: update the app.
    UpdateRequired,
    /// Could not be decrypted or decoded.
    DecryptionFailed,
    /// Sealed with a key this device does not have yet (retried after the next rotation).
    KeyPending,
    /// Sealed with a replaced key after the rotation that replaced it: not displayed.
    Suspect,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiAttachment {
    /// Position of the attachment in the payload, stable for the life of the item. What
    /// `attachment_path` and `delete_attachment` take. Equal to the index in the list as long
    /// as no attachment was removed; a removed one is not listed and the others keep their slot.
    pub slot: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Encoded image (PNG / JPEG) generated by the sender.
    pub thumbnail: Option<Vec<u8>>,
    pub thumbnail_mime: Option<String>,
    /// Absolute path of the decrypted copy while it is in the cache; null otherwise (call
    /// [`RustTomiCore::attachment_path`], which decrypts on demand).
    pub local_path: Option<String>,
    /// `false` when the file cannot be fetched any more.
    pub available: bool,
}

impl std::fmt::Debug for FfiAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiAttachment")
            .field("slot", &self.slot)
            .field("size", &self.size)
            .field("has_thumbnail", &self.thumbnail.is_some())
            .field("has_local_path", &self.local_path.is_some())
            .field("available", &self.available)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FfiItemContent {
    Readable {
        title: Option<String>,
        text: Option<TextContent>,
        url: Option<String>,
        attachments: Vec<FfiAttachment>,
        secret: bool,
    },
    Unreadable {
        reason: UnreadableKind,
    },
}

impl std::fmt::Debug for FfiItemContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FfiItemContent::Readable {
                title,
                text,
                url,
                attachments,
                secret,
            } => f
                .debug_struct("Readable")
                .field("has_title", &title.is_some())
                .field("has_text", &text.is_some())
                .field("has_url", &url.is_some())
                .field("attachments", &attachments.len())
                .field("secret", secret)
                .finish(),
            FfiItemContent::Unreadable { reason } => f
                .debug_struct("Unreadable")
                .field("reason", reason)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeliveryKind {
    Queued,
    Sent,
    Delivered,
    ExpiredBeforeDelivery,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDelivery {
    pub device_id: String,
    pub device_name: String,
    pub state: DeliveryKind,
}

/// One item of the feed (`TomiItem`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiItem {
    pub id: String,
    pub content: FfiItemContent,
    pub source_device_id: String,
    pub source_device_name: String,
    /// Milliseconds since the Unix epoch.
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub read_policy: ReadPolicyKind,
    pub read: bool,
    /// Sent from this device (the UI shows `deliveries`).
    pub outgoing: bool,
    pub deliveries: Vec<FfiDelivery>,
    /// Pinned on all devices (`spec/pin-contract.md`).
    pub pinned: bool,
    /// Time of the decision to pin (ms), for the "Pinned" section order; null if not pinned.
    pub pinned_at_ms: Option<i64>,
    /// Archived on all devices (`spec/archive-contract.md`). Never true for a pinned item (a
    /// pin wins when both states are set).
    pub archived: bool,
    /// Time of the decision to archive (ms); null if not archived.
    pub archived_at_ms: Option<i64>,
}

// `content` is masked by its own Debug; device names are metadata, not content.
impl std::fmt::Debug for FfiItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiItem")
            .field("id", &self.id)
            .field("content", &self.content)
            .field("source_device_id", &self.source_device_id)
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("read_policy", &self.read_policy)
            .field("read", &self.read)
            .field("outgoing", &self.outgoing)
            .field("deliveries", &self.deliveries.len())
            .field("pinned", &self.pinned)
            .field("pinned_at_ms", &self.pinned_at_ms)
            .field("archived", &self.archived)
            .field("archived_at_ms", &self.archived_at_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceStateKind {
    Verified,
    Unverified,
    Revoked,
    RevokedByRelay,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDevice {
    pub id: String,
    pub name: String,
    /// `android`, `windows`, `ios`, `macos`, `cli` or `unknown` (any other value may come).
    pub platform: String,
    pub state: DeviceStateKind,
    pub last_seen_at_ms: Option<i64>,
    pub is_this_device: bool,
}

/// A device the relay removed on its own authority (`take_relay_revocations`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiRelayRevocation {
    pub device_id: String,
    /// The device's name, or its id when unknown.
    pub name: String,
    /// `admin` (the relay's administrator) or `self` (the device left on its own, as far as the
    /// relay says).
    pub via: String,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDraftAttachment {
    /// File staged by the app; read before `enqueue` returns.
    pub path: String,
    pub name: String,
    pub mime: String,
    pub thumbnail: Option<Vec<u8>>,
    pub thumbnail_mime: Option<String>,
}

impl std::fmt::Debug for FfiDraftAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiDraftAttachment")
            .field("has_thumbnail", &self.thumbnail.is_some())
            .finish_non_exhaustive()
    }
}

/// `OutgoingDraft`.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDraft {
    /// Chosen by the app (UUID v4) so that a retry is idempotent; generated if null.
    pub id: Option<String>,
    pub title: Option<String>,
    pub text: Option<TextContent>,
    pub url: Option<String>,
    pub attachments: Vec<FfiDraftAttachment>,
    pub secret: bool,
    pub expires_at_ms: Option<i64>,
    pub read_policy: ReadPolicyKind,
}

impl std::fmt::Debug for FfiDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiDraft")
            .field("id", &self.id)
            .field("has_title", &self.title.is_some())
            .field("has_text", &self.text.is_some())
            .field("has_url", &self.url.is_some())
            .field("attachments", &self.attachments.len())
            .field("secret", &self.secret)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("read_policy", &self.read_policy)
            .finish()
    }
}

/// What `delete_attachment` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AttachmentRemovalKind {
    /// The attachment is gone (or already was).
    AttachmentRemoved,
    /// Nothing was left in the item: the whole item was deleted, with the same scope.
    ItemDeleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DispatchKind {
    Sent,
    RetryLater,
    NotFound,
    /// Refused for good by the relay or expired before sending.
    Failed,
}

/// `SpaceState` of the Kotlin contract (plus `Blocked` for the other blocking reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SpaceStateKind {
    Active,
    Revoked,
    UpdateRequired,
    Blocked,
    /// The relay does not find the space or this device ("space not found"). Not a proof
    /// of anything: nothing is erased (`spec/api-v1.md` §7.2, §7.9).
    NotFound,
}

/// Which departure is pending (`spec/api-v1.md` §7.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiDepartureKind {
    /// "Sign out".
    Leave,
    /// "Delete space" (last device).
    DeleteSpace,
}

/// Why a departure is still pending; nothing was erased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiPendingReason {
    /// Network error, timeout, unexpected answer.
    Network,
    /// `5xx`, `429`.
    Server,
    /// "Space not found": `can_erase_without_proof` may be true.
    NotFound,
    /// Verification only, more than 15 minutes after the gesture: still a member, the request
    /// had no effect ("Retry" sends it again).
    NotSent,
}

/// Why the relay refused a departure for good; nothing was erased, nothing is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiRefusalReason {
    /// The only device of the space: offer "Delete space".
    LastDevice,
    UpdateRequired,
    /// The relay cannot delete a space: `can_erase_without_proof` is true.
    RelayTooOld,
    Unexpected,
}

/// Result of a departure attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiDepartureOutcome {
    /// Proven by the relay, everything erased: `is_paired()` is false.
    Erased,
    Pending {
        reason: FfiPendingReason,
    },
    Refused {
        reason: FfiRefusalReason,
    },
    /// Cancelled meanwhile: nothing was erased.
    Cancelled,
    /// No departure pending (`continue_departure`).
    NothingPending,
}

/// The departure pending on this device.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDeparture {
    pub kind: FfiDepartureKind,
    /// The proof came, the erasure runs: no button.
    pub wiping: bool,
    /// Time of the last gesture; null for a marker written by an older version.
    pub since_ms: Option<i64>,
    /// Why the last attempt of this process is pending; null before the first one.
    pub last: Option<FfiPendingReason>,
}

fn ffi_pending(r: crate::runtime::PendingReason) -> FfiPendingReason {
    use crate::runtime::PendingReason as P;
    match r {
        P::Network => FfiPendingReason::Network,
        P::Server => FfiPendingReason::Server,
        P::NotFound => FfiPendingReason::NotFound,
        P::NotSent => FfiPendingReason::NotSent,
    }
}

/// The stop flag of the doorbell loop the app started last. Each call of `run_doorbell` gets a
/// flag of its own, so that a stop meant for an old loop can never be undone by the start of
/// the next one (which used to reset a single shared flag, and let two loops live).
struct DoorbellStop(Mutex<Arc<AtomicBool>>);

impl DoorbellStop {
    fn new() -> Self {
        Self(Mutex::new(Arc::new(AtomicBool::new(false))))
    }

    fn current(&self) -> Arc<AtomicBool> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Tells the loop started last to stop (a no-op if none runs).
    fn stop(&self) {
        self.current().store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn is_stopped(&self) -> bool {
        self.current().load(Ordering::SeqCst)
    }

    /// Starts a new loop: the previous one is told to stop, and the new one gets a fresh flag.
    fn begin(&self) -> Arc<AtomicBool> {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.store(true, Ordering::SeqCst);
        *slot = Arc::new(AtomicBool::new(false));
        slot.clone()
    }
}

/// After a departure attempt: the doorbell stops once the device is erased.
fn departure_done(
    doorbell_stop: &DoorbellStop,
    outcome: crate::runtime::DepartureOutcome,
) -> FfiResult<FfiDepartureOutcome> {
    if outcome == crate::runtime::DepartureOutcome::Erased {
        doorbell_stop.stop();
    }
    Ok(ffi_outcome(outcome))
}

fn ffi_outcome(o: crate::runtime::DepartureOutcome) -> FfiDepartureOutcome {
    use crate::runtime::{DepartureOutcome as O, RefusalReason as R};
    match o {
        O::Erased => FfiDepartureOutcome::Erased,
        O::Pending(r) => FfiDepartureOutcome::Pending {
            reason: ffi_pending(r),
        },
        O::Refused(r) => FfiDepartureOutcome::Refused {
            reason: match r {
                R::LastDevice => FfiRefusalReason::LastDevice,
                R::UpdateRequired => FfiRefusalReason::UpdateRequired,
                R::RelayTooOld => FfiRefusalReason::RelayTooOld,
                R::Unexpected => FfiRefusalReason::Unexpected,
            },
        },
        O::Cancelled => FfiDepartureOutcome::Cancelled,
        O::NothingPending => FfiDepartureOutcome::NothingPending,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiSyncReport {
    pub already_running: bool,
    pub events_applied: u64,
    pub new_items: u64,
    pub cursor: u64,
    pub full_resync: bool,
    pub outbox_pending: u32,
    /// The relay could not be reached.
    pub offline: bool,
    /// The relay asks for a key rotation. The core does it by itself (after a random delay of
    /// 0 to 30 s, at a later `sync` or from the doorbell loop): nothing to call. Informative.
    pub rotation_required: bool,
}

/// A history transfer task, either way.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiTransfer {
    pub transfer_id: String,
    /// `export` or `import`.
    pub role: String,
    pub peer_device_id: String,
    pub peer_name: String,
    /// `building`, `uploading`, `posted`, `received`, `downloading`, `importing`, `files`,
    /// `done`, `failed`, `cancelled`, `declined`...
    pub state: String,
    pub items_total: u64,
    pub items_done: u64,
    pub files_total: u64,
    pub files_done: u64,
    pub finished: bool,
    pub error: Option<String>,
}

/// Static configuration given at start.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoreConfig {
    /// SQLite file (created on first start).
    pub db_path: String,
    /// Directory of staged and downloaded files (app-private storage).
    pub files_dir: String,
    /// 32 bytes from the Android Keystore / DPAPI that seal the secrets of the database.
    pub wrap_key: Vec<u8>,
    /// The relay of a device that has none of its own yet: a default for development builds
    /// (`https://relay.example.org`), `None` in a release build, where the relay comes from the
    /// QR code or link of the pairing, from the "Relais" field next to the code
    /// ([`RustTomiCore::join_space`], [`RustTomiCore::set_relay`]) or from the creation link.
    /// A device that is paired keeps the relay of its space, whatever this says. The value is
    /// checked (`https` only, `InvalidInput` otherwise) and never stored by itself.
    pub relay_url: Option<String>,
    pub app_version: String,
    /// `android`, `windows`, `ios`, `macos` or `cli`.
    pub platform: String,
    /// Import an incoming history as soon as it is announced (`false`: wait for
    /// [`RustTomiCore::accept_import`]).
    pub auto_import: bool,
}

/// Low-level provisioning data, to provision this device once: the keys as they are stored.
/// For tests and tools. Pairing and space creation provision the device inside the core
/// ([`RustTomiCore::create_space`], [`FfiJoinSession`]): an app does not carry their secrets
/// through this record (`pairing-v1.md` §8.4).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProvisionData {
    pub ed25519_seed: Vec<u8>,
    pub x25519_secret: Vec<u8>,
    /// `TrustStore::to_json` (public data).
    pub trust_json: String,
    pub space_key: Vec<u8>,
    pub key_epoch: u32,
    pub device_name: String,
}

// Hand-written, redacting the secret fields: the module header promises that the `Debug` of
// this side's types does not print key material. The derived `Debug` would print the seed,
// the X25519 secret and the space key.
impl std::fmt::Debug for ProvisionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionData")
            .field("ed25519_seed", &"<redacted>")
            .field("x25519_secret", &"<redacted>")
            .field("trust_json", &self.trust_json)
            .field("space_key", &"<redacted>")
            .field("key_epoch", &self.key_epoch)
            .field("device_name", &self.device_name)
            .finish()
    }
}

impl std::fmt::Debug for CoreConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreConfig")
            .field("db_path", &self.db_path)
            .field("files_dir", &self.files_dir)
            .field("wrap_key", &"<redacted>")
            .field("relay_url", &self.relay_url)
            .field("app_version", &self.app_version)
            .field("platform", &self.platform)
            .field("auto_import", &self.auto_import)
            .finish()
    }
}

/// Why a pairing request failed (Kotlin `PairingFailure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PairingFailureKind {
    InvalidCode,
    Expired,
    Network,
    /// This core cannot pair yet (the PAKE channel of `spec/pairing-v1.md` is not
    /// implemented); only the demo core pairs.
    Unsupported,
}

/// Answer of [`RustTomiCore::pair_with_code`] / [`RustTomiCore::pair_with_qr`] (Kotlin
/// `PairingResult`). On success the device has NOT joined yet: the user compares the
/// 6-digit `verification_code`, then the app calls `confirm_pairing` or `cancel_pairing`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PairingOutcome {
    AwaitingVerification {
        device: FfiDevice,
        verification_code: String,
        /// Items of the space this device could import (0: nothing to ask).
        history_item_count: u32,
        /// Bytes of that history, an indication for the screen (0: unknown).
        history_bytes: u64,
        /// Devices of the space before this one joins, if known.
        member_count: Option<u16>,
        /// Validity of the code from now, an indication (never a cut-off on this side).
        expires_in_ms: i64,
        /// Authority (`host[:port]`) of the relay of this pairing, to show in full next to the
        /// verification code: it may come from a QR code or a field, so the user reads and
        /// confirms it (`pairing-v1.md` §4.4).
        relay_authority: String,
    },
    Failure {
        reason: PairingFailureKind,
    },
}

/// How the relay of the demo answers the next departures (demo core only, debug menus):
/// see `demo::DemoDeparture`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DemoDepartureMode {
    Normal,
    Offline,
    ServerError,
    LastDevice,
    Reset,
    OldRelay,
}

/// What [`RustTomiCore::simulate_incoming`] receives (demo core only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DemoIncomingKind {
    Link,
    Text,
    RichText,
    Image,
    File,
    Secret,
    Unreadable,
}

/// Configuration of [`RustTomiCore::open_demo`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DemoConfig {
    /// App-private directory for the demo files; emptied at every opening.
    pub files_dir: String,
    /// Bundled demo files (`demo_paysage.jpg`, `demo_main.rs`...); built-in stand-ins
    /// replace the missing ones.
    pub assets_dir: Option<String>,
    pub platform: String,
    pub app_version: String,
    /// Name of this device in the demo space.
    pub device_name: String,
    /// `true`: already in the space with the demo feed; `false`: starts unpaired.
    pub start_paired: bool,
}

/// Callback implemented by the app.
#[uniffi::export(with_foreign)]
pub trait CoreListener: Send + Sync {
    /// Something changed (feed, devices, transfers, space state): re-read what you display.
    /// Called from a core thread, never with a core lock held; an exception thrown here is
    /// swallowed. Return quickly: re-reading (`items()`) is fine, a sync is not.
    fn on_changed(&self);
}

// ---------------------------------------------------------------------------
// Pairing: records and enums
// ---------------------------------------------------------------------------

/// Where an invitation is (`InvitePhase`). The session is over in `Done`, `Cancelled` and
/// `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum InvitePhaseKind {
    /// The code is shown; the new device has not said hello yet.
    AwaitPeer,
    /// The verification code and the new device can be read ([`FfiInviteSession::prompt`]):
    /// compare, then `confirm` or `cancel`.
    Verify,
    /// Both people confirmed: the batch is exchanged and the device registered.
    Provisioning,
    /// The new device is part of the space.
    Done,
    /// Cancelled here, or closed by the other device.
    Cancelled,
    /// Any other end: wrong code, expired code, refusal of the relay... The error of the call
    /// that failed says why (a stable `pairing_*` code, see [`TomiError::Crypto`]).
    Failed,
}

/// Where a joining is (`JoinPhase`). The session is over in `Done`, `Cancelled` and `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum JoinPhaseKind {
    /// The channel is claimed; the inviter has not shown its side yet.
    AwaitInviter,
    /// The verification code and the inviter can be read ([`FfiJoinSession::prompt`]):
    /// compare, answer the history question if it is asked, then `confirm` or `cancel`.
    Verify,
    /// Confirmed here; waiting for the inviter's user and the batch.
    Provisioning,
    /// The batch is verified and sealed; waiting for the inviter to register this device.
    Completing,
    /// This device is part of the space.
    Done,
    /// Cancelled here, or closed by the other device.
    Cancelled,
    /// Any other end: wrong code, expired code, refused batch... The error of the call that
    /// failed says why (a stable `pairing_*` code, see [`TomiError::Crypto`]).
    Failed,
}

/// What [`RustTomiCore::resume_pairing`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ResumeKind {
    /// Nothing was pending, or a founder whose space the relay does not know yet (it can still
    /// call [`RustTomiCore::create_space`]), or a batch that a live session still owns.
    Nothing,
    /// The relay knows this device: it was provisioned, a session opened and a sync done.
    Provisioned,
    /// The relay does not know this device (the inviter did not register it): the pending batch
    /// was erased.
    Discarded,
}

/// The other device, as it announced itself on the authenticated channel (authentic if the two
/// verification codes are the same).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiPairingPeer {
    /// 1 to 64 characters.
    pub name: String,
    /// `android`, `windows`, `cli`... (`^[a-z0-9-]{1,32}$`).
    pub platform: String,
}

/// What the inviter offers to transfer to the new device: an estimate, for display only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiHistoryOffer {
    /// Exportable items. Never `0` in a prompt: nothing to offer means no offer.
    pub items: u64,
    /// Approximate size in bytes.
    pub bytes: u64,
}

/// A history transfer announced at pairing: the inviter exports with exactly this id, and the
/// new device imports it without asking again. Counters are for display only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiHistoryAnnouncement {
    pub transfer_id: String,
    pub items: u64,
    pub bytes: u64,
}

/// What the user of the inviter compares and decides on.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiVerifyPrompt {
    /// The verification code in two groups of three digits (`"482 913"`). **Secret**: show it,
    /// never log it (the `toString()` of the generated Kotlin record prints it), never ask the
    /// user to type it.
    pub sas: String,
    /// The new device, as it announced itself.
    pub peer: FfiPairingPeer,
}

impl std::fmt::Debug for FfiVerifyPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiVerifyPrompt")
            .field("sas", &"<redacted>")
            .field("peer", &self.peer)
            .finish()
    }
}

/// What the user of the new device compares and decides on.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiJoinPrompt {
    /// Authority (`host[:port]`) of the relay of this pairing: show it in full next to the
    /// verification code. It may have been read from a QR code or typed, so the user reads and
    /// confirms it with the code (`pairing-v1.md` §4.4).
    pub relay_authority: String,
    /// The verification code in two groups of three digits (`"482 913"`). **Secret**: show it,
    /// never log it (the `toString()` of the generated Kotlin record prints it), never ask the
    /// user to type it.
    pub sas: String,
    /// The inviter, as it announced itself.
    pub inviter: FfiPairingPeer,
    /// Devices of the space, the inviter included ("Your space has 3 devices").
    pub member_count: Option<u16>,
    /// What the inviter offers to transfer. When it is present, ask the single yes / no
    /// question and pass the answer to [`FfiJoinSession::confirm`]; when it is absent the
    /// question is not asked.
    pub history: Option<FfiHistoryOffer>,
}

impl std::fmt::Debug for FfiJoinPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiJoinPrompt")
            .field("relay_authority", &self.relay_authority)
            .field("sas", &"<redacted>")
            .field("inviter", &self.inviter)
            .field("member_count", &self.member_count)
            .field("history", &self.history)
            .finish()
    }
}

/// The new device is registered and in the trust store of the inviter.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiInviteOutcome {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    /// The history transfer announced to the new device. Its export is a task that the next
    /// [`RustTomiCore::sync`] starts and runs: call `sync` once the screen says the device was
    /// added.
    pub history: Option<FfiHistoryAnnouncement>,
}

/// This device is provisioned: it belongs to the space ([`RustTomiCore::is_paired`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiJoinOutcome {
    pub device_id: String,
    pub space_id: String,
    /// The history transfer the inviter announced (the user said yes): it is imported when it
    /// arrives, without a question. See [`RustTomiCore::awaited_history`].
    pub history: Option<FfiHistoryAnnouncement>,
}

/// A history transfer announced to this device at pairing and not received yet
/// ("History being prepared on PC-Bureau").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiAwaitedHistory {
    pub transfer_id: String,
    /// Device id of the inviter, the only sender accepted for this transfer.
    pub from_device_id: String,
    /// From the inviter's estimate (display only).
    pub items: u64,
    /// From the inviter's estimate (display only).
    pub bytes: u64,
    /// When the pairing completed, on this device's clock (ms).
    pub announced_at_ms: i64,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn to_format(f: SourceFormat) -> TextFormat {
    match f {
        SourceFormat::Plain => TextFormat::Plain,
        SourceFormat::Markdown => TextFormat::Markdown,
        SourceFormat::Html => TextFormat::Html,
    }
}

fn from_format(f: TextFormat) -> SourceFormat {
    match f {
        TextFormat::Plain => SourceFormat::Plain,
        TextFormat::Markdown => SourceFormat::Markdown,
        TextFormat::Html => SourceFormat::Html,
    }
}

fn to_text(t: DraftText) -> TextContent {
    TextContent {
        plain: t.plain,
        html: t.html,
        markdown: t.markdown,
        source_format: to_format(t.source_format),
    }
}

fn to_policy(p: ReadPolicy) -> ReadPolicyKind {
    match p {
        ReadPolicy::UntilExpiry => ReadPolicyKind::Normal,
        ReadPolicy::AfterFirstRead => ReadPolicyKind::AfterFirstRead,
    }
}

fn unreadable(r: UnreadableReason) -> UnreadableKind {
    match r {
        UnreadableReason::UpdateRequired => UnreadableKind::UpdateRequired,
        UnreadableReason::KeyPending => UnreadableKind::KeyPending,
        UnreadableReason::SuspectEpoch => UnreadableKind::Suspect,
        UnreadableReason::Undecryptable => UnreadableKind::DecryptionFailed,
    }
}

fn to_item(v: runtime::ItemView) -> FfiItem {
    let content = match v.content {
        ItemContentView::Readable {
            title,
            text,
            url,
            attachments,
            secret,
        } => FfiItemContent::Readable {
            title,
            text: text.map(to_text),
            url,
            attachments: attachments
                .into_iter()
                .map(|a| FfiAttachment {
                    slot: a.slot,
                    name: a.name,
                    mime: a.mime,
                    size: a.size,
                    thumbnail_mime: a.thumbnail.as_ref().map(|t| t.mime.clone()),
                    thumbnail: a.thumbnail.map(|t| t.data),
                    local_path: a.local_path,
                    available: a.available,
                })
                .collect(),
            secret,
        },
        ItemContentView::Unreadable { reason } => FfiItemContent::Unreadable {
            reason: unreadable(reason),
        },
    };
    FfiItem {
        id: v.id,
        content,
        source_device_id: v.source_device_id,
        source_device_name: v.source_device_name,
        created_at_ms: v.created_at_ms,
        expires_at_ms: v.expires_at_ms,
        read_policy: to_policy(v.read_policy),
        read: v.read,
        outgoing: v.origin == ItemOrigin::Sent,
        deliveries: v
            .deliveries
            .into_iter()
            .map(|d| FfiDelivery {
                device_id: d.device_id,
                device_name: d.device_name,
                state: match d.state {
                    runtime::DeliveryState::Queued => DeliveryKind::Queued,
                    runtime::DeliveryState::Sent => DeliveryKind::Sent,
                    runtime::DeliveryState::Delivered => DeliveryKind::Delivered,
                    runtime::DeliveryState::ExpiredBeforeDelivery => {
                        DeliveryKind::ExpiredBeforeDelivery
                    }
                    runtime::DeliveryState::Failed => DeliveryKind::Failed,
                },
            })
            .collect(),
        pinned: v.pinned,
        pinned_at_ms: v.pinned_at_ms,
        archived: v.archived,
        archived_at_ms: v.archived_at_ms,
    }
}

fn to_device(d: runtime::DeviceView) -> FfiDevice {
    FfiDevice {
        id: d.id,
        name: d.name,
        platform: d.platform,
        state: match d.state {
            DeviceState::Verified => DeviceStateKind::Verified,
            DeviceState::Unverified => DeviceStateKind::Unverified,
            DeviceState::Revoked => DeviceStateKind::Revoked,
            DeviceState::RevokedByRelay => DeviceStateKind::RevokedByRelay,
        },
        last_seen_at_ms: d.last_seen_at_ms,
        is_this_device: d.is_this_device,
    }
}

fn from_draft(d: FfiDraft) -> OutgoingDraft {
    OutgoingDraft {
        id: d.id,
        title: d.title,
        text: d.text.map(|t| DraftText {
            plain: t.plain,
            html: t.html,
            markdown: t.markdown,
            source_format: from_format(t.source_format),
        }),
        url: d.url,
        attachments: d
            .attachments
            .into_iter()
            .map(|a| DraftAttachment {
                path: a.path,
                name: a.name,
                mime: a.mime,
                thumbnail: a.thumbnail.map(|data| Thumbnail {
                    mime: a.thumbnail_mime.unwrap_or_else(|| "image/png".into()),
                    data,
                }),
            })
            .collect(),
        secret: d.secret,
        expires_at_ms: d.expires_at_ms,
        read_policy: match d.read_policy {
            ReadPolicyKind::Normal => ReadPolicy::UntilExpiry,
            ReadPolicyKind::AfterFirstRead => ReadPolicy::AfterFirstRead,
        },
    }
}

fn to_peer(p: PairingPeer) -> FfiPairingPeer {
    FfiPairingPeer {
        name: p.name,
        platform: p.platform,
    }
}

fn to_offer(o: HistoryOffer) -> FfiHistoryOffer {
    FfiHistoryOffer {
        items: o.items,
        bytes: o.bytes,
    }
}

fn to_announcement(h: HistoryAnnouncement) -> FfiHistoryAnnouncement {
    FfiHistoryAnnouncement {
        transfer_id: h.transfer_id,
        items: h.items,
        bytes: h.bytes,
    }
}

fn to_verify_prompt(p: VerifyPrompt) -> FfiVerifyPrompt {
    FfiVerifyPrompt {
        sas: p.sas,
        peer: to_peer(p.peer),
    }
}

fn to_join_prompt(p: JoinPrompt) -> FfiJoinPrompt {
    FfiJoinPrompt {
        relay_authority: p.relay_authority,
        sas: p.sas,
        inviter: to_peer(p.inviter),
        member_count: p.member_count,
        history: p.history.map(to_offer),
    }
}

fn to_invite_phase(p: InvitePhase) -> InvitePhaseKind {
    match p {
        InvitePhase::AwaitPeer => InvitePhaseKind::AwaitPeer,
        InvitePhase::Verify => InvitePhaseKind::Verify,
        InvitePhase::Provisioning => InvitePhaseKind::Provisioning,
        InvitePhase::Done => InvitePhaseKind::Done,
        InvitePhase::Cancelled => InvitePhaseKind::Cancelled,
        InvitePhase::Failed => InvitePhaseKind::Failed,
    }
}

fn to_join_phase(p: JoinPhase) -> JoinPhaseKind {
    match p {
        JoinPhase::AwaitInviter => JoinPhaseKind::AwaitInviter,
        JoinPhase::Verify => JoinPhaseKind::Verify,
        JoinPhase::Provisioning => JoinPhaseKind::Provisioning,
        JoinPhase::Completing => JoinPhaseKind::Completing,
        JoinPhase::Done => JoinPhaseKind::Done,
        JoinPhase::Cancelled => JoinPhaseKind::Cancelled,
        JoinPhase::Failed => JoinPhaseKind::Failed,
    }
}

fn to_invite_outcome(o: InviteOutcome) -> FfiInviteOutcome {
    FfiInviteOutcome {
        device_id: o.device_id,
        name: o.name,
        platform: o.platform,
        history: o.history.map(to_announcement),
    }
}

fn to_join_outcome(o: JoinOutcome) -> FfiJoinOutcome {
    FfiJoinOutcome {
        device_id: o.device_id,
        space_id: o.space_id,
        history: o.history.map(to_announcement),
    }
}

fn to_awaited(h: AwaitedHistory) -> FfiAwaitedHistory {
    FfiAwaitedHistory {
        transfer_id: h.transfer_id,
        from_device_id: h.from_device_id,
        items: h.items,
        bytes: h.bytes,
        announced_at_ms: h.announced_at_ms,
    }
}

/// Milliseconds left before `expires_at_ms` (a time on the relay clock), by the estimate of the
/// relay clock of the engine; never negative. For a countdown: the relay is the judge.
fn remaining_ms(engine: &Engine, expires_at_ms: i64) -> i64 {
    expires_at_ms.saturating_sub(engine.server_now_ms()).max(0)
}

// ---------------------------------------------------------------------------
// The object
// ---------------------------------------------------------------------------

/// The core, as the apps hold it for the whole life of the process.
#[derive(uniffi::Object)]
pub struct RustTomiCore {
    /// The real engine of this device and, while a demo is entered, the demo space
    /// (`runtime::unified`). Read the active engine through `engine()`.
    hub: UnifiedCore,
    doorbell_stop: DoorbellStop,
    #[allow(clippy::type_complexity)]
    listener: Mutex<Option<Arc<dyn CoreListener>>>,
    /// Set once by `open_demo`: this core IS a demo (the former demo core, no real engine behind).
    #[cfg(feature = "demo")]
    demo: OnceLock<Arc<crate::runtime::demo::DemoWorld>>,
}

#[cfg(feature = "demo")]
impl From<crate::runtime::demo::DemoPairing> for PairingOutcome {
    fn from(p: crate::runtime::demo::DemoPairing) -> Self {
        use crate::runtime::demo::DemoPairing as P;
        match p {
            P::AwaitingVerification {
                peer,
                verification_code,
                history_item_count,
                history_bytes,
                member_count,
                expires_in_ms,
                relay_authority,
            } => PairingOutcome::AwaitingVerification {
                device: to_device(peer),
                verification_code,
                history_item_count,
                history_bytes,
                member_count: Some(member_count),
                expires_in_ms,
                relay_authority,
            },
            P::InvalidCode => PairingOutcome::Failure {
                reason: PairingFailureKind::InvalidCode,
            },
        }
    }
}

#[cfg(feature = "demo")]
fn to_demo_incoming(kind: DemoIncomingKind) -> crate::runtime::demo::DemoIncoming {
    use crate::runtime::demo::DemoIncoming as D;
    match kind {
        DemoIncomingKind::Link => D::Link,
        DemoIncomingKind::Text => D::Text,
        DemoIncomingKind::RichText => D::RichText,
        DemoIncomingKind::Image => D::Image,
        DemoIncomingKind::File => D::File,
        DemoIncomingKind::Secret => D::Secret,
        DemoIncomingKind::Unreadable => D::Unreadable,
    }
}

impl RustTomiCore {
    /// Wraps an engine built by the caller (tests, desktop shells with their own transport).
    pub fn with_engine(engine: Arc<Engine>) -> Arc<Self> {
        let core = Arc::new(Self {
            hub: UnifiedCore::new(engine),
            doorbell_stop: DoorbellStop::new(),
            listener: Mutex::new(None),
            #[cfg(feature = "demo")]
            demo: OnceLock::new(),
        });
        let weak = Arc::downgrade(&core);
        core.hub.subscribe(Arc::new(move || {
            if let Some(c) = weak.upgrade() {
                // Cloned out of the lock: the callback runs with no lock held.
                let l = c.current_listener();
                if let Some(l) = l {
                    // An exception thrown by the app's callback becomes a panic here; it
                    // must not reach the engine (the engine also guards, see `notify`).
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        l.on_changed();
                    }));
                }
            }
        }));
        core
    }

    fn current_listener(&self) -> Option<Arc<dyn CoreListener>> {
        self.listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The engine the calls go to: the demo's while a demo is entered, the real one otherwise.
    fn engine(&self) -> Arc<Engine> {
        self.hub.engine()
    }

    /// The demo space behind this core: the one `open_demo` built, or the one `enter_demo`
    /// entered.
    #[cfg(feature = "demo")]
    fn demo_world(&self) -> Option<Arc<crate::runtime::demo::DemoWorld>> {
        self.demo.get().cloned().or_else(|| self.hub.demo_world())
    }

    /// After a departure attempt: the doorbell stops once the device is erased, and a demo whose
    /// device left is over.
    fn departed(
        &self,
        outcome: crate::runtime::DepartureOutcome,
    ) -> FfiResult<FfiDepartureOutcome> {
        let result = departure_done(&self.doorbell_stop, outcome);
        self.hub.settle_demo();
        result
    }
}

#[uniffi::export]
impl RustTomiCore {
    /// Opens the database and the relay client. Does no network call.
    #[uniffi::constructor]
    pub fn open(mut config: CoreConfig) -> FfiResult<Arc<Self>> {
        // `wrap_key` is a plain `Vec` in the UniFFI record (API unchanged): move it into a
        // zeroizing wrapper so that our copy is wiped as soon as the store is open, on every
        // path.
        let wrap_key = Zeroizing::new(std::mem::take(&mut config.wrap_key));
        let store = Store::open(
            &PathBuf::from(&config.db_path),
            &PathBuf::from(&config.files_dir),
            &wrap_key,
        )?;
        drop(wrap_key);
        // No relay in a release build: the device then waits for one (link, QR code, field).
        let transport = Arc::new(HttpTransport::with_relay(config.relay_url.as_deref())?);
        let engine = Engine::open(
            EngineConfig {
                app_version: config.app_version,
                platform: config.platform,
                auto_import: config.auto_import,
                default_relay_url: config.relay_url,
            },
            store,
            transport,
            Arc::new(SystemClock),
        )?;
        Ok(Self::with_engine(Arc::new(engine)))
    }

    /// Demo core (feature `demo`): the real engine on an in-memory demo space, no network,
    /// nothing kept after the process ends (`runtime::demo`). Blocks while the demo history
    /// is written (well under a second): call it off the main thread. A build without the
    /// feature answers `Internal`.
    #[uniffi::constructor]
    pub fn open_demo(config: DemoConfig) -> FfiResult<Arc<Self>> {
        #[cfg(feature = "demo")]
        {
            use crate::runtime::demo;
            let world = demo::DemoWorld::open(demo::DemoConfig {
                files_dir: PathBuf::from(config.files_dir),
                assets_dir: config.assets_dir.map(PathBuf::from),
                platform: config.platform,
                app_version: config.app_version,
                device_name: config.device_name,
                start_paired: config.start_paired,
            })?;
            let core = Self::with_engine(world.engine());
            let _ = core.demo.set(world);
            Ok(core)
        }
        #[cfg(not(feature = "demo"))]
        {
            let _ = config;
            Err(TomiError::Internal {
                reason: "demo mode not compiled in".into(),
            })
        }
    }

    /// `true` while the calls go to a demo space: a core opened by [`open_demo`](Self::open_demo), or a real core that entered the demo ([`enter_demo`](Self::enter_demo)).
    pub fn is_demo(&self) -> bool {
        #[cfg(feature = "demo")]
        {
            self.demo_world().is_some()
        }
        #[cfg(not(feature = "demo"))]
        {
            false
        }
    }

    /// `true` when this core can enter the demo ([`enter_demo`](Self::enter_demo)): the demo is
    /// compiled in (feature `demo`) and this core was opened by [`open`](Self::open), not by
    /// [`open_demo`](Self::open_demo). It says nothing about the state: `enter_demo` still
    /// refuses a device that is in a space. Constant for the life of the core.
    pub fn supports_demo_entry(&self) -> bool {
        #[cfg(feature = "demo")]
        {
            self.demo.get().is_none()
        }
        #[cfg(not(feature = "demo"))]
        {
            false
        }
    }

    /// Enters the demo from the real core: a fresh demo space (in memory, no network), already
    /// joined, with the demo feed. From then on every call of this object goes to the demo,
    /// [`is_demo`](Self::is_demo) and [`is_paired`](Self::is_paired) are `true`, and the listener
    /// is told. The real engine is neither closed nor changed. Nothing is kept after the process
    /// ends. **Blocks** while the demo history is written: call it off the main thread.
    ///
    /// `config.start_paired` is ignored (always joined). Already in the demo: nothing happens.
    /// Errors: `InvalidInput` (this device is in a space, a pairing is pending, or this core is a
    /// demo core already), `Internal` (demo not compiled in), `Storage`.
    pub fn enter_demo(&self, config: DemoConfig) -> FfiResult<()> {
        #[cfg(feature = "demo")]
        {
            use crate::runtime::demo;
            if self.demo.get().is_some() {
                return Err(TomiError::InvalidInput {
                    reason: "already a demo core".into(),
                });
            }
            Ok(self.hub.enter_demo(demo::DemoConfig {
                files_dir: PathBuf::from(config.files_dir),
                assets_dir: config.assets_dir.map(PathBuf::from),
                platform: config.platform,
                app_version: config.app_version,
                device_name: config.device_name,
                start_paired: true,
            })?)
        }
        #[cfg(not(feature = "demo"))]
        {
            let _ = config;
            Err(TomiError::Internal {
                reason: "demo mode not compiled in".into(),
            })
        }
    }

    /// Leaves the demo entered with [`enter_demo`](Self::enter_demo): the demo space is dropped
    /// (files erased), the calls go to the real engine again and the listener is told. `false`
    /// when no demo was entered (also on a core opened by `open_demo`, which cannot leave). A
    /// successful `leave_space` inside the demo leaves it too.
    pub fn leave_demo(&self) -> bool {
        self.hub.leave_demo()
    }

    /// Registers the single listener (replaces the previous one).
    pub fn set_listener(&self, listener: Option<Arc<dyn CoreListener>>) {
        *self
            .listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = listener;
    }

    // --- relay ---------------------------------------------------------------------------

    /// The relay this device talks to now (`https://host[:port]`): the one of its space once
    /// paired; before, the one a pairing or a creation under way is using, the one chosen with
    /// [`set_relay`](Self::set_relay), or the build's default; `null` when there is none.
    pub fn relay_url(&self) -> Option<String> {
        self.engine().relay_url()
    }

    /// Chooses the relay of a device that is not paired and keeps it (the "Relais" field when
    /// the app wants it before the code). `InvalidInput` for a URL that is not a valid relay
    /// (`https` only; no user information, query or fragment; a port written with digits only,
    /// without sign or leading zero; an ASCII host), for a device that is paired (the relay of
    /// its space never changes), and while a pairing or a creation is under way or pending.
    ///
    /// **One exception, once:** a device that is paired but keeps no relay (paired by an older
    /// version, opened by a build without a default relay: it has none, `relay_url()` is `null`)
    /// accepts one; it is then the relay of its space for good, and the next call is refused.
    /// Does no network call. Prefer passing the relay to [`join_space`](Self::join_space) /
    /// [`create_space`](Self::create_space): nothing is then kept before it worked.
    pub fn set_relay(&self, url: String) -> FfiResult<()> {
        Ok(self.engine().set_relay(&url)?)
    }

    // --- state ---------------------------------------------------------------

    /// `isPaired`: this device belongs to a space.
    pub fn is_paired(&self) -> bool {
        self.engine().is_provisioned()
    }

    /// Stores the identity and keys obtained by pairing or space creation, once.
    pub fn provision(&self, data: ProvisionData) -> FfiResult<()> {
        // The secret Vecs copied from the foreign side are wiped once the typed keys are built.
        let ed25519_seed = Zeroizing::new(data.ed25519_seed);
        let x25519_secret = Zeroizing::new(data.x25519_secret);
        let space_key_bytes = Zeroizing::new(data.space_key);
        let keys = DeviceKeys::from_secrets(&ed25519_seed, &x25519_secret)?;
        let trust = TrustStore::from_json(&data.trust_json)?;
        let space_key = SpaceKey::from_bytes(data.key_epoch, &space_key_bytes)?;
        Ok(self.engine().provision(Provision {
            keys,
            trust,
            space_key,
            device_name: data.device_name,
        })?)
    }

    /// `spaceState`.
    pub fn space_state(&self) -> SpaceStateKind {
        match self.engine().space_status() {
            SpaceStatus::Active => SpaceStateKind::Active,
            SpaceStatus::Revoked => SpaceStateKind::Revoked,
            SpaceStatus::UpdateRequired => SpaceStateKind::UpdateRequired,
            SpaceStatus::Blocked(_) => SpaceStateKind::Blocked,
            SpaceStatus::NotFound => SpaceStateKind::NotFound,
        }
    }

    /// Link to open when the state is `UpdateRequired`.
    pub fn update_url(&self) -> Option<String> {
        match self.engine().blocked() {
            Some(Blocked::UpdateRequired { update_url }) => update_url,
            _ => None,
        }
    }

    /// `localDevice`.
    pub fn local_device(&self) -> FfiResult<FfiDevice> {
        Ok(to_device(self.engine().local_device()?))
    }

    /// `devices`: all devices known, revoked ones included (filter on `state`).
    pub fn devices(&self) -> FfiResult<Vec<FfiDevice>> {
        Ok(self
            .engine()
            .list_devices()?
            .into_iter()
            .map(to_device)
            .collect())
    }

    // --- feed ----------------------------------------------------------------

    /// `items`: newest first, queued outgoing items included, expired ones excluded.
    pub fn items(&self) -> FfiResult<Vec<FfiItem>> {
        Ok(self
            .engine()
            .list_items()?
            .into_iter()
            .map(to_item)
            .collect())
    }

    pub fn get_item(&self, id: String) -> FfiResult<Option<FfiItem>> {
        Ok(self.engine().get_item(&id)?.map(to_item))
    }

    // --- sync ----------------------------------------------------------------

    /// `sync`: fetches the log, drains the outbox, advances transfers. Safe to call often.
    ///
    /// `push_token` (FCM) is accepted for the contract; registering it with the relay
    /// (`PUT /v1/me/push`) is not implemented yet.
    pub fn sync(&self, push_token: Option<String>) -> FfiResult<FfiSyncReport> {
        let _ = push_token;
        let r = self.engine().sync()?;
        Ok(FfiSyncReport {
            already_running: r.already_running,
            events_applied: r.events_applied,
            new_items: r.new_items,
            cursor: r.cursor,
            full_resync: r.full_resync,
            outbox_pending: r.outbox_pending,
            offline: r.offline,
            rotation_required: r.rotation_required,
        })
    }

    /// Keeps the WebSocket doorbell open until [`stop_doorbell`](Self::stop_doorbell): sync on
    /// every wake, reconnect with jitter. **Blocks**: call it from a dedicated thread while
    /// the app is in the foreground.
    ///
    /// Returns when [`stop_doorbell`](Self::stop_doorbell) was called, or for good when the
    /// device is revoked (`4003`, `403 revoked`), replaced (`4008`) or left the space. It also
    /// ends after a `4004` (space reset) when the fresh session that verifies it finds the
    /// space gone ("not found") or answers that an update is required. Outside that case, a
    /// relay that does not find the space or the device ("not found", for instance after an
    /// admin reset, or a volume not mounted yet) does not end it: it asks again with a
    /// back-off that doubles from 60 s up to 30 minutes (with jitter) and starts over when a
    /// session opens. The answers of other callers (an app `sync`) count too, so the back-off
    /// may reach its ceiling sooner. A wait that follows a "not found" ends by itself, within
    /// 100 ms, as soon as the state is no longer "not found" (a session opened by a `sync` of
    /// the app, a device that left). It never erases anything. To bring it back sooner when the app returns to
    /// the foreground or the network comes back, call `stop_doorbell` then `run_doorbell`
    /// again: it asks the relay at once if the last answer is a minute old.
    ///
    /// **Never two loops at the same time** (`spec/api-v1.md` §5.5), and the core makes sure of
    /// it. A call made while another loop still runs first tells that loop to stop, then waits
    /// for it to return (until the catch-up sync the old loop may be running is over: pages,
    /// outbox sends and a key rotation included) before it connects: it does not return at
    /// once, and it never opens a second
    /// connection beside the first one. So "stop then start again" needs no wait on the app's
    /// side, but each call blocks its thread until its own loop ends: run each one on a
    /// dedicated thread, and never on the main one. A `stop_doorbell` that arrives while a call
    /// still waits for the old loop makes it return without having connected. Never call it
    /// from a change listener (`on_changed` runs on the loop's own thread, gate held: a
    /// synchronous call there would wait for itself).
    pub fn run_doorbell(&self) {
        let stop = self.doorbell_stop.begin();
        #[cfg(feature = "demo")]
        if self.demo_world().is_some() {
            // No doorbell in a demo: its own worker keeps this device up to date. Block like
            // the real one until `stop_doorbell`, or until the demo is left.
            while !stop.load(Ordering::SeqCst) && self.demo_world().is_some() {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            return;
        }
        self.engine().run_doorbell(&stop);
    }

    /// Tells the doorbell loop started last to return. Does not wait for it. A stop that
    /// arrives before the matching `run_doorbell` has actually started (thread launched but
    /// not yet in the call) is not seen by that loop: an app that stops right after starting
    /// should repeat the stop once the call is running.
    pub fn stop_doorbell(&self) {
        self.doorbell_stop.stop();
    }

    /// `true` while the removal of a device is **not finished**: the automatic rotation that
    /// must follow a revocation failed with `device_set_mismatch` (the relay lists a member this
    /// device does not know yet) and is retried every hour. The revoked device may still hold
    /// the current space key meanwhile: show a persistent warning. Persisted; clears itself
    /// when a rotation succeeds or another member's is applied. Poll it after a `sync` or on
    /// a change notification.
    pub fn rotation_stalled(&self) -> bool {
        self.engine().rotation_stalled()
    }

    /// Devices the **relay alone** removed (`device_revoked` without a signed rotation: its
    /// administrator, `tomi-relay admin revoke`, or a departure it reports), not yet shown;
    /// each is returned once. The honest devices follow with a rotation that makes the
    /// removal permanent (a removed key never becomes a member again), without asking: show a
    /// plain, non-blocking notice such as "X was removed by the relay's administrator" (`via`
    /// = `admin`) so the user can react if that was not expected. Persisted until taken.
    pub fn take_relay_revocations(&self) -> FfiResult<Vec<FfiRelayRevocation>> {
        Ok(self
            .engine()
            .take_relay_revocations()?
            .into_iter()
            .map(|r| FfiRelayRevocation {
                device_id: r.device_id,
                name: r.name,
                via: r.via,
            })
            .collect())
    }

    /// `true` once after a full resync ("older items could not be recovered").
    pub fn take_resync_notice(&self) -> bool {
        self.engine().take_resync_notice()
    }

    // --- sending ---------------------------------------------------------------

    /// `enqueue`: stores the draft locally (queued, in the feed) and in the outbox.
    /// Attachment files are read before this returns. Returns the item id.
    pub fn enqueue(&self, draft: FfiDraft) -> FfiResult<String> {
        Ok(self.engine().enqueue(from_draft(draft))?)
    }

    /// `dispatch`: tries to push one queued item now. Safe to repeat.
    pub fn dispatch(&self, id: String) -> FfiResult<DispatchKind> {
        Ok(match self.engine().dispatch(&id)? {
            DispatchOutcome::Sent => DispatchKind::Sent,
            DispatchOutcome::RetryLater => DispatchKind::RetryLater,
            DispatchOutcome::NotFound => DispatchKind::NotFound,
            DispatchOutcome::Failed => DispatchKind::Failed,
        })
    }

    /// `cancelIfQueued`: the "Undo" of the send banner; atomic with `dispatch`.
    pub fn cancel_if_queued(&self, id: String) -> FfiResult<bool> {
        Ok(self.engine().cancel_if_queued(&id)?)
    }

    // --- deleting, reading, pinning, archiving ---------------------------------------------

    pub fn delete_local(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().delete_local(&id)?)
    }

    pub fn delete_everywhere(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().delete_everywhere(&id)?)
    }

    /// `deleteAttachment`: removes ONE attachment of an item, designated by the `slot` of
    /// [`FfiAttachment`] (never its position in the displayed list). Idempotent: an unknown
    /// item, an unknown or already removed slot do nothing, and the file is never downloaded.
    /// `everywhere = false`: this device only, no network. `everywhere = true`: signed and
    /// queued, replayed until the relay takes it; an item not sent yet answers
    /// `InvalidInput` (remove it on this device only). When nothing is left in the item
    /// (no other attachment, no text, no URL; the title is ignored) the item itself is deleted
    /// with the same scope: `ItemDeleted`. No biometry. Offer `everywhere` only when
    /// [`supports_attachment_removal`](Self::supports_attachment_removal) is true.
    pub fn delete_attachment(
        &self,
        id: String,
        slot: u32,
        everywhere: bool,
    ) -> FfiResult<AttachmentRemovalKind> {
        Ok(
            match self.engine().delete_attachment(&id, slot, everywhere)? {
                runtime::AttachmentRemoval::AttachmentRemoved => {
                    AttachmentRemovalKind::AttachmentRemoved
                }
                runtime::AttachmentRemoval::ItemDeleted => AttachmentRemovalKind::ItemDeleted,
            },
        )
    }

    /// `supportsAttachmentRemoval`: whether the relay takes the removal of an attachment
    /// "everywhere" (capability `attachment_tombstones`, read at the last session; false until
    /// a session has told). "On this device" is always possible.
    pub fn supports_attachment_removal(&self) -> bool {
        self.engine().supports_attachment_removal()
    }

    /// `markRead`; consumes an `after_first_read` item everywhere.
    pub fn mark_read(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().mark_read(&id)?)
    }

    /// `pin`: idempotent, immediate local effect, unknown id is a no-op. The network never
    /// fails this call. Errors (`spec/pin-contract.md` §2): `Storage`, `InvalidInput`
    /// (not provisioned), `Crypto { code: "malformed_statement" }` (time beyond 2^53 - 1 ms)
    /// or another refusal to sign (this device no longer trusted).
    pub fn pin(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().pin(&id)?)
    }

    pub fn unpin(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().unpin(&id)?)
    }

    /// `archive`: same guarantees as `pin` (idempotent, immediate local effect, unknown id is
    /// a no-op, the network never fails it, same errors). Never marks the item read, never
    /// sends an ack / read / release, never downloads anything. Archiving a pinned item
    /// unpins it first. Pin and archive are exclusive at display (`FfiItem.pinned` wins).
    pub fn archive(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().archive(&id)?)
    }

    pub fn unarchive(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().unarchive(&id)?)
    }

    // --- pairing (Kotlin `pairWithQr` / `pairWithCode` / `confirmPairing` / `cancelPairing`) --

    /// Pairing with the payload of a scanned QR code. The real core cannot pair yet and
    /// answers `Failure { Unsupported }`; the demo core simulates it.
    pub fn pair_with_qr(&self, payload: String) -> FfiResult<PairingOutcome> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            return Ok(world.pair_with_qr(&payload)?.into());
        }
        let _ = payload;
        Ok(PairingOutcome::Failure {
            reason: PairingFailureKind::Unsupported,
        })
    }

    /// Pairing with a short code typed by the user, and the relay typed next to it (the "Relais"
    /// field, absent when the device has its relay). Same answers as `pair_with_qr`.
    #[uniffi::method(default(relay_url = None))]
    pub fn pair_with_code(
        &self,
        code: String,
        relay_url: Option<String>,
    ) -> FfiResult<PairingOutcome> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            return Ok(world.pair_with_code(&code, relay_url.as_deref())?.into());
        }
        let _ = (code, relay_url);
        Ok(PairingOutcome::Failure {
            reason: PairingFailureKind::Unsupported,
        })
    }

    /// The user confirmed that the codes are identical; `import_history` is the single
    /// yes/no answer about the history. Without a pairing in progress nothing happens.
    pub fn confirm_pairing(&self, import_history: bool) -> FfiResult<()> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            return Ok(world.confirm_pairing(import_history)?);
        }
        let _ = import_history;
        Err(TomiError::Blocked {
            reason: "pairing_unsupported".into(),
        })
    }

    /// The codes differ, or the user left the verification: nothing is joined.
    pub fn cancel_pairing(&self) -> FfiResult<()> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            world.cancel_pairing();
        }
        Ok(())
    }

    /// Demo core only: another device sends an item of `kind` now (debug menus of the apps).
    pub fn simulate_incoming(&self, kind: DemoIncomingKind) -> FfiResult<()> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            return Ok(world.simulate_incoming(to_demo_incoming(kind))?);
        }
        let _ = kind;
        Err(TomiError::InvalidInput {
            reason: "not a demo core".into(),
        })
    }

    /// Demo core only: how the relay answers the next departures of this device, so that every
    /// screen of "Sign out" can be tried (offline, server error, last device, reset, old
    /// relay). The departure itself runs the real code against the relay double.
    pub fn simulate_departure(&self, mode: DemoDepartureMode) -> FfiResult<()> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            use crate::runtime::demo::DemoDeparture as D;
            world.simulate_departure(match mode {
                DemoDepartureMode::Normal => D::Normal,
                DemoDepartureMode::Offline => D::Offline,
                DemoDepartureMode::ServerError => D::ServerError,
                DemoDepartureMode::LastDevice => D::LastDevice,
                DemoDepartureMode::Reset => D::Reset,
                DemoDepartureMode::OldRelay => D::OldRelay,
            });
            return Ok(());
        }
        let _ = mode;
        Err(TomiError::InvalidInput {
            reason: "not a demo core".into(),
        })
    }

    /// `attachmentUri`: absolute path of the decrypted file, downloading it if needed. `index`
    /// is the `slot` of [`FfiAttachment`] (never the position in the displayed list; the
    /// parameter keeps its name so that the bindings do not change). Null for a removed
    /// attachment. At
    /// rest the file is only kept encrypted; this decrypts it on demand into a cache that
    /// [`purge_plaintext_cache`](Self::purge_plaintext_cache) empties. The app exposes the
    /// path through its own `FileProvider` and must call this again after a purge. Null if
    /// unavailable.
    pub fn attachment_path(&self, item_id: String, index: u32) -> FfiResult<Option<String>> {
        Ok(self
            .engine()
            .attachment_path(&item_id, index)?
            .map(|p| p.to_string_lossy().into_owned()))
    }

    /// Erases every decrypted attachment. Call it when the app goes to the background
    /// (Android `onStop`; window hidden or locked on Windows). Also done at start. Fails
    /// with `Storage` if a file is still open elsewhere (Windows); the rest is erased.
    pub fn purge_plaintext_cache(&self) -> FfiResult<()> {
        Ok(self.engine().purge_plaintext_cache()?)
    }

    // --- devices ---------------------------------------------------------------

    pub fn rename_device(&self, id: String, name: String) -> FfiResult<()> {
        Ok(self.engine().rename_device(&id, &name)?)
    }

    /// `revokeDevice`: revokes and rotates the space key. Needs the network.
    pub fn revoke_device(&self, id: String) -> FfiResult<()> {
        Ok(self.engine().revoke_device(&id)?)
    }

    /// Rotation required after a revocation by the relay; `false` if nothing to do. The core
    /// now does it by itself; kept for compatibility.
    pub fn maybe_rotate(&self) -> FfiResult<bool> {
        Ok(self.engine().maybe_rotate()?)
    }

    // --- leaving the space (`spec/api-v1.md` §7.9); all block: call them off the main thread ---

    /// Gesture "Sign out": one attempt (`DELETE /v1/me`). Erased only on a proof of the
    /// relay. Errors: `InvalidInput` (not paired), `Storage` (nothing erased, still pending).
    /// **Blocks** up to 520 s (about 9 minutes) on a network that answers nothing in time, see
    /// the module documentation.
    pub fn leave_space(&self) -> FfiResult<FfiDepartureOutcome> {
        self.departed(self.engine().leave_space()?)
    }

    /// Gesture "Delete space", only after `Refused { LastDevice }` and its irreversible
    /// confirmation (`DELETE /v1/space`). Same contract as `leave_space`; **blocks** up to
    /// 1060 s (about 18 minutes) in the worst case.
    pub fn delete_space(&self) -> FfiResult<FfiDepartureOutcome> {
        self.departed(self.engine().delete_space()?)
    }

    /// Gesture "Retry": sends the pending request again. `InvalidInput` if none. Blocks as
    /// long as the gesture it repeats (520 s, or 1060 s for a deletion).
    pub fn retry_departure(&self) -> FfiResult<FfiDepartureOutcome> {
        self.departed(self.engine().retry_departure()?)
    }

    /// Automatic attempt: at start-up (before `resume_pairing`), on a back-off, when the network
    /// comes back. Sends the request again only within 15 minutes of the last gesture; later it
    /// only verifies. `NothingPending` when nothing is pending. Blocks up to 520 s inside the
    /// window (1060 s for a deletion), 280 s when it only verifies.
    pub fn continue_departure(&self) -> FfiResult<FfiDepartureOutcome> {
        self.departed(self.engine().continue_departure()?)
    }

    /// "Annuler": immediate, never erases. `true` if a departure was cancelled; `false` when
    /// none was pending or the erasure already runs.
    pub fn cancel_departure(&self) -> FfiResult<bool> {
        Ok(self.engine().cancel_departure()?)
    }

    /// The departure pending on this device, null when none. Read it again on every change.
    pub fn departure(&self) -> Option<FfiDeparture> {
        self.engine().departure().map(|d| FfiDeparture {
            kind: match d.kind {
                crate::runtime::DepartureKind::Leave => FfiDepartureKind::Leave,
                crate::runtime::DepartureKind::DeleteSpace => FfiDepartureKind::DeleteSpace,
            },
            wiping: d.wiping,
            since_ms: d.since_ms,
            last: d.last.map(ffi_pending),
        })
    }

    /// "Erase anyway" (D4) is allowed right now: the relay said "not found" or "too old"
    /// less than 10 minutes ago, in this process. Never after a mere network error.
    pub fn can_erase_without_proof(&self) -> bool {
        self.engine().can_erase_without_proof()
    }

    /// D4: erases everything this device holds **without any call to the relay** (the device
    /// stays registered there if the space still exists). The interface asks for strong
    /// biometrics first, every time, behind an honest irreversible confirmation. **The core
    /// cannot check that this was done**: the app must, before calling. Errors:
    /// `InvalidInput` (not paired, or `can_erase_without_proof()` is false), `Storage`. It
    /// waits for a departure attempt in progress (see the module documentation).
    pub fn erase_without_proof(&self) -> FfiResult<()> {
        self.engine().erase_without_proof()?;
        self.doorbell_stop.stop();
        self.hub.settle_demo();
        Ok(())
    }

    // --- history transfer ----------------------------------------------------------

    /// Sends the whole history (secrets and files included) to `device_id`.
    pub fn start_history_export(&self, device_id: String) -> FfiResult<String> {
        Ok(self.engine().start_export(&device_id)?)
    }

    pub fn transfers(&self) -> FfiResult<Vec<FfiTransfer>> {
        Ok(self
            .engine()
            .transfers()?
            .into_iter()
            .map(|t| FfiTransfer {
                transfer_id: t.transfer_id,
                role: t.role,
                peer_device_id: t.peer_device_id,
                peer_name: t.peer_name,
                state: t.state,
                items_total: t.items_total,
                items_done: t.items_done,
                files_total: t.files_total,
                files_done: t.files_done,
                finished: t.finished,
                error: t.error,
            })
            .collect())
    }

    /// Lets a waiting incoming transfer run (only when `auto_import` is off).
    pub fn accept_import(&self, transfer_id: String) -> FfiResult<()> {
        Ok(self.engine().accept_import(&transfer_id)?)
    }

    pub fn decline_import(&self, transfer_id: String) -> FfiResult<()> {
        Ok(self.engine().decline_import(&transfer_id)?)
    }

    pub fn cancel_transfer(&self, transfer_id: String) -> FfiResult<()> {
        Ok(self.engine().cancel_transfer(&transfer_id)?)
    }

    // --- pairing and space creation ----------------------------------------------------
    //
    // See the module documentation. Every call blocks (network, SQLite): background thread.

    /// Creates the space from this device, which becomes its first member (`pairing-v1.md`
    /// §10). `install_token` is the token the relay prints at its start (`api-v1.md` §3.1),
    /// typed or pasted by the user: it is used for this call only, never stored, never logged.
    /// `device_name` is the name of this device in the space (1 to 64 characters).
    ///
    /// On success the device is provisioned, a session is open and the first sync done
    /// ([`is_paired`](Self::is_paired) is `true`). Errors:
    /// `Relay { 403, "install_token_invalid" }` (the keys stay sealed and the token was not
    /// consumed: call again with the right one), `Relay { 409, "space_exists" }` (another
    /// device created the space: offer "Rejoindre"), `Network` (the keys stay sealed: call
    /// again), `InvalidInput` (blank token, invalid name, already provisioned).
    ///
    /// The demo core accepts its demo token only (`demo::DEMO_INSTALL_TOKEN`) and starts from
    /// an empty feed.
    ///
    /// `relay_url` is the relay of the creation link (`parse_setup_link`) or of the "Relais"
    /// field; absent, the device uses its relay. **A relay that was read is not trusted**: show
    /// it in full and let the user confirm before calling. It is used for this call only and kept
    /// when the space is created; any other end puts the relay back. `InvalidInput` for a URL
    /// that is not a valid relay, `Crypto { relay_not_configured }` when there is no relay at all
    /// (nothing is sent), and while a pairing is under way at another relay (also when the keys
    /// of an earlier creation belong to another relay than the one in use: they are settled
    /// with their relay only when nothing else is under way).
    #[uniffi::method(default(relay_url = None))]
    pub fn create_space(
        &self,
        install_token: String,
        device_name: String,
        relay_url: Option<String>,
    ) -> FfiResult<()> {
        let token = Zeroizing::new(install_token);
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            let _ = relay_url;
            return Ok(world.create_space(&token, &device_name)?);
        }
        Ok(self
            .engine()
            .create_space_at(&token, &device_name, relay_url.as_deref())?)
    }

    /// Starts an invitation (A1 to A4): synchronises, opens a channel at the relay and returns
    /// the session whose `code()` and `link()` the screen shows. The relay drops the code after
    /// 120 s if nobody uses it ([`FfiInviteSession::expires_in_ms`]): start a new invitation.
    ///
    /// Errors: `InvalidInput` (not provisioned), `Blocked`, `Network` (offline),
    /// `Relay { 409, "too_many_devices" }` (the space is full), `Relay { 409, "channel_limit" }`
    /// (two pairings are already open: cancel one). Nothing is stored on failure.
    ///
    /// On the demo core a new device of the demo ("Tablette") answers the invitation a moment
    /// later, with the real protocol (`demo::DemoWorld::answer_invitation`).
    pub fn start_invitation(&self) -> FfiResult<Arc<FfiInviteSession>> {
        let inner = self.engine().start_invitation()?;
        #[cfg(feature = "demo")]
        if let (Some(world), Some(code)) = (self.demo_world(), inner.code()) {
            if let Err(e) = world.answer_invitation(Zeroizing::new(code)) {
                // No guest: the channel must not stay open at the relay.
                inner.cancel();
                return Err(e.into());
            }
        }
        Ok(Arc::new(FfiInviteSession {
            inner,
            engine: self.engine(),
        }))
    }

    /// Joins the space of the inviter that shows `code_or_link` (N0 to N2): a typed code
    /// (`4821-crayon-renard`) or a link (QR code, App Link, `tomi-share://pair#...`). `device_name` is
    /// the name this device will have in the space (1 to 64 characters).
    ///
    /// The code is read locally first: a typo costs nothing and consumes nothing
    /// (`InvalidInput`). The channel is claimed last: `pairing_expired` for a code that ran out,
    /// `pairing_taken` for a code another device already used. A pairing interrupted by a stop
    /// is settled first ([`resume_pairing`](Self::resume_pairing)); if that provisions this
    /// device the call fails with `InvalidInput` (already provisioned): check
    /// [`is_paired`](Self::is_paired) again.
    ///
    /// A link never starts anything by itself: call this when the user pressed "Rejoindre".
    ///
    /// **The relay.** A link (`https://<relay>/l/pair#<code>`, see `parse_pair_link`) carries the
    /// relay of the inviter; `relay_url` is the relay typed next to a code (`https://host`),
    /// absent when the device has its relay or the link names one (a link and a `relay_url` that
    /// name different relays are `InvalidInput`). A relay that was read is not trusted: when it
    /// is not the one the device already uses, the core points at it for this session only, and
    /// the app MUST show [`FfiJoinSession::relay_authority`] (also in
    /// [`FfiJoinPrompt::relay_authority`]) in full with the verification code. Nothing is kept
    /// before the pairing succeeds; a session that fails, is cancelled or expires puts the relay
    /// back. A candidate that is not a valid relay is `InvalidInput` (nothing consumed, nothing
    /// switched), no relay at all `Crypto { relay_not_configured }`, and a device that is already
    /// paired is refused whatever the relay. While another pairing or creation is under way a
    /// session at the same relay may start, one that needs another relay is `InvalidInput`
    /// (nothing switched): finish or `cancel()` the first one.
    #[uniffi::method(default(relay_url = None))]
    pub fn join_space(
        &self,
        code_or_link: String,
        device_name: String,
        relay_url: Option<String>,
    ) -> FfiResult<Arc<FfiJoinSession>> {
        // The two words of the code are the secret of the session: wiped from this side when
        // the call ends.
        let input = Zeroizing::new(code_or_link);
        let inner = self
            .engine()
            .join_space_at(&input, &device_name, relay_url.as_deref())?;
        Ok(Arc::new(FfiJoinSession {
            inner,
            engine: self.engine(),
        }))
    }

    /// Settles a pairing that a stop interrupted (`pairing-v1.md` §11.4, §10.2). Call it when
    /// the app starts and [`is_paired`](Self::is_paired) is `false`, and again if it failed with
    /// `Network`. It asks the relay whether it registered the device of a pending batch: if so
    /// the device is provisioned, a session opened and a sync run; if not the batch is erased.
    /// A batch that a live [`FfiJoinSession`] owns is left alone.
    pub fn resume_pairing(&self) -> FfiResult<ResumeKind> {
        Ok(match self.engine().resume_pairing()? {
            ResumeOutcome::Nothing => ResumeKind::Nothing,
            ResumeOutcome::Provisioned => ResumeKind::Provisioned,
            ResumeOutcome::Discarded => ResumeKind::Discarded,
        })
    }

    /// Whether a pairing was interrupted and left something sealed in the store (the verified
    /// batch of a new device, or the keys of a founder): then call
    /// [`resume_pairing`](Self::resume_pairing).
    pub fn has_pending_pairing(&self) -> FfiResult<bool> {
        Ok(self.engine().has_pending_pairing()?)
    }

    /// The history transfer announced to this device at pairing and not received yet, if any
    /// ("History being prepared on PC-Bureau"). Forgotten once the transfer exists as a
    /// task ([`transfers`](Self::transfers)), or when the relay would have dropped it.
    pub fn awaited_history(&self) -> FfiResult<Option<FfiAwaitedHistory>> {
        Ok(self.engine().awaited_history()?.map(to_awaited))
    }
}

// ---------------------------------------------------------------------------
// Pairing sessions
// ---------------------------------------------------------------------------

/// One invitation, from the code to the registered device: the inviter ("A"), an existing
/// member that shows a code and adds a new device. See the module documentation for the
/// threading and the sequence.
///
/// Call [`cancel`](Self::cancel) when the screen closes before the end: dropping the handle
/// closes nothing, and the relay then keeps the channel until its deadline, which counts
/// against the two channels it allows at once.
#[derive(uniffi::Object)]
pub struct FfiInviteSession {
    inner: Arc<InviteSession>,
    engine: Arc<Engine>,
}

impl std::fmt::Debug for FfiInviteSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the code: the channel id is public.
        f.debug_struct("FfiInviteSession")
            .field("channel_id", &self.inner.channel_id())
            .field("phase", &self.inner.phase())
            .finish_non_exhaustive()
    }
}

#[uniffi::export]
impl FfiInviteSession {
    /// The code to show, `4821-crayon-renard`; null once the session is over. **Secret**: show
    /// it in a large monospace font, never log it, never copy it to the clipboard.
    pub fn code(&self) -> Option<String> {
        self.inner.code()
    }

    /// The link of the QR code, `https://relay.example.org/l/pair#4821-crayon-renard`; null once
    /// the session is over. **Secret**: draw the QR code from exactly this string, never log it.
    pub fn link(&self) -> Option<String> {
        self.inner.link()
    }

    /// Identifier of the channel (not secret).
    pub fn channel_id(&self) -> String {
        self.inner.channel_id().to_owned()
    }

    /// Milliseconds left before the relay drops the channel if nobody used the code (never
    /// negative), by the estimate of the relay clock. At 0 start a new invitation.
    pub fn expires_in_ms(&self) -> i64 {
        remaining_ms(&self.engine, self.inner.expires_at_ms())
    }

    /// Where the invitation is. Answers at once.
    pub fn phase(&self) -> InvitePhaseKind {
        to_invite_phase(self.inner.phase())
    }

    /// The verification code and the new device, from the moment the new device said hello
    /// until the pairing is over; null before and after. Answers at once.
    pub fn prompt(&self) -> Option<FfiVerifyPrompt> {
        self.inner.prompt().map(to_verify_prompt)
    }

    /// One step: deposits what waits, reads the channel (waiting up to `wait_secs` for a frame,
    /// 0 to 25: `0` never waits) and goes as far as it can. **Blocks** for the round trip and the
    /// wait. Returns the phase after the step.
    ///
    /// A transient error (`Network`, `Storage`, `Relay` 429 or 5xx) leaves the session as it was:
    /// call again. Any other error ends it (`phase()` is `Cancelled` or `Failed`).
    pub fn poll(&self, wait_secs: u32) -> FfiResult<InvitePhaseKind> {
        Ok(to_invite_phase(self.inner.poll(wait_secs)?))
    }

    /// **Blocks** until the new device said hello and the verification code can be compared
    /// (long polls of 25 s); returns at once if that is already the case. Same errors as
    /// [`poll`](Self::poll): `pairing_expired` when the code ran out, `pairing_cancelled` when
    /// the other device closed the channel (a wrong code looks like that from here),
    /// `pairing_failed`, `pairing_unsupported`. The app then starts a new invitation.
    pub fn wait_for_peer(&self) -> FfiResult<FfiVerifyPrompt> {
        Ok(to_verify_prompt(self.inner.wait_for_peer()?))
    }

    /// The user compared the codes and pressed "Add". Valid once [`prompt`](Self::prompt) is
    /// available (`InvalidInput` before, the session goes on).
    ///
    /// **This alone never sends the space key**: it leaves this device only once the user of the
    /// new device confirmed too, and then at once. Follow with [`wait_done`](Self::wait_done).
    /// Blocks for the call to the relay that deposits the next frame, if any. Fails with
    /// `InvalidInput` if another driving call is running on this session.
    pub fn confirm(&self) -> FfiResult<()> {
        Ok(self.inner.confirm()?)
    }

    /// **Blocks** until the new device is registered. Call after [`confirm`](Self::confirm):
    /// before it, fails at once with `InvalidInput` ("confirm the verification code first").
    /// Once it returns, call [`RustTomiCore::sync`]: it starts the export of the history the new
    /// device asked for, which `wait_done` does not wait for.
    pub fn wait_done(&self) -> FfiResult<FfiInviteOutcome> {
        Ok(to_invite_outcome(self.inner.wait_done()?))
    }

    /// The outcome once the phase is `Done`, null before. Answers at once.
    pub fn outcome(&self) -> Option<FfiInviteOutcome> {
        if self.inner.phase() != InvitePhase::Done {
            return None;
        }
        // `Done` means the outcome is stored: `wait_done` returns it without waiting.
        self.inner.wait_done().ok().map(to_invite_outcome)
    }

    /// The user refused (the codes differ, "Cancel", back) or left the screen. Closes the
    /// channel at the relay and wipes the secrets of the session; nothing is registered unless
    /// the new device already is. **Blocks** on the network (it asks the relay to close the
    /// channel): background thread. Callable from any thread, also while another call of this
    /// session is blocked, which then returns `pairing_cancelled`.
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

/// One joining, from the code to the provisioned device: the new device ("N"). See the module
/// documentation for the threading and the sequence.
///
/// Call [`cancel`](Self::cancel) when the screen closes before the end: dropping the handle
/// closes nothing at the relay. A verified batch that was sealed stays sealed for
/// [`RustTomiCore::resume_pairing`] to settle.
///
/// A session holds the relay of the device while it lives (no other pairing at another relay,
/// no [`RustTomiCore::set_relay`]): it is given back when the session ends, is cancelled, or
/// the last reference to the handle is released (the core then puts the relay back, but the
/// channel stays open at the relay until its deadline). **The app must call `cancel()` and
/// release the handle** when the screen closes: a Kotlin/Swift object that is kept alive by
/// mistake keeps the relay held, and a new pairing at another relay is refused
/// (`InvalidInput`) until it is.
#[derive(uniffi::Object)]
pub struct FfiJoinSession {
    inner: Arc<JoinSession>,
    engine: Arc<Engine>,
}

impl std::fmt::Debug for FfiJoinSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiJoinSession")
            .field("channel_id", &self.inner.channel_id())
            .field("phase", &self.inner.phase())
            .finish_non_exhaustive()
    }
}

#[uniffi::export]
impl FfiJoinSession {
    /// Identifier of the channel (not secret).
    pub fn channel_id(&self) -> String {
        self.inner.channel_id().to_owned()
    }

    /// Authority (`host[:port]`) of the relay of this session, known from the start: show it in
    /// full (also in [`FfiJoinPrompt::relay_authority`]).
    pub fn relay_authority(&self) -> String {
        self.inner.relay_authority().to_owned()
    }

    /// Milliseconds left before the relay drops the channel (never negative): 120 s after the
    /// claim. **Indicative on this (joiner) side**: a device that has never opened a session has
    /// no measured relay-clock offset, so this compares the relay deadline with the local clock;
    /// a few minutes of clock skew make it too long or 0 at once. Use it for a rough countdown,
    /// not a hard cut-off. (A precise value would need the claim to carry the relay time.)
    pub fn expires_in_ms(&self) -> i64 {
        remaining_ms(&self.engine, self.inner.expires_at_ms())
    }

    /// Where the joining is. Answers at once.
    pub fn phase(&self) -> JoinPhaseKind {
        to_join_phase(self.inner.phase())
    }

    /// The verification code, the inviter and the history offer, from the moment the inviter's
    /// side was verified until the batch is verified; null before and after. Answers at once.
    pub fn prompt(&self) -> Option<FfiJoinPrompt> {
        self.inner.prompt().map(to_join_prompt)
    }

    /// One step: deposits what waits, reads the channel (waiting up to `wait_secs` for a frame,
    /// 0 to 25: `0` never waits) and goes as far as it can; once the batch is sealed, waits for
    /// the inviter to register this device and provisions. **Blocks** for the round trip and
    /// the wait. Returns the phase after the step.
    ///
    /// A transient error (`Network`, `Storage`, `Relay` 429 or 5xx) leaves the session as it was:
    /// call again. Any other error ends it (`phase()` is `Cancelled` or `Failed`).
    pub fn poll(&self, wait_secs: u32) -> FfiResult<JoinPhaseKind> {
        Ok(to_join_phase(self.inner.poll(wait_secs)?))
    }

    /// **Blocks** until the inviter's side is verified and the codes can be compared (long polls
    /// of 25 s); returns at once if that is already the case. A wrong code ends here with
    /// `pairing_failed` (the channel is closed on the way: the user asks for a new code on the
    /// other device). Also `pairing_cancelled`, `pairing_expired`, `pairing_unsupported`.
    pub fn wait_for_inviter(&self) -> FfiResult<FfiJoinPrompt> {
        Ok(to_join_prompt(self.inner.wait_for_inviter()?))
    }

    /// The user compared the codes and pressed "Join", with the answer to the history question:
    /// the single yes / no ("get the history of the space?", secrets and files included, no
    /// per-type choice). `import_history` is ignored (sent as "no") when the inviter offered
    /// nothing. Valid once [`prompt`](Self::prompt) is available (`InvalidInput` before, the
    /// session goes on). Follow with [`wait_done`](Self::wait_done). Blocks for the call to the
    /// relay that deposits the confirmation. Fails with `InvalidInput` if another driving call
    /// is running on this session.
    pub fn confirm(&self, import_history: bool) -> FfiResult<()> {
        Ok(self.inner.confirm(import_history)?)
    }

    /// **Blocks** until this device is provisioned: the batch is verified, sealed and
    /// acknowledged, the inviter registers the device, the device is provisioned, its session
    /// opened and its first sync done (the [`CoreListener`] is told). Call after
    /// [`confirm`](Self::confirm): before it, fails at once with `InvalidInput`.
    ///
    /// `pairing_expired` also covers a channel purged before this device read "completed" and
    /// whose inviter did not register it (the batch is erased); `pairing_cancelled` when the
    /// inviter closed the channel. If the app stops meanwhile, [`RustTomiCore::resume_pairing`]
    /// settles it at the next start.
    pub fn wait_done(&self) -> FfiResult<FfiJoinOutcome> {
        Ok(to_join_outcome(self.inner.wait_done()?))
    }

    /// The outcome once the phase is `Done`, null before. Answers at once.
    pub fn outcome(&self) -> Option<FfiJoinOutcome> {
        if self.inner.phase() != JoinPhase::Done {
            return None;
        }
        // `Done` means the outcome is stored: `wait_done` returns it without waiting.
        self.inner.wait_done().ok().map(to_join_outcome)
    }

    /// The user refused ("they are different", "Cancel", back) or left the screen. Closes the
    /// channel at the relay, wipes the secrets of the session and erases the batch if one is
    /// sealed. If the inviter had already registered this device, the user removes it from the
    /// device list. **Blocks** on the network (it asks the relay to close the channel):
    /// background thread. Callable from any thread, also while another call of this session is
    /// blocked, which then returns `pairing_cancelled`.
    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

/// Whether `name` is a valid device name (`api-v1.md` §2.3), using the core's own rule
/// (Unicode scalars, no leading/trailing blank, no control character). Exposed so a client
/// validates a name with the exact rule the pairing machines apply, instead of a Kotlin
/// `String.length` (UTF-16 units) approximation; once the name is valid and the device is not
/// paired, the only remaining `InvalidInput` of `join_space` is an unreadable code or link.
#[uniffi::export]
pub fn is_valid_device_name(name: String) -> bool {
    crate::pairing::is_valid_device_name(name.trim())
}

/// What a pairing link contains (`parse_pair_link`). **Secret**: the code. The generated
/// `toString()` prints it: never log it.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairLinkParts {
    /// `https://<authority>` of the link, normalised; `null` for the fallback `tomi-share://pair#<code>`.
    /// **Not trusted**: whoever shows the QR code chose it. Show it in full and let the user
    /// confirm; pass it to `join_space`.
    pub relay_url: Option<String>,
    /// The code in its canonical form (`4821-crayon-renard`).
    pub code: String,
}

impl std::fmt::Debug for PairLinkParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairLinkParts")
            .field("relay_url", &self.relay_url)
            .field("code", &"<redacted>")
            .finish()
    }
}

/// What a space creation link contains (`parse_setup_link`). **Secret**: the token.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct SetupLinkParts {
    /// `https://<authority>` of the link, normalised. **Not trusted** (see
    /// [`PairLinkParts::relay_url`]).
    pub relay_url: String,
    /// The installation token as written in the link.
    pub token: String,
}

impl std::fmt::Debug for SetupLinkParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupLinkParts")
            .field("relay_url", &self.relay_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Reads a pairing link, the single reading shared by the three platforms (`pairing-v1.md`
/// §4.4): `https://<relay>/l/pair#<code>` for any valid https relay, or the fallback
/// `tomi-share://pair#<code>` (no relay). `null` for anything else (another scheme or path, user
/// information in the host, an invalid port or host, a code that cannot be read). Pure: no
/// network, nothing stored; a link never starts a pairing by itself.
#[uniffi::export]
pub fn parse_pair_link(text: String) -> Option<PairLinkParts> {
    let text = Zeroizing::new(text);
    crate::relay_url::parse_pair_link(&text).map(|p| PairLinkParts {
        relay_url: p.relay_url,
        code: p.code,
    })
}

/// Reads a space creation link: `https://<relay>/create/<token>` or `https://<relay>/create#<token>`
/// (the form a relay shows). `null` for anything else, a token of the wrong shape included.
/// Pure. The bare token (a QR code of the relay administration) is not a link: check it with
/// `is_install_token_shape`.
#[uniffi::export]
pub fn parse_setup_link(text: String) -> Option<SetupLinkParts> {
    let text = Zeroizing::new(text);
    crate::relay_url::parse_setup_link(&text).map(|s| SetupLinkParts {
        relay_url: s.relay_url,
        token: s.token,
    })
}

/// Whether `text` has the shape of an installation token: once dashes and blanks are removed
/// and the text lowercased, 24 to 40 characters of the relay's base32 alphabet (digits and
/// letters without `i`, `l`, `o`, `u`). The relay decides whether it is valid.
#[uniffi::export]
pub fn is_install_token_shape(text: String) -> bool {
    let text = Zeroizing::new(text);
    crate::relay_url::is_install_token_shape(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tests::World;
    use std::sync::atomic::AtomicUsize;

    struct Counter(AtomicUsize);
    impl CoreListener for Counter {
        fn on_changed(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn debug_of_ffi_content_records_never_prints_content() {
        let needles = [
            "SECRET-TITLE",
            "SECRET-TEXT",
            "SECRET-HTML",
            "https://secret.example",
            "SECRET-FILE.pdf",
            "/secret/path",
            "SECRET-MD",
            // The thumbnail bytes of b"SECRET-TEXT" as a `Vec<u8>` Debug would print them.
            "83, 69, 67, 82, 69, 84",
        ];
        let text = TextContent {
            plain: "SECRET-TEXT".into(),
            html: Some("SECRET-HTML".into()),
            markdown: Some("SECRET-MD".into()),
            source_format: TextFormat::Plain,
        };
        let attachment = FfiAttachment {
            slot: 0,
            name: "SECRET-FILE.pdf".into(),
            mime: "application/pdf".into(),
            size: 12,
            thumbnail: Some(b"SECRET-TEXT".to_vec()),
            thumbnail_mime: Some("image/png".into()),
            local_path: Some("/secret/path".into()),
            available: true,
        };
        let content = FfiItemContent::Readable {
            title: Some("SECRET-TITLE".into()),
            text: Some(text.clone()),
            url: Some("https://secret.example".into()),
            attachments: vec![attachment.clone()],
            secret: true,
        };
        let item = FfiItem {
            id: "item-1".into(),
            content: content.clone(),
            source_device_id: "dev-1".into(),
            source_device_name: "Phone".into(),
            created_at_ms: 1,
            expires_at_ms: None,
            read_policy: ReadPolicyKind::Normal,
            read: false,
            outgoing: false,
            deliveries: vec![],
            pinned: false,
            pinned_at_ms: None,
            archived: false,
            archived_at_ms: None,
        };
        let draft_attachment = FfiDraftAttachment {
            path: "/secret/path".into(),
            name: "SECRET-FILE.pdf".into(),
            mime: "application/pdf".into(),
            thumbnail: Some(b"SECRET-TEXT".to_vec()),
            thumbnail_mime: None,
        };
        let draft = FfiDraft {
            id: None,
            title: Some("SECRET-TITLE".into()),
            text: Some(text.clone()),
            url: Some("https://secret.example".into()),
            attachments: vec![draft_attachment.clone()],
            secret: true,
            expires_at_ms: None,
            read_policy: ReadPolicyKind::Normal,
        };
        for dbg in [
            format!("{item:?}"),
            format!("{item:#?}"),
            format!("{content:?}"),
            format!("{attachment:?}"),
            format!("{text:?}"),
            format!("{draft:?}"),
            format!("{draft_attachment:?}"),
        ] {
            for n in needles {
                assert!(!dbg.contains(n), "Debug leaks {n:?}: {dbg}");
            }
        }
        let dbg = format!("{item:?}");
        assert!(
            dbg.contains("item-1") && dbg.contains("secret: true"),
            "{dbg}"
        );
    }

    fn draft(text: &str) -> FfiDraft {
        FfiDraft {
            id: None,
            title: Some("t".into()),
            text: Some(TextContent {
                plain: text.into(),
                html: None,
                markdown: None,
                source_format: TextFormat::Plain,
            }),
            url: None,
            attachments: vec![],
            secret: false,
            expires_at_ms: None,
            read_policy: ReadPolicyKind::Normal,
        }
    }

    #[test]
    fn contract_flow_through_the_binding_layer() {
        let w = World::new(2, 2);
        let a = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        let b = RustTomiCore::with_engine(w.nodes[1].engine.clone());
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        b.set_listener(Some(counter.clone()));
        assert!(a.is_paired());
        assert_eq!(a.space_state(), SpaceStateKind::Active);

        let id = a.enqueue(draft("via uniffi")).unwrap();
        let queued = a.get_item(id.clone()).unwrap().unwrap();
        assert!(queued.outgoing);
        assert!(queued
            .deliveries
            .iter()
            .all(|d| d.state == DeliveryKind::Queued));
        assert_eq!(a.dispatch(id.clone()).unwrap(), DispatchKind::Sent);

        let report = b.sync(None).unwrap();
        assert_eq!(report.new_items, 1);
        assert!(
            counter.0.load(Ordering::SeqCst) > 0,
            "the listener was told"
        );
        let items = b.items().unwrap();
        assert_eq!(items.len(), 1);
        assert!(
            matches!(&items[0].content, FfiItemContent::Readable { text: Some(t), .. } if t.plain == "via uniffi")
        );

        // Pin: immediate on B, then on A through the log; the network never fails the call.
        b.pin(id.clone()).unwrap();
        assert!(b.get_item(id.clone()).unwrap().unwrap().pinned);
        w.relay.set_offline(true);
        b.pin(id.clone()).unwrap();
        b.unpin("00000000-0000-4000-8000-000000000000".into())
            .unwrap();
        w.relay.set_offline(false);
        b.sync(None).unwrap();
        a.sync(None).unwrap();
        let on_a = a.get_item(id.clone()).unwrap().unwrap();
        assert!(on_a.pinned);
        assert!(on_a.pinned_at_ms.is_some());

        // Archive: same guarantees; exclusive with the pin at display; never marks read.
        a.archive(id.clone()).unwrap();
        let archived = a.get_item(id.clone()).unwrap().unwrap();
        assert!(archived.archived && archived.archived_at_ms.is_some());
        assert!(!archived.pinned, "archiving unpins");
        a.archive("00000000-0000-4000-8000-000000000000".into())
            .unwrap();
        a.sync(None).unwrap();
        b.sync(None).unwrap();
        let on_b = b.get_item(id.clone()).unwrap().unwrap();
        assert!(on_b.archived && !on_b.pinned && !on_b.read);
        b.unarchive(id.clone()).unwrap();
        assert!(!b.get_item(id.clone()).unwrap().unwrap().archived);
        b.sync(None).unwrap();
        a.sync(None).unwrap();
        assert!(!a.get_item(id.clone()).unwrap().unwrap().archived);

        assert_eq!(a.devices().unwrap().len(), 2);
        assert!(a.local_device().unwrap().is_this_device);
        a.delete_everywhere(id.clone()).unwrap();
        a.sync(None).unwrap();
        b.sync(None).unwrap();
        assert!(b.items().unwrap().is_empty());
    }

    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let started = std::time::Instant::now();
        while !cond() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "never happened: {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn starting_the_doorbell_again_stops_the_old_loop_and_never_runs_two_at_once() {
        let w = World::new(2, 2);
        let core = RustTomiCore::with_engine(w.nodes[1].engine.clone());
        let c = core.clone();
        let first = std::thread::spawn(move || c.run_doorbell());
        wait_for("the first loop connects", || {
            w.relay.count("open_wake") >= 1
        });

        // "Stop then start again" with no wait: the old loop is told to stop by the new call
        // (which no longer resets its flag), and ends by itself.
        core.stop_doorbell();
        let c = core.clone();
        let second = std::thread::spawn(move || c.run_doorbell());
        wait_for("the first loop returns", || first.is_finished());
        first.join().unwrap();
        assert!(!second.is_finished(), "the new loop goes on");
        let opened = w.relay.count("open_wake");
        wait_for("the new loop connects", || {
            w.relay.count("open_wake") > opened
        });

        // A third call without any stop: the second loop is told to stop, then replaced.
        let c = core.clone();
        let third = std::thread::spawn(move || c.run_doorbell());
        wait_for("the second loop returns", || second.is_finished());
        second.join().unwrap();
        assert!(!third.is_finished());
        let opened = w.relay.count("open_wake");
        wait_for("the third loop connects", || {
            w.relay.count("open_wake") > opened
        });

        // One stop ends the loop started last.
        core.stop_doorbell();
        third.join().unwrap();
        assert!(w.nodes[1].engine.doorbell_gate.try_lock().is_ok());
    }

    #[test]
    fn leaving_through_the_binding_layer() {
        let w = World::new(2, 2);
        let b = RustTomiCore::with_engine(w.nodes[1].engine.clone());
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        b.set_listener(Some(counter.clone()));
        assert!(b.is_paired());
        assert_eq!(b.departure(), None);
        let before = counter.0.load(Ordering::SeqCst);
        assert_eq!(b.leave_space().unwrap(), FfiDepartureOutcome::Erased);
        assert!(!b.is_paired());
        assert!(b.doorbell_stop.is_stopped(), "the doorbell is told to stop");
        assert_eq!(b.space_state(), SpaceStateKind::Active);
        assert!(matches!(b.items(), Err(TomiError::InvalidInput { .. })));
        assert_eq!(
            b.continue_departure().unwrap(),
            FfiDepartureOutcome::NothingPending
        );
        assert!(
            counter.0.load(Ordering::SeqCst) > before,
            "the listener was told"
        );
        // Leaving twice: nothing to leave any more.
        assert!(matches!(
            b.leave_space(),
            Err(TomiError::InvalidInput { .. })
        ));

        // The only device of a space: refused, then "Delete space".
        let w = World::new(1, 1);
        let only = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        assert_eq!(
            only.leave_space().unwrap(),
            FfiDepartureOutcome::Refused {
                reason: FfiRefusalReason::LastDevice
            }
        );
        assert!(only.is_paired());
        assert!(!only.doorbell_stop.is_stopped());
        assert_eq!(only.delete_space().unwrap(), FfiDepartureOutcome::Erased);
        assert!(!only.is_paired());
        assert!(only.doorbell_stop.is_stopped());
    }

    #[test]
    fn a_pending_departure_and_erasing_without_proof_through_the_binding_layer() {
        let w = World::new(2, 2);
        let b = RustTomiCore::with_engine(w.nodes[1].engine.clone());
        w.relay.set_offline(true);
        assert_eq!(
            b.leave_space().unwrap(),
            FfiDepartureOutcome::Pending {
                reason: FfiPendingReason::Network
            }
        );
        let d = b.departure().unwrap();
        assert_eq!(d.kind, FfiDepartureKind::Leave);
        assert!(!d.wiping);
        assert!(d.since_ms.is_some());
        assert_eq!(d.last, Some(FfiPendingReason::Network));
        assert!(!b.can_erase_without_proof());
        assert!(matches!(
            b.erase_without_proof(),
            Err(TomiError::InvalidInput { .. })
        ));
        assert!(b.cancel_departure().unwrap());
        assert_eq!(b.departure(), None);
        assert!(b.is_paired());

        // The relay is empty: "space not found", D4 allowed, nothing erased by itself.
        w.relay.set_offline(false);
        w.relay.uninitialize();
        assert_eq!(
            b.leave_space().unwrap(),
            FfiDepartureOutcome::Pending {
                reason: FfiPendingReason::NotFound
            }
        );
        assert_eq!(b.space_state(), SpaceStateKind::NotFound);
        assert!(b.can_erase_without_proof());
        assert_eq!(
            b.retry_departure().unwrap(),
            FfiDepartureOutcome::Pending {
                reason: FfiPendingReason::NotFound
            }
        );
        assert!(b.is_paired());
        let calls = w.relay.with(|s| s.calls.len());
        b.erase_without_proof().unwrap();
        assert_eq!(w.relay.with(|s| s.calls.len()), calls, "no request");
        assert!(!b.is_paired());
        assert!(b.doorbell_stop.is_stopped());
        assert_eq!(b.space_state(), SpaceStateKind::Active);
        assert!(matches!(
            b.erase_without_proof(),
            Err(TomiError::InvalidInput { .. })
        ));
    }

    #[test]
    fn a_relay_too_old_through_the_binding_layer() {
        let w = World::new(1, 1);
        let only = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        w.relay.fail_next(
            "delete_space",
            crate::runtime::RelayError::api(405, "method_not_allowed"),
        );
        assert_eq!(
            only.delete_space().unwrap(),
            FfiDepartureOutcome::Refused {
                reason: FfiRefusalReason::RelayTooOld
            }
        );
        assert!(only.can_erase_without_proof());
        assert_eq!(only.departure(), None);
    }

    #[test]
    fn archive_is_exposed_on_the_items_and_the_pin_wins() {
        let w = World::new(2, 2);
        let a = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        let id = a.enqueue(draft("a archiver")).unwrap();
        assert_eq!(a.dispatch(id.clone()).unwrap(), DispatchKind::Sent);
        a.archive(id.clone()).unwrap();
        let item = a.get_item(id.clone()).unwrap().unwrap();
        assert!(item.archived && item.archived_at_ms.is_some() && !item.pinned);
        assert_eq!(a.items().unwrap().iter().filter(|i| i.archived).count(), 1);
        a.pin(id.clone()).unwrap();
        let item = a.get_item(id.clone()).unwrap().unwrap();
        assert!(item.pinned && !item.archived && item.archived_at_ms.is_none());
        a.archive(id.clone()).unwrap();
        a.unarchive(id.clone()).unwrap();
        assert!(!a.get_item(id).unwrap().unwrap().archived);
    }

    #[test]
    fn a_real_core_answers_the_demo_and_pairing_calls_without_failing() {
        let w = World::new(1, 1);
        let a = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        assert!(!a.is_demo());
        assert_eq!(
            a.pair_with_code("4821-crayon-renard".into(), None).unwrap(),
            PairingOutcome::Failure {
                reason: PairingFailureKind::Unsupported
            }
        );
        assert_eq!(
            a.pair_with_qr("https://relay.example.org/l/pair#x".into())
                .unwrap(),
            PairingOutcome::Failure {
                reason: PairingFailureKind::Unsupported
            }
        );
        assert!(matches!(
            a.confirm_pairing(true),
            Err(TomiError::Blocked { reason }) if reason == "pairing_unsupported"
        ));
        a.cancel_pairing().unwrap();
        assert!(matches!(
            a.simulate_incoming(DemoIncomingKind::Link),
            Err(TomiError::InvalidInput { .. })
        ));
    }

    #[cfg(not(feature = "demo"))]
    #[test]
    fn a_build_without_the_demo_feature_refuses_open_demo() {
        let dir = tempfile::tempdir().unwrap();
        let r = RustTomiCore::open_demo(DemoConfig {
            files_dir: dir.path().to_string_lossy().into(),
            assets_dir: None,
            platform: "ios".into(),
            app_version: "0.1.0".into(),
            device_name: "iPhone".into(),
            start_paired: true,
        });
        assert!(matches!(r, Err(TomiError::Internal { .. })));
    }

    #[cfg(not(feature = "demo"))]
    #[test]
    fn a_build_without_the_demo_feature_refuses_enter_demo() {
        let w = World::new(1, 1);
        let a = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        assert!(!a.supports_demo_entry());
        let dir = tempfile::tempdir().unwrap();
        let r = a.enter_demo(DemoConfig {
            files_dir: dir.path().to_string_lossy().into(),
            assets_dir: None,
            platform: "ios".into(),
            app_version: "0.1.0".into(),
            device_name: "iPhone".into(),
            start_paired: true,
        });
        assert!(matches!(r, Err(TomiError::Internal { .. })));
        assert!(!a.leave_demo());
    }

    #[cfg(feature = "demo")]
    #[test]
    fn a_core_in_a_space_refuses_the_demo() {
        let w = World::new(1, 1);
        let a = RustTomiCore::with_engine(w.nodes[0].engine.clone());
        assert!(a.supports_demo_entry());
        let dir = tempfile::tempdir().unwrap();
        let r = a.enter_demo(DemoConfig {
            files_dir: dir.path().to_string_lossy().into(),
            assets_dir: None,
            platform: "ios".into(),
            app_version: "0.1.0".into(),
            device_name: "iPhone".into(),
            start_paired: true,
        });
        assert!(matches!(r, Err(TomiError::InvalidInput { .. })));
        assert!(!a.is_demo());
    }

    #[cfg(feature = "demo")]
    #[test]
    fn the_demo_core_goes_through_the_binding_layer() {
        let dir = tempfile::tempdir().unwrap();
        let core = RustTomiCore::open_demo(DemoConfig {
            files_dir: dir.path().to_string_lossy().into(),
            assets_dir: None,
            platform: "ios".into(),
            app_version: "0.1.0".into(),
            device_name: "iPhone".into(),
            start_paired: false,
        })
        .unwrap();
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        core.set_listener(Some(counter.clone()));
        assert!(core.is_demo());
        assert!(!core.is_paired());
        match core.pair_with_code("7-crayon-lune".into(), None).unwrap() {
            PairingOutcome::AwaitingVerification {
                device,
                verification_code,
                history_item_count,
                history_bytes,
                member_count,
                expires_in_ms,
                relay_authority,
            } => {
                assert_eq!(device.name, "PC-Bureau");
                assert_eq!(relay_authority, "relay.example.org");
                assert_eq!(device.platform, "windows");
                assert_eq!(verification_code, "482913");
                assert_eq!(
                    history_item_count as usize,
                    crate::runtime::demo::expected_feed().items
                );
                assert!(history_bytes > 0);
                assert_eq!(member_count, Some(2));
                assert_eq!(expires_in_ms, 120_000);
            }
            other => panic!("unexpected {other:?}"),
        }
        core.confirm_pairing(true).unwrap();
        assert!(core.is_paired());
        assert!(counter.0.load(Ordering::SeqCst) > 0, "the app is told");
        assert_eq!(core.space_state(), SpaceStateKind::Active);
        assert!(core.local_device().unwrap().is_this_device);
        let items = core.items().unwrap();
        let expected = crate::runtime::demo::expected_feed();
        assert_eq!(items.len(), expected.items);
        assert_eq!(
            items.iter().filter(|i| i.archived).count(),
            expected.archived
        );
        assert_eq!(items.iter().filter(|i| i.pinned).count(), expected.pinned);
        assert!(!items.iter().any(|i| i.archived && i.pinned));

        core.simulate_incoming(DemoIncomingKind::Secret).unwrap();
        let items = core.items().unwrap();
        assert_eq!(items.len(), expected.items + 1);
        let secret = &items[0];
        assert!(matches!(
            &secret.content,
            FfiItemContent::Readable { secret: true, .. }
        ));
        assert_eq!(secret.read_policy, ReadPolicyKind::AfterFirstRead);
        // Reading it once consumes it, like on the real product.
        core.mark_read(secret.id.clone()).unwrap();
        assert!(core.get_item(secret.id.clone()).unwrap().is_none());
        assert_eq!(core.items().unwrap().len(), expected.items);
    }

    #[test]
    fn errors_map_to_the_flat_binding_error() {
        assert!(matches!(
            TomiError::from(CoreError::Network { reason: "x".into() }),
            TomiError::Network { .. }
        ));
        assert!(matches!(
            TomiError::from(CoreError::Relay {
                status: 409,
                code: "c".into()
            }),
            TomiError::Relay { status: 409, .. }
        ));
        assert!(
            matches!(TomiError::from(CoreError::DecryptionFailed), TomiError::Crypto { code } if code == "decryption_failed")
        );
        // An unprovisioned core refuses to work instead of panicking.
        let dir = tempfile::tempdir().unwrap();
        let core = RustTomiCore::open(CoreConfig {
            db_path: dir.path().join("a.db").to_string_lossy().into(),
            files_dir: dir.path().join("f").to_string_lossy().into(),
            wrap_key: vec![3; 32],
            relay_url: Some("https://relay.example.org".into()),
            app_version: "0.1.0".into(),
            platform: "android".into(),
            auto_import: true,
        })
        .unwrap();
        assert!(!core.is_paired());
        assert!(matches!(core.items(), Err(TomiError::InvalidInput { .. })));
        // A key that is not 32 bytes is refused.
        let bad = RustTomiCore::open(CoreConfig {
            db_path: dir.path().join("b.db").to_string_lossy().into(),
            files_dir: dir.path().join("g").to_string_lossy().into(),
            wrap_key: vec![3; 5],
            relay_url: Some("https://relay.example.org".into()),
            app_version: "0.1.0".into(),
            platform: "android".into(),
            auto_import: true,
        });
        assert!(bad.is_err());
        // Plain http is refused outside the local debug hosts.
        let insecure = RustTomiCore::open(CoreConfig {
            db_path: dir.path().join("c.db").to_string_lossy().into(),
            files_dir: dir.path().join("h").to_string_lossy().into(),
            wrap_key: vec![3; 32],
            relay_url: Some("http://relay.example.org".into()),
            app_version: "0.1.0".into(),
            platform: "android".into(),
            auto_import: true,
        });
        assert!(insecure.is_err());
    }

    #[test]
    fn a_core_without_a_relay_asks_for_one_and_keeps_the_one_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let config = |relay: Option<&str>| CoreConfig {
            db_path: dir.path().join("a.db").to_string_lossy().into(),
            files_dir: dir.path().join("f").to_string_lossy().into(),
            wrap_key: vec![3; 32],
            relay_url: relay.map(str::to_owned),
            app_version: "0.1.0".into(),
            platform: "android".into(),
            auto_import: true,
        };
        // A release build: no relay. The core opens, answers a clear error, never panics.
        let core = RustTomiCore::open(config(None)).unwrap();
        assert_eq!(core.relay_url(), None);
        for e in [
            core.join_space("4821-crayon-renard".into(), "Pixel".into(), None)
                .err()
                .unwrap(),
            core.create_space(
                "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns".into(),
                "PC".into(),
                None,
            )
            .unwrap_err(),
        ] {
            assert!(
                matches!(&e, TomiError::Crypto { code } if code == "relay_not_configured"),
                "{e:?}"
            );
        }
        // Invalid relays are refused, a valid one is kept across two openings.
        for bad in [
            "http://relay.example.org",
            "https://u@relay.example.org",
            "",
        ] {
            assert!(matches!(
                core.set_relay(bad.into()),
                Err(TomiError::InvalidInput { .. })
            ));
        }
        assert_eq!(core.relay_url(), None);
        core.set_relay("https://relay.example.org".into()).unwrap();
        drop(core);
        let core = RustTomiCore::open(config(None)).unwrap();
        assert_eq!(
            core.relay_url().as_deref(),
            Some("https://relay.example.org")
        );
    }

    #[test]
    fn the_links_are_read_by_the_shared_functions() {
        let p =
            parse_pair_link("https://relay.example.org/l/pair#4821-crayon-renard".into()).unwrap();
        assert_eq!(p.relay_url.as_deref(), Some("https://relay.example.org"));
        assert_eq!(p.code, "4821-crayon-renard");
        assert!(!format!("{p:?}").contains("crayon"));
        let p = parse_pair_link("tomi-share://pair#4821-crayon-renard".into()).unwrap();
        assert_eq!(p.relay_url, None);
        assert!(
            parse_pair_link("http://relay.example.org/l/pair#4821-crayon-renard".into()).is_none()
        );
        let token = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";
        let s = parse_setup_link(format!("https://relay.example.org/create#{token}")).unwrap();
        assert_eq!(
            (s.relay_url.as_str(), s.token.as_str()),
            ("https://relay.example.org", token)
        );
        assert!(!format!("{s:?}").contains("h7k2"));
        assert!(parse_setup_link(format!("https://relay.example.org/create/{token}")).is_some());
        assert!(parse_setup_link("https://relay.example.org/create#abc".into()).is_none());
        assert!(is_install_token_shape(token.into()));
        assert!(!is_install_token_shape("4821-crayon-renard".into()));
    }

    // -----------------------------------------------------------------------
    // Pairing through the binding layer
    // -----------------------------------------------------------------------
    //
    // Both devices are the real core on the in-memory relay, driven only through the exported
    // API. They are interleaved on one thread with `poll(0)`: every test is deterministic, and a
    // protocol that gets stuck panics instead of hanging. The blocking waits are run on threads
    // in `blocking_waits_on_two_threads` and `a_cancel_from_another_thread_ends_a_blocked_wait`.

    mod pairing_flows {
        use super::*;
        use crate::runtime::mock::{MockRelay, TestClock};
        use std::sync::mpsc;
        use std::time::Duration;

        const T0: i64 = 1_793_836_800_000;
        const TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";
        const WRAP: [u8; 32] = [9u8; 32];

        #[cfg(feature = "demo")]
        #[test]
        fn a_real_core_enters_the_demo_and_comes_back() {
            let lab = Lab::new();
            let phone = lab.phone("ios");
            let core = phone.core.clone();
            let counter = Arc::new(Counter(AtomicUsize::new(0)));
            core.set_listener(Some(counter.clone()));
            assert!(core.supports_demo_entry());
            assert!(!core.is_demo() && !core.is_paired());

            let demo_dir = tempfile::tempdir().unwrap();
            let config = || DemoConfig {
                files_dir: demo_dir.path().to_string_lossy().into(),
                assets_dir: None,
                platform: "ios".into(),
                app_version: "0.1.0".into(),
                device_name: "iPhone".into(),
                start_paired: false,
            };
            core.enter_demo(config()).unwrap();
            assert!(core.is_demo() && core.is_paired());
            assert!(counter.0.load(Ordering::SeqCst) > 0, "the app is told");
            assert_eq!(
                core.items().unwrap().len(),
                crate::runtime::demo::expected_feed().items
            );
            assert_eq!(core.devices().unwrap().len(), 3);
            // The simulated pairing and the demo token are out of reach: already in the space.
            assert!(core.pair_with_code("7-crayon-lune".into(), None).is_err());
            assert!(core
                .create_space(
                    "2345-6789-abcd-efgh-jkmn-pqrs-tvwx-yz23".into(),
                    "iPhone".into(),
                    None
                )
                .is_err());

            assert!(core.leave_demo());
            assert!(!core.is_demo() && !core.is_paired());
            assert!(core.items().map(|i| i.is_empty()).unwrap_or(true));
            assert!(!core.leave_demo());

            // "Se déconnecter" inside the demo leaves it too.
            core.enter_demo(config()).unwrap();
            assert_eq!(core.leave_space().unwrap(), FfiDepartureOutcome::Erased);
            assert!(!core.is_demo() && !core.is_paired());
        }

        /// One relay that was just installed, one clock, and the devices that talk to them.
        struct Lab {
            relay: MockRelay,
            clock: Arc<TestClock>,
        }

        /// A device: its core, and the files it lives on (to stop it and start it again).
        struct Phone {
            core: Arc<RustTomiCore>,
            dir: tempfile::TempDir,
            cfg: EngineConfig,
        }

        impl Lab {
            fn new() -> Self {
                let clock = TestClock::new(T0);
                let relay = MockRelay::uninitialized(clock.clone(), TOKEN);
                Self { relay, clock }
            }

            /// A device that is not provisioned.
            fn phone(&self, platform: &str) -> Phone {
                self.phone_with(EngineConfig {
                    platform: platform.into(),
                    ..EngineConfig::default()
                })
            }

            fn phone_with(&self, cfg: EngineConfig) -> Phone {
                let dir = tempfile::tempdir().unwrap();
                let core = self.open(&dir, &cfg);
                Phone { core, dir, cfg }
            }

            fn open(&self, dir: &tempfile::TempDir, cfg: &EngineConfig) -> Arc<RustTomiCore> {
                let store = Store::open(
                    &dir.path().join("tomi.db"),
                    &dir.path().join("files"),
                    &WRAP,
                )
                .unwrap();
                let engine = Engine::open(
                    cfg.clone(),
                    store,
                    self.relay.transport(),
                    self.clock.clone(),
                )
                .unwrap();
                RustTomiCore::with_engine(Arc::new(engine))
            }

            /// The first device of a space.
            fn founder(&self, name: &str) -> Phone {
                let phone = self.phone("windows");
                phone
                    .core
                    .create_space(TOKEN.into(), name.into(), None)
                    .unwrap();
                phone
            }
        }

        impl Phone {
            /// The app stops and starts again on the same files.
            fn restart(self, lab: &Lab) -> Phone {
                let Phone { core, dir, cfg } = self;
                drop(core);
                let core = lab.open(&dir, &cfg);
                Phone { core, dir, cfg }
            }
        }

        /// A guard that makes a hung test fail: after `secs` of real time the relay goes
        /// offline, so every blocked call ends with a network error.
        struct Watchdog {
            _stop: mpsc::Sender<()>,
        }

        impl Watchdog {
            fn start(relay: &MockRelay, secs: u64) -> Self {
                let (tx, rx) = mpsc::channel::<()>();
                let relay = relay.clone();
                std::thread::spawn(move || {
                    let timeout = rx.recv_timeout(Duration::from_secs(secs));
                    if matches!(timeout, Err(mpsc::RecvTimeoutError::Timeout)) {
                        relay.set_offline(true);
                    }
                });
                Self { _stop: tx }
            }
        }

        fn both_verify(a: InvitePhaseKind, n: JoinPhaseKind) -> bool {
            a == InvitePhaseKind::Verify && n == JoinPhaseKind::Verify
        }

        fn both_done(a: InvitePhaseKind, n: JoinPhaseKind) -> bool {
            a == InvitePhaseKind::Done && n == JoinPhaseKind::Done
        }

        /// Polls both sessions, alternately, until `until` holds. Panics if it does not within
        /// a bound.
        fn drive(
            invite: &FfiInviteSession,
            join: &FfiJoinSession,
            until: impl Fn(InvitePhaseKind, JoinPhaseKind) -> bool,
        ) {
            for _ in 0..60 {
                let a = invite.poll(0).unwrap();
                let n = join.poll(0).unwrap();
                if until(a, n) {
                    return;
                }
            }
            panic!(
                "the pairing did not get there: inviter {:?}, new device {:?}",
                invite.phase(),
                join.phase()
            );
        }

        /// Drives both devices to the point where the new device acknowledged the batch and the
        /// inviter has not yet read the acknowledgement.
        fn up_to_the_acknowledgement(invite: &FfiInviteSession, join: &FfiJoinSession) {
            drive(invite, join, both_verify);
            invite.confirm().unwrap();
            join.confirm(false).unwrap();
            invite.poll(0).unwrap();
            assert_eq!(join.poll(0).unwrap(), JoinPhaseKind::Completing);
        }

        /// The stable `pairing_*` code of an error.
        fn pairing_code(e: &TomiError) -> &str {
            match e {
                TomiError::Crypto { code } => code,
                other => panic!("not a pairing code: {other:?}"),
            }
        }

        /// `code` with its second word replaced by the next word of the list: a well-formed code
        /// that is not the one shown.
        fn with_wrong_word(code: &str) -> String {
            let mut parts: Vec<String> = code.split('-').map(str::to_owned).collect();
            let rank = crate::pairing::rank_of(&parts[2]).unwrap();
            parts[2] = crate::pairing::word((rank + 1) % 2048).unwrap().to_owned();
            parts.join("-")
        }

        #[test]
        fn pairing_a_new_device_through_the_binding_layer() {
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");
            assert!(a.core.is_paired());
            assert!(!a.core.has_pending_pairing().unwrap());
            let n = lab.phone("android");
            assert!(!n.core.is_paired());
            assert_eq!(n.core.resume_pairing().unwrap(), ResumeKind::Nothing);
            assert!(n.core.awaited_history().unwrap().is_none());
            let heard = Arc::new(Counter(AtomicUsize::new(0)));
            n.core.set_listener(Some(heard.clone()));

            // A shows a code and the link of its QR code, valid for two minutes.
            let invite = a.core.start_invitation().unwrap();
            assert_eq!(invite.phase(), InvitePhaseKind::AwaitPeer);
            let (code, link) = (invite.code().unwrap(), invite.link().unwrap());
            assert_eq!(link, format!("https://relay.example.org/l/pair#{code}"));
            assert_eq!(invite.expires_in_ms(), 120_000);
            lab.clock.advance(30_000);
            assert_eq!(invite.expires_in_ms(), 90_000);
            assert!(invite.prompt().is_none() && invite.outcome().is_none());

            // N scans the QR code.
            let join = n
                .core
                .join_space(link.clone(), "Pixel".into(), None)
                .unwrap();
            assert_eq!(join.channel_id(), invite.channel_id());
            assert_eq!(join.expires_in_ms(), 120_000, "two minutes after the claim");
            assert!(join.prompt().is_none() && join.outcome().is_none());

            drive(&invite, &join, both_verify);
            let (for_a, for_n) = (invite.prompt().unwrap(), join.prompt().unwrap());
            assert_eq!(for_a.sas, for_n.sas, "both screens show the same code");
            assert_eq!(for_a.sas.len(), 7, "six digits in two groups of three");
            assert_eq!(
                for_a.peer,
                FfiPairingPeer {
                    name: "Pixel".into(),
                    platform: "android".into()
                }
            );
            assert_eq!(
                for_n.inviter,
                FfiPairingPeer {
                    name: "PC-Bureau".into(),
                    platform: "windows".into()
                }
            );
            assert_eq!(for_n.member_count, Some(1));
            assert_eq!(for_n.history, None, "nothing to offer, no question");

            // The secrets are for the screen, not for the logs.
            let printed = format!("{for_a:?} {for_n:?} {invite:?} {join:?}");
            for secret in [&for_a.sas, &for_a.sas.replace(' ', ""), &code, &link] {
                assert!(!printed.contains(secret.as_str()), "{secret} printed");
            }

            invite.confirm().unwrap();
            join.confirm(false).unwrap();
            drive(&invite, &join, both_done);

            // The outcomes agree, and the secrets of the sessions are gone.
            let (invited, joined) = (invite.outcome().unwrap(), join.outcome().unwrap());
            assert_eq!(invited, invite.wait_done().unwrap());
            assert_eq!(joined, join.wait_done().unwrap());
            assert_eq!(invited.device_id, joined.device_id);
            assert_eq!(joined.device_id, n.core.local_device().unwrap().id);
            assert_eq!(joined.space_id, a.core.engine().space_id().unwrap());
            assert_eq!(
                (invited.name.as_str(), invited.platform.as_str()),
                ("Pixel", "android")
            );
            assert!(invited.history.is_none() && joined.history.is_none());
            assert!(invite.code().is_none() && invite.link().is_none());
            assert!(invite.prompt().is_none() && join.prompt().is_none());

            // N belongs to the space, its listener was told, nothing is left pending.
            assert!(n.core.is_paired());
            assert!(
                heard.0.load(Ordering::SeqCst) > 0,
                "the listener was told of the provisioning"
            );
            assert!(!n.core.has_pending_pairing().unwrap());
            assert_eq!(n.core.space_state(), SpaceStateKind::Active);
            assert_eq!(a.core.devices().unwrap().len(), 2);
            assert_eq!(n.core.devices().unwrap().len(), 2);
            assert!(n.core.local_device().unwrap().is_this_device);

            // An item of A reaches N, under the same space key.
            let id = a.core.enqueue(draft("de A")).unwrap();
            assert_eq!(a.core.dispatch(id).unwrap(), DispatchKind::Sent);
            n.core.sync(None).unwrap();
            let items = n.core.items().unwrap();
            assert_eq!(items.len(), 1);
            assert!(
                matches!(&items[0].content, FfiItemContent::Readable { text: Some(t), .. } if t.plain == "de A")
            );

            // A third device joins with the typed code: capitals are welcome.
            let c = lab.phone("windows");
            let invite = a.core.start_invitation().unwrap();
            let typed = invite.code().unwrap().to_uppercase();
            let join = c.core.join_space(typed, "Portable".into(), None).unwrap();
            drive(&invite, &join, both_verify);
            assert_eq!(join.prompt().unwrap().member_count, Some(2));
            invite.confirm().unwrap();
            join.confirm(false).unwrap();
            drive(&invite, &join, both_done);
            assert!(c.core.is_paired());
            assert_eq!(a.core.devices().unwrap().len(), 3);
        }

        #[test]
        fn the_space_key_waits_for_both_people_through_the_binding_layer() {
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");

            // 1. The user of the new device says yes, the user of the inviter has not.
            let n = lab.phone("android");
            let invite = a.core.start_invitation().unwrap();
            let channel = invite.channel_id();
            let join = n
                .core
                .join_space(invite.code().unwrap(), "Pixel".into(), None)
                .unwrap();
            // Nothing to confirm before the verification code exists: refused, and the sessions
            // go on. No wait for a confirmation nobody gave either.
            assert!(matches!(
                invite.confirm(),
                Err(TomiError::InvalidInput { .. })
            ));
            assert!(matches!(
                join.confirm(true),
                Err(TomiError::InvalidInput { .. })
            ));
            assert!(matches!(
                invite.wait_done(),
                Err(TomiError::InvalidInput { .. })
            ));
            assert!(matches!(
                join.wait_done(),
                Err(TomiError::InvalidInput { .. })
            ));
            drive(&invite, &join, both_verify);
            join.confirm(false).unwrap();
            for _ in 0..6 {
                invite.poll(0).unwrap();
                join.poll(0).unwrap();
            }
            assert_eq!(
                invite.phase(),
                InvitePhaseKind::Verify,
                "its user has not said yes"
            );
            assert_eq!(
                lab.relay.channel_messages(&channel),
                Some((2, 3)),
                "commit and reveal only: no batch before the user of the inviter confirmed"
            );
            assert!(matches!(
                invite.wait_done(),
                Err(TomiError::InvalidInput { .. })
            ));
            assert!(!n.core.is_paired());
            invite.confirm().unwrap();
            drive(&invite, &join, both_done);
            assert_eq!(lab.relay.channel_messages(&channel), Some((3, 4)));
            assert!(n.core.is_paired());

            // 2. The user of the inviter says yes, the user of the new device has not.
            let m = lab.phone("android");
            let invite = a.core.start_invitation().unwrap();
            let channel = invite.channel_id();
            let join = m
                .core
                .join_space(invite.code().unwrap(), "Tablette".into(), None)
                .unwrap();
            drive(&invite, &join, both_verify);
            invite.confirm().unwrap();
            assert_eq!(invite.phase(), InvitePhaseKind::Provisioning);
            for _ in 0..6 {
                invite.poll(0).unwrap();
                join.poll(0).unwrap();
            }
            assert_eq!(
                join.phase(),
                JoinPhaseKind::Verify,
                "its user has not said yes"
            );
            assert_eq!(
                lab.relay.channel_messages(&channel),
                Some((2, 2)),
                "commit and reveal only: no batch before the user of the new device confirmed"
            );
            assert!(!m.core.is_paired());
            // The user of the inviter finds the other device is not the expected one.
            invite.cancel();
            let ended = join.poll(0).unwrap_err();
            assert_eq!(pairing_code(&ended), "pairing_cancelled");
            assert!(!m.core.is_paired() && !m.core.has_pending_pairing().unwrap());
            assert_eq!(
                a.core.devices().unwrap().len(),
                2,
                "only the first one joined"
            );
        }

        #[test]
        fn blocking_waits_on_two_threads_through_the_binding_layer() {
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");
            let n = lab.phone("android");
            let _dog = Watchdog::start(&lab.relay, 60);
            let invite = a.core.start_invitation().unwrap();
            let code = invite.code().unwrap();
            let (sas_a, seen_a) = mpsc::channel::<String>();
            let (sas_n, seen_n) = mpsc::channel::<String>();
            let (press_a, wait_a) = mpsc::channel::<()>();
            let (press_n, wait_n) = mpsc::channel::<()>();

            let inviter = {
                let invite = invite.clone();
                std::thread::spawn(move || {
                    let prompt = invite.wait_for_peer().unwrap();
                    assert_eq!(prompt.peer.name, "Pixel");
                    sas_a.send(prompt.sas).unwrap();
                    wait_a.recv().unwrap();
                    invite.confirm().unwrap();
                    invite.wait_done().unwrap()
                })
            };
            let joiner = {
                let core = n.core.clone();
                std::thread::spawn(move || {
                    let join = core.join_space(code, "Pixel".into(), None).unwrap();
                    let prompt = join.wait_for_inviter().unwrap();
                    assert_eq!(prompt.inviter.name, "PC-Bureau");
                    sas_n.send(prompt.sas).unwrap();
                    wait_n.recv().unwrap();
                    join.confirm(false).unwrap();
                    join.wait_done().unwrap()
                })
            };
            // The two users compare the codes on their screens, then press the button.
            assert_eq!(seen_a.recv().unwrap(), seen_n.recv().unwrap());
            press_a.send(()).unwrap();
            press_n.send(()).unwrap();
            let invited = inviter.join().unwrap();
            let joined = joiner.join().unwrap();
            assert_eq!(invited.device_id, joined.device_id);
            assert!(n.core.is_paired());
        }

        #[test]
        fn a_cancel_from_another_thread_ends_a_blocked_wait() {
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");
            let n = lab.phone("android");
            let _dog = Watchdog::start(&lab.relay, 60);

            // The inviter waits for a device that never comes; the user leaves the screen.
            let invite = a.core.start_invitation().unwrap();
            let waiting = {
                let invite = invite.clone();
                std::thread::spawn(move || invite.wait_for_peer())
            };
            std::thread::sleep(Duration::from_millis(100));
            invite.cancel();
            let ended = waiting.join().unwrap().unwrap_err();
            assert_eq!(pairing_code(&ended), "pairing_cancelled");
            assert_eq!(invite.phase(), InvitePhaseKind::Cancelled);
            assert!(invite.code().is_none() && invite.link().is_none());
            assert_eq!(
                pairing_code(&invite.wait_for_peer().unwrap_err()),
                "pairing_cancelled",
                "a cancelled session stays cancelled"
            );

            // The new device waits for an inviter that never answers (it never polls).
            let invite = a.core.start_invitation().unwrap();
            let join = n
                .core
                .join_space(invite.code().unwrap(), "Pixel".into(), None)
                .unwrap();
            let waiting = {
                let join = join.clone();
                std::thread::spawn(move || join.wait_for_inviter())
            };
            std::thread::sleep(Duration::from_millis(100));
            join.cancel();
            let ended = waiting.join().unwrap().unwrap_err();
            assert_eq!(pairing_code(&ended), "pairing_cancelled");
            assert_eq!(join.phase(), JoinPhaseKind::Cancelled);
            assert!(join.prompt().is_none());
            // The channel was closed at the relay: the inviter sees it.
            let seen = invite.poll(0).unwrap_err();
            assert_eq!(pairing_code(&seen), "pairing_cancelled");
            assert!(!n.core.is_paired() && !n.core.has_pending_pairing().unwrap());
        }

        #[test]
        fn pairing_errors_keep_their_stable_codes() {
            // The mapping of the core errors the pairing returns.
            for (error, code) in [
                (CoreError::PairingFailed, "pairing_failed"),
                (CoreError::PairingExpired, "pairing_expired"),
                (CoreError::PairingTaken, "pairing_taken"),
                (CoreError::PairingCancelled, "pairing_cancelled"),
                (CoreError::PairingUnsupported, "pairing_unsupported"),
            ] {
                let mapped = TomiError::from(error);
                assert_eq!(pairing_code(&mapped), code);
            }
            assert!(matches!(
                TomiError::from(CoreError::Relay {
                    status: 409,
                    code: "too_many_devices".into()
                }),
                TomiError::Relay { status: 409, .. }
            ));

            // And the cases through the exported API.
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");
            let n = lab.phone("android");

            // Not provisioned: no invitation. Provisioned: no joining.
            assert!(matches!(
                n.core.start_invitation(),
                Err(TomiError::InvalidInput { .. })
            ));
            let invite = a.core.start_invitation().unwrap();
            let code = invite.code().unwrap();
            assert!(matches!(
                a.core.join_space(code.clone(), "Encore".into(), None),
                Err(TomiError::InvalidInput { .. })
            ));

            // A code or a link that cannot be read, or a name that is not one, costs nothing.
            for bad in [
                "",
                "n'importe quoi",
                "4821-crayon",
                "http://relay.example.org/l/pair#4821-crayon-renard",
            ] {
                assert!(
                    matches!(
                        n.core.join_space(bad.into(), "Pixel".into(), None),
                        Err(TomiError::InvalidInput { .. })
                    ),
                    "{bad:?}"
                );
            }
            assert!(matches!(
                n.core.join_space(code.clone(), "".into(), None),
                Err(TomiError::InvalidInput { .. })
            ));

            // Somebody else already used the code.
            let first = n
                .core
                .join_space(code.clone(), "Pixel".into(), None)
                .unwrap();
            let other = lab.phone("android");
            let taken = other
                .core
                .join_space(code.clone(), "Autre".into(), None)
                .unwrap_err();
            assert_eq!(pairing_code(&taken), "pairing_taken");
            first.cancel();
            invite.cancel();

            // A wrong word: the new device cannot open the first sealed frame, and says nothing
            // more precise; it closed the channel, which the inviter sees.
            let invite = a.core.start_invitation().unwrap();
            let join = n
                .core
                .join_space(
                    with_wrong_word(&invite.code().unwrap()),
                    "Pixel".into(),
                    None,
                )
                .unwrap();
            let mut failure = None;
            for _ in 0..10 {
                invite.poll(0).unwrap();
                if let Err(e) = join.poll(0) {
                    failure = Some(e);
                    break;
                }
            }
            assert_eq!(pairing_code(&failure.unwrap()), "pairing_failed");
            assert_eq!(join.phase(), JoinPhaseKind::Failed);
            let closed = invite.poll(0).unwrap_err();
            assert_eq!(pairing_code(&closed), "pairing_cancelled");
            assert_eq!(invite.phase(), InvitePhaseKind::Cancelled);
            assert!(!n.core.is_paired() && !n.core.has_pending_pairing().unwrap());

            // A code nobody used runs out.
            let invite = a.core.start_invitation().unwrap();
            let code = invite.code().unwrap();
            lab.clock.advance(121_000);
            let ran_out = invite.poll(0).unwrap_err();
            assert_eq!(pairing_code(&ran_out), "pairing_expired");
            assert_eq!(invite.phase(), InvitePhaseKind::Failed);
            assert_eq!(invite.expires_in_ms(), 0);
            let late = n.core.join_space(code, "Pixel".into(), None).unwrap_err();
            assert_eq!(pairing_code(&late), "pairing_expired");
        }

        #[test]
        fn a_stop_before_the_end_is_settled_by_resume_pairing_through_the_binding_layer() {
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");

            // The inviter registered the device; the new device never read "completed": the
            // app is killed.
            let n = lab.phone("android");
            let invite = a.core.start_invitation().unwrap();
            let join = n
                .core
                .join_space(invite.code().unwrap(), "Pixel".into(), None)
                .unwrap();
            up_to_the_acknowledgement(&invite, &join);
            assert_eq!(invite.poll(0).unwrap(), InvitePhaseKind::Done);
            drop(join);
            let n = n.restart(&lab);
            assert!(!n.core.is_paired());
            assert!(n.core.has_pending_pairing().unwrap());
            assert_eq!(n.core.resume_pairing().unwrap(), ResumeKind::Provisioned);
            assert!(n.core.is_paired());
            assert!(!n.core.has_pending_pairing().unwrap());
            assert_eq!(n.core.resume_pairing().unwrap(), ResumeKind::Nothing);

            // The inviter gave up; the app of the new device was killed before it learned it.
            let m = lab.phone("android");
            let invite = a.core.start_invitation().unwrap();
            let join = m
                .core
                .join_space(invite.code().unwrap(), "Tablette".into(), None)
                .unwrap();
            up_to_the_acknowledgement(&invite, &join);
            invite.cancel();
            drop(join);
            let m = m.restart(&lab);
            assert!(m.core.has_pending_pairing().unwrap());
            // Offline, nothing is decided and the batch stays.
            lab.relay.set_offline(true);
            assert!(matches!(
                m.core.resume_pairing(),
                Err(TomiError::Network { .. })
            ));
            assert!(m.core.has_pending_pairing().unwrap());
            lab.relay.set_offline(false);
            // The relay does not know the device: the batch is erased.
            assert_eq!(m.core.resume_pairing().unwrap(), ResumeKind::Discarded);
            assert!(!m.core.has_pending_pairing().unwrap());
            assert!(!m.core.is_paired());

            // No stop this time: the handle owns the sealed batch while it is alive, and a
            // resumption leaves it alone; released, the batch is settled with the relay.
            let p = lab.phone("android");
            let invite = a.core.start_invitation().unwrap();
            let join = p
                .core
                .join_space(invite.code().unwrap(), "Portable".into(), None)
                .unwrap();
            up_to_the_acknowledgement(&invite, &join);
            assert_eq!(invite.poll(0).unwrap(), InvitePhaseKind::Done);
            assert_eq!(
                p.core.resume_pairing().unwrap(),
                ResumeKind::Nothing,
                "a live handle owns the batch"
            );
            assert!(p.core.has_pending_pairing().unwrap());
            drop(join);
            assert_eq!(p.core.resume_pairing().unwrap(), ResumeKind::Provisioned);
            assert!(p.core.is_paired());
        }

        #[test]
        fn creating_the_space_through_the_binding_layer() {
            let lab = Lab::new();
            let d = lab.phone("windows");
            // A wrong token provisions nothing; the keys wait for another try.
            let refused = d
                .core
                .create_space("aaaa-bbbb-cccc-dddd".into(), "PC-Bureau".into(), None)
                .unwrap_err();
            assert!(
                matches!(&refused, TomiError::Relay { status: 403, code } if code == "install_token_invalid"),
                "{refused:?}"
            );
            assert!(!d.core.is_paired());
            assert!(d.core.has_pending_pairing().unwrap());
            // A blank token or a bad name never reaches the relay.
            assert!(matches!(
                d.core.create_space("   ".into(), "PC-Bureau".into(), None),
                Err(TomiError::InvalidInput { .. })
            ));
            assert!(matches!(
                d.core.create_space(TOKEN.into(), "".into(), None),
                Err(TomiError::InvalidInput { .. })
            ));

            d.core
                .create_space(TOKEN.into(), "  PC-Bureau ".into(), None)
                .unwrap();
            assert!(d.core.is_paired());
            assert!(!d.core.has_pending_pairing().unwrap());
            assert_eq!(d.core.local_device().unwrap().name, "PC-Bureau");
            assert_eq!(d.core.space_state(), SpaceStateKind::Active);
            // No second space, from this device or from another one.
            assert!(matches!(
                d.core.create_space(TOKEN.into(), "Encore".into(), None),
                Err(TomiError::InvalidInput { .. })
            ));
            let other = lab.phone("android");
            let exists = other
                .core
                .create_space(TOKEN.into(), "Autre".into(), None)
                .unwrap_err();
            assert!(
                matches!(&exists, TomiError::Relay { status: 409, code } if code == "space_exists"),
                "{exists:?}"
            );
            assert!(!other.core.is_paired() && !other.core.has_pending_pairing().unwrap());
            // The founder invites, which is what the other device then does.
            assert!(d.core.start_invitation().is_ok());
        }

        #[test]
        fn the_history_choice_is_one_yes_or_no_through_the_binding_layer() {
            let lab = Lab::new();
            let a = lab.founder("PC-Bureau");
            // A has a history: a text and a secret.
            let first = a.core.enqueue(draft("premier")).unwrap();
            assert_eq!(a.core.dispatch(first).unwrap(), DispatchKind::Sent);
            let mut secret = draft("mot de passe");
            secret.secret = true;
            let second = a.core.enqueue(secret).unwrap();
            assert_eq!(a.core.dispatch(second).unwrap(), DispatchKind::Sent);
            a.core.sync(None).unwrap();

            // Yes: the question is asked once, with what is offered. The new device would wait
            // for the user before importing anything, but the answer was given at pairing.
            let n = lab.phone_with(EngineConfig {
                platform: "android".into(),
                auto_import: false,
                ..EngineConfig::default()
            });
            let invite = a.core.start_invitation().unwrap();
            let join = n
                .core
                .join_space(invite.code().unwrap(), "Pixel".into(), None)
                .unwrap();
            drive(&invite, &join, both_verify);
            let offer = join.prompt().unwrap().history.unwrap();
            assert_eq!(offer.items, 2);
            assert!(offer.bytes > 0);
            invite.confirm().unwrap();
            join.confirm(true).unwrap();
            drive(&invite, &join, both_done);
            let (invited, joined) = (invite.wait_done().unwrap(), join.wait_done().unwrap());
            let announced = joined.history.clone().unwrap();
            assert_eq!(
                invited.history, joined.history,
                "both sides hold the same announcement"
            );
            assert_eq!(
                (announced.items, announced.bytes),
                (offer.items, offer.bytes)
            );
            let awaited = n.core.awaited_history().unwrap().unwrap();
            assert_eq!(awaited.transfer_id, announced.transfer_id);
            assert_eq!(awaited.from_device_id, a.core.local_device().unwrap().id);
            assert_eq!(
                (awaited.items, awaited.bytes),
                (announced.items, announced.bytes)
            );
            // The next sync of A runs the export; the next sync of N imports it.
            a.core.sync(None).unwrap();
            n.core.sync(None).unwrap();
            let imports = n.core.transfers().unwrap();
            assert_eq!(imports.len(), 1);
            assert_eq!(imports[0].transfer_id, announced.transfer_id);
            assert_eq!(imports[0].state, "done", "{imports:?}");
            assert!(n.core.awaited_history().unwrap().is_none());
            assert_eq!(n.core.items().unwrap().len(), 2);

            // No: an empty feed, no announcement, no second export.
            let m = lab.phone("android");
            let invite = a.core.start_invitation().unwrap();
            let join = m
                .core
                .join_space(invite.code().unwrap(), "Tablette".into(), None)
                .unwrap();
            drive(&invite, &join, both_verify);
            assert!(join.prompt().unwrap().history.is_some());
            invite.confirm().unwrap();
            join.confirm(false).unwrap();
            drive(&invite, &join, both_done);
            assert!(invite.wait_done().unwrap().history.is_none());
            assert!(join.wait_done().unwrap().history.is_none());
            assert!(m.core.awaited_history().unwrap().is_none());
            a.core.sync(None).unwrap();
            m.core.sync(None).unwrap();
            assert_eq!(
                a.core.transfers().unwrap().len(),
                1,
                "only the first export"
            );
            assert!(m.core.transfers().unwrap().is_empty());
            assert!(m.core.items().unwrap().is_empty());
        }

        #[test]
        fn the_binding_layer_has_no_logger() {
            // Nothing prints, and nothing logs: a code, a verification code or a token in a
            // trace would have to be added on purpose. (The names are split so that this test
            // does not find itself.)
            let src = include_str!("ffi.rs");
            // Split on a marker that does not depend on line endings (the file is checked out
            // with CRLF on Windows). The only `#[cfg(test)]` is this module, below.
            let production = &src[..src.find("#[cfg(test)]").unwrap()];
            for forbidden in [
                ["print", "ln!"],
                ["eprint", "ln!"],
                ["print", "!("],
                ["eprint", "!("],
                ["d", "bg!"],
                ["tracing", "::"],
                ["lo", "g::"],
            ] {
                let forbidden = forbidden.concat();
                assert!(
                    !production.contains(&forbidden),
                    "{forbidden} in the binding layer"
                );
            }
        }
    }
}
