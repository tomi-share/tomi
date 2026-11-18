//! The seam between this app and the Tomi core.
//!
//! [`crate::real_core::RealCore`] implements [`TomiCore`] on top of the `runtime` of the
//! `tomi-core` crate (SQLite store, sync engine, outbox, history transfer);
//! [`crate::fake_core::FakeCore`] is the in-memory stand-in of the demo mode (debug builds
//! only). This module must stay free of any crypto or protocol detail: everything
//! sensitive lives in the core, the app only calls this trait.
//!
//! The trait mirrors the Kotlin `TomiCore` (`apps/android/.../core/TomiCore.kt`): same
//! operations and the same semantics for what both share (the pin follows
//! `spec/pin-contract.md`). A few shapes differ on purpose, to fit a desktop host:
//! * sending is one call here ([`TomiCore::send_item`]) where Kotlin splits `enqueue`,
//!   `dispatch` and `cancelIfQueued`; the 3 s undo window is kept in the Windows UI and
//!   `enqueue` is called only after it, so a sync cannot outrun the cancel;
//! * history transfer is exposed directly ([`TomiCore::send_history`], `transfers`,
//!   `answer_history`, `cancel_transfer`);
//! * pairing (`spec/pairing-v1.md`) is a screen driven from Rust: the core's blocking
//!   sessions run on threads of the host, and the webview reads a [`PairingView`] (and is told
//!   when it changes) instead of waiting on them. Key material never reaches the webview;
//!   the code, its link and the verification code do, because the user has to see them;
//! * renaming and revoking a device (`rename_device`, `revoke_device`) mirror the Kotlin
//!   `renameDevice` and `revokeDevice`. They go straight to the relay (no outbox): offline is an
//!   error, not a queue. Android asks for strong biometrics before a revocation; this PC only
//!   asks the window for a confirmation there. Windows Hello ([`crate::strong_auth`]) is asked
//!   for the irreversible departure actions ("Supprimer l'espace", "Effacer quand même");
//! * leaving the space, deleting it and erasing without proof (`leave_space`, `delete_space`,
//!   `retry_departure`, `continue_departure`, `cancel_departure`, `departure`,
//!   `can_erase_without_proof`, `erase_without_proof`) mirror the Kotlin departure API; the real
//!   core answers them with the engine's own (`runtime::leave`), which erases only on a positive
//!   proof of the relay. [`CoreError::DepartureUnavailable`] is for a core that has no engine.
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::error::AppError;
use crate::model::{Device, Item, ItemDraft, ItemId, Transfer};
use crate::qr::Qr;
use crate::secret::SecretText;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("item not found: {0}")]
    NotFound(String),
    #[error("offline")]
    Offline,
    /// The core or the relay refused the operation for good (stable code, no content).
    #[error("rejected: {0}")]
    Rejected(String),
    /// This device does not belong to a space yet.
    #[error("not paired")]
    NotPaired,
    /// The device cannot use the space any more (revoked, app too old...).
    #[error("blocked: {0}")]
    Blocked(String),
    /// The local database or its files failed (or cannot be opened).
    #[error("storage: {0}")]
    Storage(String),
    /// A pairing step failed (stable short code: `failed`, `expired`, `taken`, `cancelled`,
    /// `token_invalid`... see `pairing::map_pairing_failure`). Never carries the code of the
    /// pairing nor any byte of a frame.
    #[error("pairing: {0}")]
    Pairing(String),
    /// The departure (leave the space, delete it, erase without proof) is not offered by this
    /// core. Nothing was sent and nothing was erased. Only the stand-in core (debug builds,
    /// tests) builds it: the real core has an engine, hence a departure, as soon as its store
    /// opens (a store that cannot be opened answers `Storage`, and `departure_available` is false).
    #[error("departure not available")]
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    DepartureUnavailable,
    #[error("core error: {0}")]
    Other(String),
}

/// The name a device will have once renamed: blanks at either end removed, then 1 to 64
/// characters and no control character. This is the rule of the core and of the relay
/// (`spec/api-v1.md` section 2.3), checked here only to spare a round trip and to answer the same
/// way offline (the demo core uses it too). The relay stays the judge: a name it refuses anyway
/// comes back as `name_invalid`.
pub fn clean_device_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    let n = trimmed.chars().count();
    ((1..=64).contains(&n) && !trimmed.chars().any(char::is_control)).then(|| trimmed.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendProgress {
    pub sent_bytes: u64,
    pub total_bytes: u64,
}

pub type ProgressFn = Arc<dyn Fn(SendProgress) + Send + Sync>;

/// What `send_item` leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub item_id: ItemId,
    /// Stored on this PC and in the core's persistent outbox, not accepted by the relay
    /// yet (offline, relay busy): it goes out with the next synchronisation.
    pub queued: bool,
}

