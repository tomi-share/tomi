//! In-memory double of the relay for tests (no network, no real server).
//!
//! It implements the contract of `api-v1.md` closely enough to exercise the engine:
//! challenge/response authentication (with the real signature check), idempotent writes,
//! `stale_key_epoch`, `joined_seq` filtering, cursor floor (`cursor_expired`), blobs with
//! resumable upload, tombstones, item states, rotations and history transfers. It also
//! offers fault injection (`set_offline`, `fail_next`) and a call log.
//!
//! Built for the tests and for the demo mode of the apps (feature `demo`, `runtime::demo`);
//! the real relay is a different program. Outside the tests nothing here may panic: locks
//! recover from poisoning and errors become relay errors.

// The demo mode uses only part of the double (no fault injection, no assertions).
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;

use crate::attachment_tombstone::AttachmentTombstone;
use crate::history::HistoryTransfer;
use crate::pin::read_item_state_header;
use crate::relay_auth::verify_relay_auth;
use crate::runtime::engine::Clock;
use crate::runtime::transport::*;
use crate::tombstone::{DeletionScope, Tombstone};
use crate::{device_id_from_ed25519, DeviceCert, KeyRotation};

/// Locks without panicking: a panic while the state was held leaves it usable for a double.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Manual clock.
#[derive(Debug)]
pub struct TestClock(AtomicI64);

impl TestClock {
    pub fn new(start_ms: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(start_ms)))
    }
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct MockDevice {
    record: DeviceRecord,
    revoked: bool,
}

struct MockBlob {
    owner: String,
    size: u64,
    data: Vec<u8>,
    attached: bool,
}

struct PostedItem {
    sender: String,
    req: PostItem,
    seq: u64,
}

struct MockTransfer {
    from: String,
    to: String,
    manifest: Vec<u8>,
    blob_ids: Vec<String>,
    open: bool,
}

/// A pairing channel (`api-v1.md` §4.6): a mailbox with one message list per side and one index
/// counter for both.
struct MockChannel {
    /// Device id of the inviter that created it.
    creator: String,
    state: ChannelState,
    expires_ms: i64,
    /// The `pairing_token` handed out by the single successful claim.
    token: Option<String>,
    inviter: Vec<(u64, Vec<u8>)>,
    joiner: Vec<(u64, Vec<u8>)>,
    next_index: u64,
}

/// The rules of `api-v1.md` §2.3 for a device name and a platform.
fn valid_name(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.chars().count() <= 64 && !t.chars().any(char::is_control)
}

