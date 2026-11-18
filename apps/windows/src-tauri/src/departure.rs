//! The departure screens of this PC, driven from Rust: "Se déconnecter", "Supprimer
//! l'espace", the pending-departure banner, "Effacer quand même".
//!
//! This is the counterpart of the Kotlin `LeaveSpaceViewModel` + `DepartureRetrier`.
//! The webview only draws a [`DepartureSnapshot`] and
//! sends the clicks; every rule that protects the user lives here, where a compromised or
//! buggy page cannot skip it:
//!
//! * "Supprimer l'espace" and "Effacer quand même" are confirmed by Windows Hello first, every
//!   time ([`crate::strong_auth`]), and so is a "Réessayer" on a pending deletion of the space;
//!   without Windows Hello they are refused, never bypassed;
//! * their confirm buttons are armed 2 s after the dialog opened (anti-tap), and "Effacer
//!   quand même" also needs its checkbox;
//! * "Effacer quand même" is offered only when the core allows it
//!   ([`TomiCore::can_erase_without_proof`]): after a recent "not found" or "relay too old"
//!   verdict, never after a network error, a timeout, a `5xx` or a `429`;
//! * a departure that is waiting is always visible (banner) and cancellable, and nothing is
//!   erased by this module: the core erases, on a proof, and this module only reads the
//!   outcome.
//!
//! The retrier follows A4: after a gesture, an attempt at 2, 5, 15, 30, 60 and 120 s (each
//! lengthened by 0 to 20 %), while the answer is "network" or "server"; then one attempt a minute
//! (this app has no network-change hook of its own on Windows, so the slow tail polls instead of
//! waiting for the network to come back: never more than one attempt a minute), and one when the
//! window comes back (focus, popover shown) or the webview reports `online`, also at most once a
//! minute.
//!
//! "Annuler" is available while the departure waits or an attempt runs ([`LeaveFlow::cancel`],
//! [`LeaveFlow::cancel_running`]): an attempt can take 520 s ("Se déconnecter") or 1060 s
//! ("Supprimer l'espace") on a bad network, and the core's cancel does not wait for it. Neither
//! the erasure itself (`wiping`), nor the Windows Hello prompt, nor "Effacer quand même" once it
//! was confirmed can be cancelled from here: both entry points answer "nothing" (the page also
//! disables the button, but this is where the rule is kept).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use crate::core_api::{
    CoreError, DepartureKind, DepartureOutcome, DepartureView, PendingReason, RefusalReason,
    TomiCore,
};
use crate::error::AppError;
use crate::strings;
use crate::strong_auth::{self, StrongAuth};

/// The confirm button of an irreversible dialog is armed this long after the dialog opened.
pub const ARM_MS: i64 = 2_000;
/// Delays of the automatic attempts after a gesture (A4), before the random 0 to 20 %.
pub const RETRY_DELAYS_MS: [u64; 6] = [2_000, 5_000, 15_000, 30_000, 60_000, 120_000];
/// After the six delays, at most one automatic attempt per this long (the slow tail of the
/// series, and the window coming back).
pub const POKE_MIN_INTERVAL_MS: i64 = 60_000;

// ---------------------------------------------------------------- what the window draws

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LeaveScreen {
    #[default]
    Hidden,
    /// "Se déconnecter de ton espace ?"
    ConfirmLeave,
    /// "Supprimer ton espace ?" after `last_device` (D3).
    ConfirmDelete,
    /// "Effacer ce PC sans confirmation du serveur ?" (D4).
    EraseWithoutProof,
}