/// Change notifications from the core: the "flux des items".
///
/// The `Item` prefix is kept on purpose: the variants mirror the item stream of
/// `TomiCore` on the other platforms. The payload of `ItemUpdated` is for the consumers
/// that want the new state without asking again (the windows refetch the feed instead).
#[allow(dead_code, clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// A new item: received from another device, or sent from this one.
    ItemAdded(Item),
    ItemUpdated(Item),
    ItemRemoved(ItemId),
    /// Something that is not an item changed: the devices, a history transfer, the state
    /// of the space.
    Changed,
    /// The pairing screen has something new to show: read [`TomiCore::pairing_view`] again.
    PairingChanged,
    /// The departure marker appeared (a gesture wrote it, the engine only tells the app when the
    /// attempt ends) or the departure was settled outside the departure flow: the windows read
    /// [`TomiCore::departure`] again.
    DepartureChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpaceState {
    /// Normal operation.
    Active,
    /// This device was removed from the space: nothing can be sent or read any more.
    Revoked,
    /// The space uses a protocol version newer than this app.
    UpdateRequired,
    /// Blocked for another reason (format limited by another device...).
    Blocked,
    /// The relay no longer finds the space or does not recognise this PC (`401` confirmed,
    /// `409 space_not_initialized`). Never a proof of anything: nothing was erased, and
    /// "Effacer quand même" may be offered (departure plan v2, D4/D5).
    NotFound,
}

/// What the app needs to know about the core before showing the feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    /// This device belongs to a space.
    pub paired: bool,
    pub space: SpaceState,
    /// Why the local store cannot be used (machine code), when it cannot.
    pub unavailable: Option<String>,
}

// ---------------------------------------------------------------- pairing

/// What this PC can do about pairing right now (`spec/pairing-v1.md` §10.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingOptions {
    /// This PC belongs to a space: it can invite another device, not create or join one.
    pub paired: bool,
    /// The name this PC will have in the space (the computer name).
    pub device_name: String,
    /// Not paired only. Whether the relay holds a space at all (`GET /v1/hello`). A relay can
    /// hold one space per person, so this says nothing about the space of whoever installs
    /// this PC: "Créer" and "Rejoindre" are always offered. Only `None` is used by the window:
    /// the relay does not answer, and a note says so.
    pub space_initialized: Option<bool>,
    /// An interrupted pairing left something sealed on this PC and is being settled.
    pub pending: bool,
    /// Not paired only. The relay this PC would use (`host[:port]`), `None` when there is none:
    /// a release build has no default relay, so it comes from a link, a QR code or the "Relais"
    /// field (`spec/pairing-v1.md` §4.4, §10).
    pub relay_authority: Option<String>,
    /// What the automatic resumption of an interrupted pairing concluded, until dismissed.
    pub resumed: Option<ResumeNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResumeNotice {
    /// The other device had registered this PC: it is now part of the space.
    Provisioned,
    /// The other device never registered this PC: the interrupted pairing was dropped.
    Discarded,
}

/// Where a pairing screen is (`spec/pairing-v1.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingStage {
    /// Invite: the code is shown and the new device has not answered. Join: the inviter is
    /// not ready yet.
    Waiting,
    /// The verification codes are on screen: the user compares them and decides.
    Verify,
    /// Both users agreed; the device is being added.
    Provisioning,
    Done,
    /// Cancelled here or on the other device, or the code ran out.
    Cancelled,
    Failed,
}

impl PairingStage {
    pub fn is_over(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled | Self::Failed)
    }
}

/// The other device, as it announced itself on the authenticated channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairingPeerView {
    pub name: String,
    pub platform: String,
}

/// A history, in items and bytes: an estimate to display, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HistoryView {
    pub items: u64,
    pub bytes: u64,
}