fn valid_platform(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub struct State {
    pub space_id: String,
    pub key_epoch: u32,
    pub head: u64,
    pub floor: u64,
    pub events: Vec<Event>,
    devices: BTreeMap<String, MockDevice>,
    challenges: Vec<(String, Vec<u8>)>,
    sessions: BTreeMap<String, (String, i64)>,
    items: BTreeMap<String, PostedItem>,
    tombstones: BTreeMap<String, u64>,
    /// `(item_id, slot) -> seq` of the attachment removals already accepted.
    attachment_tombstones: BTreeMap<(String, u32), u64>,
    item_states: BTreeMap<String, (String, Vec<u8>, u32, u64)>,
    receipts: BTreeSet<(String, String, bool)>,
    blobs: BTreeMap<String, MockBlob>,
    transfers: BTreeMap<String, MockTransfer>,
    rotations: Vec<RotationRecord>,
    pub offline: bool,
    faults: VecDeque<(String, usize, RelayError)>,
    pub calls: Vec<String>,
    clock: Arc<dyn Clock>,
    pub session_ttl_ms: i64,
    /// Smallest `format_max` a client must announce (to test `update_required`).
    pub min_client_format: u32,
    pub limits: Limits,
    /// Capabilities announced in `hello` and `SpaceState`. Empty = a relay older than the
    /// attachment removal: the route answers `404`.
    pub features: Vec<String>,
    /// `put_blob_part` answers without moving the offset (a buggy relay).
    pub stall_uploads: bool,
    /// `events` answers `has_more` with an empty page and the same `next_after` (a buggy or
    /// hostile relay that makes a client loop).
    pub stall_events: bool,
    wakers: Vec<(String, Sender<Result<Wake, u16>>)>,
    /// Close codes for the next doorbell connections, in order ([`MockRelay::close_next_doorbell`]).
    next_wake_closes: VecDeque<u16>,
    /// Events a hostile relay serves ahead of the next page of `events`, whatever their `seq`
    /// (see [`MockRelay::inject_events`]).
    injected_events: Vec<Event>,
    /// Devices a hostile relay leaves out of `GET /v1/devices` (no event, no rotation).
    pub hidden_devices: BTreeSet<String>,
    counter: u64,
    /// `false` until `POST /v1/space` (a relay that was just installed).
    pub initialized: bool,
    install_token: Option<String>,
    channels: BTreeMap<String, MockChannel>,
    /// `pairing_ttl_secs`: a channel lives this long after its creation, and after its claim.
    pub pairing_ttl_ms: i64,
    /// `pairing_max_open`: channels `open` or `claimed` at once.
    pub pairing_max_open: usize,
    /// `pairing_max_messages`: messages per side and per channel.
    pub pairing_max_messages: usize,
    /// `pairing_max_message_bytes`.
    pub pairing_max_message_bytes: usize,
    /// Real time a long poll waits at most, whatever `wait` asks: tests do not wait 25 s.
    pub long_poll_cap_ms: u64,
    /// The `wait` (seconds) of every read of a pairing channel, failed ones included.
    pub pairing_waits: Vec<u32>,
    /// Calls whose answer is lost after they were processed.
    lost: VecDeque<String>,
    /// `space.rotation_required`: set by a departure (`DELETE /v1/me`), cleared by the next
    /// rotation (`api-v1.md` §4.4).
    pub rotation_required: bool,
    /// Runs once, right after the next `auth_session` was processed and before its answer is
    /// handed back (tests: something happens to the device while its session is being opened).
    after_auth_session: Option<Box<dyn FnOnce() + Send>>,
    /// Hooks of [`MockRelay::during_next`]: method, then what runs once after its next call.
    during: Vec<(String, Hook)>,
    /// `deletion_id` of every space deleted by `DELETE /v1/space` (the relay keeps only a
    /// hash; the double does not need to).
    deletions: BTreeSet<[u8; 32]>,
    /// The space was deleted by its last device (`DELETE /v1/space`).
    pub space_deleted: bool,
    /// The `deletion_id` of every `DELETE /v1/space` that reached the transport, whatever came
    /// of it (injected failures included), in order.
    delete_ids: Vec<[u8; 32]>,
    /// Authority this relay answers to: a transport pointed at another one reaches nothing
    /// (`network` error), like a relay URL that is not this relay.
    pub authority: String,
}

type Hook = Box<dyn FnOnce() + Send>;

impl State {
    fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Drops the channels past their deadline (`404` afterwards).
    fn purge_channels(&mut self) {
        let now = self.now();
        self.channels.retain(|_, c| c.expires_ms > now);
    }

    /// Applies a queued "answer lost": the call was processed, the caller sees no answer.
    fn lose<R>(&mut self, method: &str, r: TResult<R>) -> TResult<R> {
        if r.is_ok() {
            if let Some(pos) = self.lost.iter().position(|m| m == method) {
                self.lost.remove(pos);
                return Err(RelayError::Network("response lost".into()));
            }
        }
        r
    }

    /// Whether the relay still holds the blob (tests).
    pub fn has_blob(&self, blob_id: &str) -> bool {
        self.blobs.contains_key(blob_id)
    }

    pub fn push(&mut self, data: EventData, author: Option<&str>) -> u64 {
        self.head += 1;
        let seq = self.head;
        let ev = Event {
            seq,
            at_ms: self.now(),
            data,
        };
        self.events.push(ev);
        for (dev, tx) in &self.wakers {
            if Some(dev.as_str()) != author {
                let _ = tx.send(Ok(Wake { seq, ring: false }));
            }
        }
        seq
    }

    fn space_info(&self) -> SpaceInfo {
        SpaceInfo {
            space_id: self.space_id.clone(),
            effective_format: 1,
            key_epoch: self.key_epoch,
            rotation_required: self.rotation_required
                || self
                    .devices
                    .values()
                    .any(|d| d.revoked && d.record.joined_seq == u64::MAX),
            head_seq: self.head,
            server_time_ms: self.now(),
            quota_used_bytes: 0,
            quota_max_bytes: 2 << 30,
            limits: self.limits.clone(),
            features: self.features.clone(),
        }
    }
}

/// The shared relay; hand out one [`MockTransport`] per device.
#[derive(Clone)]
pub struct MockRelay {
    pub st: Arc<Mutex<State>>,
    /// Signalled by every change a long poll may wait for.
    cv: Arc<Condvar>,
}

impl MockRelay {
    /// A relay that was just installed: no space yet, `POST /v1/space` creates it with
    /// `install_token` (`api-v1.md` §3.1, §3.2).
    pub fn uninitialized(clock: Arc<dyn Clock>, install_token: &str) -> Self {
        let relay = Self::new("", clock);
        relay.with(|s| {
            s.initialized = false;
            s.install_token = Some(install_token.to_owned());
        });
        relay
    }

    pub fn new(space_id: &str, clock: Arc<dyn Clock>) -> Self {
        let st = State {
            space_id: space_id.into(),
            key_epoch: 1,
            head: 0,
            floor: 0,
            events: Vec::new(),
            devices: BTreeMap::new(),
            challenges: Vec::new(),
            sessions: BTreeMap::new(),
            items: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            attachment_tombstones: BTreeMap::new(),
            features: vec!["attachment_tombstones".to_owned()],
            item_states: BTreeMap::new(),
            receipts: BTreeSet::new(),
            blobs: BTreeMap::new(),
            transfers: BTreeMap::new(),
            rotations: Vec::new(),
            offline: false,
            faults: VecDeque::new(),
            calls: Vec::new(),
            clock,
            session_ttl_ms: 3_600_000,
            min_client_format: 1,
            limits: Limits::default(),
            stall_uploads: false,
            stall_events: false,
            wakers: Vec::new(),
            next_wake_closes: VecDeque::new(),
            injected_events: Vec::new(),
            hidden_devices: BTreeSet::new(),
            counter: 0,
            initialized: true,
            install_token: None,
            channels: BTreeMap::new(),
            pairing_ttl_ms: 120_000,
            pairing_max_open: 2,
            pairing_max_messages: 8,
            pairing_max_message_bytes: 8192,
            long_poll_cap_ms: 20,
            pairing_waits: Vec::new(),
            lost: VecDeque::new(),
            rotation_required: false,
            after_auth_session: None,
            during: Vec::new(),
            deletions: BTreeSet::new(),
            space_deleted: false,
            delete_ids: Vec::new(),
            authority: "relay.example.org".into(),
        };
        Self {
            st: Arc::new(Mutex::new(st)),
            cv: Arc::new(Condvar::new()),
        }
    }

    /// The next call of `method` is processed normally, and its answer is lost (the caller gets
    /// a network error): the case of a request that reached the relay and whose reply did not
    /// come back.
    pub fn lose_next_response(&self, method: &str) {
        self.with(|s| s.lost.push_back(method.into()));
    }

    /// Forgets a device and its sessions, as a relay that was reset: it is unknown from now on
    /// (`401 unauthorized` when it authenticates).
    pub fn forget_device(&self, device_id: &str) {
        self.with(|s| {
            s.devices.remove(device_id);
            s.sessions.retain(|_, (d, _)| d != device_id);
        });
    }

    /// Runs `f` once, after the next `auth_session` was processed and before its answer comes
    /// back to the caller (outside the relay's lock).
    pub fn after_next_auth_session(&self, f: impl FnOnce() + Send + 'static) {
        self.with(|s| s.after_auth_session = Some(Box::new(f)));
    }

    /// Runs `f` once, after the next call of `method` was processed (whatever its result) and
    /// before its answer comes back to the caller, outside the relay's lock: races between a
    /// call in flight and something else the device does (an erasure, a cancellation).
    pub fn during_next(&self, method: &str, f: impl FnOnce() + Send + 'static) {
        self.with(|s| s.during.push((method.into(), Box::new(f))));
    }

    fn run_during(&self, method: &str) {
        let hook = self.with(|s| {
            s.during
                .iter()
                .position(|(m, _)| m == method)
                .map(|pos| s.during.remove(pos).1)
        });
        if let Some(hook) = hook {
            hook();
        }
    }

    /// A relay that lost its data, as one started on an empty `data_dir` (volume not mounted)
    /// would look: every space route answers `409 space_not_initialized`. The devices are kept
    /// (the double can be brought back with `with(|s| s.initialized = true)`). Never a proof
    /// of anything (`api-v1.md` §1.4 rule 4).
    pub fn uninitialize(&self) {
        self.with(|s| s.initialized = false);
    }

    /// Brings back a relay that [`uninitialize`](Self::uninitialize) emptied.
    #[cfg_attr(not(feature = "demo"), allow(dead_code))]
    pub fn reinitialize(&self) {
        self.with(|s| s.initialized = true);
    }

    /// Marks a device as removed from the space (or brings it back), without an event: the demo
    /// uses it to make this device the last one.
    #[cfg_attr(not(feature = "demo"), allow(dead_code))]
    pub fn set_revoked(&self, device_id: &str, revoked: bool) {
        self.with(|s| {
            if let Some(d) = s.devices.get_mut(device_id) {
                d.revoked = revoked;
            }
        });
    }

    /// Drops the faults injected and not met yet.
    #[cfg_attr(not(feature = "demo"), allow(dead_code))]
    pub fn clear_faults(&self) {
        self.with(|s| s.faults.clear());
    }

    /// Records a receipt for `deletion_id`, as a deletion that succeeded would.
    pub fn add_receipt(&self, deletion_id: [u8; 32]) {
        self.with(|s| {
            s.deletions.insert(deletion_id);
        });
    }

    /// The `deletion_id` of every `DELETE /v1/space` sent so far, failed ones included.
    pub fn deletion_ids_sent(&self) -> Vec<[u8; 32]> {
        self.with(|s| s.delete_ids.clone())
    }

    /// Whether a receipt exists for `deletion_id`.
    pub fn has_receipt(&self, deletion_id: &[u8; 32]) -> bool {
        self.with(|s| s.deletions.contains(deletion_id))
    }

    /// The single `deletion_id` the relay holds a receipt for, if exactly one.
    pub fn only_receipt(&self) -> Option<[u8; 32]> {
        self.with(|s| {
            if s.deletions.len() == 1 {
                s.deletions.iter().next().copied()
            } else {
                None
            }
        })
    }

    /// State of a pairing channel, `None` if it does not exist (never, or expired).
    pub fn channel_state(&self, channel_id: &str) -> Option<ChannelState> {
        self.with(|s| {
            s.purge_channels();
            s.channels.get(channel_id).map(|c| c.state)
        })
    }

    /// How many messages the inviter and the new device deposited in a channel.
    pub fn channel_messages(&self, channel_id: &str) -> Option<(usize, usize)> {
        self.with(|s| {
            s.purge_channels();
            s.channels
                .get(channel_id)
                .map(|c| (c.inviter.len(), c.joiner.len()))
        })
    }

    /// Alters a message of a channel in place (a hostile relay): `index` is the index the relay
    /// gave it.
    pub fn tamper_message(&self, channel_id: &str, index: u64, f: impl FnOnce(&mut Vec<u8>)) {
        self.with(|s| {
            if let Some(c) = s.channels.get_mut(channel_id) {
                let found = c
                    .inviter
                    .iter_mut()
                    .chain(c.joiner.iter_mut())
                    .find(|m| m.0 == index);
                if let Some((_, body)) = found {
                    f(body);
                }
            }
        });
    }

    /// Makes the relay say a channel is in `state` (a hostile relay lying about it).
    pub fn force_channel_state(&self, channel_id: &str, state: ChannelState) {
        self.with(|s| {
            if let Some(c) = s.channels.get_mut(channel_id) {
                c.state = state;
            }
        });
    }

    /// How many sessions the relay holds for a device.
    pub fn sessions_of(&self, device_id: &str) -> usize {
        self.with(|s| s.sessions.values().filter(|(d, _)| d == device_id).count())
    }

    /// The `pairing_token` the claim of a channel handed out (a secret, for assertions).
    pub fn channel_token(&self, channel_id: &str) -> Option<String> {
        self.with(|s| s.channels.get(channel_id).and_then(|c| c.token.clone()))
    }

    /// The `wait` (seconds) every read of a pairing channel asked for so far, in order.
    pub fn pairing_waits(&self) -> Vec<u32> {
        self.with(|s| s.pairing_waits.clone())
    }

    /// The identifiers of the channels the relay holds.
    pub fn channel_ids(&self) -> Vec<String> {
        self.with(|s| {
            s.purge_channels();
            s.channels.keys().cloned().collect()
        })
    }

    /// The raw bytes of every message of a channel, in order, for assertions (the relay sees
    /// nothing else).
    pub fn channel_bodies(&self, channel_id: &str) -> Vec<Vec<u8>> {
        self.with(|s| {
            let Some(c) = s.channels.get(channel_id) else {
                return Vec::new();
            };
            let mut all: Vec<&(u64, Vec<u8>)> = c.inviter.iter().chain(c.joiner.iter()).collect();
            all.sort_by_key(|m| m.0);
            all.into_iter().map(|m| m.1.clone()).collect()
        })
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut lock(&self.st))
    }

    /// Registers a device (the founder, or a paired one: an event `device_added` is logged).
    pub fn add_device(&self, record: DeviceRecord) {
        self.with(|s| {
            let mut record = record;
            record.joined_seq = s.head;
            let ev = EventData::DeviceAdded {
                device: record.clone(),
            };
            s.devices.insert(
                record.device_id.clone(),
                MockDevice {
                    record,
                    revoked: false,
                },
            );
            s.push(ev, None);
        });
    }

    /// What `GET /v1/devices` reports from now on for `device_id`: `last_seen_at` (`None` =
    /// absent) and `active`. `active: None` makes the record look like one of a relay that
    /// does not report the activity at all (both fields absent).
    // Demo/test helper: an unknown device is a caller bug, so failing loudly is intended.
    #[allow(clippy::expect_used)]
    pub fn set_device_activity(
        &self,
        device_id: &str,
        last_seen_at_ms: Option<i64>,
        active: Option<bool>,
    ) {
        self.with(|s| {
            // An unknown device is ignored (no panic outside the tests: the demo links this).
            if let Some(d) = s.devices.get_mut(device_id) {
                d.record.last_seen_at_ms = last_seen_at_ms;
                d.record.active = active;
            }
        });
    }

    /// A transport pointed at this relay ([`DEFAULT_RELAY_URL`]).
    pub fn transport(&self) -> Arc<MockTransport> {
        self.transport_at(Some(DEFAULT_RELAY_URL))
    }

    /// A transport pointed at `url`, or at no relay (`None`). It reaches this relay only when the
    /// authority of `url` is the one of the relay (the authority of the relay (`State::authority`)).
    pub fn transport_at(&self, url: Option<&str>) -> Arc<MockTransport> {
        Arc::new(MockTransport {
            relay: self.clone(),
            token: Mutex::new(None),
            url: Mutex::new(url.map(str::to_owned)),
        })
    }

    pub fn set_offline(&self, offline: bool) {
        self.with(|s| s.offline = offline);
    }

    /// Makes the next call of `method` (e.g. `"post_item"`) fail with `err`.
    pub fn fail_next(&self, method: &str, err: RelayError) {
        self.fail_next_after(method, 0, err);
    }

    /// Lets `skip` calls of `method` succeed, then makes the next one fail.
    pub fn fail_next_after(&self, method: &str, skip: usize, err: RelayError) {
        self.with(|s| s.faults.push_back((method.into(), skip, err)));
    }

    /// Alters the manifest stored for a transfer (a hostile relay).
    pub fn corrupt_manifest(&self, transfer_id: &str) {
        self.with(|s| {
            if let Some(t) = s.transfers.get_mut(transfer_id) {
                if let Some(b) = t.manifest.last_mut() {
                    *b ^= 1;
                }
            }
        });
    }

    pub fn count(&self, method: &str) -> usize {
        self.with(|s| s.calls.iter().filter(|c| c.as_str() == method).count())
    }

    pub fn head(&self) -> u64 {
        self.with(|s| s.head)
    }

    /// Drops the log before `floor` (compaction): older cursors get `cursor_expired`.
    pub fn compact_to(&self, floor: u64) {
        self.with(|s| {
            s.floor = floor;
            s.events.retain(|e| e.seq > floor);
        });
    }

    /// A hostile relay: the next `events` answer starts with `events`, with whatever `seq` they
    /// carry (lower than the cursor, out of order...), before the real ones.
    pub fn inject_events(&self, events: Vec<Event>) {
        self.with(|s| s.injected_events.extend(events));
    }

    /// Simulates a restore from an older backup: the head goes back to `seq`.
    pub fn rewind_head(&self, seq: u64) {
        self.with(|s| {
            s.head = seq;
            s.events.retain(|e| e.seq <= seq);
        });
    }

    /// A doorbell connection for `device_id`. If [`close_next_doorbell`](Self::close_next_doorbell)
    /// queued a code, the connection is closed with it right away.
    pub fn wake_source(&self, device_id: &str) -> MockWake {
        let (tx, rx) = channel();
        self.with(|s| {
            if let Some(code) = s.next_wake_closes.pop_front() {
                let _ = tx.send(Err(code));
            }
            s.wakers.push((device_id.into(), tx));
        });
        MockWake(rx)
    }

    /// The next doorbell connection opened is closed at once with `code` (`api-v1.md` §5.4):
    /// a deterministic stand-in for a `4004` or a `4003` that arrives while the loop runs.
    pub fn close_next_doorbell(&self, code: u16) {
        self.with(|s| s.next_wake_closes.push_back(code));
    }

    /// Closes every open doorbell with a WebSocket close code (`api-v1.md` section 5.4).
    pub fn close_doorbells(&self, code: u16) {
        self.with(|s| {
            for (_, tx) in s.wakers.drain(..) {
                let _ = tx.send(Err(code));
            }
        });
    }

    /// Events of the log, for assertions.
    pub fn events(&self) -> Vec<Event> {
        self.with(|s| s.events.clone())
    }

    pub fn expire_sessions(&self) {
        self.with(|s| s.sessions.clear());
    }
}

