//! The pairing host: the screens of `spec/pairing-v1.md` (§7 invite and join, §10 create the
//! space, §11 errors and resumption) driven from Rust.
//!
//! The core does the protocol; its sessions (`InviteSession`, `JoinSession`) are blocking
//! handles meant to be driven from a thread of the app. This module owns those threads, one per
//! session, and publishes what the screen shows as a [`PairingView`]. The webview never waits
//! on a session: it reads the view and is told when it changes, so a window that is closed and
//! reopened finds the same screen, and an unanswered request never stays pending in it.
//!
//! ## Who calls the session, and when
//!
//! Only the driver thread calls `poll` and `confirm` of a session. The core refuses a
//! `confirm` that arrives while a `poll` runs, and a long poll lasts up to 25 s, so a command
//! of the webview never calls them: it records an *intent* (confirm with an answer, cancel)
//! and wakes the driver, which acts between two polls. While the user compares the codes the
//! driver does not wait on the relay: it sleeps on the intent and glances at the channel every
//! [`Timing::verify_tick`] (`poll(0)`), so a cancellation on the other device shows up and a
//! click is taken at once. `cancel` is the one call that may come from elsewhere (the core
//! designed it so): it is made on a short thread, because it closes the channel on the network.
//!
//! ## What is kept, what is shown
//!
//! The code, its link and the verification code are secrets (`SecretText`): they live in the
//! view only while the screen shows them, are wiped from it when the pairing is over, and are
//! never logged (this module has no log, and its errors are stable short codes). The keys and
//! the space key never come here: the core provisions the device itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tomi_core::pairing::{word, HistoryAnnouncement, HistoryOffer, PairingCode, WORD_COUNT};
use tomi_core::runtime::{
    Engine, InviteOutcome, InvitePhase, InviteSession, JoinOutcome, JoinPhase, JoinPrompt,
    JoinSession, RelayTransport, ResumeOutcome, VerifyPrompt,
};
use tomi_core::CoreError as Failure;
use zeroize::Zeroizing;

use crate::core_api::{
    CoreError, HistoryView, PairingOptions, PairingPeerView, PairingSession, PairingStage,
    PairingView, ResumeNotice,
};
use crate::error::AppError;
use crate::model::now_ms;
use crate::qr;
use crate::real_core::map_failure;
use crate::secret::SecretText;

/// Renewals of a code that nobody used, by the screen itself (`pairing-v1.md` §4.2). A device
/// may open three channels in a burst, then one every six minutes (`rl_device_pairing`,
/// `api-v1.md` §8.5): the first code and two renewals use the burst, and the screen then waits
/// for the user, who may ask for a new code when they want one.
const MAX_RENEWALS: u32 = 2;
/// Attempts of the resumption at start before giving up until the next start.
const RESUME_ATTEMPTS: u32 = 12;

/// Delays of the host, tuned down by the tests.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// While the user compares the codes: how often the channel is looked at.
    pub verify_tick: Duration,
    /// First pause after a failure that leaves the session alive; doubled up to `retry_max`.
    pub retry_first: Duration,
    pub retry_max: Duration,
    /// Same, for the resumption at start.
    pub resume_first: Duration,
    pub resume_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            // Under `rl_pairing_poll` (60 a minute, `api-v1.md` §8.5).
            verify_tick: Duration::from_millis(1500),
            retry_first: Duration::from_secs(1),
            retry_max: Duration::from_secs(10),
            resume_first: Duration::from_secs(2),
            resume_max: Duration::from_secs(60),
        }
    }
}

// ---------------------------------------------------------------------------
// What the host needs from the core
// ---------------------------------------------------------------------------

/// An invitation, as the host drives it. `InviteSession` is the real one; the tests script
/// their own.
///
/// The waits are the core's own (`wait_for_peer`, `wait_done`): they hold the long polls of the
/// relay and the pause that keeps a relay which answers at once from being asked in a loop.
/// `poll` is used only to glance at the channel while the user compares the codes.
pub trait Invitation: Send + Sync {
    /// `4821-crayon-renard`; secret.
    fn code(&self) -> Option<String>;
    /// `https://<relay>/l/pair#<code>`, what the QR code carries; secret.
    fn link(&self) -> Option<String>;
    /// When the relay drops the code if nobody claimed it, on the relay clock.
    fn expires_at_ms(&self) -> i64;
    fn phase(&self) -> InvitePhase;
    fn prompt(&self) -> Option<VerifyPrompt>;
    /// Blocks until the new device said hello and the codes can be compared (A5 to A7).
    fn wait_for_peer(&self) -> Result<VerifyPrompt, Failure>;
    /// One step of the session: never waits when `wait_secs` is 0.
    fn poll(&self, wait_secs: u32) -> Result<InvitePhase, Failure>;
    fn confirm(&self) -> Result<(), Failure>;
    /// Blocks until the new device is registered (A9 to A13); at once if it already is.
    fn wait_done(&self) -> Result<InviteOutcome, Failure>;
    fn cancel(&self);
}

impl Invitation for InviteSession {
    fn code(&self) -> Option<String> {
        InviteSession::code(self)
    }
    fn link(&self) -> Option<String> {
        InviteSession::link(self)
    }
    fn expires_at_ms(&self) -> i64 {
        InviteSession::expires_at_ms(self)
    }
    fn phase(&self) -> InvitePhase {
        InviteSession::phase(self)
    }
    fn prompt(&self) -> Option<VerifyPrompt> {
        InviteSession::prompt(self)
    }
    fn wait_for_peer(&self) -> Result<VerifyPrompt, Failure> {
        InviteSession::wait_for_peer(self)
    }
    fn poll(&self, wait_secs: u32) -> Result<InvitePhase, Failure> {
        InviteSession::poll(self, wait_secs)
    }
    fn confirm(&self) -> Result<(), Failure> {
        InviteSession::confirm(self)
    }
    fn wait_done(&self) -> Result<InviteOutcome, Failure> {
        InviteSession::wait_done(self)
    }
    fn cancel(&self) {
        InviteSession::cancel(self);
    }
}

/// A joining, as the host drives it.
pub trait Joining: Send + Sync {
    fn phase(&self) -> JoinPhase;
    fn prompt(&self) -> Option<JoinPrompt>;
    /// Blocks until the inviter is verified and the codes can be compared (N3 to N7).
    fn wait_for_inviter(&self) -> Result<JoinPrompt, Failure>;
    /// One step of the session: never waits when `wait_secs` is 0.
    fn poll(&self, wait_secs: u32) -> Result<JoinPhase, Failure>;
    fn confirm(&self, import_history: bool) -> Result<(), Failure>;
    /// Blocks until this device is provisioned (N10 to N13); at once if it already is.
    fn wait_done(&self) -> Result<JoinOutcome, Failure>;
    fn cancel(&self);
}

impl Joining for JoinSession {
    fn phase(&self) -> JoinPhase {
        JoinSession::phase(self)
    }
    fn prompt(&self) -> Option<JoinPrompt> {
        JoinSession::prompt(self)
    }
    fn wait_for_inviter(&self) -> Result<JoinPrompt, Failure> {
        JoinSession::wait_for_inviter(self)
    }
    fn poll(&self, wait_secs: u32) -> Result<JoinPhase, Failure> {
        JoinSession::poll(self, wait_secs)
    }
    fn confirm(&self, import_history: bool) -> Result<(), Failure> {
        JoinSession::confirm(self, import_history)
    }
    fn wait_done(&self) -> Result<JoinOutcome, Failure> {
        JoinSession::wait_done(self)
    }
    fn cancel(&self) {
        JoinSession::cancel(self);
    }
}