/// What a pairing screen shows. The code, its link and the verification code are secrets:
/// [`SecretText`] hides them from `Debug`, and they are dropped from the view as soon as the
/// pairing is over.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingSession {
    pub stage: PairingStage,
    /// Invite, while waiting: the code to type, `4821-crayon-renard`.
    pub code: Option<SecretText>,
    /// Invite, while waiting: the link the QR code carries. Kept on the Rust side only (it is
    /// baked into `qr`); never serialised to the webview, which has no use for the plaintext.
    #[serde(skip)]
    pub link: Option<SecretText>,
    pub qr: Option<Qr>,
    /// Invite, while waiting: when the code stops being valid, in ms since the Unix epoch on
    /// the clock of this PC.
    pub expires_at: Option<i64>,
    /// A previous code led nowhere and this one replaces it ("Un nouveau code est prêt").
    pub renewed: bool,
    /// The verification code in groups of three digits (`482 913`), to compare by eye.
    pub sas: Option<SecretText>,
    /// Join: the relay this PC joins through (`host[:port]`), shown IN FULL next to the
    /// verification code. It may have been read from a link or typed: whoever shows the QR code
    /// chooses it, so the user confirms it together with the code (`pairing-v1.md` §4.4).
    pub relay_authority: Option<String>,
    pub peer: Option<PairingPeerView>,
    /// Join: devices in the space, the inviter included.
    pub member_count: Option<u16>,
    /// Join, while verifying: what the inviter offers, which is the yes / no question.
    /// Invite, once done: the history the new device asked for, in preparation.
    pub history: Option<HistoryView>,
    /// The relay cannot be reached right now; the pairing goes on when it can.
    pub offline: bool,
    pub error: Option<AppError>,
}

impl PairingSession {
    /// A screen at `stage` with nothing to show yet.
    pub fn new(stage: PairingStage) -> Self {
        Self {
            stage,
            code: None,
            link: None,
            qr: None,
            expires_at: None,
            renewed: false,
            sas: None,
            relay_authority: None,
            peer: None,
            member_count: None,
            history: None,
            offline: false,
            error: None,
        }
    }

    /// Takes the secrets of the session off the screen (the pairing is over, or past the point
    /// where they are shown).
    pub fn wipe_secrets(&mut self) {
        self.code = None;
        self.link = None;
        self.qr = None;
        self.expires_at = None;
        self.sas = None;
    }
}

/// The whole pairing screen: nothing, a device that invites, or a device that joins.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum PairingView {
    None,
    Invite(PairingSession),
    Join(PairingSession),
}

impl PairingView {
    #[cfg(test)]
    pub fn session(&self) -> Option<&PairingSession> {
        match self {
            Self::None => None,
            Self::Invite(s) | Self::Join(s) => Some(s),
        }
    }
}

// ---------------------------------------------------------------- departure

// The shapes below follow the signatures of the engine (`spec/api-v1.md` section 7.9) and the Kotlin `TomiCore` (A2); `real_core.rs` maps
// the engine's types onto them one to one. The relay's answers are all in [`DepartureOutcome`];
// an `Err` is a local failure (storage), a refusal of the engine's own (`Rejected`) or an
// unavailable departure ([`CoreError::DepartureUnavailable`]), and in every case nothing was
// erased.

/// Which gesture started the pending departure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DepartureKind {
    /// "Se déconnecter": this PC leaves the space (`DELETE /v1/me`).
    Leave,
    /// "Supprimer l'espace": the last device deletes the space (`DELETE /v1/space`).
    DeleteSpace,
}

/// Why a departure is still waiting. In every case nothing was erased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PendingReason {
    /// Network error, timeout, truncated or unexpected body.
    Network,
    /// `5xx`, `503`, `429`.
    Server,
    /// `401` confirmed or `409 space_not_initialized`: "espace introuvable". "Effacer quand
    /// même" may be offered.
    NotFound,
    /// Verification only (outside the 15-minute window): this PC is still a member and the
    /// request had no effect.
    NotSent,
}

/// Why the relay (or the app) refused the departure for good. The marker is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefusalReason {
    /// This PC is the last device: only "Supprimer l'espace" is left.
    LastDevice,
    UpdateRequired,
    /// The relay has no `DELETE /v1/space`: "Effacer quand même" may be offered.
    RelayTooOld,
    Unexpected,
}

/// What one attempt of the departure concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum DepartureOutcome {
    /// A positive proof arrived and this PC erased everything it held.
    Erased,
    Pending {
        reason: PendingReason,
    },
    Refused {
        reason: RefusalReason,
    },
    /// Cancelled meanwhile: nothing was erased.
    Cancelled,
    /// No departure pending (`continue_departure` only).
    NothingPending,
}