pub struct MockWake(pub Receiver<Result<Wake, u16>>);

impl WakeSource for MockWake {
    fn next(&mut self) -> Result<Option<Wake>, WakeClosed> {
        match self.0.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(Ok(w)) => Ok(Some(w)),
            Ok(Err(code)) => Err(WakeClosed { code: Some(code) }),
            Err(_) => Err(WakeClosed { code: None }),
        }
    }
}

/// Relay URL of a transport made by [`MockRelay::transport`].
pub const DEFAULT_RELAY_URL: &str = "https://relay.example.org";

pub struct MockTransport {
    relay: MockRelay,
    token: Mutex<Option<String>>,
    /// The relay this transport is pointed at ([`RelayTransport::set_relay`]).
    url: Mutex<Option<String>>,
}

fn authority_of(url: &str) -> Option<String> {
    crate::relay_url::check_base_url(url)
        .ok()
        .map(|b| b.relay_authority())
}

fn api<T>(status: u16, code: &str) -> TResult<T> {
    Err(RelayError::api(status, code))
}

impl MockTransport {
    /// Common prologue: call log, offline, injected faults.
    fn enter(&self, s: &mut State, method: &str) -> TResult<()> {
        // Nothing is sent without a relay, and nothing arrives at another one.
        let Some(url) = lock(&self.url).clone() else {
            return Err(RelayError::Network("no relay configured".into()));
        };
        if authority_of(&url).as_deref() != Some(s.authority.as_str()) {
            return Err(RelayError::Network("relay unreachable".into()));
        }
        s.calls.push(method.into());
        if s.offline {
            return Err(RelayError::Network("offline".into()));
        }
        if let Some(pos) = s.faults.iter().position(|(m, _, _)| m == method) {
            if s.faults[pos].1 > 0 {
                s.faults[pos].1 -= 1;
            } else if let Some((_, _, e)) = s.faults.remove(pos) {
                return Err(e);
            }
        }
        Ok(())
    }