/// The dialog on screen. Flat on purpose: the webview switches on `screen`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveUi {
    pub screen: LeaveScreen,
    /// A request, a verification or an erasure is under way: the dialog cannot be closed (the
    /// leave and the deletion can be cancelled with "Annuler", see [`LeaveFlow::cancel_running`]).
    pub busy: bool,
    /// The last attempt was refused for good (shown in the dialog).
    pub error: Option<RefusalReason>,
    /// The attempt failed locally (storage): nothing was erased, try again.
    pub failed: bool,
    /// `EraseWithoutProof`: the checkbox "J'ai compris : c'est définitif."
    pub checked: bool,
    /// `EraseWithoutProof`: the core does not allow it right now.
    pub not_allowed: bool,
    /// `EraseWithoutProof`: the core is being asked whether it allows it.
    pub checking: bool,
    /// `ConfirmDelete` and `EraseWithoutProof`: the confirm button is enabled from this
    /// instant (ms since the Unix epoch, clock of this PC).
    pub armed_at_ms: Option<i64>,
    /// The window of the gesture that opened the dialog (`popover` or `history`): only that
    /// window draws it, so a dialog never shows up in another window without a gesture there.
    pub window: Option<String>,
}

/// Everything the windows need to draw the departure, read in one call.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepartureSnapshot {
    /// The core can do a departure at all (false: the entries are disabled, with a note).
    pub available: bool,
    /// The pending departure, for the banner.
    pub departure: Option<DepartureView>,
    /// "Effacer quand même" is allowed by the core right now.
    pub can_erase_without_proof: bool,
    pub ui: LeaveUi,
    /// An attempt (dialog, "Réessayer", automatic) is running: "Réessayer" is off, "Annuler" is not.
    pub working: bool,
    /// Windows Hello is on screen (nothing was sent yet).
    pub authenticating: bool,
    /// "Annuler" was pressed while an attempt was running; it goes on to its end and erases
    /// nothing ("Annulation en cours").
    pub cancelling: bool,
}

/// What "Annuler" cancelled, for the sentence that follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cancelled {
    pub cancelled: bool,
    pub kind: Option<DepartureKind>,
}

impl Cancelled {
    const NOTHING: Self = Self {
        cancelled: false,
        kind: None,
    };
}

// ---------------------------------------------------------------- the host

/// What the flow needs from the application around it: tell the windows, show a notice, and
/// clean up what the app holds outside the core once this PC is erased.
#[async_trait]
pub trait DepartureHost: Send + Sync {
    /// The snapshot changed: the windows read it again.
    fn changed(&self);
    /// A sentence for the user that does not belong in a dialog (a snackbar).
    fn notice(&self, error: AppError);
    /// This PC was erased: caches, clipboard, notifications, then back to the welcome screen.
    async fn erased(&self);
    /// Windows Hello is about to be shown over a window of this app: the popover must neither hide
    /// when the prompt takes the focus nor stay in front of it. Balanced by
    /// [`DepartureHost::prompt_closed`], always (even when the prompt fails).
    fn prompt_open(&self) {}
    /// The window (`popover` or `history`) is on screen. A refusal that reopens a dialog in a
    /// window nobody sees is also told as a notice, so that it does not vanish unread.
    fn window_visible(&self, _window: &str) -> bool {
        true
    }
    fn prompt_closed(&self) {}
}

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
pub type Spawner = Arc<dyn Fn(Pin<Box<dyn Future<Output = ()> + Send>>) + Send + Sync>;
pub type Jitter = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Where the flow got a result from, which decides how a refusal or an error is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// A gesture in a dialog: the dialog shows the refusal.
    Dialog(DepartureKind),
    /// "Réessayer" on the banner: a refusal reopens the dialog it belongs to.
    Banner(DepartureKind),
    /// Start-up, back-off, window shown: never opens anything, only tells.
    Auto,
    /// The result of an attempt that "Annuler" cancelled meanwhile: it opens and tells nothing
    /// (only an erasure that was already proven is carried out).
    Detached,
}

/// An attempt under way. While it lives "Réessayer" is off; when the last one ends the
/// "Annulation en cours" sentence goes away.
struct Running {
    flow: Arc<LeaveFlow>,
}

impl Running {
    fn new(flow: &Arc<LeaveFlow>) -> Self {
        flow.running.fetch_add(1, Ordering::SeqCst);
        flow.host.changed();
        Self { flow: flow.clone() }
    }

    /// The only attempt: `None` if another one is already running ("Réessayer" twice).
    fn exclusive(flow: &Arc<LeaveFlow>) -> Option<Self> {
        flow.running
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        flow.host.changed();
        Some(Self { flow: flow.clone() })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.flow.running.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.flow.cancelling.store(false, Ordering::SeqCst);
        }
        self.flow.host.changed();
    }
}