/// The departure pending on this PC, as the banner shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepartureView {
    pub kind: DepartureKind,
    /// A proof was received and the erasure is under way: no button, not cancellable.
    pub wiping: bool,
    /// Time of the last gesture (ms since the Unix epoch); `None` for a marker written by an
    /// older version.
    pub since_ms: Option<i64>,
    /// Reason of the last attempt of this process, `None` before the first one.
    pub last: Option<PendingReason>,
}

#[async_trait]
pub trait TomiCore: Send + Sync {
    fn status(&self) -> CoreStatus;

    /// This device (a placeholder named after the PC until it is paired).
    fn this_device(&self) -> Device;

    /// Stores the draft (files are read and encrypted before this returns) and tries to
    /// push it to the relay. A refusal to push *for now* (offline) is not an error: the
    /// item stays in the core's outbox and `queued` is set.
    async fn send_item(
        &self,
        draft: ItemDraft,
        progress: ProgressFn,
    ) -> Result<SendOutcome, CoreError>;

    /// All local items, newest first, queued outgoing items included. Items past their
    /// explicit expiration are not returned. The core never reorders for a pin: the UI
    /// builds the "Épinglés" section (`spec/pin-contract.md`).
    async fn list_items(&self) -> Result<Vec<Item>, CoreError>;

    async fn get_item(&self, id: &str) -> Result<Item, CoreError>;

    /// Stream of item changes.
    fn subscribe(&self) -> broadcast::Receiver<CoreEvent>;

    /// Removes the local copy only.
    async fn delete_local(&self, id: &str) -> Result<(), CoreError>;

    /// Removes everywhere (signed tombstone to the relay and to all devices).
    async fn delete_everywhere(&self, id: &str) -> Result<(), CoreError>;

    /// Marks an item read (synchronised to other devices). An item with the
    /// `afterFirstRead` policy is consumed by it: erased on every device.
    async fn mark_read(&self, id: &str) -> Result<(), CoreError>;

    /// Pins the item on all devices. Idempotent, unknown id: no effect, immediate local
    /// effect (the item is re-emitted with `pinned` up to date). A network error is never
    /// an error of this call: the decision leaves through the outbox.
    async fn pin(&self, id: &str) -> Result<(), CoreError>;

    /// Unpins the item on all devices. Same semantics as [`TomiCore::pin`].
    async fn unpin(&self, id: &str) -> Result<(), CoreError>;

    /// Archives the item on all devices (`spec/archive-contract.md`). Idempotent, unknown id:
    /// no effect, immediate local effect (the item is re-emitted with `archived` up to date).
    /// Never marks the item read. Archiving a pinned item unpins it first. A network error
    /// is never an error of this call: the decision leaves through the outbox.
    async fn archive(&self, id: &str) -> Result<(), CoreError>;

    /// Unarchives the item on all devices. Same semantics as [`TomiCore::archive`].
    async fn unarchive(&self, id: &str) -> Result<(), CoreError>;

    async fn devices(&self) -> Result<Vec<Device>, CoreError>;

    /// Renames a device of the space, this PC included. The name is trimmed and must be 1 to 64
    /// characters without control character ([`clean_device_name`]); otherwise
    /// `Rejected("name_invalid")`. Needs the network (`Offline` without it). Unknown device:
    /// `Rejected("unknown_device")`.
    async fn rename_device(&self, id: &str, name: &str) -> Result<(), CoreError>;

    /// Removes a device from the space and renews the space key, so that it cannot read what is
    /// sent from now on. Never possible on this PC itself: `Rejected("revoke_self")`. Needs the
    /// network (`Offline` without it). Other refusals: `Rejected("unknown_device")` (not in the
    /// space any more), `Rejected("device_set_mismatch")` / `Rejected("epoch_conflict")` (the
    /// devices or the key changed while the key was renewed: ask the user to retry).
    async fn revoke_device(&self, id: &str) -> Result<(), CoreError>;

    /// Local path of the decrypted bytes of an attachment, named like the file. The core
    /// keeps files encrypted at rest: this decrypts on demand (downloading first if
    /// needed) into a cache that [`TomiCore::purge_plaintext_cache`] empties. `slot` is
    /// `Attachment::slot` (see `Item::attachment_slot`), NOT the position in the list: they
    /// differ once a file was removed from the item.
    async fn attachment_path(&self, id: &str, slot: usize) -> Result<PathBuf, CoreError>;