    fn open<R>(&self, method: &str, f: impl FnOnce(&mut State) -> TResult<R>) -> TResult<R> {
        let r = {
            let mut s = lock(&self.relay.st);
            self.enter(&mut s, method).and_then(|()| f(&mut s))
        };
        self.relay.run_during(method);
        r
    }

    /// Authenticated call: resolves the caller from the session token.
    fn authed<R>(
        &self,
        method: &str,
        f: impl FnOnce(&mut State, &str) -> TResult<R>,
    ) -> TResult<R> {
        let token = lock(&self.token).clone();
        let r = {
            let mut s = lock(&self.relay.st);
            (|| {
                self.enter(&mut s, method)?;
                // A relay that hosts no space answers so to every space route (§1.4 rule 4).
                if !s.initialized {
                    return api(409, "space_not_initialized");
                }
                let Some(token) = token else {
                    return api(401, "unauthorized");
                };
                let Some((dev, exp)) = s.sessions.get(&token).cloned() else {
                    return api(401, "unauthorized");
                };
                if exp <= s.now() {
                    return api(401, "session_expired");
                }
                if s.devices.get(&dev).is_none_or(|d| d.revoked) {
                    return api(403, "revoked");
                }
                f(&mut s, &dev)
            })()
        };
        self.relay.run_during(method);
        r
    }
}

fn blob_status(id: &str, b: &MockBlob) -> BlobStatus {
    BlobStatus {
        blob_id: id.into(),
        size: b.size,
        upload_offset: b.data.len() as u64,
        complete: b.data.len() as u64 == b.size,
        attached: b.attached,
    }
}

impl RelayTransport for MockTransport {
    fn set_session_token(&self, token: Option<String>) {
        *lock(&self.token) = token;
    }

    fn hello(&self) -> TResult<Hello> {
        self.open("hello", |s| {
            Ok(Hello {
                server_time_ms: s.now(),
                space_initialized: s.initialized,
                limits: Some(s.limits.clone()),
                features: s.features.clone(),
            })
        })
    }

    fn auth_challenge(&self, device_id: &str) -> TResult<Challenge> {
        self.open("auth_challenge", |s| {
            if !s.initialized {
                return api(409, "space_not_initialized");
            }
            let ch = crate::crypto::random_array::<32>()
                .map_err(|_| RelayError::api(500, "internal"))?
                .to_vec();
            s.challenges.push((device_id.into(), ch.clone()));
            Ok(Challenge {
                challenge: ch,
                expires_at_ms: s.now() + 60_000,
            })
        })
    }

    fn auth_session(&self, req: &SessionRequest) -> TResult<SessionGrant> {
        let answer = self.open("auth_session", |s| {
            if !s.initialized {
                return api(409, "space_not_initialized");
            }
            // The challenge is consumed whatever the result.
            let pos = s
                .challenges
                .iter()
                .position(|(d, c)| *d == req.device_id && *c == req.challenge);
            let Some(pos) = pos else {
                return api(401, "unauthorized");
            };
            s.challenges.remove(pos);
            let Some(dev) = s.devices.get(&req.device_id) else {
                return api(401, "unauthorized");
            };
            if dev.revoked {
                return api(403, "revoked");
            }
            let space_id = s.space_id.clone();
            if verify_relay_auth(
                &space_id,
                &dev.record.sign_pk,
                &req.challenge,
                &req.signature,
            )
            .is_err()
            {
                return api(401, "unauthorized");
            }
            if req.client.format_max < s.min_client_format {
                return Err(RelayError::problem(ApiProblem {
                    status: 409,
                    code: "update_required".into(),
                    update_url: Some("https://relay.test/updates/latest.json".into()),
                    ..ApiProblem::default()
                }));
            }
            s.counter += 1;
            let token = format!("nvs1.test-{}-{}", req.device_id, s.counter);
            let expires = s.now() + s.session_ttl_ms;
            s.sessions
                .insert(token.clone(), (req.device_id.clone(), expires));
            Ok(SessionGrant {
                token,
                expires_at_ms: expires,
                device_id: req.device_id.clone(),
                space: s.space_info(),
            })
        });
        let hook = self.relay.with(|s| s.after_auth_session.take());
        if let Some(hook) = hook {
            hook();
        }
        answer
    }

    fn delete_session(&self) -> TResult<()> {
        let token = lock(&self.token).clone();
        self.open("delete_session", |s| {
            if let Some(t) = token {
                s.sessions.remove(&t);
            }
            Ok(())
        })
    }

    /// `DELETE /v1/me` (`api-v1.md` §4.4): refused for the last device; otherwise the caller
    /// is revoked (`via: "self"`), its doorbells close with `4003`, a rotation is required.
    fn leave_space(&self) -> TResult<()> {
        self.authed("leave_space", |s, caller| {
            let r = (|| {
                if s.devices.values().filter(|d| !d.revoked).count() <= 1 {
                    return api(409, "last_device");
                }
                if let Some(d) = s.devices.get_mut(caller) {
                    d.revoked = true;
                }
                s.rotation_required = true;
                s.push(
                    EventData::DeviceRevoked {
                        device_id: caller.into(),
                        via: "self".into(),
                    },
                    Some(caller),
                );
                let (mine, others): (Vec<_>, Vec<_>) =
                    s.wakers.drain(..).partition(|(dev, _)| dev == caller);
                s.wakers = others;
                for (_, tx) in mine {
                    let _ = tx.send(Err(4003));
                }
                Ok(())
            })();
            s.lose("leave_space", r)
        })
    }

    /// `DELETE /v1/space` (`api-v1.md` §4.4.1): refused unless the caller is the only active
    /// member; otherwise the space is gone (the double hosts one space, so the relay answers
    /// `409 space_not_initialized` afterwards), every doorbell closes with `4004` and the
    /// receipt is kept.
    fn delete_space(&self, deletion_id: &[u8; 32]) -> TResult<()> {
        self.relay.with(|s| s.delete_ids.push(*deletion_id));
        self.authed("delete_space", |s, _caller| {
            let r = (|| {
                if s.devices.values().filter(|d| !d.revoked).count() != 1 {
                    return api(409, "not_last_device");
                }
                s.space_deleted = true;
                s.initialized = false;
                s.sessions.clear();
                s.devices.clear();
                s.challenges.clear();
                for (_, tx) in s.wakers.drain(..) {
                    let _ = tx.send(Err(4004));
                }
                s.deletions.insert(*deletion_id);
                Ok(())
            })();
            s.lose("delete_space", r)
        })
    }

    /// `POST /v1/space/receipt` (`api-v1.md` §4.4.2): global route, no session, never
    /// `409 space_not_initialized`.
    fn space_deletion_receipt(&self, deletion_id: &[u8; 32]) -> TResult<()> {
        self.open("space_deletion_receipt", |s| {
            if s.deletions.contains(deletion_id) {
                Ok(())
            } else {
                api(404, "not_found")
            }
        })
    }

    fn space(&self) -> TResult<SpaceInfo> {
        self.authed("space", |s, _| Ok(s.space_info()))
    }

    fn keys(&self) -> TResult<KeysInfo> {
        self.authed("keys", |s, _| {
            Ok(KeysInfo {
                key_epoch: s.key_epoch,
                rotations: s.rotations.clone(),
            })
        })
    }

    fn devices(&self) -> TResult<Vec<DeviceRecord>> {
        self.authed("devices", |s, _| {
            Ok(s.devices
                .values()
                .filter(|d| !d.revoked && !s.hidden_devices.contains(&d.record.device_id))
                .map(|d| d.record.clone())
                .collect())
        })
    }

    fn rename_device(&self, device_id: &str, name: &str) -> TResult<()> {
        self.authed("rename_device", |s, _| {
            let Some(d) = s.devices.get_mut(device_id) else {
                return api(404, "not_found");
            };
            d.record.name = name.into();
            s.push(
                EventData::DeviceUpdated {
                    device_id: device_id.into(),
                    name: name.into(),
                },
                None,
            );
            Ok(())
        })
    }