/// Windows Hello is on screen. Guards the popover's focus and shows "authenticating"; released
/// on every path, so a failed prompt never leaves the popover frozen.
struct Prompt<'a> {
    flow: &'a LeaveFlow,
}

impl<'a> Prompt<'a> {
    fn open(flow: &'a LeaveFlow) -> Self {
        flow.authenticating.store(true, Ordering::SeqCst);
        flow.host.prompt_open();
        flow.host.changed();
        Self { flow }
    }
}

impl Drop for Prompt<'_> {
    fn drop(&mut self) {
        self.flow.authenticating.store(false, Ordering::SeqCst);
        self.flow.host.prompt_closed();
        self.flow.host.changed();
    }
}

pub struct LeaveFlow {
    core: Arc<dyn TomiCore>,
    auth: Arc<dyn StrongAuth>,
    host: Arc<dyn DepartureHost>,
    clock: Clock,
    spawn: Spawner,
    jitter: Jitter,
    ui: Mutex<LeaveUi>,
    /// The window of the last gesture (stamped on the dialogs it opens).
    origin: Mutex<String>,
    /// Attempts under way (a dialog, "Réessayer", an automatic one).
    running: AtomicUsize,
    /// Windows Hello is on screen.
    authenticating: AtomicBool,
    /// "Annuler" was pressed during an attempt that has not ended.
    cancelling: AtomicBool,
    /// Bumped by every "Annuler": the result of an attempt started before it is detached.
    epoch: AtomicU64,
    /// Bumped by every `kick`: a series whose number is not the current one stops.
    generation: AtomicU64,
    /// A series has not ended: pokes leave the attempts to it.
    series_running: AtomicBool,
    /// Time of the last automatic attempt (series or poke), for the once-a-minute rule.
    last_auto_ms: AtomicI64,
}