    /// Removes ONE attachment of an item (`slot` is `Attachment::slot`, never the position),
    /// from this PC only or `everywhere` (signed declaration to the relay and the devices). The
    /// others stay; when nothing else is left in the item (no file, no text, no URL) the whole item
    /// is deleted with the same scope: `ItemDeleted`. Idempotent: an unknown or already removed
    /// slot does nothing. Refused (`Rejected`) while the item is still in the outbox, for both
    /// scopes. No biometry, like deleting an item.
    async fn delete_attachment(
        &self,
        id: &str,
        slot: usize,
        everywhere: bool,
    ) -> Result<crate::model::AttachmentRemoval, CoreError>;

    /// Whether the relay takes the removal "everywhere" (capability heard at the last session).
    /// `false` until a session has told: the UI then offers "de cet appareil" only.
    async fn supports_attachment_removal(&self) -> bool;

    // ---- history transfer (decision D2: everything, one yes or no) ----

    async fn transfers(&self) -> Result<Vec<Transfer>, CoreError>;

    /// Sends the whole history (secrets and files included) to a verified device, sealed
    /// for that device only.
    async fn send_history(&self, device_id: &str) -> Result<(), CoreError>;

    /// The user's answer to an offered history: yes (`accept`) or no.
    async fn answer_history(&self, transfer_id: &str, accept: bool) -> Result<(), CoreError>;

    async fn cancel_transfer(&self, transfer_id: &str) -> Result<(), CoreError>;

    // ---- pairing (`spec/pairing-v1.md`) ----

    /// What this PC can do about pairing: invite a device (paired), create the space or join
    /// one (not paired). A PC that is not paired asks the relay (`hello`) whether it answers; a
    /// relay that does not answer is not an error (`space_initialized` is then `None`).
    async fn pairing_options(&self) -> Result<PairingOptions, CoreError>;

    /// The pairing screen as it is now. No network. The pairing runs in the core, not in the
    /// window: closing and reopening the window finds the same screen.
    fn pairing_view(&self) -> PairingView;

    /// First device: creates the space and becomes its first member (§10). The install token
    /// is used for this call and kept nowhere. `device_name` defaults to the computer name.
    #[allow(dead_code)] // The app goes through the `_at` form; this one is for the tests.
    async fn pairing_create_space(
        &self,
        install_token: &str,
        device_name: Option<&str>,
    ) -> Result<(), CoreError> {
        self.pairing_create_space_at(install_token, device_name, None)
            .await
    }

    /// [`TomiCore::pairing_create_space`] at a relay: `install_token` is a bare token or a
    /// creation link (`https://<relay>/create#<token>`, `/create/<token>`), `relay_url` the
    /// "Relais" field (`https://host`). A relay that was read is not trusted: the window shows
    /// it before "Créer", and nothing is kept before the creation succeeds. No relay at all is
    /// `Pairing("relay_not_configured")`, a relay that is not one `Pairing("relay_invalid")`.
    async fn pairing_create_space_at(
        &self,
        install_token: &str,
        device_name: Option<&str>,
        relay_url: Option<&str>,
    ) -> Result<(), CoreError>;

    /// A member shows a code and a QR code for a new device (A1 to A4). The pairing then
    /// advances by itself; follow it with [`TomiCore::pairing_view`] and
    /// [`CoreEvent::PairingChanged`].
    async fn pairing_invite(&self) -> Result<PairingView, CoreError>;

    /// A device that is not paired joins the space of the one that shows `code_or_link`: a
    /// typed code, a pasted link, or a `tomi-share://pair#` link (N0 to N2). The code is read
    /// locally first, so a typo consumes nothing.
    #[allow(dead_code)] // The app goes through the `_at` form; this one is for the tests.
    async fn pairing_join(
        &self,
        code_or_link: &str,
        device_name: Option<&str>,
    ) -> Result<PairingView, CoreError> {
        self.pairing_join_at(code_or_link, device_name, None).await
    }

    /// [`TomiCore::pairing_join`] at a relay: `relay_url` is the "Relais" field (`https://host`),
    /// absent when this PC has its relay or the link names one. The relay actually used comes
    /// back as [`PairingSession::relay_authority`] and must be shown in full with the
    /// verification code; nothing is stored before the user confirmed.
    async fn pairing_join_at(
        &self,
        code_or_link: &str,
        device_name: Option<&str>,
        relay_url: Option<&str>,
    ) -> Result<PairingView, CoreError>;