    fn post_rotation(&self, rotation: &[u8]) -> TResult<u64> {
        self.authed("post_rotation", |s, caller| {
            let rot = KeyRotation::from_bytes(rotation)
                .map_err(|_| RelayError::api(422, "validation_failed"))?;
            if rot.new_epoch != s.key_epoch + 1 {
                return Err(RelayError::problem(ApiProblem {
                    status: 409,
                    code: "epoch_conflict".into(),
                    key_epoch: Some(s.key_epoch),
                    ..ApiProblem::default()
                }));
            }
            let caller_pk = s.devices[caller].record.sign_pk.clone();
            if rot.issuer_ed25519 != caller_pk || rot.verify_signature().is_err() {
                return api(422, "validation_failed");
            }
            let revoked_ids: Vec<String> = s
                .devices
                .iter()
                .filter(|(_, d)| !d.revoked && rot.revoked.contains(&d.record.sign_pk))
                .map(|(id, _)| id.clone())
                .collect();
            let expected: BTreeSet<Vec<u8>> = s
                .devices
                .values()
                .filter(|d| !d.revoked && !rot.revoked.contains(&d.record.sign_pk))
                .map(|d| d.record.sign_pk.clone())
                .collect();
            let got: BTreeSet<Vec<u8>> = rot.recipients.iter().map(|r| r.ed25519.clone()).collect();
            if expected != got {
                return api(409, "device_set_mismatch");
            }
            s.key_epoch = rot.new_epoch;
            s.rotation_required = false;
            let seq = s.push(
                EventData::Rotation {
                    key_epoch: rot.new_epoch,
                    device_id: caller.into(),
                    revoked_device_ids: revoked_ids.clone(),
                    rotation: rotation.to_vec(),
                },
                Some(caller),
            );
            s.rotations.push(RotationRecord {
                key_epoch: rot.new_epoch,
                seq,
                rotation: rotation.to_vec(),
            });
            for id in revoked_ids {
                if let Some(d) = s.devices.get_mut(&id) {
                    d.revoked = true;
                }
                s.push(
                    EventData::DeviceRevoked {
                        device_id: id,
                        via: "rotation".into(),
                    },
                    Some(caller),
                );
            }
            Ok(seq)
        })
    }

    fn events(&self, after: u64, limit: u32) -> TResult<EventPage> {
        self.authed("events", |s, caller| {
            let joined = s.devices[caller].record.joined_seq;
            let after = after.max(joined);
            if after > s.head {
                return Err(RelayError::problem(ApiProblem {
                    status: 409,
                    code: "cursor_ahead".into(),
                    head_seq: Some(s.head),
                    ..ApiProblem::default()
                }));
            }
            if after < s.floor {
                return api(410, "cursor_expired");
            }
            if s.stall_events {
                return Ok(EventPage {
                    events: Vec::new(),
                    head_seq: s.head,
                    has_more: true,
                    next_after: after,
                });
            }
            let all: Vec<Event> = s.events.iter().filter(|e| e.seq > after).cloned().collect();
            let limit = limit.clamp(1, 500) as usize;
            let has_more = all.len() > limit;
            let mut events: Vec<Event> = std::mem::take(&mut s.injected_events);
            events.extend(all.into_iter().take(limit));
            let next_after = if has_more {
                events.last().map_or(after, |e| e.seq)
            } else {
                s.head
            };
            Ok(EventPage {
                events,
                head_seq: s.head,
                has_more,
                next_after,
            })
        })
    }

    fn post_item(&self, item: &PostItem) -> TResult<Accepted> {
        self.authed("post_item", |s, caller| {
            if let Some(prev) = s.items.get(&item.item_id) {
                return if prev.sender == caller && prev.req == *item {
                    Ok(Accepted {
                        seq: prev.seq,
                        replayed: true,
                    })
                } else {
                    api(409, "item_id_conflict")
                };
            }
            if item.key_epoch != s.key_epoch {
                return Err(RelayError::problem(ApiProblem {
                    status: 409,
                    code: "stale_key_epoch".into(),
                    key_epoch: Some(s.key_epoch),
                    ..ApiProblem::default()
                }));
            }
            if item.expires_at_ms <= s.now() {
                return api(422, "already_expired");
            }
            if item.expires_at_ms > s.now() + (s.limits.max_ttl_secs as i64) * 1000 {
                return api(422, "validation_failed");
            }
            let mut blobs = Vec::new();
            for id in &item.blob_ids {
                match s.blobs.get(id) {
                    Some(b) if b.owner == caller => {
                        if b.data.len() as u64 != b.size {
                            return api(409, "blob_incomplete");
                        }
                        if b.attached {
                            return api(409, "blob_attached");
                        }
                        blobs.push(BlobRef {
                            blob_id: id.clone(),
                            size: b.size,
                        });
                    }
                    _ => return api(404, "not_found"),
                }
            }
            for id in &item.blob_ids {
                if let Some(b) = s.blobs.get_mut(id) {
                    b.attached = true;
                }
            }
            let created = s.now();
            let seq = s.push(
                EventData::Item {
                    item_id: item.item_id.clone(),
                    sender_device_id: caller.into(),
                    created_at_ms: created,
                    expires_at_ms: item.expires_at_ms,
                    key_epoch: item.key_epoch,
                    envelope: item.envelope.clone(),
                    blobs,
                    ring: false,
                },
                Some(caller),
            );
            s.items.insert(
                item.item_id.clone(),
                PostedItem {
                    sender: caller.into(),
                    req: item.clone(),
                    seq,
                },
            );
            Ok(Accepted {
                seq,
                replayed: false,
            })
        })
    }

    fn ack_item(&self, item_id: &str, release: bool) -> TResult<()> {
        let _ = release;
        self.authed("ack_item", |s, caller| {
            let Some(it) = s.items.get(item_id) else {
                return api(404, "not_found");
            };
            if it.sender == caller {
                return Ok(());
            }
            if s.receipts.insert((item_id.into(), caller.into(), false)) {
                let at = s.now();
                s.push(
                    EventData::Ack {
                        item_id: item_id.into(),
                        device_id: caller.into(),
                        received_at_ms: at,
                    },
                    Some(caller),
                );
            }
            Ok(())
        })
    }

    fn read_item(&self, item_id: &str) -> TResult<()> {
        self.authed("read_item", |s, caller| {
            if !s.items.contains_key(item_id) {
                return api(404, "not_found");
            }
            if s.receipts.insert((item_id.into(), caller.into(), true)) {
                let at = s.now();
                s.push(
                    EventData::Read {
                        item_id: item_id.into(),
                        device_id: caller.into(),
                        read_at_ms: at,
                    },
                    Some(caller),
                );
            }
            Ok(())
        })
    }

    fn post_tombstone(&self, tombstone: &[u8]) -> TResult<Accepted> {
        self.authed("post_tombstone", |s, caller| {
            let t = Tombstone::from_bytes(tombstone)
                .map_err(|_| RelayError::api(422, "validation_failed"))?;
            if t.scope != DeletionScope::Everywhere
                || t.space_id != s.space_id
                || t.signer_ed25519 != s.devices[caller].record.sign_pk
                || t.verify_signature().is_err()
            {
                return api(422, "validation_failed");
            }
            if let Some(seq) = s.tombstones.get(&t.item_id) {
                return Ok(Accepted {
                    seq: *seq,
                    replayed: true,
                });
            }
            s.items.remove(&t.item_id);
            let seq = s.push(
                EventData::Tombstone {
                    item_id: t.item_id.clone(),
                    device_id: caller.into(),
                    deleted_at_ms: t.deleted_at_ms as i64,
                    tombstone: tombstone.to_vec(),
                },
                Some(caller),
            );
            s.tombstones.insert(t.item_id, seq);
            Ok(Accepted {
                seq,
                replayed: false,
            })
        })
    }