impl LeaveFlow {
    pub fn new(
        core: Arc<dyn TomiCore>,
        auth: Arc<dyn StrongAuth>,
        host: Arc<dyn DepartureHost>,
        clock: Clock,
        spawn: Spawner,
        jitter: Jitter,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            auth,
            host,
            clock,
            spawn,
            jitter,
            ui: Mutex::new(LeaveUi::default()),
            origin: Mutex::new("history".to_string()),
            running: AtomicUsize::new(0),
            authenticating: AtomicBool::new(false),
            cancelling: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            series_running: AtomicBool::new(false),
            last_auto_ms: AtomicI64::new(i64::MIN / 2),
        })
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn ui_lock(&self) -> MutexGuard<'_, LeaveUi> {
        self.ui.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_ui(&self, mut ui: LeaveUi) {
        if ui.screen != LeaveScreen::Hidden && ui.window.is_none() {
            ui.window = Some(
                self.origin
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            );
        }
        *self.ui_lock() = ui;
        self.host.changed();
    }

    fn edit_ui(&self, f: impl FnOnce(&mut LeaveUi)) {
        f(&mut self.ui_lock());
        self.host.changed();
    }

    fn armed_in_two_seconds(&self) -> Option<i64> {
        Some(self.now() + ARM_MS)
    }

    /// The window of the gesture that follows (`popover` or `history`): the dialogs it opens
    /// belong to it and are drawn there only.
    pub fn note_origin(&self, window: &str) {
        *self.origin.lock().unwrap_or_else(|p| p.into_inner()) = window.to_string();
    }

    /// A window was hidden (or is about to be shown again): a dialog of that window that waits
    /// for a confirmation is closed. Its 2 s anti-tap would otherwise be long over when the user
    /// comes back, and an irreversible question must not wait for a stray key. Nothing under way
    /// is touched.
    pub fn window_hidden(&self, window: &str) {
        let closed = {
            let mut ui = self.ui_lock();
            if ui.screen != LeaveScreen::Hidden && !ui.busy && ui.window.as_deref() == Some(window)
            {
                *ui = LeaveUi::default();
                true
            } else {
                false
            }
        };
        if closed {
            self.host.changed();
        }
    }

    pub fn snapshot(&self) -> DepartureSnapshot {
        let running = self.running.load(Ordering::SeqCst) > 0;
        DepartureSnapshot {
            available: self.core.departure_available(),
            departure: self.core.departure(),
            can_erase_without_proof: self.core.can_erase_without_proof(),
            ui: self.ui_lock().clone(),
            working: running,
            authenticating: self.authenticating.load(Ordering::SeqCst),
            cancelling: running && self.cancelling.load(Ordering::SeqCst),
        }
    }

    // ------------------------------------------------------------ opening the dialogs

    /// Menu entry "Se déconnecter". Nothing happens while a departure is pending or an attempt
    /// still runs (the banner owns it), or another dialog is up.
    pub fn open_leave(&self) -> Result<DepartureSnapshot, AppError> {
        if !self.core.departure_available() {
            return Err(AppError::new("error.departure_unavailable"));
        }
        let status = self.core.status();
        if !status.paired {
            return Err(AppError::new("error.not_paired"));
        }
        if self.core.departure().is_none()
            && self.running.load(Ordering::SeqCst) == 0
            && self.ui_lock().screen == LeaveScreen::Hidden
        {
            self.set_ui(LeaveUi {
                screen: LeaveScreen::ConfirmLeave,
                ..LeaveUi::default()
            });
        }
        Ok(self.snapshot())
    }

    /// Closes the dialog. Ignored while something is under way.
    pub fn dismiss(&self) -> DepartureSnapshot {
        let closed = {
            let mut ui = self.ui_lock();
            if ui.busy {
                false
            } else {
                *ui = LeaveUi::default();
                true
            }
        };
        if closed {
            self.host.changed();
        }
        self.snapshot()
    }

    // ------------------------------------------------------------ "Se déconnecter"

    pub async fn confirm_leave(self: &Arc<Self>) -> Result<DepartureSnapshot, AppError> {
        self.begin(LeaveScreen::ConfirmLeave)?;
        let epoch = self.epoch.load(Ordering::SeqCst);
        let result = {
            let _running = Running::new(self);
            self.core.leave_space().await
        };
        self.settle(epoch, Ctx::Dialog(DepartureKind::Leave), result)
            .await;
        Ok(self.snapshot())
    }

    // ------------------------------------------------------------ "Supprimer l'espace"

    /// Confirms the deletion of the space: armed after 2 s, Windows Hello first.
    pub async fn confirm_delete(
        self: &Arc<Self>,
        owner: Option<isize>,
    ) -> Result<DepartureSnapshot, AppError> {
        self.check_armed()?;
        self.begin(LeaveScreen::ConfirmDelete)?;
        if let Err(e) = self.ask_hello(owner, strings::hello_delete_space()).await {
            self.edit_ui(|ui| ui.busy = false);
            return Err(e);
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let result = {
            let _running = Running::new(self);
            self.core.delete_space().await
        };
        self.settle(epoch, Ctx::Dialog(DepartureKind::DeleteSpace), result)
            .await;
        Ok(self.snapshot())
    }

    /// Windows Hello, on a thread of its own, with the popover's focus held for the duration.
    async fn ask_hello(&self, owner: Option<isize>, message: &'static str) -> Result<(), AppError> {
        let _prompt = Prompt::open(self);
        strong_auth::require(&self.auth, owner, message).await
    }

    // ------------------------------------------------------------ "Effacer quand même"

    /// Opens the dialog of "Effacer quand même" (banner, or the refusal "relay too old").
    /// The core is asked whether it allows it; if not, it is told to look again (a
    /// verification, or a synchronisation) before the dialog says "not allowed".
    pub async fn open_erase(self: &Arc<Self>) -> Result<DepartureSnapshot, AppError> {
        if !self.core.departure_available() {
            return Err(AppError::new("error.departure_unavailable"));
        }
        {
            let ui = self.ui_lock();
            if ui.busy || ui.screen == LeaveScreen::EraseWithoutProof {
                drop(ui);
                return Ok(self.snapshot());
            }
        }
        self.set_ui(LeaveUi {
            screen: LeaveScreen::EraseWithoutProof,
            checking: true,
            armed_at_ms: self.armed_in_two_seconds(),
            ..LeaveUi::default()
        });
        let mut allowed = self.core.can_erase_without_proof();
        if !allowed {
            // A fresh look: a pending departure is verified, otherwise the space is synchronised.
            if self.core.departure().is_some() {
                let result = {
                    let _running = Running::new(self);
                    self.core.continue_departure().await
                };
                if self.settled_by(&result).await {
                    return Ok(self.snapshot());
                }
                // A refusal removed the marker: say so, the dialog would only say "not allowed".
                if let Ok(DepartureOutcome::Refused { reason }) = result {
                    self.host.notice(refusal_notice(reason));
                }
            } else {
                let _ = self.core.sync().await;
            }
            allowed = self.core.can_erase_without_proof();
        }
        self.edit_ui(|ui| {
            if ui.screen == LeaveScreen::EraseWithoutProof {
                ui.checking = false;
                ui.not_allowed = !allowed;
            }
        });
        Ok(self.snapshot())
    }

    pub fn set_erase_checked(&self, checked: bool) -> DepartureSnapshot {
        let changed = {
            let mut ui = self.ui_lock();
            if ui.screen == LeaveScreen::EraseWithoutProof && !ui.busy {
                ui.checked = checked;
                true
            } else {
                false
            }
        };
        if changed {
            self.host.changed();
        }
        self.snapshot()
    }

    /// Confirms "Effacer quand même": checkbox ticked, 2 s elapsed, allowed by the core,
    /// Windows Hello passed. Only then is the core asked to erase without proof.
    pub async fn confirm_erase(
        self: &Arc<Self>,
        owner: Option<isize>,
    ) -> Result<DepartureSnapshot, AppError> {
        {
            let ui = self.ui_lock();
            if ui.screen != LeaveScreen::EraseWithoutProof || ui.busy || ui.checking {
                return Ok(self.snapshot());
            }
            if ui.not_allowed {
                return Err(AppError::new("error.erase_not_allowed"));
            }
            if !ui.checked {
                return Err(AppError::new("error.erase_not_checked"));
            }
        }
        // Re-read the core's answer: the verdict may have expired while the dialog was open.
        if !self.core.can_erase_without_proof() {
            self.edit_ui(|ui| ui.not_allowed = true);
            return Err(AppError::new("error.erase_not_allowed"));
        }
        self.check_armed()?;
        self.begin(LeaveScreen::EraseWithoutProof)?;
        if let Err(e) = self.ask_hello(owner, strings::hello_erase()).await {
            self.edit_ui(|ui| ui.busy = false);
            return Err(e);
        }
        match self.core.erase_without_proof().await {
            Ok(()) => {
                self.set_ui(LeaveUi::default());
                self.host.erased().await;
            }
            Err(CoreError::Rejected(_)) => {
                // The core no longer allows it (its verdict is too old): nothing was erased.
                self.edit_ui(|ui| {
                    ui.busy = false;
                    ui.not_allowed = true;
                });
            }
            Err(_) => self.edit_ui(|ui| {
                ui.busy = false;
                ui.failed = true;
            }),
        }
        Ok(self.snapshot())
    }

    // ------------------------------------------------------------ the banner

    /// "Réessayer": a gesture, the request is sent again. For a pending deletion of the space it
    /// is as irreversible as the first gesture: Windows Hello is asked again, every time.
    pub async fn retry(
        self: &Arc<Self>,
        owner: Option<isize>,
    ) -> Result<DepartureSnapshot, AppError> {
        let Some(view) = self.core.departure() else {
            return Ok(self.snapshot());
        };
        if view.wiping {
            return Ok(self.snapshot());
        }
        let Some(running) = Running::exclusive(self) else {
            return Ok(self.snapshot());
        };
        if view.kind == DepartureKind::DeleteSpace {
            self.ask_hello(owner, strings::hello_delete_space()).await?;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let result = self.core.retry_departure().await;
        drop(running);
        self.settle(epoch, Ctx::Banner(view.kind), result).await;
        Ok(self.snapshot())
    }

    /// "Annuler" on the banner: immediate, never erases, works while an attempt runs. The attempt
    /// that had left goes on to its end and erases nothing (the core checks its marker before
    /// every erasure).
    pub async fn cancel(self: &Arc<Self>) -> Result<Cancelled, AppError> {
        self.cancel_as(None).await
    }

    /// [`Self::cancel`]; `hint` says what was being done when the core has no marker to tell (the
    /// gesture's marker is not written yet: the page would otherwise say "Déconnexion annulée"
    /// for a deletion of the space).
    async fn cancel_as(
        self: &Arc<Self>,
        hint: Option<DepartureKind>,
    ) -> Result<Cancelled, AppError> {
        let view = self.core.departure();
        if view.is_some_and(|v| v.wiping) {
            return Ok(Cancelled::NOTHING); // the erasure itself is never cancellable
        }
        // Windows Hello is on screen, or "Effacer quand même" was confirmed: the user is about to
        // erase this PC (or is being asked to). A "reste lisible" said in another window must not
        // be followed by the erasure going through.
        if self.authenticating.load(Ordering::SeqCst) || self.erase_confirmed() {
            return Ok(Cancelled::NOTHING);
        }
        let kind = view.map(|d| d.kind).or(hint);
        // Before the core's answer: whatever an attempt in flight concludes from now on is
        // detached from the dialogs.
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let cancelled = self.core.cancel_departure().await?;
        // A series of attempts has nothing left to do.
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.series_running.store(false, Ordering::SeqCst);
        if cancelled && self.running.load(Ordering::SeqCst) > 0 {
            self.cancelling.store(true, Ordering::SeqCst);
        }
        self.host.changed();
        Ok(Cancelled {
            cancelled,
            kind: kind.filter(|_| cancelled),
        })
    }

    /// "Effacer quand même" has been confirmed and is under way (Windows Hello, then the erasure).
    fn erase_confirmed(&self) -> bool {
        let ui = self.ui_lock();
        ui.busy && ui.screen == LeaveScreen::EraseWithoutProof
    }

    /// "Annuler" in a dialog whose attempt is running ("Se déconnecter", "Supprimer l'espace"):
    /// the dialog closes at once and the departure is cancelled in the core, which does not wait
    /// for the attempt (up to 520 s or 1060 s on a bad network). Not while Windows Hello is on
    /// screen (nothing is under way yet: the prompt has its own Cancel), nor for the erasure.
    pub async fn cancel_running(self: &Arc<Self>) -> Result<Cancelled, AppError> {
        let hint = {
            let ui = self.ui_lock();
            let hint = match ui.screen {
                LeaveScreen::ConfirmLeave => DepartureKind::Leave,
                LeaveScreen::ConfirmDelete => DepartureKind::DeleteSpace,
                _ => return Ok(Cancelled::NOTHING),
            };
            if !ui.busy || self.authenticating.load(Ordering::SeqCst) {
                return Ok(Cancelled::NOTHING);
            }
            hint
        };
        // Detach the in-flight attempt before the dialog goes: its result must not reopen it.
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.set_ui(LeaveUi::default());
        // The dialog knows what the gesture was even when the core has no marker to tell yet.
        self.cancel_as(Some(hint)).await
    }

    // ------------------------------------------------------------ automatic attempts

    /// At start-up, before anything else (`spec/api-v1.md` 7.9.4): one attempt of the pending
    /// departure, then the back-off if it is only waiting for the network or the server.
    pub async fn resume_at_startup(self: &Arc<Self>) {
        if !self.core.departure_available() || self.core.departure().is_none() {
            return;
        }
        if self.auto_attempt().await {
            self.kick();
        }
    }

    /// The window came back (popover shown, full window focused): one attempt of a pending
    /// departure, at most once a minute, and only when no series is already working on it.
    pub async fn poke(self: &Arc<Self>) {
        if !self.core.departure_available()
            || self.core.departure().is_none()
            || self.series_running.load(Ordering::SeqCst)
            || !self.claim_auto_slot()
        {
            return;
        }
        self.auto_attempt().await;
    }

    /// Takes the right to make one automatic attempt now: at most one per minute, and of several
    /// windows poking at the same moment only one gets it.
    fn claim_auto_slot(&self) -> bool {
        let now = self.now();
        let last = self.last_auto_ms.load(Ordering::SeqCst);
        if now - last < POKE_MIN_INTERVAL_MS {
            return false;
        }
        self.last_auto_ms
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Starts the series of attempts (after a gesture, or at start-up). A series already
    /// running is replaced.
    pub fn kick(self: &Arc<Self>) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.series_running.store(true, Ordering::SeqCst);
        let flow = self.clone();
        (self.spawn)(Box::pin(async move { flow.series(generation).await }));
    }

    /// The six delays of A4, then one attempt a minute for as long as the answer is only
    /// "network" or "server". Ends on anything else: a proof, a refusal, "not found", "not
    /// sent", "Annuler".
    async fn series(self: Arc<Self>, generation: u64) {
        let delays = RETRY_DELAYS_MS
            .into_iter()
            .chain(std::iter::repeat(POKE_MIN_INTERVAL_MS as u64));
        for delay in delays {
            let wait = delay as f64 * (1.0 + (self.jitter)().clamp(0.0, 0.2));
            tokio::time::sleep(Duration::from_millis(wait as u64)).await;
            if self.generation.load(Ordering::SeqCst) != generation {
                return; // a newer series, or "Annuler"
            }
            if self.core.departure().is_none() || !self.auto_attempt().await {
                self.end_series(generation);
                return;
            }
        }
    }

    fn end_series(&self, generation: u64) {
        if self.generation.load(Ordering::SeqCst) == generation {
            self.series_running.store(false, Ordering::SeqCst);
        }
    }

    /// One automatic attempt. `true` while the departure only waits for the network or the
    /// server (the back-off goes on).
    async fn auto_attempt(self: &Arc<Self>) -> bool {
        self.last_auto_ms.store(self.now(), Ordering::SeqCst);
        let epoch = self.epoch.load(Ordering::SeqCst);
        let result = {
            let _running = Running::new(self);
            self.core.continue_departure().await
        };
        let keep_waiting = matches!(
            result,
            Ok(DepartureOutcome::Pending {
                reason: PendingReason::Network | PendingReason::Server
            })
        );
        self.settle(epoch, Ctx::Auto, result).await;
        keep_waiting
    }

    // ------------------------------------------------------------ plumbing

    /// Marks the dialog `busy` if it is the one expected and nothing else is going on.
    fn begin(&self, expected: LeaveScreen) -> Result<(), AppError> {
        {
            let mut ui = self.ui_lock();
            if ui.screen != expected {
                return Err(AppError::new("error.departure_state"));
            }
            if ui.busy {
                return Err(AppError::new("error.departure_busy"));
            }
            ui.busy = true;
            ui.failed = false;
        }
        self.host.changed();
        Ok(())
    }

    /// The 2 s anti-tap: refused before the button is armed, whatever the page claims.
    fn check_armed(&self) -> Result<(), AppError> {
        let armed = self.ui_lock().armed_at_ms;
        match armed {
            Some(at) if self.now() >= at => Ok(()),
            _ => Err(AppError::new("error.departure_not_armed")),
        }
    }

    /// Reads an outcome obtained outside a dialog. `true` if it ended the departure (the PC
    /// was erased), so the caller has nothing more to do.
    async fn settled_by(self: &Arc<Self>, result: &Result<DepartureOutcome, CoreError>) -> bool {
        if matches!(result, Ok(DepartureOutcome::Erased)) {
            self.set_ui(LeaveUi::default());
            self.host.erased().await;
            return true;
        }
        false
    }

    /// Reads the result of an attempt started when the cancel counter was `epoch`: if "Annuler"
    /// came since, it opens and tells nothing.
    async fn settle(
        self: &Arc<Self>,
        epoch: u64,
        ctx: Ctx,
        result: Result<DepartureOutcome, CoreError>,
    ) {
        let ctx = if self.epoch.load(Ordering::SeqCst) == epoch {
            ctx
        } else {
            Ctx::Detached
        };
        self.apply(ctx, result).await;
    }

    /// Turns what the core answered into what the window shows.
    async fn apply(self: &Arc<Self>, ctx: Ctx, result: Result<DepartureOutcome, CoreError>) {
        match result {
            Ok(DepartureOutcome::Erased) => {
                self.set_ui(LeaveUi::default());
                self.host.erased().await;
            }
            Ok(DepartureOutcome::Pending { .. }) => match ctx {
                Ctx::Dialog(_) | Ctx::Banner(_) => {
                    self.set_ui(LeaveUi::default());
                    self.kick();
                }
                Ctx::Auto | Ctx::Detached => self.host.changed(),
            },
            Ok(DepartureOutcome::Cancelled | DepartureOutcome::NothingPending) => match ctx {
                Ctx::Dialog(_) | Ctx::Banner(_) => self.set_ui(LeaveUi::default()),
                Ctx::Auto | Ctx::Detached => self.host.changed(),
            },
            Ok(DepartureOutcome::Refused { reason }) => self.refused(ctx, reason),
            Err(CoreError::DepartureUnavailable) => {
                if matches!(ctx, Ctx::Dialog(_) | Ctx::Banner(_)) {
                    self.set_ui(LeaveUi::default());
                    self.host
                        .notice(AppError::new("error.departure_unavailable"));
                }
            }
            Err(e) => match ctx {
                Ctx::Dialog(_) => self.edit_ui(|ui| {
                    ui.busy = false;
                    ui.failed = true;
                }),
                Ctx::Banner(_) => {
                    self.set_ui(LeaveUi::default());
                    self.host.notice(e.into());
                }
                // Nothing the user asked for (or no longer): the banner keeps saying what it said.
                Ctx::Auto | Ctx::Detached => self.host.changed(),
            },
        }
    }

    /// A final refusal: the marker is gone. A gesture reopens the dialog it belongs to; an
    /// automatic attempt only tells; an attempt that was cancelled says nothing.
    fn refused(&self, ctx: Ctx, reason: RefusalReason) {
        let kind = match ctx {
            Ctx::Dialog(k) | Ctx::Banner(k) => k,
            Ctx::Auto => {
                self.host.notice(refusal_notice(reason));
                return;
            }
            Ctx::Detached => {
                self.host.changed();
                return;
            }
        };
        // `last_device` always leads to the deletion of the space; a refusal of the deletion
        // stays in its own dialog.
        let delete = reason == RefusalReason::LastDevice || kind == DepartureKind::DeleteSpace;
        let ui = if delete {
            LeaveUi {
                screen: LeaveScreen::ConfirmDelete,
                // `last_device` is the reason to ask, not an error to show.
                error: (reason != RefusalReason::LastDevice).then_some(reason),
                armed_at_ms: self.armed_in_two_seconds(),
                ..LeaveUi::default()
            }
        } else {
            LeaveUi {
                screen: LeaveScreen::ConfirmLeave,
                error: Some(reason),
                ..LeaveUi::default()
            }
        };
        self.set_ui(ui);
        // The dialog belongs to the window of the gesture. If that window was hidden meanwhile
        // (the popover loses focus as soon as the user clicks elsewhere), nobody sees the dialog
        // and it is closed when the window comes back: the refusal is told now instead.
        let window = self.ui_lock().window.clone();
        if window.is_some_and(|w| !self.host.window_visible(&w)) {
            self.host.notice(refusal_notice(reason));
        }
    }
}

/// The sentence for a refusal met by an automatic attempt.
fn refusal_notice(reason: RefusalReason) -> AppError {
    AppError::new(match reason {
        RefusalReason::LastDevice => "error.departure_refused_last_device",
        RefusalReason::UpdateRequired => "error.departure_refused_update",
        RefusalReason::RelayTooOld => "error.departure_refused_relay_too_old",
        RefusalReason::Unexpected => "error.departure_refused_unexpected",
    })
}

/// A random 0..1 for the jitter of the back-off. Not a cryptographic use.
pub fn random_jitter() -> f64 {
    let mut b = [0u8; 4];
    if getrandom::fill(&mut b).is_err() {
        return 0.0;
    }
    f64::from(u32::from_le_bytes(b)) / f64::from(u32::MAX) * 0.2
}

#[cfg(test)]
mod tests;