    /// The user compared the verification codes and agrees (A8, N8). `import_history` is the
    /// answer to the history question of the new device; the inviter ignores it. Returns at
    /// once: the pairing goes on in the background.
    fn pairing_confirm(&self, import_history: bool) -> Result<(), CoreError>;

    /// The user refuses ("ils sont différents") or cancels, at any stage. Returns at once; the
    /// channel is closed and the secrets of the session wiped in the background.
    fn pairing_cancel(&self);

    /// The pairing screen was left (another tab, the window closed): a code on screen, or codes
    /// being compared, are abandoned (`pairing-v1.md` §11.2); a pairing that has moved on to
    /// adding the device goes on in the background. A finished pairing that was on screen is
    /// closed.
    fn pairing_leave(&self);

    /// Forgets a finished pairing (done, cancelled, failed) and the notice of an interrupted
    /// one that was settled at start.
    fn pairing_dismiss(&self);

    // ---- housekeeping ----

    /// Fetches what waits on the relay and pushes the outbox. Being offline is not an
    /// error.
    async fn sync(&self) -> Result<(), CoreError>;

    /// Erases the decrypted copies of attachments (the window is hidden or the session
    /// ends). Files still open in another program stay until the next call.
    async fn purge_plaintext_cache(&self) -> Result<(), CoreError>;

    // ---- departure (`spec/api-v1.md` 7.9) ----
    //
    // The core erases this PC only on a positive proof of the relay (7.9.1) or, for
    // `erase_without_proof`, when the app asks for it after its own strong verification. The
    // asking window never decides on its own what is a proof: it reads the outcome.

    /// Whether this core can do a departure: it has an engine that gives proofs (paired or not:
    /// an unpaired device still settles a leftover marker). `false`: the window disables the
    /// entry instead of offering something that cannot work.
    fn departure_available(&self) -> bool;

    /// "Se déconnecter": writes the marker, then makes one attempt (T1, T2). Blocks for up to
    /// 520 s on a bad network (the real bound of the core, retries included), and never throws on
    /// a relay answer. "Annuler" ([`TomiCore::cancel_departure`]) does not wait for it.
    async fn leave_space(&self) -> Result<DepartureOutcome, CoreError>;

    /// "Supprimer l'espace", after `Refused { LastDevice }` and its confirmation (T4, T1, T3).
    /// Up to 1060 s on a bad network, also not awaited by "Annuler".
    async fn delete_space(&self) -> Result<DepartureOutcome, CoreError>;

    /// "Réessayer": a gesture, the request is sent again whatever the age of the marker.
    async fn retry_departure(&self) -> Result<DepartureOutcome, CoreError>;

    /// Automatic (start-up, back-off, window shown): resends the request only within 15
    /// minutes of the last gesture and only verifies after that.
    async fn continue_departure(&self) -> Result<DepartureOutcome, CoreError>;

    /// "Annuler": removes the marker at once (the erasure itself is never cancellable).
    /// `true` if something was cancelled, or, when a gesture of this core has not written its
    /// marker yet, if the cancellation is guaranteed to be carried out the moment it does (the
    /// attempt then goes on to its end and erases nothing). Never `true` for a cancel that
    /// nothing will carry out.
    async fn cancel_departure(&self) -> Result<bool, CoreError>;

    /// The pending departure, `None` when none. No network.
    fn departure(&self) -> Option<DepartureView>;

    /// The core allows "Effacer quand même" right now: its last verdict (10 minutes at most,
    /// this process) is "not found" or "relay too old". Never after a plain network error.
    fn can_erase_without_proof(&self) -> bool;

    /// "Effacer quand même": erases without any call to the relay. Refused unless
    /// [`TomiCore::can_erase_without_proof`]. The caller has already obtained a strong
    /// verification; the core cannot ask for it.
    async fn erase_without_proof(&self) -> Result<(), CoreError>;

    /// Start-up, once the pending departure was settled (`spec/api-v1.md` 7.9.4: before any
    /// `resume_pairing`): settles a pairing that a stop interrupted. The stand-in core has nothing
    /// to settle.
    fn resume_pairing(&self) {}

    /// The app quits: stops the background work and erases the decrypted copies.
    fn shutdown(&self);
}