    fn post_attachment_tombstone(&self, statement: &[u8]) -> TResult<Accepted> {
        self.authed("post_attachment_tombstone", |s, caller| {
            if !s.features.iter().any(|f| f == "attachment_tombstones") {
                return api(404, "not_found");
            }
            let t = AttachmentTombstone::from_bytes(statement)
                .map_err(|_| RelayError::api(422, "validation_failed"))?;
            if t.space_id != s.space_id
                || t.signer_ed25519 != s.devices[caller].record.sign_pk
                || t.verify_signature().is_err()
            {
                return api(422, "validation_failed");
            }
            let key = (t.item_id.clone(), t.slot);
            if let Some(seq) = s.attachment_tombstones.get(&key) {
                return Ok(Accepted {
                    seq: *seq,
                    replayed: true,
                });
            }
            // Deletes the blob only when it is the one this item announced at this slot.
            let attached_here = s
                .items
                .get(&t.item_id)
                .is_some_and(|i| i.req.blob_ids.get(t.slot as usize) == Some(&t.blob_id));
            if s.blobs.contains_key(&t.blob_id) {
                if !attached_here {
                    return api(422, "validation_failed");
                }
                s.blobs.remove(&t.blob_id);
            }
            let seq = s.push(
                EventData::AttachmentTombstone {
                    item_id: t.item_id.clone(),
                    slot: t.slot,
                    blob_id: t.blob_id.clone(),
                    device_id: caller.into(),
                    deleted_at_ms: t.deleted_at_ms as i64,
                    statement: statement.to_vec(),
                },
                Some(caller),
            );
            s.attachment_tombstones.insert(key, seq);
            Ok(Accepted {
                seq,
                replayed: false,
            })
        })
    }

    fn post_item_state(
        &self,
        event_id: &str,
        key_epoch: u32,
        envelope: &[u8],
    ) -> TResult<Accepted> {
        self.authed("post_item_state", |s, caller| {
            if let Some((sender, env, epoch, seq)) = s.item_states.get(event_id) {
                return if sender == caller && env == envelope && *epoch == key_epoch {
                    Ok(Accepted {
                        seq: *seq,
                        replayed: true,
                    })
                } else {
                    api(409, "event_id_conflict")
                };
            }
            if key_epoch != s.key_epoch {
                return Err(RelayError::problem(ApiProblem {
                    status: 409,
                    code: "stale_key_epoch".into(),
                    key_epoch: Some(s.key_epoch),
                    ..ApiProblem::default()
                }));
            }
            let h = read_item_state_header(envelope)
                .map_err(|_| RelayError::api(422, "validation_failed"))?;
            if h.event_id != event_id || h.key_epoch != key_epoch {
                return api(422, "validation_failed");
            }
            let seq = s.push(
                EventData::ItemState {
                    event_id: event_id.into(),
                    device_id: caller.into(),
                    key_epoch,
                    envelope: envelope.to_vec(),
                },
                Some(caller),
            );
            s.item_states.insert(
                event_id.into(),
                (caller.into(), envelope.to_vec(), key_epoch, seq),
            );
            Ok(Accepted {
                seq,
                replayed: false,
            })
        })
    }

    fn create_blob(&self, blob_id: &str, size: u64) -> TResult<BlobStatus> {
        self.authed("create_blob", |s, caller| {
            if size == 0 || size > s.limits.max_blob_bytes {
                return api(413, "payload_too_large");
            }
            if let Some(b) = s.blobs.get(blob_id) {
                return if b.owner == caller && b.size == size {
                    Ok(blob_status(blob_id, b))
                } else {
                    api(409, "blob_id_conflict")
                };
            }
            let b = MockBlob {
                owner: caller.into(),
                size,
                data: Vec::new(),
                attached: false,
            };
            let st = blob_status(blob_id, &b);
            s.blobs.insert(blob_id.into(), b);
            Ok(st)
        })
    }

    fn blob_status(&self, blob_id: &str) -> TResult<BlobStatus> {
        self.authed("blob_status", |s, caller| match s.blobs.get(blob_id) {
            Some(b) if b.owner == caller => Ok(blob_status(blob_id, b)),
            _ => api(404, "not_found"),
        })
    }

    fn put_blob_part(&self, blob_id: &str, offset: u64, data: &[u8]) -> TResult<u64> {
        self.authed("put_blob_part", |s, caller| {
            let max_part = s.limits.max_upload_part_bytes;
            let stall = s.stall_uploads;
            let Some(b) = s.blobs.get_mut(blob_id).filter(|b| b.owner == caller) else {
                return api(404, "not_found");
            };
            if b.attached || b.data.len() as u64 == b.size {
                return api(409, "blob_attached");
            }
            if offset != b.data.len() as u64 {
                return Err(RelayError::problem(ApiProblem {
                    status: 409,
                    code: "upload_offset_mismatch".into(),
                    upload_offset: Some(b.data.len() as u64),
                    ..ApiProblem::default()
                }));
            }
            if data.is_empty() {
                return api(422, "validation_failed");
            }
            if data.len() as u64 > max_part || offset + data.len() as u64 > b.size {
                return api(413, "payload_too_large");
            }
            if stall {
                // A buggy relay: accepts the part but does not move the offset.
                return Ok(offset);
            }
            b.data.extend_from_slice(data);
            Ok(b.data.len() as u64)
        })
    }

    fn get_blob_range(&self, blob_id: &str, offset: u64, len: u64) -> TResult<Vec<u8>> {
        self.authed("get_blob_range", |s, _caller| {
            let Some(b) = s.blobs.get(blob_id) else {
                return api(404, "not_found");
            };
            if b.data.len() as u64 != b.size {
                return api(409, "blob_incomplete");
            }
            if offset >= b.size {
                return api(416, "range_not_satisfiable");
            }
            let end = (offset + len).min(b.size) as usize;
            Ok(b.data[offset as usize..end].to_vec())
        })
    }

    fn delete_blob(&self, blob_id: &str) -> TResult<()> {
        self.authed("delete_blob", |s, caller| {
            match s.blobs.get(blob_id) {
                Some(b) if b.owner != caller => return api(403, "forbidden"),
                Some(b) if b.attached => return api(409, "blob_attached"),
                Some(_) => {}
                None => return api(404, "not_found"),
            }
            s.blobs.remove(blob_id);
            Ok(())
        })
    }

    fn open_wake(&self) -> TResult<Box<dyn WakeSource>> {
        let token = lock(&self.token).clone();
        let device = {
            let mut s = lock(&self.relay.st);
            self.enter(&mut s, "open_wake")?;
            if !s.initialized {
                return api(409, "space_not_initialized");
            }
            token
                .and_then(|t| s.sessions.get(&t).map(|(d, _)| d.clone()))
                .ok_or_else(|| RelayError::api(401, "unauthorized"))?
        };
        Ok(Box::new(self.relay.wake_source(&device)))
    }

