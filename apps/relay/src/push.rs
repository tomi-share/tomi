//! Push doorbell towards sleeping phones (`api-v1.md` §6.3): the [`PushSender`] interface,
//! the dispatcher (anti-burst of 2 s per device, urgency merging, token clean-up) and the
//! FCM health shown by `admin status`.
//!
//! The content of a wake-up is constant: the sender never receives an item id, a type, a
//! `seq` or a counter, only the urgency.
//!
//! One dispatcher serves every space. A `device_id` belongs to one space only (the relay
//! refuses a second registration of a key), so the token of the device is looked up in the
//! database of *its* space, and the health of the sender (one FCM account for the whole relay)
//! is recorded in `routing.db`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::params;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::clock::Clock;
use crate::db::lock;
use crate::hub::Hub;
use crate::spaces::Spaces;

/// How urgent a wake-up is. `High` is used only for an item that must ring on that device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Normal,
    High,
}

/// Where to send a wake-up. The token is a secret: never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub provider: String,
    pub token: String,
}

impl std::fmt::Debug for PushTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushTarget")
            .field("provider", &self.provider)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Why a send failed (§6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushError {
    /// The token is dead (`UNREGISTERED`, or `INVALID_ARGUMENT` about `message.token`).
    Unregistered,
    /// The service-account credentials are refused (401/403, or no OAuth token).
    AuthFailure,
    /// Any other `INVALID_ARGUMENT`: a bug of the relay, the token is kept.
    InvalidArgument,
    /// 429/5xx/network: retry later.
    Transient { retry_after: Option<Duration> },
    /// No sender is configured for the provider of this registration: nothing is sent, the
    /// token is kept (the provider may be configured later).
    Unsupported,
}

/// Sends one content-free wake-up. Implementations never log the token.
#[async_trait]
pub trait PushSender: Send + Sync {
    async fn wake(&self, target: &PushTarget, urgency: Urgency) -> Result<(), PushError>;
}

/// Sends each wake-up through the sender of its provider: FCM for Android, APNs for the Apple
/// devices. A registration whose provider has no sender here is left alone.
#[derive(Default)]
pub struct ProviderSenders {
    pub fcm: Option<Arc<dyn PushSender>>,
    pub apns: Option<Arc<dyn PushSender>>,
}

impl ProviderSenders {
    pub fn is_empty(&self) -> bool {
        self.fcm.is_none() && self.apns.is_none()
    }
}

#[async_trait]
impl PushSender for ProviderSenders {
    async fn wake(&self, target: &PushTarget, urgency: Urgency) -> Result<(), PushError> {
        let sender = match target.provider.as_str() {
            "fcm" => self.fcm.as_ref(),
            crate::apns::PROVIDER | crate::apns::PROVIDER_SANDBOX => self.apns.as_ref(),
            _ => None,
        };
        match sender {
            Some(sender) => sender.wake(target, urgency).await,
            None => Err(PushError::Unsupported),
        }
    }
}

/// A request to wake a device.
struct WakeReq {
    device: String,
    urgency: Urgency,
}

/// Entry point of the push doorbell. Cheap to clone from the application state.
pub struct PushDispatcher {
    tx: Option<mpsc::UnboundedSender<WakeReq>>,
}

impl PushDispatcher {
    /// No push configured: every request is ignored.
    pub fn disabled() -> Self {
        Self { tx: None }
    }

    /// Whether a sender is configured.
    pub fn enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Starts the dispatcher task (must run inside a Tokio runtime).
    pub fn spawn(
        sender: Arc<dyn PushSender>,
        spaces: Arc<Spaces>,
        hub: Arc<Hub>,
        clock: Arc<dyn Clock>,
        min_interval: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker {
            sender,
            spaces,
            hub,
            clock,
            min_interval,
            health: Arc::new(Mutex::new(VecDeque::new())),
        };
        tokio::spawn(worker.run(rx));
        Self { tx: Some(tx) }
    }

    /// Asks for a wake-up of `device` (the dispatcher decides whether and when to send).
    pub fn notify(&self, device: &str, urgency: Urgency) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(WakeReq {
                device: device.to_owned(),
                urgency,
            });
        }
    }
}

#[derive(Default)]
struct DevState {
    /// End of the last real send: the anti-burst window starts there.
    last_sent: Option<Instant>,
    /// Wake-ups merged while the window was open or a send was running (highest urgency).
    pending: Option<Urgency>,
    in_flight: bool,
}

#[derive(Clone)]
struct Worker {
    sender: Arc<dyn PushSender>,
    spaces: Arc<Spaces>,
    hub: Arc<Hub>,
    clock: Arc<dyn Clock>,
    min_interval: Duration,
    /// Outcomes of the last 20 sends (`true` = temporary failure), for `degraded`.
    health: Arc<Mutex<VecDeque<bool>>>,
}