/// The engine, as far as pairing is concerned.
pub trait PairingEngine: Send + Sync + 'static {
    fn is_provisioned(&self) -> bool;
    /// An interrupted pairing left something sealed on this device.
    fn has_pending(&self) -> bool;
    /// The relay clock, to turn the expiry of a code into a time of this PC.
    fn server_now_ms(&self) -> i64;
    /// The authority of the relay this PC uses now (`host[:port]`), `None` when it has none (a
    /// release build has no default relay: it comes from a link, a QR code or the "Relais" field).
    fn relay_authority(&self) -> Option<String>;
    /// `GET /v1/hello`: does the relay hold a space (any)? `None` if it does not answer.
    fn space_initialized(&self) -> Option<bool>;
    fn start_invitation(&self) -> Result<Arc<dyn Invitation>, Failure>;
    /// `relay_url`: the "Relais" field. A relay read from a link or a field is not trusted by
    /// itself: the core points the transport at it for this session only and gives the authority
    /// back in the prompt; nothing is stored before the user confirmed.
    fn join_space(
        &self,
        code_or_link: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<Arc<dyn Joining>, Failure>;
    fn create_space(
        &self,
        install_token: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<(), Failure>;
    fn resume_pairing(&self) -> Result<ResumeOutcome, Failure>;
    /// A synchronisation: the export of a history announced at pairing is started by the next.
    fn sync(&self);
}

/// The real engine, and the transport it talks through (for `hello` and the relay authority).
pub struct EnginePairing {
    pub engine: Arc<Engine>,
    pub relay: Option<Arc<dyn RelayTransport>>,
}

impl PairingEngine for EnginePairing {
    fn is_provisioned(&self) -> bool {
        self.engine.is_provisioned()
    }
    fn has_pending(&self) -> bool {
        self.engine.has_pending_pairing().unwrap_or(false)
    }
    fn server_now_ms(&self) -> i64 {
        self.engine.server_now_ms()
    }
    fn relay_authority(&self) -> Option<String> {
        self.relay.as_ref().and_then(|r| r.relay_authority())
    }
    fn space_initialized(&self) -> Option<bool> {
        let hello = self.relay.as_ref()?.hello().ok()?;
        Some(hello.space_initialized)
    }
    fn start_invitation(&self) -> Result<Arc<dyn Invitation>, Failure> {
        let session: Arc<InviteSession> = self.engine.start_invitation()?;
        Ok(session)
    }
    fn join_space(
        &self,
        code_or_link: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<Arc<dyn Joining>, Failure> {
        let session: Arc<JoinSession> =
            self.engine
                .join_space_at(code_or_link, device_name, relay_url)?;
        Ok(session)
    }
    fn create_space(
        &self,
        install_token: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<(), Failure> {
        self.engine
            .create_space_at(install_token, device_name, relay_url)
    }
    fn resume_pairing(&self) -> Result<ResumeOutcome, Failure> {
        self.engine.resume_pairing()
    }
    fn sync(&self) {
        let _ = self.engine.sync();
    }
}

/// What the host tells the app.
#[derive(Clone)]
pub struct Hooks {
    /// The pairing screen has something new to show.
    pub changed: Arc<dyn Fn() + Send + Sync>,
    /// This PC just became part of a space (it joined, or it created it): start what a
    /// paired PC runs (the doorbell).
    pub provisioned: Arc<dyn Fn() + Send + Sync>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The short code of a failure that the pairing screens explain (`pairing-v1.md` §11.5,
/// `api-v1.md` §3.2, §4.3, §4.6); `None` for the others.
fn pairing_code(e: &Failure) -> Option<&'static str> {
    Some(match e {
        Failure::PairingFailed => "failed",
        Failure::PairingExpired => "expired",
        Failure::PairingTaken => "taken",
        Failure::PairingCancelled => "cancelled",
        Failure::PairingUnsupported => "unsupported",
        // No relay at all: neither the device, nor the link, nor the "Relais" field names one.
        Failure::RelayNotConfigured => "relay_not_configured",
        Failure::Relay { status: 403, code } if code == "install_token_invalid" => "token_invalid",
        // Too many codes asked for, or too many claims (`api-v1.md` §8.5): not "offline".
        Failure::Relay { status: 429, .. } => "rate_limited",
        Failure::Relay { status: 409, code } => match code.as_str() {
            "too_many_devices" => "too_many_devices",
            "channel_limit" => "busy",
            "epoch_conflict" => "epoch_conflict",
            "device_exists" => "device_exists",
            "space_exists" => "space_exists",
            _ => return None,
        },
        _ => return None,
    })
}

/// A failure of a pairing call, as the app reports it.
pub fn map_pairing_failure(e: Failure) -> CoreError {
    match pairing_code(&e) {
        Some(code) => CoreError::Pairing(code.to_owned()),
        None => map_failure(e),
    }
}

/// The refusals of the core to the arguments of `create_space` and `join_space`, which are
/// the user's to fix. They are told apart by the reason the core gives (the tests below call
/// the core to keep these in step).
fn map_input_failure(e: Failure) -> CoreError {
    if let Failure::InvalidInput { reason } = &e {
        let code = if reason.contains("install token") {
            Some("token_invalid")
        } else if reason.contains("already provisioned") {
            Some("already_paired")
        } else if reason.contains("device name") {
            Some("name_invalid")
        } else if reason.contains("different relays") {
            // A link and the "Relais" field that name two relays.
            Some("relay_mismatch")
        } else if reason.contains("relay URL") {
            // "relay URL: ..." and "the relay URL must be https://".
            Some("relay_invalid")
        } else if reason.contains("in progress") {
            Some("in_progress")
        } else {
            None
        };
        if let Some(code) = code {
            return CoreError::Pairing(code.to_owned());
        }
    }
    map_pairing_failure(e)
}

/// The relay cannot be reached, or asks to wait: the session is still alive.
fn is_transient(e: &Failure) -> bool {
    match e {
        Failure::Network { .. } | Failure::Storage { .. } => true,
        Failure::Relay { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

/// The relay itself is the problem (as opposed to the local database).
fn is_offline(e: &Failure) -> bool {
    matches!(e, Failure::Network { .. }) || matches!(e, Failure::Relay { .. } if is_transient(e))
}

/// A code that led nowhere before anyone compared anything: a new one is shown.
fn is_renewable(e: &Failure) -> bool {
    matches!(
        e,
        Failure::PairingExpired | Failure::PairingCancelled | Failure::PairingFailed
    )
}

fn app_error(e: &Failure) -> AppError {
    AppError::from(map_pairing_failure(e.clone()))
}

/// What a pasted or scanned text is (`read_input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InputKind {
    /// `https://<relay>/l/pair#<code>` or `tomi-share://pair#<code>`.
    PairLink,
    /// `https://<relay>/create#<token>` or `/create/<token>`.
    SetupLink,
    /// A pairing code, `4821-crayon-renard`.
    Code,
    /// A bare installation token.
    Token,
    Unknown,
}

/// What the form learns from a text without sending anything: its kind, and the relay it names
/// (`https://host[:port]`, normalised by the core) with its authority. Neither the code nor the
/// token comes back. **The relay is not trusted**: the form puts it in the "Relais" field and the
/// verification screen shows it in full (`spec/pairing-v1.md` §4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputRead {
    pub kind: InputKind,
    pub relay_url: Option<String>,
    pub relay_authority: Option<String>,
}

/// Reads a text typed or pasted in the pairing or the creation form, with the reading of the
/// core (`parse_pair_link`, `parse_setup_link`, `is_install_token_shape`). Local only.
pub fn read_input(text: &str) -> InputRead {
    let of = |kind: InputKind, relay_url: Option<String>| InputRead {
        kind,
        relay_authority: relay_url
            .as_deref()
            .map(|u| u.trim_start_matches("https://").to_owned()),
        relay_url,
    };
    if text.contains("://") {
        if let Some(link) = tomi_core::parse_pair_link(text) {
            return of(InputKind::PairLink, link.relay_url);
        }
        if let Some(link) = tomi_core::parse_setup_link(text) {
            return of(InputKind::SetupLink, Some(link.relay_url));
        }
        return of(InputKind::Unknown, None);
    }
    if PairingCode::parse(text).is_ok() {
        of(InputKind::Code, None)
    } else if tomi_core::is_install_token_shape(text) {
        of(InputKind::Token, None)
    } else {
        of(InputKind::Unknown, None)
    }
}

/// The word of the code list that `prefix` identifies, to complete a code as it is typed
/// (`spec/pairing-v1.md` §4.4: the first four letters of a word are enough to name it, and the
/// screen should offer it). `prefix` is lowercase ASCII letters, as the screen folds them. The
/// list is public: nothing here is secret and nothing is asked of the relay.
pub fn complete_word(prefix: &str) -> Option<String> {
    let prefix = prefix.trim();
    if !(4..=8).contains(&prefix.len()) || !prefix.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let words = (0..u16::try_from(WORD_COUNT).unwrap_or(u16::MAX)).filter_map(word);
    let mut found = words.filter(|w| w.starts_with(prefix));
    let first = found.next()?;
    // Four letters name one word; a prefix that fits two names none.
    found.next().is_none().then(|| first.to_owned())
}

/// The name of this PC for the space: the computer name.
pub fn default_device_name() -> String {
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    let name: String = name.trim().chars().take(64).collect();
    if name.is_empty() {
        "Ce PC".to_owned()
    } else {
        name
    }
}

// ---------------------------------------------------------------------------
// Intents: what the user decided, for the driver to act on
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Intents {
    /// The user agreed; the answer to the history question (ignored by the inviter).
    confirm: Option<bool>,
    cancel: bool,
}

#[derive(Default)]
struct Signal {
    intents: Mutex<Intents>,
    wake: Condvar,
}

enum Woken {
    Confirm(bool),
    Cancel,
    Nothing,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Signal {
    fn confirm(&self, import_history: bool) {
        lock(&self.intents).confirm = Some(import_history);
        self.wake.notify_all();
    }

    fn cancel(&self) {
        lock(&self.intents).cancel = true;
        self.wake.notify_all();
    }

    fn is_cancelled(&self) -> bool {
        lock(&self.intents).cancel
    }

    /// Waits for a decision of the user, at most `max`.
    fn wait(&self, max: Duration) -> Woken {
        let deadline = Instant::now() + max;
        let mut intents = lock(&self.intents);
        loop {
            if intents.cancel {
                return Woken::Cancel;
            }
            if let Some(answer) = intents.confirm.take() {
                return Woken::Confirm(answer);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Woken::Nothing;
            }
            intents = self
                .wake
                .wait_timeout(intents, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Pauses for `d`. `false` if the user cancelled meanwhile. A confirmation is left alone.
    fn pause(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        let mut intents = lock(&self.intents);
        loop {
            if intents.cancel {
                return false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            intents = self
                .wake
                .wait_timeout(intents, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Pauses that grow while a failure lasts.
struct Backoff {
    next: Duration,
    first: Duration,
    max: Duration,
}

impl Backoff {
    fn new(first: Duration, max: Duration) -> Self {
        Self {
            next: first,
            first,
            max,
        }
    }

    fn pause(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next * 2).min(self.max);
        now
    }

    fn reset(&mut self) {
        self.next = self.first;
    }
}

// ---------------------------------------------------------------------------
// The host
// ---------------------------------------------------------------------------

enum Handle {
    Invite(Arc<dyn Invitation>),
    Join(Arc<dyn Joining>),
}

/// How far a pairing has gone *in the core*, which is what decides whether leaving the screen
/// or quitting the app may cancel it. The published [`PairingStage`] can lag a step behind the
/// core (a driver that is between two polls has not refreshed the screen yet), and it folds
/// two join phases into one `Provisioning`, so these decisions read the core, not the screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Progress {
    /// The code is shown, or the codes are being compared: nothing is committed. Cancelling
    /// here strands nothing.
    Open,
    /// Both users agreed and the device is being added, but no batch is sealed yet (invite in
    /// `Provisioning`, join in `Provisioning`). Cancelling would strand a device the relay may
    /// already have registered, so leaving the screen keeps going; quitting cancels.
    Committed,
    /// A join sealed its verified batch and only has to settle it with the relay (join in
    /// `Completing`). Quitting must keep it: the next start settles it, cancelling would erase
    /// it (`pairing-v1.md` §11.4).
    Sealed,
    /// The pairing is over (done, cancelled or failed) in the core.
    Over,
}

impl Handle {
    fn cancel(&self) {
        match self {
            Handle::Invite(i) => i.cancel(),
            Handle::Join(j) => j.cancel(),
        }
    }

    /// The real progress of the pairing, read from the core.
    fn progress(&self) -> Progress {
        match self {
            Handle::Invite(i) => match i.phase() {
                InvitePhase::AwaitPeer | InvitePhase::Verify => Progress::Open,
                InvitePhase::Provisioning => Progress::Committed,
                InvitePhase::Done | InvitePhase::Cancelled | InvitePhase::Failed => Progress::Over,
            },
            Handle::Join(j) => match j.phase() {
                JoinPhase::AwaitInviter | JoinPhase::Verify => Progress::Open,
                JoinPhase::Provisioning => Progress::Committed,
                JoinPhase::Completing => Progress::Sealed,
                JoinPhase::Done | JoinPhase::Cancelled | JoinPhase::Failed => Progress::Over,
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Invite,
    Join,
}

struct Live {
    generation: u64,
    role: Role,
    session: PairingSession,
    handle: Handle,
    signal: Arc<Signal>,
}

#[derive(Default)]
struct State {
    generation: u64,
    live: Option<Live>,
    /// A session is being started: the core call that opens it has not returned yet.
    starting: bool,
    notice: Option<ResumeNotice>,
}

pub struct PairingHost {
    engine: Arc<dyn PairingEngine>,
    hooks: Hooks,
    timing: Timing,
    state: Mutex<State>,
    /// The app is quitting.
    stop: AtomicBool,
}

/// Holds the right to start a session (one at a time on this PC).
struct Slot<'a>(&'a PairingHost);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).starting = false;
    }
}

fn invite_stage(p: InvitePhase) -> PairingStage {
    match p {
        InvitePhase::AwaitPeer => PairingStage::Waiting,
        InvitePhase::Verify => PairingStage::Verify,
        InvitePhase::Provisioning => PairingStage::Provisioning,
        InvitePhase::Done => PairingStage::Done,
        InvitePhase::Cancelled => PairingStage::Cancelled,
        InvitePhase::Failed => PairingStage::Failed,
    }
}

fn join_stage(p: JoinPhase) -> PairingStage {
    match p {
        JoinPhase::AwaitInviter => PairingStage::Waiting,
        JoinPhase::Verify => PairingStage::Verify,
        JoinPhase::Provisioning | JoinPhase::Completing => PairingStage::Provisioning,
        JoinPhase::Done => PairingStage::Done,
        JoinPhase::Cancelled => PairingStage::Cancelled,
        JoinPhase::Failed => PairingStage::Failed,
    }
}

fn history_view(items: u64, bytes: u64) -> HistoryView {
    HistoryView { items, bytes }
}

impl PairingHost {
    pub fn new(engine: Arc<dyn PairingEngine>, hooks: Hooks, timing: Timing) -> Arc<Self> {
        Arc::new(Self {
            engine,
            hooks,
            timing,
            state: Mutex::new(State::default()),
            stop: AtomicBool::new(false),
        })
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn changed(&self) {
        (self.hooks.changed)();
    }

    /// Whether `generation` is still the pairing on screen, and not over. A driver whose
    /// pairing was cancelled, dismissed or replaced stops.
    fn alive(&self, generation: u64) -> bool {
        lock(&self.state)
            .live
            .as_ref()
            .is_some_and(|l| l.generation == generation && !l.session.stage.is_over())
    }

    // ------------------------------------------------------------ what the screen reads

    pub fn view(&self) -> PairingView {
        match &lock(&self.state).live {
            None => PairingView::None,
            Some(l) if l.role == Role::Invite => PairingView::Invite(l.session.clone()),
            Some(l) => PairingView::Join(l.session.clone()),
        }
    }

    /// What this PC can do. Asks the relay (so it blocks) when this PC is not paired.
    pub fn options(&self, device_name: String) -> PairingOptions {
        let paired = self.engine.is_provisioned();
        let space_initialized = if paired {
            None
        } else {
            self.engine.space_initialized()
        };
        PairingOptions {
            paired,
            device_name,
            space_initialized,
            pending: self.engine.has_pending(),
            relay_authority: if paired {
                None
            } else {
                self.engine.relay_authority()
            },
            resumed: lock(&self.state).notice,
        }
    }

    // ------------------------------------------------------------ starting

    /// One pairing at a time on this PC: refuses while another one runs or is being started.
    fn reserve(&self) -> Result<Slot<'_>, CoreError> {
        let mut st = lock(&self.state);
        let running = st.live.as_ref().is_some_and(|l| !l.session.stage.is_over());
        if st.starting || running {
            return Err(CoreError::Pairing("in_progress".into()));
        }
        st.starting = true;
        Ok(Slot(self))
    }

    /// First device: creates the space (`pairing-v1.md` §10). Blocks on the network.
    #[allow(dead_code)] // The app goes through the `_at` form; this one is for the tests.
    pub fn create_space(&self, install_token: &str, device_name: &str) -> Result<(), CoreError> {
        self.create_space_at(install_token, device_name, None)
    }

    /// [`create_space`](Self::create_space) at a relay: `install_token` is a bare token or a
    /// creation link (`https://<relay>/create#<token>`), `relay_url` the "Relais" field. A link
    /// and a field that name two relays are refused (`relay_mismatch`). The relay is the core's
    /// to judge (`relay_invalid`, `relay_not_configured`); the screen shows it before "Créer".
    pub fn create_space_at(
        &self,
        install_token: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<(), CoreError> {
        let (token, relay) = if install_token.contains("://") {
            let link = tomi_core::parse_setup_link(install_token)
                .ok_or_else(|| CoreError::Pairing("setup_link_invalid".into()))?;
            let typed = relay_url.map(str::trim).filter(|u| !u.is_empty());
            if let Some(typed) = typed {
                if !typed
                    .trim_end_matches('/')
                    .eq_ignore_ascii_case(&link.relay_url)
                {
                    return Err(CoreError::Pairing("relay_mismatch".into()));
                }
            }
            (Zeroizing::new(link.token), Some(link.relay_url))
        } else {
            (
                Zeroizing::new(install_token.trim().to_owned()),
                relay_url.map(str::to_owned),
            )
        };
        let _slot = self.reserve()?;
        self.engine
            .create_space(&token, device_name, relay.as_deref())
            .map_err(map_input_failure)?;
        (self.hooks.provisioned)();
        self.changed();
        Ok(())
    }

    /// A member shows a code (A1 to A4), then the pairing advances on its own thread. Blocks
    /// until the code can be shown.
    pub fn start_invite(self: &Arc<Self>) -> Result<PairingView, CoreError> {
        let slot = self.reserve()?;
        let inv = self
            .engine
            .start_invitation()
            .map_err(map_pairing_failure)?;
        let Some(session) = self.invite_screen(&*inv, false) else {
            inv.cancel();
            return Err(CoreError::Pairing("failed".into()));
        };
        let signal = Arc::new(Signal::default());
        let generation = self.install(Role::Invite, session, Handle::Invite(inv.clone()), &signal);
        drop(slot);
        // What the screen shows first, before the driver can move it.
        let first = self.view();
        let host = Arc::clone(self);
        let driver = std::thread::Builder::new()
            .name("tomi-pairing".into())
            .spawn(move || host.drive_invite(generation, inv, &signal));
        if driver.is_err() {
            self.cancel();
            return Err(CoreError::Other("thread".into()));
        }
        self.changed();
        Ok(first)
    }

    /// A new device joins with a code or a link (N0 to N2), then the pairing advances on its
    /// own thread. The code is read here first: a typo costs nothing and consumes nothing.
    #[allow(dead_code)] // The app goes through the `_at` form; this one is for the tests.
    pub fn start_join(
        self: &Arc<Self>,
        code_or_link: &str,
        device_name: &str,
    ) -> Result<PairingView, CoreError> {
        self.start_join_at(code_or_link, device_name, None)
    }

    /// [`start_join`](Self::start_join) at a relay: `relay_url` is the "Relais" field, absent
    /// when this PC has its relay or the link names one. A relay that was read is not trusted:
    /// the core points the transport at it for this session only and the verification screen
    /// shows its authority in full; nothing is stored before the user confirmed.
    pub fn start_join_at(
        self: &Arc<Self>,
        code_or_link: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<PairingView, CoreError> {
        if self.engine.is_provisioned() {
            return Err(CoreError::Pairing("already_paired".into()));
        }
        // The same reading the core makes before the claim, so that the screen can say what is
        // wrong: a link that is not Tomi's (any valid https relay is fine, see `pairing-v1.md`
        // §4.4), or a code that is not a code. The relay is the core's to judge.
        let readable = if code_or_link.contains("://") {
            tomi_core::parse_pair_link(code_or_link).is_some()
        } else {
            PairingCode::parse(code_or_link).is_ok()
        };
        if !readable {
            let code = if code_or_link.contains("://") {
                "link_invalid"
            } else {
                "code_invalid"
            };
            return Err(CoreError::Pairing(code.into()));
        }
        let slot = self.reserve()?;
        let joining = self
            .engine
            .join_space(code_or_link, device_name, relay_url)
            .map_err(map_input_failure)?;
        let signal = Arc::new(Signal::default());
        let generation = self.install(
            Role::Join,
            PairingSession::new(PairingStage::Waiting),
            Handle::Join(joining.clone()),
            &signal,
        );
        drop(slot);
        let first = self.view();
        let host = Arc::clone(self);
        let driver = std::thread::Builder::new()
            .name("tomi-pairing".into())
            .spawn(move || host.drive_join(generation, joining, &signal));
        if driver.is_err() {
            self.cancel();
            return Err(CoreError::Other("thread".into()));
        }
        self.changed();
        Ok(first)
    }

    fn install(
        &self,
        role: Role,
        session: PairingSession,
        handle: Handle,
        signal: &Arc<Signal>,
    ) -> u64 {
        let mut st = lock(&self.state);
        st.generation += 1;
        let generation = st.generation;
        st.notice = None;
        st.live = Some(Live {
            generation,
            role,
            session,
            handle,
            signal: signal.clone(),
        });
        generation
    }

    /// The screen of a code that was just obtained.
    fn invite_screen(&self, inv: &dyn Invitation, renewed: bool) -> Option<PairingSession> {
        let (code, link) = (inv.code()?, inv.link()?);
        let remaining = inv.expires_at_ms() - self.engine.server_now_ms();
        let mut s = PairingSession::new(PairingStage::Waiting);
        s.qr = qr::encode(&link);
        s.code = Some(SecretText::new(code));
        s.link = Some(SecretText::new(link));
        s.expires_at = Some(now_ms().saturating_add(remaining));
        s.renewed = renewed;
        Some(s)
    }

    // ------------------------------------------------------------ what the user decides

    /// The user compared the codes and agrees. Taken by the driver between two polls.
    pub fn confirm(&self, import_history: bool) -> Result<(), CoreError> {
        let st = lock(&self.state);
        let Some(live) = st.live.as_ref() else {
            return Err(CoreError::Pairing("no_session".into()));
        };
        if live.session.stage != PairingStage::Verify {
            return Err(CoreError::Pairing("not_ready".into()));
        }
        live.signal.confirm(import_history);
        Ok(())
    }

    /// The user refuses, cancels, or leaves the screen; also the way out of a finished
    /// pairing. The screen is clear at once; the channel is closed on a short thread. The
    /// receiver is told when that is done.
    pub fn cancel(&self) -> Option<mpsc::Receiver<()>> {
        let live = lock(&self.state).live.take()?;
        live.signal.cancel();
        self.changed();
        if live.session.stage.is_over() {
            return None;
        }
        let (done, finished) = mpsc::channel();
        let closing = std::thread::Builder::new()
            .name("tomi-pairing-cancel".into())
            .spawn(move || {
                live.handle.cancel();
                let _ = done.send(());
            });
        closing.ok().map(|_| finished)
    }

    /// The screen was left (another tab, the window closed). A code on screen, or codes being
    /// compared, are abandoned (`pairing-v1.md` §11.2). A pairing that has moved on to adding
    /// the device goes on in the background: nothing secret is shown any more, and cancelling
    /// would only strand a device the relay may already have registered; a reopened screen
    /// finds it again. A finished pairing that was on screen is closed. The decision reads the
    /// core, not the screen, because a driver between two polls can leave the screen a step
    /// behind (and folds two join phases into one `Provisioning`).
    pub fn leave(&self) -> Option<mpsc::Receiver<()>> {
        let keep = {
            let st = lock(&self.state);
            st.live.as_ref().is_some_and(|l| {
                matches!(l.handle.progress(), Progress::Committed | Progress::Sealed)
            })
        };
        if keep {
            None
        } else {
            self.cancel()
        }
    }

    /// Forgets a finished pairing and the notice of a settled interruption.
    pub fn dismiss(&self) {
        let removed = {
            let mut st = lock(&self.state);
            st.notice = None;
            match &st.live {
                Some(l) if l.session.stage.is_over() => st.live.take().is_some(),
                _ => false,
            }
        };
        if removed {
            self.changed();
        }
    }

    /// The app quits: the driver threads stop and the pairing in progress is cancelled (the
    /// channel closed if the relay answers within a moment). The one exception is a new device
    /// whose verified batch is already sealed in the store: the next start settles it with the
    /// relay (`pairing-v1.md` §11.4), which cancelling would erase. A join that is still
    /// provisioning (nothing sealed yet) is cancelled like the rest. The decision reads the
    /// core, not the screen.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let keeps_its_batch = lock(&self.state)
            .live
            .as_ref()
            .is_some_and(|l| l.handle.progress() == Progress::Sealed);
        if keeps_its_batch {
            return;
        }
        if let Some(done) = self.cancel() {
            let _ = done.recv_timeout(Duration::from_millis(1500));
        }
    }

    // ------------------------------------------------------------ publishing

    /// Applies `f` to the screen of the session `generation`, unless that session is over or
    /// was replaced (a driver that is late never touches the screen of the next session).
    fn publish(&self, generation: u64, f: impl FnOnce(&mut PairingSession)) {
        let changed = {
            let mut st = lock(&self.state);
            let Some(live) = st.live.as_mut().filter(|l| l.generation == generation) else {
                return;
            };
            if live.session.stage.is_over() {
                return;
            }
            let before = live.session.clone();
            f(&mut live.session);
            live.session != before
        };
        if changed {
            self.changed();
        }
    }

    /// The pairing ended with `e`: cancelled by the other device, or failed.
    fn fail(&self, generation: u64, e: &Failure) {
        let stage = if matches!(e, Failure::PairingCancelled) {
            PairingStage::Cancelled
        } else {
            PairingStage::Failed
        };
        let error = app_error(e);
        self.publish(generation, |s| {
            s.stage = stage;
            s.error = Some(error);
            s.offline = false;
            s.wipe_secrets();
        });
    }

    fn offline(&self, generation: u64, e: &Failure) {
        let offline = is_offline(e);
        self.publish(generation, |s| s.offline = offline);
    }

    // ------------------------------------------------------------ the inviter

    fn refresh_invite(&self, generation: u64, inv: &dyn Invitation) {
        let stage = invite_stage(inv.phase());
        let prompt = inv.prompt();
        self.publish(generation, |s| {
            s.offline = false;
            // The terminal stages are published by whoever ends the pairing, with their data.
            if !stage.is_over() {
                s.stage = stage;
            }
            if let Some(p) = prompt {
                s.sas = Some(SecretText::new(p.sas));
                s.peer = Some(PairingPeerView {
                    name: p.peer.name,
                    platform: p.peer.platform,
                });
            }
            if stage != PairingStage::Waiting {
                s.code = None;
                s.link = None;
                s.qr = None;
                s.expires_at = None;
            }
            if stage != PairingStage::Verify {
                s.sas = None;
            }
        });
    }

    fn drive_invite(self: Arc<Self>, generation: u64, first: Arc<dyn Invitation>, signal: &Signal) {
        let mut inv = first;
        let mut renewals = 0;
        let mut backoff = Backoff::new(self.timing.retry_first, self.timing.retry_max);
        loop {
            if self.stopping() || signal.is_cancelled() || !self.alive(generation) {
                return;
            }
            let phase = inv.phase();
            let step = match phase {
                // Ended from outside (a cancellation); nothing is left to show.
                InvitePhase::Cancelled | InvitePhase::Failed => return,
                // The code is shown: the core waits for the new device (long polls).
                InvitePhase::AwaitPeer => inv.wait_for_peer().map(|_| ()),
                InvitePhase::Verify => match signal.wait(self.timing.verify_tick) {
                    Woken::Cancel => return,
                    Woken::Confirm(_) => inv.confirm(),
                    // A glance at the channel: the other device may have walked away.
                    Woken::Nothing => inv.poll(0).map(|_| ()),
                },
                // Both users agreed: the core exchanges the batch and registers the device.
                InvitePhase::Provisioning | InvitePhase::Done => match inv.wait_done() {
                    Ok(outcome) => {
                        self.invite_done(generation, &outcome);
                        return;
                    }
                    Err(e) => Err(e),
                },
            };
            let e = match step {
                Ok(()) => {
                    backoff.reset();
                    self.refresh_invite(generation, &*inv);
                    continue;
                }
                Err(e) => e,
            };
            if signal.is_cancelled() {
                return;
            }
            if is_transient(&e) {
                // Publish the phase the core reached before pausing, so the screen is not a
                // step behind while offline (`pairing-v1.md` §11.2).
                self.refresh_invite(generation, &*inv);
                self.offline(generation, &e);
                if !signal.pause(backoff.pause()) {
                    return;
                }
                continue;
            }
            // Nobody compared anything yet: a new code is better than an error.
            if phase == InvitePhase::AwaitPeer && is_renewable(&e) && renewals < MAX_RENEWALS {
                renewals += 1;
                let renewed = !matches!(e, Failure::PairingExpired);
                match self.renew(generation, renewed, signal) {
                    Ok(Some(next)) => {
                        inv = next;
                        backoff.reset();
                        continue;
                    }
                    Ok(None) => return,
                    Err(e2) => {
                        self.fail(generation, &e2);
                        return;
                    }
                }
            }
            self.fail(generation, &e);
            return;
        }
    }

    /// Opens a new channel and shows its code in place of the one that ran out. `Ok(None)`
    /// if the user cancelled meanwhile.
    fn renew(
        &self,
        generation: u64,
        renewed: bool,
        signal: &Signal,
    ) -> Result<Option<Arc<dyn Invitation>>, Failure> {
        // No channel is opened for a screen that was closed meanwhile.
        if signal.is_cancelled() {
            return Ok(None);
        }
        let next = self.engine.start_invitation()?;
        let screen = self.invite_screen(&*next, renewed);
        let attached = {
            let mut st = lock(&self.state);
            match (
                &screen,
                st.live.as_mut().filter(|l| l.generation == generation),
            ) {
                (Some(screen), Some(live)) if !signal.is_cancelled() => {
                    live.session = screen.clone();
                    // The next `cancel` must reach the code that is on screen.
                    live.handle = Handle::Invite(next.clone());
                    true
                }
                _ => false,
            }
        };
        if !attached {
            next.cancel();
            return if signal.is_cancelled() {
                Ok(None)
            } else {
                Err(Failure::PairingFailed)
            };
        }
        self.changed();
        Ok(Some(next))
    }

    fn invite_done(&self, generation: u64, outcome: &InviteOutcome) {
        self.publish(generation, |s| {
            s.stage = PairingStage::Done;
            s.offline = false;
            s.wipe_secrets();
            s.peer = Some(PairingPeerView {
                name: outcome.name.clone(),
                platform: outcome.platform.clone(),
            });
            s.history = outcome
                .history
                .as_ref()
                .map(|h: &HistoryAnnouncement| history_view(h.items, h.bytes));
        });
        // The export of the history announced to the new device is a task that the next
        // synchronisation starts (`InviteOutcome::history`).
        if outcome.history.is_some() {
            self.engine.sync();
        }
    }

    // ------------------------------------------------------------ the new device

    fn refresh_join(&self, generation: u64, joining: &dyn Joining) {
        let stage = join_stage(joining.phase());
        let prompt = joining.prompt();
        self.publish(generation, |s| {
            s.offline = false;
            if !stage.is_over() {
                s.stage = stage;
            }
            if let Some(p) = prompt {
                s.sas = Some(SecretText::new(p.sas));
                // In full, next to the code: the relay may come from a link (§4.4).
                s.relay_authority = Some(p.relay_authority);
                s.peer = Some(PairingPeerView {
                    name: p.inviter.name,
                    platform: p.inviter.platform,
                });
                s.member_count = p.member_count;
                s.history = p
                    .history
                    .map(|h: HistoryOffer| history_view(h.items, h.bytes));
            }
            if stage != PairingStage::Verify {
                s.sas = None;
            }
        });
    }

    fn drive_join(self: Arc<Self>, generation: u64, joining: Arc<dyn Joining>, signal: &Signal) {
        let mut backoff = Backoff::new(self.timing.retry_first, self.timing.retry_max);
        loop {
            if self.stopping() || signal.is_cancelled() || !self.alive(generation) {
                return;
            }
            let step = match joining.phase() {
                JoinPhase::Cancelled | JoinPhase::Failed => return,
                JoinPhase::AwaitInviter => joining.wait_for_inviter().map(|_| ()),
                JoinPhase::Verify => match signal.wait(self.timing.verify_tick) {
                    Woken::Cancel => return,
                    Woken::Confirm(answer) => joining.confirm(answer),
                    Woken::Nothing => joining.poll(0).map(|_| ()),
                },
                // Confirmed here: the batch is verified and sealed, the inviter registers this
                // device, and the core provisions it (N10 to N13).
                JoinPhase::Provisioning | JoinPhase::Completing | JoinPhase::Done => {
                    match joining.wait_done() {
                        Ok(outcome) => {
                            self.join_done(generation, &outcome);
                            return;
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            match step {
                Ok(()) => {
                    backoff.reset();
                    self.refresh_join(generation, &*joining);
                }
                Err(_) if signal.is_cancelled() => return,
                Err(e) if is_transient(&e) => {
                    // Publish the phase the core reached before pausing, so the screen is not a
                    // step behind while offline (`pairing-v1.md` §11.2, §11.4).
                    self.refresh_join(generation, &*joining);
                    self.offline(generation, &e);
                    if !signal.pause(backoff.pause()) {
                        return;
                    }
                }
                Err(e) => {
                    self.fail(generation, &e);
                    return;
                }
            }
        }
    }

    fn join_done(&self, generation: u64, outcome: &JoinOutcome) {
        self.publish(generation, |s| {
            s.stage = PairingStage::Done;
            s.offline = false;
            s.wipe_secrets();
            s.history = outcome
                .history
                .as_ref()
                .map(|h| history_view(h.items, h.bytes));
        });
        // Provisioned: the doorbell and the rest of what a paired PC runs.
        (self.hooks.provisioned)();
    }

    // ------------------------------------------------------------ resumption at start

    /// Settles a pairing that a stop interrupted (`pairing-v1.md` §11.4, §10.2) on its own
    /// thread, without blocking the start. A relay that does not answer is asked again later.
    pub fn spawn_resume(self: &Arc<Self>) {
        if self.engine.is_provisioned() || !self.engine.has_pending() {
            return;
        }
        let host = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("tomi-pairing-resume".into())
            .spawn(move || host.resume());
    }

    fn resume(&self) {
        let mut backoff = Backoff::new(self.timing.resume_first, self.timing.resume_max);
        for _ in 0..RESUME_ATTEMPTS {
            if self.stopping() || self.engine.is_provisioned() || !self.engine.has_pending() {
                return;
            }
            match self.engine.resume_pairing() {
                Ok(ResumeOutcome::Provisioned) => {
                    lock(&self.state).notice = Some(ResumeNotice::Provisioned);
                    (self.hooks.provisioned)();
                    self.changed();
                    return;
                }
                Ok(ResumeOutcome::Discarded) => {
                    lock(&self.state).notice = Some(ResumeNotice::Discarded);
                    self.changed();
                    return;
                }
                // Nothing to settle, or the keys of a founder whose space does not exist (yet):
                // they stay for `create_space`.
                Ok(ResumeOutcome::Nothing) => return,
                Err(e) if is_transient(&e) => {
                    if !self.sleep_unless_stopping(backoff.pause()) {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    }

    fn sleep_unless_stopping(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            if self.stopping() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100).min(d));
        }
        !self.stopping()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use tomi_core::runtime::PairingPeer;

    const CODE: &str = "4821-crayon-renard";
    const LINK: &str = "https://relay.example.org/l/pair#4821-crayon-renard";
    const SAS: &str = "482 913";

    fn quick() -> Timing {
        Timing {
            verify_tick: Duration::from_millis(15),
            retry_first: Duration::from_millis(5),
            retry_max: Duration::from_millis(20),
            resume_first: Duration::from_millis(5),
            resume_max: Duration::from_millis(20),
        }
    }

    /// Waits (up to 5 s) for `cond`; the drivers run on their own threads.
    fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if cond() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting for: {what}");
    }

    // ------------------------------------------------------------ scripted sessions

    /// What the next `poll` of a scripted session does.
    enum Step {
        /// The relay has nothing new.
        Hold,
        /// The session moves to this phase.
        Phase(InvitePhase),
        Fail(Failure),
    }

    struct InviteScript {
        phase: InvitePhase,
        steps: VecDeque<Step>,
        prompt: VerifyPrompt,
        outcome: InviteOutcome,
        polls: Vec<u32>,
        confirms: usize,
        cancels: usize,
    }

    struct FakeInvite(Mutex<InviteScript>, String);

    impl FakeInvite {
        fn new(steps: Vec<Step>) -> Arc<Self> {
            Self::coded(CODE, steps)
        }

        /// An invitation that shows `code`: several of them can be told apart on screen.
        fn coded(code: &str, steps: Vec<Step>) -> Arc<Self> {
            Arc::new(Self(
                Mutex::new(InviteScript {
                    phase: InvitePhase::AwaitPeer,
                    steps: steps.into(),
                    prompt: VerifyPrompt {
                        sas: SAS.into(),
                        peer: PairingPeer {
                            name: "Pixel".into(),
                            platform: "android".into(),
                        },
                    },
                    outcome: InviteOutcome {
                        device_id: "d".into(),
                        name: "Pixel".into(),
                        platform: "android".into(),
                        history: None,
                    },
                    polls: Vec::new(),
                    confirms: 0,
                    cancels: 0,
                }),
                code.to_owned(),
            ))
        }

        fn with_history(self: Arc<Self>, items: u64, bytes: u64) -> Arc<Self> {
            lock(&self.0).outcome.history = Some(HistoryAnnouncement {
                transfer_id: "t".into(),
                items,
                bytes,
            });
            self
        }

        fn confirms(&self) -> usize {
            lock(&self.0).confirms
        }
        fn cancels(&self) -> usize {
            lock(&self.0).cancels
        }
        fn polls(&self) -> Vec<u32> {
            lock(&self.0).polls.clone()
        }

        /// One step of the script, as one round trip to the relay would take it. A session
        /// that was cancelled answers as the core's does.
        fn advance(&self) -> Result<InvitePhase, Failure> {
            let mut s = lock(&self.0);
            if s.phase == InvitePhase::Cancelled {
                return Err(Failure::PairingCancelled);
            }
            match s.steps.pop_front() {
                Some(Step::Phase(p)) => {
                    s.phase = p;
                    Ok(p)
                }
                Some(Step::Fail(e)) => {
                    if !is_transient(&e) {
                        s.phase = if e == Failure::PairingCancelled {
                            InvitePhase::Cancelled
                        } else {
                            InvitePhase::Failed
                        };
                    }
                    Err(e)
                }
                Some(Step::Hold) | None => {
                    let phase = s.phase;
                    drop(s);
                    std::thread::sleep(Duration::from_millis(3));
                    Ok(phase)
                }
            }
        }
    }

    impl Invitation for FakeInvite {
        fn code(&self) -> Option<String> {
            Some(self.1.clone())
        }
        fn link(&self) -> Option<String> {
            Some(format!("https://relay.example.org/l/pair#{}", self.1))
        }
        fn expires_at_ms(&self) -> i64 {
            1_120_000
        }
        fn phase(&self) -> InvitePhase {
            lock(&self.0).phase
        }
        fn prompt(&self) -> Option<VerifyPrompt> {
            let s = lock(&self.0);
            matches!(s.phase, InvitePhase::Verify | InvitePhase::Provisioning)
                .then(|| s.prompt.clone())
        }
        fn wait_for_peer(&self) -> Result<VerifyPrompt, Failure> {
            loop {
                if let Some(p) = self.prompt() {
                    return Ok(p);
                }
                self.advance()?;
            }
        }
        fn poll(&self, wait_secs: u32) -> Result<InvitePhase, Failure> {
            lock(&self.0).polls.push(wait_secs);
            self.advance()
        }
        fn confirm(&self) -> Result<(), Failure> {
            let mut s = lock(&self.0);
            s.confirms += 1;
            s.phase = InvitePhase::Provisioning;
            Ok(())
        }
        fn wait_done(&self) -> Result<InviteOutcome, Failure> {
            loop {
                if self.phase() == InvitePhase::Done {
                    return Ok(lock(&self.0).outcome.clone());
                }
                self.advance()?;
            }
        }
        fn cancel(&self) {
            let mut s = lock(&self.0);
            s.cancels += 1;
            if s.phase != InvitePhase::Done {
                s.phase = InvitePhase::Cancelled;
            }
        }
    }

    enum JoinStep {
        Hold,
        Phase(JoinPhase),
        Fail(Failure),
    }

    struct JoinScript {
        phase: JoinPhase,
        steps: VecDeque<JoinStep>,
        prompt: JoinPrompt,
        outcome: JoinOutcome,
        answers: Vec<bool>,
        cancels: usize,
    }

    struct FakeJoin(Mutex<JoinScript>);

    impl FakeJoin {
        fn new(steps: Vec<JoinStep>, offer: Option<HistoryOffer>) -> Arc<Self> {
            Arc::new(Self(Mutex::new(JoinScript {
                phase: JoinPhase::AwaitInviter,
                steps: steps.into(),
                prompt: JoinPrompt {
                    relay_authority: "relay.example.org".into(),
                    sas: SAS.into(),
                    inviter: PairingPeer {
                        name: "PC-Bureau".into(),
                        platform: "windows".into(),
                    },
                    member_count: Some(3),
                    history: offer,
                },
                outcome: JoinOutcome {
                    device_id: "d".into(),
                    space_id: "s".into(),
                    history: None,
                },
                answers: Vec::new(),
                cancels: 0,
            })))
        }

        fn answers(&self) -> Vec<bool> {
            lock(&self.0).answers.clone()
        }
        fn cancels(&self) -> usize {
            lock(&self.0).cancels
        }

        fn advance(&self) -> Result<JoinPhase, Failure> {
            let mut s = lock(&self.0);
            if s.phase == JoinPhase::Cancelled {
                return Err(Failure::PairingCancelled);
            }
            match s.steps.pop_front() {
                Some(JoinStep::Phase(p)) => {
                    s.phase = p;
                    Ok(p)
                }
                Some(JoinStep::Fail(e)) => {
                    if !is_transient(&e) {
                        s.phase = if e == Failure::PairingCancelled {
                            JoinPhase::Cancelled
                        } else {
                            JoinPhase::Failed
                        };
                    }
                    Err(e)
                }
                Some(JoinStep::Hold) | None => {
                    let phase = s.phase;
                    drop(s);
                    std::thread::sleep(Duration::from_millis(3));
                    Ok(phase)
                }
            }
        }
    }

    impl Joining for FakeJoin {
        fn phase(&self) -> JoinPhase {
            lock(&self.0).phase
        }
        fn prompt(&self) -> Option<JoinPrompt> {
            let s = lock(&self.0);
            matches!(s.phase, JoinPhase::Verify | JoinPhase::Provisioning).then(|| s.prompt.clone())
        }
        fn wait_for_inviter(&self) -> Result<JoinPrompt, Failure> {
            loop {
                if let Some(p) = self.prompt() {
                    return Ok(p);
                }
                self.advance()?;
            }
        }
        fn poll(&self, _wait_secs: u32) -> Result<JoinPhase, Failure> {
            self.advance()
        }
        fn confirm(&self, import_history: bool) -> Result<(), Failure> {
            let mut s = lock(&self.0);
            s.answers.push(import_history);
            s.phase = JoinPhase::Provisioning;
            Ok(())
        }
        fn wait_done(&self) -> Result<JoinOutcome, Failure> {
            loop {
                if self.phase() == JoinPhase::Done {
                    return Ok(lock(&self.0).outcome.clone());
                }
                self.advance()?;
            }
        }
        fn cancel(&self) {
            let mut s = lock(&self.0);
            s.cancels += 1;
            if s.phase != JoinPhase::Done {
                s.phase = JoinPhase::Cancelled;
            }
        }
    }

    // ------------------------------------------------------------ the engine, scripted

    #[derive(Default)]
    struct FakeEngine {
        provisioned: AtomicBool,
        pending: AtomicBool,
        hello: Mutex<Option<bool>>,
        hellos: AtomicUsize,
        invitations: Mutex<VecDeque<Result<Arc<FakeInvite>, Failure>>>,
        invitation_calls: AtomicUsize,
        joinings: Mutex<VecDeque<Result<Arc<FakeJoin>, Failure>>>,
        join_calls: Mutex<Vec<String>>,
        /// The `relay_url` each call to join_space / create_space carried.
        join_relays: Mutex<Vec<Option<String>>>,
        create_relays: Mutex<Vec<Option<String>>>,
        creations: Mutex<VecDeque<Result<(), Failure>>>,
        created: Mutex<Vec<(String, String)>>,
        resumes: Mutex<VecDeque<Result<ResumeOutcome, Failure>>>,
        resume_calls: AtomicUsize,
        syncs: AtomicUsize,
    }

    impl PairingEngine for FakeEngine {
        fn is_provisioned(&self) -> bool {
            self.provisioned.load(Ordering::SeqCst)
        }
        fn has_pending(&self) -> bool {
            self.pending.load(Ordering::SeqCst)
        }
        fn server_now_ms(&self) -> i64 {
            1_000_000
        }
        fn relay_authority(&self) -> Option<String> {
            Some("relay.example.org".into())
        }
        fn space_initialized(&self) -> Option<bool> {
            self.hellos.fetch_add(1, Ordering::SeqCst);
            *lock(&self.hello)
        }
        fn start_invitation(&self) -> Result<Arc<dyn Invitation>, Failure> {
            self.invitation_calls.fetch_add(1, Ordering::SeqCst);
            match lock(&self.invitations).pop_front() {
                Some(Ok(i)) => Ok(i),
                Some(Err(e)) => Err(e),
                None => Err(Failure::Network {
                    reason: "unscripted".into(),
                }),
            }
        }
        fn join_space(
            &self,
            code_or_link: &str,
            device_name: &str,
            relay_url: Option<&str>,
        ) -> Result<Arc<dyn Joining>, Failure> {
            lock(&self.join_calls).push(format!("{code_or_link}|{device_name}"));
            lock(&self.join_relays).push(relay_url.map(str::to_owned));
            match lock(&self.joinings).pop_front() {
                Some(Ok(j)) => Ok(j),
                Some(Err(e)) => Err(e),
                None => Err(Failure::Network {
                    reason: "unscripted".into(),
                }),
            }
        }
        fn create_space(
            &self,
            install_token: &str,
            device_name: &str,
            relay_url: Option<&str>,
        ) -> Result<(), Failure> {
            lock(&self.created).push((install_token.into(), device_name.into()));
            lock(&self.create_relays).push(relay_url.map(str::to_owned));
            lock(&self.creations).pop_front().unwrap_or(Ok(()))
        }
        fn resume_pairing(&self) -> Result<ResumeOutcome, Failure> {
            self.resume_calls.fetch_add(1, Ordering::SeqCst);
            lock(&self.resumes)
                .pop_front()
                .unwrap_or(Ok(ResumeOutcome::Nothing))
        }
        fn sync(&self) {
            self.syncs.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Rig {
        host: Arc<PairingHost>,
        engine: Arc<FakeEngine>,
        changed: Arc<AtomicUsize>,
        provisioned: Arc<AtomicUsize>,
    }

    impl Rig {
        fn new() -> Self {
            Self::with(quick())
        }

        fn with(timing: Timing) -> Self {
            let engine = Arc::new(FakeEngine::default());
            let changed = Arc::new(AtomicUsize::new(0));
            let provisioned = Arc::new(AtomicUsize::new(0));
            let (c, p) = (changed.clone(), provisioned.clone());
            let host = PairingHost::new(
                engine.clone(),
                Hooks {
                    changed: Arc::new(move || {
                        c.fetch_add(1, Ordering::SeqCst);
                    }),
                    provisioned: Arc::new(move || {
                        p.fetch_add(1, Ordering::SeqCst);
                    }),
                },
                timing,
            );
            Self {
                host,
                engine,
                changed,
                provisioned,
            }
        }

        fn queue_invitation(&self, i: &Arc<FakeInvite>) {
            lock(&self.engine.invitations).push_back(Ok(i.clone()));
        }

        fn queue_joining(&self, j: &Arc<FakeJoin>) {
            lock(&self.engine.joinings).push_back(Ok(j.clone()));
        }

        fn session(&self) -> PairingSession {
            self.host.view().session().cloned().expect("a session")
        }

        fn stage(&self) -> Option<PairingStage> {
            self.host.view().session().map(|s| s.stage)
        }

        fn changes(&self) -> usize {
            self.changed.load(Ordering::SeqCst)
        }
    }

    // ------------------------------------------------------------ the inviter

    #[test]
    fn an_invitation_shows_its_code_then_the_codes_then_the_new_device() {
        let rig = Rig::new();
        // The script ends at the codes: what comes after depends on the click.
        let inv = FakeInvite::new(vec![Step::Hold, Step::Phase(InvitePhase::Verify)])
            .with_history(812, 340_000_000);
        rig.queue_invitation(&inv);

        // A1 to A4: the code, its link, the QR code of that very link, the time it runs out.
        let PairingView::Invite(first) = rig.host.start_invite().unwrap() else {
            panic!("an invitation")
        };
        assert_eq!(first.stage, PairingStage::Waiting);
        assert_eq!(first.code.as_ref().unwrap().expose(), CODE);
        assert_eq!(first.link.as_ref().unwrap().expose(), LINK);
        assert!(first.qr.is_some());
        // 120 s from now on this PC's clock (the relay clock reads 1 000 000, the code ends at
        // 1 120 000).
        let left = first.expires_at.unwrap() - now_ms();
        assert!((118_000..=120_000).contains(&left), "{left}");
        assert!(!first.renewed && first.sas.is_none() && first.peer.is_none());

        // A7: the codes to compare and the new device. The code of the invitation is gone.
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        let verify = rig.session();
        assert_eq!(verify.sas.as_ref().unwrap().expose(), SAS);
        let peer = verify.peer.clone().unwrap();
        assert_eq!(
            (peer.name.as_str(), peer.platform.as_str()),
            ("Pixel", "android")
        );
        assert!(verify.code.is_none() && verify.link.is_none() && verify.qr.is_none());
        // The channel is glanced at while the user compares, never left alone for long.
        eventually("a glance at the channel", || inv.polls().contains(&0));

        // A8: the user agrees; the driver takes it between two polls.
        rig.host.confirm(true).unwrap();
        eventually("the confirmation", || inv.confirms() == 1);
        eventually("the provisioning", || {
            rig.stage() == Some(PairingStage::Provisioning)
        });
        // The batch is out and the new device is registered.
        lock(&inv.0)
            .steps
            .extend([Step::Hold, Step::Phase(InvitePhase::Done)]);
        eventually("the end", || rig.stage() == Some(PairingStage::Done));
        let done = rig.session();
        assert_eq!(done.peer.as_ref().unwrap().name, "Pixel");
        // The history the new device asked for is in preparation, and a sync starts its export.
        assert_eq!(
            done.history,
            Some(HistoryView {
                items: 812,
                bytes: 340_000_000
            })
        );
        eventually("the sync", || rig.engine.syncs.load(Ordering::SeqCst) == 1);
        // Nothing secret is left on the screen, and an inviter is already provisioned.
        assert!(done.sas.is_none() && done.code.is_none() && done.qr.is_none());
        assert_eq!(rig.provisioned.load(Ordering::SeqCst), 0);
        assert!(rig.changes() >= 4);
    }

    #[test]
    fn without_a_history_nothing_is_synced_and_nothing_is_announced() {
        let rig = Rig::new();
        let inv = FakeInvite::new(vec![Step::Phase(InvitePhase::Verify)]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host.confirm(false).unwrap();
        lock(&inv.0).steps.push_back(Step::Phase(InvitePhase::Done));
        eventually("the end", || rig.stage() == Some(PairingStage::Done));
        assert!(rig.session().history.is_none());
        assert_eq!(rig.engine.syncs.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_click_is_taken_at_once_even_when_the_channel_is_looked_at_rarely() {
        // The driver sleeps on the intent: it does not wait for its tick to see the click.
        let rig = Rig::with(Timing {
            verify_tick: Duration::from_secs(30),
            ..quick()
        });
        let inv = FakeInvite::new(vec![Step::Phase(InvitePhase::Verify)]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        let clicked = Instant::now();
        rig.host.confirm(true).unwrap();
        eventually("the confirmation", || inv.confirms() == 1);
        assert!(clicked.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_user_cannot_confirm_what_is_not_on_screen() {
        let rig = Rig::new();
        assert_eq!(
            rig.host.confirm(true),
            Err(CoreError::Pairing("no_session".into()))
        );
        let inv = FakeInvite::new(vec![]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        // The code is shown, nobody answered: there is nothing to confirm yet.
        assert_eq!(
            rig.host.confirm(true),
            Err(CoreError::Pairing("not_ready".into()))
        );
        assert_eq!(inv.confirms(), 0);
        rig.host.cancel();
    }

    #[test]
    fn cancelling_clears_the_screen_and_closes_the_channel() {
        let rig = Rig::new();
        let inv = FakeInvite::new(vec![]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        assert_eq!(rig.stage(), Some(PairingStage::Waiting));

        let closed = rig.host.cancel().expect("the channel is being closed");
        // The screen is clear before the network is touched.
        assert_eq!(rig.host.view(), PairingView::None);
        closed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(inv.cancels(), 1);
        // The driver stops; nothing is polled any more.
        std::thread::sleep(Duration::from_millis(40));
        let polls = inv.polls().len();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(inv.polls().len(), polls);
        // Nothing to cancel any more.
        assert!(rig.host.cancel().is_none());
        assert_eq!(rig.host.view(), PairingView::None);
    }

    #[test]
    fn leaving_the_screen_abandons_a_code_on_show_and_codes_being_compared() {
        // The code is on screen.
        let rig = Rig::new();
        let shown = FakeInvite::new(vec![]);
        rig.queue_invitation(&shown);
        rig.host.start_invite().unwrap();
        rig.host
            .leave()
            .expect("the channel is being closed")
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(shown.cancels(), 1);
        assert_eq!(rig.host.view(), PairingView::None);

        // The codes are being compared, on either side.
        let comparing = FakeInvite::new(vec![Step::Phase(InvitePhase::Verify)]);
        rig.queue_invitation(&comparing);
        rig.host.start_invite().unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host
            .leave()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(comparing.cancels(), 1);

        let joining = FakeJoin::new(vec![JoinStep::Phase(JoinPhase::Verify)], None);
        rig.queue_joining(&joining);
        rig.host.start_join(CODE, "PC").unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host
            .leave()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(joining.cancels(), 1);
        assert_eq!(rig.host.view(), PairingView::None);
    }

    #[test]
    fn a_pairing_both_users_confirmed_goes_on_without_the_screen() {
        let rig = Rig::new();
        let joining = FakeJoin::new(vec![JoinStep::Phase(JoinPhase::Verify)], None);
        rig.queue_joining(&joining);
        rig.host.start_join(CODE, "PC").unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host.confirm(true).unwrap();
        eventually("the wait for the inviter", || {
            rig.stage() == Some(PairingStage::Provisioning)
        });

        // The window is closed: nothing is cancelled, the pairing is still there to find.
        assert!(rig.host.leave().is_none());
        assert_eq!(joining.cancels(), 0);
        assert_eq!(rig.stage(), Some(PairingStage::Provisioning));
        lock(&joining.0)
            .steps
            .push_back(JoinStep::Phase(JoinPhase::Done));
        eventually("the end, without a screen", || {
            rig.stage() == Some(PairingStage::Done)
        });
        eventually("the provisioning hook", || {
            rig.provisioned.load(Ordering::SeqCst) == 1
        });

        // A finished pairing that was on screen is closed when the screen is left.
        assert!(rig.host.leave().is_none());
        assert_eq!(rig.host.view(), PairingView::None);
    }

    #[test]
    fn quitting_keeps_only_the_sealed_batch_of_a_new_device_and_closes_everything_else() {
        // A new device whose batch is sealed (`JoinPhase::Completing`): the next start settles
        // it, so quitting leaves it alone. Cancelling would erase it (`pairing-v1.md` §11.4).
        let rig = Rig::new();
        let sealed = FakeJoin::new(
            vec![
                JoinStep::Phase(JoinPhase::Verify),
                // Confirming moves the fake to `Provisioning`; the batch gets sealed next.
                JoinStep::Phase(JoinPhase::Completing),
            ],
            None,
        );
        rig.queue_joining(&sealed);
        rig.host.start_join(CODE, "PC").unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host.confirm(false).unwrap();
        eventually("the sealed batch", || {
            sealed.phase() == JoinPhase::Completing
        });
        rig.host.shutdown();
        assert_eq!(
            sealed.cancels(),
            0,
            "cancelling would erase the sealed batch"
        );

        // A new device that only confirmed, with nothing sealed yet (`JoinPhase::Provisioning`):
        // there is no batch to settle, so quitting cancels like the rest.
        let rig = Rig::new();
        let provisioning = FakeJoin::new(vec![JoinStep::Phase(JoinPhase::Verify)], None);
        rig.queue_joining(&provisioning);
        rig.host.start_join(CODE, "PC").unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host.confirm(false).unwrap();
        eventually("the wait for the inviter", || {
            provisioning.phase() == JoinPhase::Provisioning
        });
        rig.host.shutdown();
        assert_eq!(
            provisioning.cancels(),
            1,
            "nothing is sealed yet, so there is nothing to keep"
        );

        // An inviter that confirmed is always cancelled on quit: it seals no batch of its own,
        // nobody is left to wait for it.
        let rig = Rig::new();
        let inv = FakeInvite::new(vec![Step::Phase(InvitePhase::Verify)]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host.confirm(true).unwrap();
        eventually("the provisioning", || {
            inv.phase() == InvitePhase::Provisioning
        });
        rig.host.shutdown();
        assert_eq!(inv.cancels(), 1);
    }

    #[test]
    fn leaving_and_quitting_read_the_core_phase_not_the_screen() {
        // A driver between two polls, or paused while offline, leaves the published stage a step
        // behind the core (and the screen folds `Provisioning` and `Completing` into one stage).
        // `leave` and `shutdown` must decide on the real core phase, so a custom handle that
        // reports a core phase out of step with any published stage is the sharp case.
        struct AtPhase(JoinPhase, AtomicUsize);
        impl Joining for AtPhase {
            fn phase(&self) -> JoinPhase {
                self.0
            }
            fn prompt(&self) -> Option<JoinPrompt> {
                None
            }
            fn wait_for_inviter(&self) -> Result<JoinPrompt, Failure> {
                unreachable!("not driven")
            }
            fn poll(&self, _: u32) -> Result<JoinPhase, Failure> {
                unreachable!("not driven")
            }
            fn confirm(&self, _: bool) -> Result<(), Failure> {
                unreachable!("not driven")
            }
            fn wait_done(&self) -> Result<JoinOutcome, Failure> {
                unreachable!("not driven")
            }
            fn cancel(&self) {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
        }

        // Installs a live join at `phase`, with the screen deliberately left at `Waiting`.
        fn rig_at(phase: JoinPhase) -> (Rig, Arc<AtPhase>) {
            let rig = Rig::new();
            let handle = Arc::new(AtPhase(phase, AtomicUsize::new(0)));
            let signal = Arc::new(Signal::default());
            rig.host.install(
                Role::Join,
                PairingSession::new(PairingStage::Waiting),
                Handle::Join(handle.clone()),
                &signal,
            );
            (rig, handle)
        }

        // How many times `leave` cancelled the core handle (it closes the channel on a thread).
        fn leave_cancels(phase: JoinPhase) -> usize {
            let (rig, handle) = rig_at(phase);
            if let Some(done) = rig.host.leave() {
                done.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            handle.1.load(Ordering::SeqCst)
        }

        // `shutdown` blocks on the close, so the counter is settled when it returns.
        fn shutdown_cancels(phase: JoinPhase) -> usize {
            let (rig, handle) = rig_at(phase);
            rig.host.shutdown();
            handle.1.load(Ordering::SeqCst)
        }

        // Leaving: a committed or sealed join keeps going; a join still comparing is abandoned.
        assert_eq!(leave_cancels(JoinPhase::Verify), 1);
        assert_eq!(leave_cancels(JoinPhase::Provisioning), 0);
        assert_eq!(leave_cancels(JoinPhase::Completing), 0);

        // Quitting: only a sealed batch is kept; a join merely provisioning is cancelled.
        assert_eq!(shutdown_cancels(JoinPhase::Completing), 0);
        assert_eq!(shutdown_cancels(JoinPhase::Provisioning), 1);
        assert_eq!(shutdown_cancels(JoinPhase::Verify), 1);
    }

    #[test]
    fn a_code_that_ran_out_is_replaced_until_the_limit() {
        let rig = Rig::new();
        // Three codes: the first and two renewals; the third runs out as well, and the screen
        // then waits for the user instead of using up the codes the relay allows.
        let all: Vec<_> = (0..4)
            .map(|_| FakeInvite::new(vec![Step::Fail(Failure::PairingExpired)]))
            .collect();
        for i in &all {
            rig.queue_invitation(i);
        }
        rig.host.start_invite().unwrap();
        eventually("the end", || rig.stage() == Some(PairingStage::Failed));
        assert_eq!(rig.engine.invitation_calls.load(Ordering::SeqCst), 3);
        let s = rig.session();
        assert_eq!(s.error.unwrap().code, "error.pairing_expired");
        assert!(s.code.is_none() && s.link.is_none() && s.qr.is_none());
        // Each code was the live one for a while; the last one is not cancelled twice.
        assert!(all.iter().all(|i| i.cancels() <= 1));
    }

    #[test]
    fn a_renewed_code_is_on_screen_and_the_cancel_reaches_it() {
        let rig = Rig::new();
        let first = FakeInvite::new(vec![Step::Fail(Failure::PairingExpired)]);
        let second = FakeInvite::coded("5555-abaisser-zoologie", vec![]);
        rig.queue_invitation(&first);
        rig.queue_invitation(&second);
        rig.host.start_invite().unwrap();
        eventually("the second code", || {
            rig.host
                .view()
                .session()
                .and_then(|s| s.code.as_ref())
                .is_some_and(|c| c.expose() == "5555-abaisser-zoologie")
        });
        // An expired code is replaced without a word.
        assert!(!rig.session().renewed);
        rig.host
            .cancel()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            second.cancels(),
            1,
            "the code on screen is the one that is cancelled"
        );
        assert_eq!(first.cancels(), 0);
    }

    #[test]
    fn a_code_that_led_nowhere_is_replaced_and_says_so() {
        // The other device closed the channel before anything was compared: a wrong code, or
        // a device that gave up (`pairing-v1.md` §11.1). A new code is ready.
        let rig = Rig::new();
        let first = FakeInvite::new(vec![Step::Fail(Failure::PairingCancelled)]);
        let second = FakeInvite::new(vec![]);
        rig.queue_invitation(&first);
        rig.queue_invitation(&second);
        rig.host.start_invite().unwrap();
        eventually("the new code", || {
            rig.engine.invitation_calls.load(Ordering::SeqCst) == 2 && rig.session().renewed
        });
        assert_eq!(rig.stage(), Some(PairingStage::Waiting));
        rig.host.cancel();
    }

    #[test]
    fn a_refusal_after_the_codes_were_shown_is_final() {
        let rig = Rig::new();
        let inv = FakeInvite::new(vec![
            Step::Phase(InvitePhase::Verify),
            Step::Fail(Failure::PairingCancelled),
        ]);
        let spare = FakeInvite::new(vec![]);
        rig.queue_invitation(&inv);
        rig.queue_invitation(&spare);
        rig.host.start_invite().unwrap();
        eventually("the end", || rig.stage() == Some(PairingStage::Cancelled));
        let s = rig.session();
        assert_eq!(s.error.unwrap().code, "error.pairing_cancelled");
        assert!(s.sas.is_none());
        // No new code behind the user's back.
        assert_eq!(rig.engine.invitation_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failure_the_user_must_know_about_ends_the_pairing() {
        for (failure, code) in [
            (Failure::PairingUnsupported, "error.pairing_unsupported"),
            (
                Failure::Relay {
                    status: 409,
                    code: "too_many_devices".into(),
                },
                "error.pairing_too_many_devices",
            ),
            (
                Failure::Relay {
                    status: 409,
                    code: "epoch_conflict".into(),
                },
                "error.pairing_epoch_conflict",
            ),
        ] {
            let rig = Rig::new();
            let inv = FakeInvite::new(vec![Step::Fail(failure)]);
            rig.queue_invitation(&inv);
            rig.host.start_invite().unwrap();
            eventually("the end", || rig.stage() == Some(PairingStage::Failed));
            assert_eq!(rig.session().error.unwrap().code, code);
        }
    }

    #[test]
    fn a_network_failure_keeps_the_pairing_alive_and_says_so() {
        let rig = Rig::with(Timing {
            retry_first: Duration::from_millis(40),
            retry_max: Duration::from_millis(40),
            ..quick()
        });
        let down = || Step::Fail(Failure::Network { reason: "x".into() });
        let inv = FakeInvite::new(vec![down(), down(), Step::Phase(InvitePhase::Verify)]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        eventually("the offline notice", || {
            rig.host.view().session().is_some_and(|s| s.offline)
        });
        assert_eq!(rig.stage(), Some(PairingStage::Waiting));
        eventually("the recovery", || rig.stage() == Some(PairingStage::Verify));
        assert!(!rig.session().offline);
        rig.host.cancel();
    }

    #[test]
    fn a_driver_that_is_late_never_touches_the_next_pairing() {
        let rig = Rig::new();
        let a = FakeInvite::new(vec![]);
        let b = FakeInvite::new(vec![]);
        rig.queue_invitation(&a);
        rig.queue_invitation(&b);
        rig.host.start_invite().unwrap();
        rig.host
            .cancel()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        rig.host.start_invite().unwrap();
        // The first session is long gone: whatever its driver still says is dropped.
        rig.host.publish(1, |s| s.stage = PairingStage::Failed);
        rig.host.fail(1, &Failure::PairingFailed);
        assert_eq!(rig.stage(), Some(PairingStage::Waiting));
        // And a pairing that is over stays as it ended.
        rig.host.publish(2, |s| s.stage = PairingStage::Cancelled);
        rig.host.publish(2, |s| s.stage = PairingStage::Verify);
        assert_eq!(rig.stage(), Some(PairingStage::Cancelled));
        rig.host.dismiss();
        assert_eq!(rig.host.view(), PairingView::None);
    }

    #[test]
    fn one_pairing_at_a_time_on_this_pc() {
        let rig = Rig::new();
        let a = FakeInvite::new(vec![]);
        let b = FakeInvite::new(vec![]);
        rig.queue_invitation(&a);
        rig.queue_invitation(&b);
        rig.host.start_invite().unwrap();
        assert_eq!(
            rig.host.start_invite().unwrap_err(),
            CoreError::Pairing("in_progress".into())
        );
        assert_eq!(
            rig.host.create_space("tok", "PC").unwrap_err(),
            CoreError::Pairing("in_progress".into())
        );
        // The second invitation was never opened.
        assert_eq!(rig.engine.invitation_calls.load(Ordering::SeqCst), 1);
        rig.host.cancel();
        // The way is free again.
        rig.host.start_invite().unwrap();
        rig.host.cancel();
    }

    #[test]
    fn an_invitation_that_cannot_start_leaves_the_way_free() {
        let rig = Rig::new();
        lock(&rig.engine.invitations).push_back(Err(Failure::Relay {
            status: 409,
            code: "channel_limit".into(),
        }));
        assert_eq!(
            rig.host.start_invite().unwrap_err(),
            CoreError::Pairing("busy".into())
        );
        assert_eq!(rig.host.view(), PairingView::None);
        let inv = FakeInvite::new(vec![]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        rig.host.cancel();
    }

    // ------------------------------------------------------------ the new device

    #[test]
    fn a_new_device_compares_the_codes_answers_the_history_question_and_joins() {
        let rig = Rig::new();
        let joining = FakeJoin::new(
            vec![JoinStep::Hold, JoinStep::Phase(JoinPhase::Verify)],
            Some(HistoryOffer {
                items: 812,
                bytes: 340_000_000,
            }),
        );
        lock(&joining.0).outcome.history = Some(HistoryAnnouncement {
            transfer_id: "t".into(),
            items: 812,
            bytes: 340_000_000,
        });
        rig.queue_joining(&joining);

        let PairingView::Join(first) = rig
            .host
            .start_join("4821 Crayon RENARD", "PC-Test")
            .unwrap()
        else {
            panic!("a joining")
        };
        assert_eq!(first.stage, PairingStage::Waiting);
        assert_eq!(
            lock(&rig.engine.join_calls).as_slice(),
            ["4821 Crayon RENARD|PC-Test"]
        );

        // N7, N8: the codes, the inviter, the size of the space, the history question.
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        let s = rig.session();
        assert_eq!(s.sas.as_ref().unwrap().expose(), SAS);
        assert_eq!(s.peer.as_ref().unwrap().name, "PC-Bureau");
        assert_eq!(s.member_count, Some(3));
        assert_eq!(
            s.history,
            Some(HistoryView {
                items: 812,
                bytes: 340_000_000
            })
        );

        // N9: "no, start from an empty feed" is what the core is given.
        rig.host.confirm(false).unwrap();
        eventually("the confirmation", || joining.answers() == [false]);
        eventually("the wait for the inviter", || {
            rig.stage() == Some(PairingStage::Provisioning)
        });
        lock(&joining.0).steps.extend([
            JoinStep::Hold,
            JoinStep::Phase(JoinPhase::Completing),
            JoinStep::Phase(JoinPhase::Done),
        ]);
        eventually("the end", || rig.stage() == Some(PairingStage::Done));
        let done = rig.session();
        assert!(done.sas.is_none());
        assert_eq!(
            done.history,
            Some(HistoryView {
                items: 812,
                bytes: 340_000_000
            })
        );
        // N13 is done: the doorbell and the rest start.
        eventually("the provisioning hook", || {
            rig.provisioned.load(Ordering::SeqCst) == 1
        });
    }

    #[test]
    fn the_new_device_can_say_yes_to_the_history() {
        let rig = Rig::new();
        let joining = FakeJoin::new(
            vec![JoinStep::Phase(JoinPhase::Verify)],
            Some(HistoryOffer {
                items: 1,
                bytes: 10,
            }),
        );
        rig.queue_joining(&joining);
        rig.host.start_join(CODE, "PC-Test").unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host.confirm(true).unwrap();
        eventually("the confirmation", || joining.answers() == [true]);
        rig.host.cancel();
    }

    #[test]
    fn a_wrong_code_costs_nothing_and_is_told_apart_from_a_wrong_link() {
        let rig = Rig::new();
        let joining = FakeJoin::new(vec![], None);
        rig.queue_joining(&joining);
        for bad in [
            "",
            "garbage",
            "4821-crayon",
            "0-crayon-renard",
            "4821-crayon-lune",
        ] {
            assert_eq!(
                rig.host.start_join(bad, "PC").unwrap_err(),
                CoreError::Pairing("code_invalid".into()),
                "{bad:?}"
            );
        }
        for bad in [
            "http://relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org/l/other#4821-crayon-renard",
            "https://relay.example.org/l/pair?code=4821-crayon-renard",
            "tomi-share://send#4821-crayon-renard",
        ] {
            assert_eq!(
                rig.host.start_join(bad, "PC").unwrap_err(),
                CoreError::Pairing("link_invalid".into()),
                "{bad:?}"
            );
        }
        // Not one of them reached the core, so not one consumed a code.
        assert!(lock(&rig.engine.join_calls).is_empty());
        assert_eq!(rig.host.view(), PairingView::None);
        // The links of the spec are fine, and so is a typo-free code with accents or blanks.
        for good in [
            LINK,
            // Any valid https relay: it is shown in full at the verification, not refused here.
            "https://other.example.net/l/pair#4821-crayon-renard",
            "tomi-share://pair#4821-crayon-renard",
            "  4821 Crayon-RENARD ",
            "4821-crâyon-renard",
        ] {
            let rig = Rig::new();
            rig.queue_joining(&FakeJoin::new(vec![], None));
            rig.host.start_join(good, "PC").unwrap();
            rig.host.cancel();
        }
    }

    #[test]
    fn a_paired_pc_cannot_join() {
        let rig = Rig::new();
        rig.engine.provisioned.store(true, Ordering::SeqCst);
        assert_eq!(
            rig.host.start_join(CODE, "PC").unwrap_err(),
            CoreError::Pairing("already_paired".into())
        );
        assert!(lock(&rig.engine.join_calls).is_empty());
    }

    #[test]
    fn the_relay_of_the_field_reaches_the_core_and_the_verification_shows_it_in_full() {
        let rig = Rig::new();
        rig.queue_joining(&FakeJoin::new(
            vec![JoinStep::Phase(JoinPhase::Verify)],
            None,
        ));
        rig.host
            .start_join_at(CODE, "PC", Some("https://relay.example.org"))
            .unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        assert_eq!(
            lock(&rig.engine.join_relays).as_slice(),
            [Some("https://relay.example.org".to_owned())]
        );
        // The authority is on screen with the verification code, not a shortened form of it.
        assert_eq!(
            rig.session().relay_authority.as_deref(),
            Some("relay.example.org")
        );
        rig.host.cancel();
    }

    #[test]
    fn a_relay_that_is_missing_or_wrong_is_told_before_anything_is_stored() {
        for (failure, code) in [
            (Failure::RelayNotConfigured, "relay_not_configured"),
            (
                Failure::InvalidInput {
                    reason: "relay URL: invalid host".into(),
                },
                "relay_invalid",
            ),
            (
                Failure::InvalidInput {
                    reason: "the relay URL must be https://".into(),
                },
                "relay_invalid",
            ),
            (
                Failure::InvalidInput {
                    reason: "the link and the relay entered name different relays".into(),
                },
                "relay_mismatch",
            ),
        ] {
            let rig = Rig::new();
            lock(&rig.engine.joinings).push_back(Err(failure.clone()));
            assert_eq!(
                rig.host.start_join(CODE, "PC").unwrap_err(),
                CoreError::Pairing(code.into())
            );
            assert_eq!(rig.host.view(), PairingView::None);
            lock(&rig.engine.creations).push_back(Err(failure));
            assert_eq!(
                rig.host.create_space("tok", "PC").unwrap_err(),
                CoreError::Pairing(code.into())
            );
        }
    }

    const TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";

    #[test]
    fn a_creation_link_gives_its_token_and_its_relay() {
        for link in [
            format!("https://relay.example.org/create#{TOKEN}"),
            format!("https://relay.example.org/create/{TOKEN}"),
        ] {
            let rig = Rig::new();
            rig.host.create_space(&link, "PC").unwrap();
            assert_eq!(
                lock(&rig.engine.created).as_slice(),
                [(TOKEN.to_owned(), "PC".to_owned())]
            );
            assert_eq!(
                lock(&rig.engine.create_relays).as_slice(),
                [Some("https://relay.example.org".to_owned())]
            );
        }
        // The field may repeat the relay of the link, not name another one.
        let link = format!("https://relay.example.org/create#{TOKEN}");
        let rig = Rig::new();
        rig.host
            .create_space_at(&link, "PC", Some("https://Relay.Example.org/"))
            .unwrap();
        assert_eq!(
            rig.host
                .create_space_at(&link, "PC", Some("https://evil.example"))
                .unwrap_err(),
            CoreError::Pairing("relay_mismatch".into())
        );
        // Not a creation link: nothing reached the core.
        for bad in [
            "https://relay.example.org/create#abc",
            "http://relay.example.org/create#h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns",
            "https://relay.example.org/other#h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns",
        ] {
            assert_eq!(
                rig.host.create_space(bad, "PC").unwrap_err(),
                CoreError::Pairing("setup_link_invalid".into()),
                "{bad:?}"
            );
        }
        assert_eq!(lock(&rig.engine.created).len(), 1);
    }

    #[test]
    fn a_bare_token_and_the_field_go_through_as_typed() {
        let rig = Rig::new();
        rig.host
            .create_space_at(
                &format!("  {TOKEN} "),
                "PC",
                Some("https://relay.example.org"),
            )
            .unwrap();
        assert_eq!(lock(&rig.engine.created)[0].0, TOKEN);
        assert_eq!(
            lock(&rig.engine.create_relays).as_slice(),
            [Some("https://relay.example.org".to_owned())]
        );
    }

    #[test]
    fn what_the_forms_read_of_a_text() {
        let read = |t: &str| {
            let r = read_input(t);
            (r.kind, r.relay_url, r.relay_authority)
        };
        let relay = Some("https://relay.example.org".to_owned());
        let authority = Some("relay.example.org".to_owned());
        assert_eq!(
            read(LINK),
            (InputKind::PairLink, relay.clone(), authority.clone())
        );
        assert_eq!(
            read("https://RELAY.example.org:8443/l/pair#4821-crayon-renard").2,
            Some("relay.example.org:8443".to_owned())
        );
        // The fallback link names no relay.
        assert_eq!(
            read("tomi-share://pair#4821-crayon-renard"),
            (InputKind::PairLink, None, None)
        );
        assert_eq!(
            read(&format!("https://relay.example.org/create#{TOKEN}")),
            (InputKind::SetupLink, relay, authority)
        );
        assert_eq!(read(CODE), (InputKind::Code, None, None));
        assert_eq!(read(TOKEN), (InputKind::Token, None, None));
        for other in [
            "",
            "hello",
            "http://relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org/other#4821-crayon-renard",
        ] {
            assert_eq!(read(other).0, InputKind::Unknown, "{other:?}");
        }
    }

    #[test]
    fn the_claim_failures_are_told() {
        for (failure, code) in [
            (Failure::PairingTaken, "taken"),
            (Failure::PairingExpired, "expired"),
            (Failure::PairingUnsupported, "unsupported"),
        ] {
            let rig = Rig::new();
            lock(&rig.engine.joinings).push_back(Err(failure));
            assert_eq!(
                rig.host.start_join(CODE, "PC").unwrap_err(),
                CoreError::Pairing(code.into())
            );
            assert_eq!(rig.host.view(), PairingView::None);
        }
        // A relay that cannot be reached is "offline", as everywhere else in the app.
        let rig = Rig::new();
        assert_eq!(
            rig.host.start_join(CODE, "PC").unwrap_err(),
            CoreError::Offline
        );
    }

    #[test]
    fn a_wrong_code_ends_the_joining_with_one_error_whatever_the_cause() {
        let rig = Rig::new();
        let joining = FakeJoin::new(vec![JoinStep::Fail(Failure::PairingFailed)], None);
        rig.queue_joining(&joining);
        rig.host.start_join(CODE, "PC").unwrap();
        eventually("the end", || rig.stage() == Some(PairingStage::Failed));
        let s = rig.session();
        assert_eq!(s.error.unwrap().code, "error.pairing_failed");
        assert!(s.sas.is_none() && s.peer.is_none());
        assert_eq!(rig.provisioned.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn leaving_the_screen_cancels_a_joining() {
        let rig = Rig::new();
        let joining = FakeJoin::new(vec![JoinStep::Phase(JoinPhase::Verify)], None);
        rig.queue_joining(&joining);
        rig.host.start_join(CODE, "PC").unwrap();
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        rig.host
            .cancel()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(joining.cancels(), 1);
        assert_eq!(rig.host.view(), PairingView::None);
        assert_eq!(rig.provisioned.load(Ordering::SeqCst), 0);
    }

    // ------------------------------------------------------------ the first device

    #[test]
    fn the_first_device_creates_the_space_with_the_token_it_was_given() {
        let rig = Rig::new();
        rig.host.create_space("h7k2-m9qv", "PC-Test").unwrap();
        assert_eq!(
            lock(&rig.engine.created).as_slice(),
            [("h7k2-m9qv".to_owned(), "PC-Test".to_owned())]
        );
        assert_eq!(rig.provisioned.load(Ordering::SeqCst), 1);
        // Nothing is left on the screen of the host, the token least of all.
        assert_eq!(rig.host.view(), PairingView::None);
    }

    #[test]
    fn the_refusals_of_the_creation_are_told() {
        let relay = |status: u16, code: &str| Failure::Relay {
            status,
            code: code.into(),
        };
        let input = |reason: &str| Failure::InvalidInput {
            reason: reason.into(),
        };
        for (failure, expected) in [
            (
                relay(403, "install_token_invalid"),
                CoreError::Pairing("token_invalid".into()),
            ),
            (
                relay(409, "space_exists"),
                CoreError::Pairing("space_exists".into()),
            ),
            (
                input("invalid install token"),
                CoreError::Pairing("token_invalid".into()),
            ),
            (
                input("device already provisioned"),
                CoreError::Pairing("already_paired".into()),
            ),
            (
                input("invalid device name (1 to 64 characters, no control character)"),
                CoreError::Pairing("name_invalid".into()),
            ),
            (Failure::Network { reason: "x".into() }, CoreError::Offline),
            (relay(503, "unavailable"), CoreError::Offline),
        ] {
            let rig = Rig::new();
            lock(&rig.engine.creations).push_back(Err(failure.clone()));
            assert_eq!(
                rig.host.create_space("tok", "PC").unwrap_err(),
                expected,
                "{failure:?}"
            );
            assert_eq!(rig.provisioned.load(Ordering::SeqCst), 0);
            // The way is free for another try with the right token.
            rig.host.create_space("tok", "PC").unwrap();
            assert_eq!(rig.provisioned.load(Ordering::SeqCst), 1);
        }
    }

    // ------------------------------------------------------------ options and resumption

    #[test]
    fn a_paired_pc_does_not_ask_the_relay_whether_a_space_exists() {
        let rig = Rig::new();
        rig.engine.provisioned.store(true, Ordering::SeqCst);
        let o = rig.host.options("PC-Test".into());
        assert!(o.paired && o.space_initialized.is_none() && !o.pending);
        assert_eq!(rig.engine.hellos.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_unpaired_pc_reads_from_the_relay_whether_it_answers_and_whether_it_holds_a_space() {
        let rig = Rig::new();
        for answer in [Some(true), Some(false), None] {
            *lock(&rig.engine.hello) = answer;
            let o = rig.host.options("PC-Test".into());
            assert!(!o.paired);
            assert_eq!(o.space_initialized, answer);
            assert_eq!(o.device_name, "PC-Test");
        }
    }

    #[test]
    fn an_interrupted_pairing_is_settled_at_start_and_the_notice_waits_to_be_dismissed() {
        let rig = Rig::new();
        rig.engine.pending.store(true, Ordering::SeqCst);
        let down = Failure::Network { reason: "x".into() };
        lock(&rig.engine.resumes).extend([
            Err(down.clone()),
            Err(down),
            Ok(ResumeOutcome::Provisioned),
        ]);
        assert!(rig.host.options("PC".into()).pending);
        rig.host.spawn_resume();
        eventually("the provisioning hook", || {
            rig.provisioned.load(Ordering::SeqCst) == 1
        });
        // The relay was asked again after each failure.
        assert_eq!(rig.engine.resume_calls.load(Ordering::SeqCst), 3);
        eventually("the notice", || {
            rig.host.options("PC".into()).resumed == Some(ResumeNotice::Provisioned)
        });
        rig.host.dismiss();
        assert_eq!(rig.host.options("PC".into()).resumed, None);
    }

    #[test]
    fn a_batch_the_relay_does_not_know_is_dropped_and_told() {
        let rig = Rig::new();
        rig.engine.pending.store(true, Ordering::SeqCst);
        lock(&rig.engine.resumes).push_back(Ok(ResumeOutcome::Discarded));
        rig.host.spawn_resume();
        eventually("the notice", || {
            rig.host.options("PC".into()).resumed == Some(ResumeNotice::Discarded)
        });
        assert_eq!(rig.provisioned.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn there_is_nothing_to_settle_when_nothing_is_pending_or_the_pc_is_paired() {
        let rig = Rig::new();
        rig.host.spawn_resume();
        rig.engine.pending.store(true, Ordering::SeqCst);
        rig.engine.provisioned.store(true, Ordering::SeqCst);
        rig.host.spawn_resume();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(rig.engine.resume_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_keys_of_a_founder_that_the_relay_does_not_know_stay_for_the_next_creation() {
        let rig = Rig::new();
        rig.engine.pending.store(true, Ordering::SeqCst);
        lock(&rig.engine.resumes).push_back(Ok(ResumeOutcome::Nothing));
        rig.host.spawn_resume();
        eventually("one attempt", || {
            rig.engine.resume_calls.load(Ordering::SeqCst) == 1
        });
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(rig.engine.resume_calls.load(Ordering::SeqCst), 1);
        assert_eq!(rig.host.options("PC".into()).resumed, None);
    }

    #[test]
    fn quitting_stops_the_drivers_and_the_resumption() {
        let rig = Rig::new();
        let inv = FakeInvite::new(vec![]);
        rig.queue_invitation(&inv);
        rig.host.start_invite().unwrap();
        rig.host.shutdown();
        assert_eq!(inv.cancels(), 1);
        assert_eq!(rig.host.view(), PairingView::None);

        rig.engine.pending.store(true, Ordering::SeqCst);
        rig.host.spawn_resume();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(rig.engine.resume_calls.load(Ordering::SeqCst), 0);
    }

    // ------------------------------------------------------------ secrets

    #[test]
    fn the_secrets_of_a_pairing_never_reach_a_debug_print() {
        let rig = Rig::new();
        let inv = FakeInvite::new(vec![Step::Phase(InvitePhase::Verify)]);
        rig.queue_invitation(&inv);
        let waiting = rig.host.start_invite().unwrap();
        let shown = format!("{waiting:?}");
        for secret in ["crayon", "renard", SAS, "l/pair"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        eventually("the verification", || {
            rig.stage() == Some(PairingStage::Verify)
        });
        let shown = format!("{:?}", rig.host.view());
        assert!(!shown.contains("482"), "{shown}");
        assert!(shown.contains("Pixel"), "{shown}");
        rig.host.cancel();
    }

    #[test]
    fn the_view_is_what_the_webview_reads() {
        let idle = serde_json::to_value(PairingView::None).unwrap();
        assert_eq!(idle, serde_json::json!({ "role": "none" }));

        let mut s = PairingSession::new(PairingStage::Verify);
        s.sas = Some(SecretText::new(SAS));
        s.peer = Some(PairingPeerView {
            name: "Pixel".into(),
            platform: "android".into(),
        });
        s.history = Some(HistoryView { items: 3, bytes: 4 });
        let json = serde_json::to_value(PairingView::Join(s)).unwrap();
        assert_eq!(json["role"], "join");
        assert_eq!(json["stage"], "verify");
        assert_eq!(json["sas"], SAS);
        assert_eq!(json["peer"]["name"], "Pixel");
        assert_eq!(json["history"]["items"], 3);
        assert_eq!(json["memberCount"], serde_json::Value::Null);
        assert_eq!(json["renewed"], false);
        assert_eq!(json["error"], serde_json::Value::Null);
    }

    /// The contract with `src/lib/api.ts` (`PairingSession`, `PairingOptions`): the names the
    /// webview reads, in full, for the screen that shows a code.
    #[test]
    fn the_code_on_show_carries_every_field_the_webview_reads() {
        let rig = Rig::new();
        rig.queue_invitation(&FakeInvite::new(vec![]));
        let PairingView::Invite(shown) = rig.host.start_invite().unwrap() else {
            panic!("an invitation")
        };
        let json = serde_json::to_value(PairingView::Invite(shown)).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        // `link` is held on the Rust side only (it is baked into `qr`) and never serialised to
        // the webview: it is absent from the contract on purpose.
        let mut want = [
            "role",
            "stage",
            "code",
            "qr",
            "expiresAt",
            "renewed",
            "sas",
            "relayAuthority",
            "peer",
            "memberCount",
            "history",
            "offline",
            "error",
        ]
        .map(str::to_owned)
        .to_vec();
        want.sort();
        assert_eq!(keys, want);
        assert!(!json.as_object().unwrap().contains_key("link"));
        assert_eq!(json["role"], "invite");
        assert_eq!(json["stage"], "waiting");
        assert_eq!(json["code"], CODE);
        assert!(json["expiresAt"].is_i64());
        let mut qr_keys: Vec<_> = json["qr"].as_object().unwrap().keys().cloned().collect();
        qr_keys.sort();
        assert_eq!(qr_keys, ["path", "quiet", "size"]);
        assert_eq!(json["offline"], false);

        let options = serde_json::to_value(rig.host.options("PC".into())).unwrap();
        let mut keys: Vec<_> = options.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "deviceName",
                "paired",
                "pending",
                "relayAuthority",
                "resumed",
                "spaceInitialized"
            ]
        );
        rig.host.cancel();
    }

    #[test]
    fn failures_become_the_codes_the_screens_explain() {
        let code = |f: Failure| match map_pairing_failure(f) {
            CoreError::Pairing(c) => c,
            other => format!("not a pairing error: {other:?}"),
        };
        assert_eq!(code(Failure::PairingFailed), "failed");
        assert_eq!(code(Failure::PairingExpired), "expired");
        assert_eq!(code(Failure::PairingTaken), "taken");
        assert_eq!(code(Failure::PairingCancelled), "cancelled");
        assert_eq!(code(Failure::PairingUnsupported), "unsupported");
        assert_eq!(
            code(Failure::Relay {
                status: 429,
                code: "rate_limited".into()
            }),
            "rate_limited"
        );
        // Anything else goes the way it goes everywhere else.
        assert_eq!(
            map_pairing_failure(Failure::Relay {
                status: 422,
                code: "validation_failed".into()
            }),
            CoreError::Rejected("validation_failed".into())
        );
        assert_eq!(
            AppError::from(map_pairing_failure(Failure::PairingTaken)).code,
            "error.pairing_taken"
        );
    }

    #[test]
    fn a_word_is_completed_from_its_first_four_letters() {
        for typed in ["cray", "crayo", "crayon"] {
            assert_eq!(complete_word(typed).as_deref(), Some("crayon"), "{typed}");
        }
        assert_eq!(complete_word("rena").as_deref(), Some("renard"));
        assert_eq!(complete_word("zool").as_deref(), Some("zoologie"));
        // Too short to name a word, not a word, not letters of the list, too long.
        for none in ["", "cra", "crayonnes", "cr4y", "CRAY", "zzzz", "lune", "é"] {
            assert_eq!(complete_word(none), None, "{none:?}");
        }
        // Every word of the list is completed to itself from its four first letters.
        for rank in 0..u16::try_from(WORD_COUNT).unwrap() {
            let w = word(rank).unwrap();
            assert_eq!(complete_word(&w[..4]).as_deref(), Some(w), "{w}");
        }
    }

    #[test]
    fn the_computer_name_is_a_valid_device_name() {
        let name = default_device_name();
        assert!(!name.is_empty() && name.chars().count() <= 64);
        assert_eq!(name, name.trim());
    }
}