    fn post_transfer(&self, transfer: &PostTransfer) -> TResult<Accepted> {
        self.authed("post_transfer", |s, caller| {
            if let Some(t) = s.transfers.get(&transfer.transfer_id) {
                return if t.from == caller
                    && t.manifest == transfer.manifest
                    && t.blob_ids == transfer.blob_ids
                {
                    Ok(Accepted {
                        seq: 0,
                        replayed: true,
                    })
                } else {
                    api(409, "transfer_id_conflict")
                };
            }
            let m = HistoryTransfer::from_bytes(&transfer.manifest)
                .map_err(|_| RelayError::api(422, "validation_failed"))?;
            if m.verify_signature().is_err()
                || m.transfer_id != transfer.transfer_id
                || m.sender_ed25519 != s.devices[caller].record.sign_pk
            {
                return api(422, "validation_failed");
            }
            let to = s
                .devices
                .iter()
                .find(|(_, d)| {
                    !d.revoked
                        && d.record.sign_pk == m.recipient_ed25519
                        && d.record.device_id != caller
                })
                .map(|(id, d)| (id.clone(), d.record.joined_seq));
            let Some((to, joined)) = to else {
                return api(422, "validation_failed");
            };
            if m.as_of_seq < joined || m.as_of_seq > s.head {
                return api(422, "validation_failed");
            }
            for p in m.part_blob_ids() {
                if !transfer.blob_ids.contains(&p) {
                    return api(422, "validation_failed");
                }
            }
            let mut total = 0;
            for id in &transfer.blob_ids {
                match s.blobs.get(id) {
                    Some(b) if b.owner == caller => {
                        if b.data.len() as u64 != b.size {
                            return api(409, "blob_incomplete");
                        }
                        if b.attached {
                            return api(409, "blob_attached");
                        }
                        total += b.size;
                    }
                    _ => return api(404, "not_found"),
                }
            }
            let open = s
                .transfers
                .values()
                .filter(|t| t.open && t.to == to)
                .count() as u64;
            if open >= s.limits.transfer_max_open.unwrap_or(2) {
                return api(409, "transfer_limit");
            }
            for id in &transfer.blob_ids {
                if let Some(b) = s.blobs.get_mut(id) {
                    b.attached = true;
                }
            }
            s.transfers.insert(
                transfer.transfer_id.clone(),
                MockTransfer {
                    from: caller.into(),
                    to: to.clone(),
                    manifest: transfer.manifest.clone(),
                    blob_ids: transfer.blob_ids.clone(),
                    open: true,
                },
            );
            let exp = s.now() + 7 * 86_400_000;
            let seq = s.push(
                EventData::HistoryTransfer {
                    transfer_id: transfer.transfer_id.clone(),
                    from_device_id: caller.into(),
                    to_device_id: to,
                    total_bytes: total,
                    expires_at_ms: exp,
                },
                Some(caller),
            );
            Ok(Accepted {
                seq,
                replayed: false,
            })
        })
    }

    fn get_transfer(&self, transfer_id: &str) -> TResult<TransferInfo> {
        self.authed("get_transfer", |s, caller| {
            let Some(t) = s
                .transfers
                .get(transfer_id)
                .filter(|t| t.open && (t.from == caller || t.to == caller))
            else {
                return api(404, "not_found");
            };
            Ok(TransferInfo {
                transfer_id: transfer_id.into(),
                from_device_id: t.from.clone(),
                to_device_id: t.to.clone(),
                manifest: t.manifest.clone(),
                blobs: t
                    .blob_ids
                    .iter()
                    .map(|id| BlobRef {
                        blob_id: id.clone(),
                        size: s.blobs[id].size,
                    })
                    .collect(),
                expires_at_ms: s.now() + 7 * 86_400_000,
            })
        })
    }

    fn delete_transfer(&self, transfer_id: &str, outcome: &str) -> TResult<()> {
        self.authed("delete_transfer", |s, caller| {
            let Some(t) = s.transfers.get_mut(transfer_id) else {
                return api(404, "not_found");
            };
            if t.from != caller && t.to != caller {
                return api(404, "not_found");
            }
            if !t.open {
                return Ok(());
            }
            t.open = false;
            let ids = t.blob_ids.clone();
            for id in ids {
                s.blobs.remove(&id);
            }
            s.push(
                EventData::HistoryTransferClosed {
                    transfer_id: transfer_id.into(),
                    outcome: outcome.into(),
                },
                Some(caller),
            );
            Ok(())
        })
    }

    fn relay_authority(&self) -> Option<String> {
        lock(&self.url).as_deref().and_then(authority_of)
    }

    fn relay_url(&self) -> Option<String> {
        lock(&self.url).clone()
    }

    fn set_relay(&self, url: Option<&str>) -> TResult<()> {
        if let Some(u) = url {
            crate::relay_url::check_base_url(u).map_err(|e| RelayError::Network(e.to_string()))?;
        }
        // The session goes first (see `HttpTransport::set_relay`).
        *lock(&self.token) = None;
        *lock(&self.url) = url.map(str::to_owned);
        Ok(())
    }

    /// `POST /v1/space` (`api-v1.md` §3.2), checks in the order of the spec.
    fn post_space(&self, req: &NewSpace) -> TResult<CreatedSpace> {
        let r = self.open("post_space", |s| {
            let r = (|| {
                if s.initialized {
                    return api(409, "space_exists");
                }
                // Dashes and blanks are ignored, the case too (§3.1).
                let norm = |t: &str| {
                    t.chars()
                        .filter(|c| *c != '-' && !c.is_whitespace())
                        .collect::<String>()
                        .to_lowercase()
                };
                if s.install_token.as_deref().map(norm) != Some(norm(&req.install_token)) {
                    return api(403, "install_token_invalid");
                }
                let invalid = || RelayError::api(422, "validation_failed");
                let cert = DeviceCert::from_bytes(&req.certificate).map_err(|_| invalid())?;
                if cert.space_id != req.space_id
                    || !cert.is_self_signed()
                    || cert.verify_signature().is_err()
                    || !valid_name(&req.name)
                    || !valid_platform(&req.platform)
                {
                    return Err(invalid());
                }
                let sign_pk = cert.subject.ed25519.clone();
                let device_id = device_id_from_ed25519(&sign_pk).map_err(|_| invalid())?;
                s.initialized = true;
                s.install_token = None;
                s.space_id = req.space_id.clone();
                s.key_epoch = 1;
                let now = s.now();
                let record = DeviceRecord {
                    device_id: device_id.clone(),
                    name: req.name.trim().into(),
                    platform: req.platform.clone(),
                    sign_pk,
                    kx_pk: cert.subject.x25519.clone(),
                    certificate: req.certificate.clone(),
                    issuer_device_id: device_id.clone(),
                    joined_seq: 0,
                    registered_at_ms: now,
                    last_seen_at_ms: None,
                    active: Some(false),
                };
                s.devices.insert(
                    device_id.clone(),
                    MockDevice {
                        record: record.clone(),
                        revoked: false,
                    },
                );
                let seq = s.push(EventData::DeviceAdded { device: record }, None);
                Ok(CreatedSpace {
                    space_id: req.space_id.clone(),
                    created_at_ms: now,
                    device_id,
                    key_epoch: 1,
                    head_seq: seq,
                })
            })();
            s.lose("post_space", r)
        });
        self.relay.cv.notify_all();
        r
    }

    /// `POST /v1/devices` (`api-v1.md` §4.3), checks in the order of the spec.
    fn post_device(&self, req: &NewDevice) -> TResult<DeviceRecord> {
        let r = self.authed("post_device", |s, caller| {
            s.purge_channels();
            let r = (|| {
                let state = match s.channels.get(&req.channel_id) {
                    Some(c) if c.creator == caller => c.state,
                    _ => return api(404, "not_found"),
                };
                if state != ChannelState::Claimed {
                    return api(409, "channel_closed");
                }
                if req.key_epoch != s.key_epoch {
                    return Err(RelayError::problem(ApiProblem {
                        status: 409,
                        code: "epoch_conflict".into(),
                        key_epoch: Some(s.key_epoch),
                        ..ApiProblem::default()
                    }));
                }
                let invalid = || RelayError::api(422, "validation_failed");
                let cert = DeviceCert::from_bytes(&req.certificate).map_err(|_| invalid())?;
                let caller_pk = s.devices[caller].record.sign_pk.clone();
                if cert.space_id != s.space_id
                    || cert.issuer_ed25519 != caller_pk
                    || cert.is_self_signed()
                    || cert.verify_signature().is_err()
                    || !valid_name(&req.name)
                    || !valid_platform(&req.platform)
                {
                    return Err(invalid());
                }
                let sign_pk = cert.subject.ed25519.clone();
                if s.devices.values().any(|d| d.record.sign_pk == sign_pk) {
                    return api(409, "device_exists");
                }
                let members = s.devices.values().filter(|d| !d.revoked).count() as u64;
                if members >= s.limits.max_devices {
                    return Err(RelayError::problem(ApiProblem {
                        status: 409,
                        code: "too_many_devices".into(),
                        ..ApiProblem::default()
                    }));
                }
                let device_id = device_id_from_ed25519(&sign_pk).map_err(|_| invalid())?;
                let record = DeviceRecord {
                    device_id: device_id.clone(),
                    name: req.name.trim().into(),
                    platform: req.platform.clone(),
                    sign_pk,
                    kx_pk: cert.subject.x25519.clone(),
                    certificate: req.certificate.clone(),
                    issuer_device_id: caller.into(),
                    joined_seq: s.head,
                    registered_at_ms: s.now(),
                    last_seen_at_ms: None,
                    active: Some(false),
                };
                s.devices.insert(
                    device_id,
                    MockDevice {
                        record: record.clone(),
                        revoked: false,
                    },
                );
                s.push(
                    EventData::DeviceAdded {
                        device: record.clone(),
                    },
                    Some(caller),
                );
                if let Some(c) = s.channels.get_mut(&req.channel_id) {
                    c.state = ChannelState::Completed;
                }
                Ok(record)
            })();
            s.lose("post_device", r)
        });
        self.relay.cv.notify_all();
        r
    }