impl Worker {
    async fn run(self, mut rx: mpsc::UnboundedReceiver<WakeReq>) {
        let mut devices: HashMap<String, DevState> = HashMap::new();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<(String, bool)>();
        loop {
            // The next moment at which a merged wake-up becomes due.
            let due = devices
                .values()
                .filter(|s| s.pending.is_some() && !s.in_flight)
                .filter_map(|s| s.last_sent.map(|t| t + self.min_interval))
                .min();
            let sleeper = async {
                match due {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                req = rx.recv() => {
                    let Some(req) = req else { return };
                    let now = Instant::now();
                    let st = devices.entry(req.device.clone()).or_default();
                    let in_window = st.last_sent.is_some_and(|t| now.duration_since(t) < self.min_interval);
                    if st.in_flight || in_window {
                        st.pending = Some(st.pending.map_or(req.urgency, |p| p.max(req.urgency)));
                    } else {
                        st.in_flight = true;
                        self.spawn_send(req.device, req.urgency, done_tx.clone());
                    }
                }
                Some((dev, sent)) = done_rx.recv() => {
                    if let Some(st) = devices.get_mut(&dev) {
                        st.in_flight = false;
                        if sent {
                            st.last_sent = Some(Instant::now());
                        } else if let Some(u) = st.pending.take() {
                            // Nothing was sent (no token, or a WebSocket is open): no window to wait for.
                            st.in_flight = true;
                            self.spawn_send(dev, u, done_tx.clone());
                        }
                    }
                }
                () = sleeper => {
                    let now = Instant::now();
                    for (dev, st) in &mut devices {
                        let is_due = st.pending.is_some()
                            && !st.in_flight
                            && st.last_sent.is_none_or(|t| now.duration_since(t) >= self.min_interval);
                        if is_due {
                            if let Some(u) = st.pending.take() {
                                st.in_flight = true;
                                self.spawn_send(dev.clone(), u, done_tx.clone());
                            }
                        }
                    }
                    // Forget idle devices so the map does not grow without bound.
                    devices.retain(|_, s| {
                        s.in_flight
                            || s.pending.is_some()
                            || s.last_sent.is_some_and(|t| now.duration_since(t) < self.min_interval * 4)
                    });
                }
            }
        }
    }

    fn spawn_send(
        &self,
        device: String,
        urgency: Urgency,
        done: mpsc::UnboundedSender<(String, bool)>,
    ) {
        let w = self.clone();
        tokio::spawn(async move {
            let sent = w.send(&device, urgency).await;
            let _ = done.send((device, sent));
        });
    }

    /// One wake-up: only a device with a token and **no** open WebSocket is woken (§6.3).
    /// Returns whether a message was really sent (it then opens the anti-burst window).
    async fn send(&self, device: &str, urgency: Urgency) -> bool {
        if self.hub.has_connection(device) {
            return false;
        }
        let Some(space) = self.spaces.by_device(device) else {
            return false;
        };
        let dev = device.to_owned();
        let target = space
            .db
            .read(move |tx| {
                let r = tx.query_row(
                    "SELECT provider, token FROM push_registrations WHERE device_id = ?1",
                    params![dev],
                    |r| {
                        Ok(PushTarget {
                            provider: r.get(0)?,
                            token: r.get(1)?,
                        })
                    },
                );
                match r {
                    Ok(t) => Ok(Some(t)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e.into()),
                }
            })
            .await;
        let Ok(Some(target)) = target else {
            return false;
        };
        let mut delay = Duration::from_secs(1);
        for attempt in 1..=3 {
            match self.sender.wake(&target, urgency).await {
                Ok(()) => {
                    self.record(Outcome::Success).await;
                    return true;
                }
                Err(PushError::Unregistered) => {
                    self.record(Outcome::Success).await;
                    self.drop_token(device, &target.token).await;
                    return true;
                }
                Err(PushError::InvalidArgument) => {
                    tracing::error!(
                        "push: the provider rejected a message as invalid (relay bug), token kept"
                    );
                    return true;
                }
                Err(PushError::AuthFailure) => {
                    self.record(Outcome::AuthError).await;
                    return true;
                }
                // Nothing was sent: no window to wait for, no health to record.
                Err(PushError::Unsupported) => return false,
                Err(PushError::Transient { retry_after }) => {
                    self.record(Outcome::Transient).await;
                    if attempt == 3 {
                        return true;
                    }
                    tokio::time::sleep(retry_after.unwrap_or(delay).min(Duration::from_secs(60)))
                        .await;
                    delay = (delay * 4).min(Duration::from_secs(60));
                }
            }
        }
        true
    }
    async fn drop_token(&self, device: &str, token: &str) {
        let (dev, tok) = (device.to_owned(), token.to_owned());
        let now = self.clock.now_ms();
        if let Some(space) = self.spaces.by_device(device) {
            let r = space
                .db
                .write(move |tx| {
                    tx.execute(
                        "DELETE FROM push_registrations WHERE device_id = ?1 AND token = ?2",
                        params![dev, tok],
                    )?;
                    Ok(())
                })
                .await;
            if r.is_err() {
                tracing::warn!("push: could not drop a dead token");
            }
        }
        // The statistic of `admin status` is global.
        let r = self
            .spaces
            .routing()
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO push_events (at, kind) VALUES (?1, 'unregistered')",
                    params![now],
                )?;
                Ok(())
            })
            .await;
        if r.is_err() {
            tracing::warn!("push: could not record a dead token");
        }
    }

    async fn record(&self, outcome: Outcome) {
        let now = self.clock.now_ms();
        let degraded = {
            let mut h = lock(&self.health);
            h.push_back(matches!(outcome, Outcome::Transient));
            while h.len() > 20 {
                h.pop_front();
            }
            h.len() >= 4 && h.iter().filter(|b| **b).count() * 2 > h.len()
        };
        let (state, error): (&'static str, Option<&'static str>) = match outcome {
            Outcome::Success => ("ok", None),
            Outcome::AuthError => ("auth_error", Some("auth_failure")),
            Outcome::Transient => (if degraded { "degraded" } else { "ok" }, Some("transient")),
        };
        let is_success = matches!(outcome, Outcome::Success);
        let r = self
            .spaces
            .routing()
            .write(move |tx| {
                set_health(tx, now, state, error, is_success)?;
                Ok(())
            })
            .await;
        if r.is_err() {
            tracing::warn!("push: could not record the health of the push sender");
        }
    }
}