    fn create_pairing_channel(&self) -> TResult<PairingChannel> {
        let r = self.authed("create_pairing_channel", |s, caller| {
            s.purge_channels();
            let open = s
                .channels
                .values()
                .filter(|c| matches!(c.state, ChannelState::Open | ChannelState::Claimed))
                .count();
            if open >= s.pairing_max_open {
                return api(409, "channel_limit");
            }
            // Random in 1..=9999, not used by a channel that exists.
            let channel_id = loop {
                let Ok(random) = crate::crypto::random_array::<4>() else {
                    return api(500, "internal");
                };
                let n = u32::from_le_bytes(random) % 9999 + 1;
                let id = n.to_string();
                if !s.channels.contains_key(&id) {
                    break id;
                }
            };
            let expires_ms = s.now() + s.pairing_ttl_ms;
            s.channels.insert(
                channel_id.clone(),
                MockChannel {
                    creator: caller.into(),
                    state: ChannelState::Open,
                    expires_ms,
                    token: None,
                    inviter: Vec::new(),
                    joiner: Vec::new(),
                    next_index: 1,
                },
            );
            Ok(PairingChannel {
                channel_id,
                state: ChannelState::Open,
                expires_at_ms: expires_ms,
            })
        });
        self.relay.cv.notify_all();
        r
    }

    fn claim_pairing_channel(&self, channel_id: &str) -> TResult<PairingClaim> {
        let r = self.open("claim_pairing_channel", |s| {
            s.purge_channels();
            let now = s.now();
            let ttl = s.pairing_ttl_ms;
            let Some(c) = s.channels.get_mut(channel_id) else {
                return api(404, "not_found");
            };
            match c.state {
                ChannelState::Open => {
                    let Ok(random) = crate::crypto::random_array::<24>() else {
                        return api(500, "internal");
                    };
                    let token = format!("tmp1.{}", B64.encode(random));
                    c.state = ChannelState::Claimed;
                    c.token = Some(token.clone());
                    // One prolongation: 120 s after the claim.
                    c.expires_ms = now + ttl;
                    Ok(PairingClaim {
                        pairing_token: token,
                        expires_at_ms: c.expires_ms,
                    })
                }
                ChannelState::Claimed => api(409, "channel_claimed"),
                ChannelState::Completed | ChannelState::Closed => api(409, "channel_closed"),
            }
        });
        self.relay.cv.notify_all();
        r
    }

    fn post_pairing_message(
        &self,
        channel_id: &str,
        auth: PairingAuth<'_>,
        body: &[u8],
    ) -> TResult<u64> {
        let session = lock(&self.token).clone();
        let r = self.open("post_pairing_message", |s| {
            s.purge_channels();
            let r = (|| {
                let side = pairing_side(s, channel_id, auth, session.as_deref())?;
                let (max_bytes, max_msgs) = (s.pairing_max_message_bytes, s.pairing_max_messages);
                if body.is_empty() {
                    return api(422, "validation_failed");
                }
                if body.len() > max_bytes {
                    return api(413, "payload_too_large");
                }
                let Some(c) = s.channels.get_mut(channel_id) else {
                    return api(404, "not_found");
                };
                if matches!(c.state, ChannelState::Completed | ChannelState::Closed) {
                    return api(409, "channel_closed");
                }
                let mine = match side {
                    PairingSide::Inviter => &mut c.inviter,
                    PairingSide::Joiner => &mut c.joiner,
                };
                if mine.len() >= max_msgs {
                    return api(409, "channel_limit");
                }
                let index = c.next_index;
                c.next_index += 1;
                mine.push((index, body.to_vec()));
                Ok(index)
            })();
            s.lose("post_pairing_message", r)
        });
        self.relay.cv.notify_all();
        r
    }

    fn pairing_messages(
        &self,
        channel_id: &str,
        auth: PairingAuth<'_>,
        after: u64,
        wait_secs: u32,
    ) -> TResult<PairingMessages> {
        let session = lock(&self.token).clone();
        let mut s = lock(&self.relay.st);
        s.pairing_waits.push(wait_secs);
        self.enter(&mut s, "pairing_messages")?;
        let cap = Duration::from_millis(s.long_poll_cap_ms.min(u64::from(wait_secs) * 1000));
        let started = Instant::now();
        let mut baseline: Option<ChannelState> = None;
        loop {
            s.purge_channels();
            let side = pairing_side(&s, channel_id, auth, session.as_deref())?;
            let Some(c) = s.channels.get(channel_id) else {
                return api(404, "not_found");
            };
            let other = match side {
                PairingSide::Inviter => (&c.joiner, PairingSide::Joiner),
                PairingSide::Joiner => (&c.inviter, PairingSide::Inviter),
            };
            let messages: Vec<PairingMessage> = other
                .0
                .iter()
                .filter(|(index, _)| *index > after)
                .map(|(index, body)| PairingMessage {
                    index: *index,
                    side: other.1,
                    body: body.clone(),
                })
                .collect();
            let state = c.state;
            let expires_at_ms = c.expires_ms;
            // A terminated channel (completed or closed) answers at once, whatever `wait` asks:
            // no message can arrive any more (matches the real relay, `api-v1.md` §4.6). A read
            // that waited on it would only burn `rl_pairing_poll` against the live relay.
            let terminated = matches!(state, ChannelState::Completed | ChannelState::Closed);
            // Otherwise a message or a change of state ends the wait; so does the cap.
            let base = *baseline.get_or_insert(state);
            if terminated || !messages.is_empty() || state != base || started.elapsed() >= cap {
                return Ok(PairingMessages {
                    state,
                    expires_at_ms,
                    messages,
                });
            }
            let remaining = cap.saturating_sub(started.elapsed());
            s = self
                .relay
                .cv
                .wait_timeout(s, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn close_pairing_channel(&self, channel_id: &str, auth: PairingAuth<'_>) -> TResult<()> {
        let session = lock(&self.token).clone();
        let r = self.open("close_pairing_channel", |s| {
            s.purge_channels();
            pairing_side(s, channel_id, auth, session.as_deref())?;
            if let Some(c) = s.channels.get_mut(channel_id) {
                // Closed: the messages are erased; the channel stays until its deadline.
                c.state = ChannelState::Closed;
                c.inviter.clear();
                c.joiner.clear();
            }
            Ok(())
        });
        self.relay.cv.notify_all();
        r
    }
}

/// Which side the caller of a pairing route is, or the error the relay answers
/// (`api-v1.md` §2.4, §4.6): the inviter by its session, only on a channel it created; the new
/// device by the token of the claim.
fn pairing_side(
    s: &State,
    channel_id: &str,
    auth: PairingAuth<'_>,
    session: Option<&str>,
) -> TResult<PairingSide> {
    match auth {
        PairingAuth::Inviter => {
            let Some((device, expires)) = session.and_then(|t| s.sessions.get(t)) else {
                return api(401, "unauthorized");
            };
            if *expires <= s.now() {
                return api(401, "session_expired");
            }
            if s.devices.get(device).is_none_or(|d| d.revoked) {
                return api(403, "revoked");
            }
            match s.channels.get(channel_id) {
                Some(c) if c.creator == *device => Ok(PairingSide::Inviter),
                _ => api(404, "not_found"),
            }
        }
        PairingAuth::Joiner(token) => match s.channels.get(channel_id) {
            None => api(404, "not_found"),
            Some(c) if c.token.as_deref() == Some(token) => Ok(PairingSide::Joiner),
            Some(_) => api(401, "unauthorized"),
        },
    }
}