enum Outcome {
    Success,
    AuthError,
    Transient,
}

/// Writes the single `push_health` row of `routing.db`.
pub fn set_health(
    tx: &rusqlite::Connection,
    now: i64,
    state: &str,
    error: Option<&str>,
    success: bool,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO push_health (id, last_success_at, last_error_at, last_error, state)
         VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
           last_success_at = COALESCE(?1, last_success_at),
           last_error_at   = COALESCE(?2, last_error_at),
           last_error      = COALESCE(?3, last_error),
           state           = ?4",
        params![success.then_some(now), error.map(|_| now), error, state],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counting(std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl PushSender for Counting {
        async fn wake(&self, _: &PushTarget, _: Urgency) -> Result<(), PushError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn counting() -> Arc<Counting> {
        Arc::new(Counting(std::sync::atomic::AtomicUsize::new(0)))
    }

    fn sent(c: &Counting) -> usize {
        c.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn target_of(provider: &str) -> PushTarget {
        PushTarget {
            provider: provider.to_owned(),
            token: "t".to_owned(),
        }
    }

    #[tokio::test]
    async fn each_registration_goes_to_the_sender_of_its_provider() {
        let (fcm, apns) = (counting(), counting());
        let senders = ProviderSenders {
            fcm: Some(fcm.clone()),
            apns: Some(apns.clone()),
        };
        senders
            .wake(&target_of("fcm"), Urgency::High)
            .await
            .unwrap();
        senders
            .wake(&target_of("apns"), Urgency::High)
            .await
            .unwrap();
        senders
            .wake(&target_of("apns-sandbox"), Urgency::Normal)
            .await
            .unwrap();
        assert_eq!((sent(&fcm), sent(&apns)), (1, 2));
        assert_eq!(
            senders.wake(&target_of("unifiedpush"), Urgency::High).await,
            Err(PushError::Unsupported)
        );
    }

    #[tokio::test]
    async fn a_provider_that_is_not_configured_sends_nothing() {
        let fcm = counting();
        let only_fcm = ProviderSenders {
            fcm: Some(fcm.clone()),
            apns: None,
        };
        assert_eq!(
            only_fcm.wake(&target_of("apns"), Urgency::High).await,
            Err(PushError::Unsupported)
        );
        assert_eq!(sent(&fcm), 0, "never handed to the other provider");
        assert!(ProviderSenders::default().is_empty());
        assert!(!only_fcm.is_empty());
    }

    #[test]
    fn target_debug_hides_the_token() {
        let t = PushTarget {
            provider: "fcm".into(),
            token: "super-secret-token".into(),
        };
        assert!(!format!("{t:?}").contains("super-secret"));
    }

    #[test]
    fn urgency_orders_high_above_normal() {
        assert!(Urgency::High > Urgency::Normal);
        assert_eq!(Urgency::Normal.max(Urgency::High), Urgency::High);
    }
}
