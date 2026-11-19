//! Shared application state and the effects that follow a committed transaction.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap};
use ipnet::IpNet;
use rusqlite::Transaction;
use tokio::sync::watch;

use crate::blobstore;
use crate::clock::Clock;
use crate::config::{Config, Timing};
use crate::db::{lock, Tx};
use crate::disk::DiskProbe;
use crate::error::ApiError;
use crate::hub::{close, Hub};
use crate::push::{PushDispatcher, Urgency};
use crate::ratelimit::Limiter;
use crate::spaces::{DeleteOutcome, RegistryGuard, Space, Spaces};
use crate::store::Budget;

/// The state shared by every request and background task.
pub struct Inner {
    pub cfg: Config,
    pub timing: Timing,
    pub clock: Arc<dyn Clock>,
    /// The spaces of the relay, `routing.db` and the routing index.
    pub spaces: Arc<Spaces>,
    pub hub: Arc<Hub>,
    pub limiter: Limiter,
    pub disk: Arc<dyn DiskProbe>,
    pub push: PushDispatcher,
    pub trusted: Vec<IpNet>,
    pub pairing: PairingWatch,
    /// The web administration panel; `None` unless `admin_web` is on (and the relay may write).
    pub admin_web: Option<crate::admin_web::AdminWeb>,
    /// Blobs whose part is being written right now (one part at a time per blob).
    pub blob_busy: Mutex<HashSet<[u8; 16]>>,
    /// Held for the lifetime of a serving relay (`relay.lock`, §8.7).
    pub lock_file: Mutex<Option<std::fs::File>>,
}

/// Cheap handle on [`Inner`].
pub type AppState = Arc<Inner>;

impl Inner {
    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Free bytes on the filesystem of `data_dir`.
    pub fn disk_free(&self) -> u64 {
        self.disk.available_bytes(&self.cfg.data_dir)
    }

    /// Address of the caller: the TCP peer, or the client announced by a trusted proxy in
    /// `X-Forwarded-For` (read from the right, skipping trusted hops, §8.5).
    pub fn client_ip(&self, headers: &HeaderMap, extensions: &Extensions) -> IpAddr {
        let peer = extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |c| c.0.ip())
            .to_canonical();
        if !self.is_trusted(peer) {
            return peer;
        }
        // Several header lines are one list (RFC 9110 §5.3): a proxy that adds its own line
        // instead of appending to the client's must not let the client's line win.
        let mut chain: Vec<&str> = Vec::new();
        for line in headers.get_all("x-forwarded-for") {
            let Ok(line) = line.to_str() else {
                return peer;
            };
            chain.extend(line.split(','));
        }
        // Read from the right: the entries written by the trusted proxies come last. The first
        // one that is not a trusted proxy is the client. An entry that is not an address, met
        // before that point, makes the whole header untrustworthy: the TCP peer (the proxy)
        // counts, which is the conservative bucket, never one the client could choose.
        let mut last = None;
        for hop in chain.iter().rev() {
            let Ok(ip) = hop.trim().parse::<IpAddr>() else {
                return peer;
            };
            let ip = ip.to_canonical();
            if !self.is_trusted(ip) {
                return ip;
            }
            last = Some(ip);
        }
        // Every hop is a trusted proxy: the outermost one is the best that is known.
        last.unwrap_or(peer)
    }

    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trusted.iter().any(|n| n.contains(&ip))
    }

    /// What `space` may still take: its quota, the room left in the total of the relay, the
    /// free disk (§8.4). Blocking: it reads the other spaces, so it is never called from inside
    /// a database closure.
    pub fn budget_sync(&self, space: &Space) -> Result<Budget, ApiError> {
        Ok(Budget {
            others_used: self.spaces.others_used(Some(&space.id))?,
            space_max: self.spaces.space_max(&space.id, &self.cfg),
            relay_max: self.cfg.max_relay_bytes,
            min_free_disk: self.cfg.min_free_disk_bytes,
            disk_free: self.disk_free(),
        })
    }

    /// [`Inner::budget_sync`] on the blocking pool.
    pub async fn budget(self: &Arc<Self>, space: &Arc<Space>) -> Result<Budget, ApiError> {
        let (st, space) = (Arc::clone(self), Arc::clone(space));
        tokio::task::spawn_blocking(move || st.budget_sync(&space))
            .await
            .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    }

    /// A write transaction of `space` that reserves room (an item, an item state, a blob, a
    /// transfer). The reservation lock is held from the moment the budget is read until the
    /// commit, so that two spaces cannot both take the last bytes of the total (§8.4). The
    /// closure receives the budget for `store::check_quota`.
    pub async fn reserve<R, F>(self: &Arc<Self>, space: &Arc<Space>, f: F) -> Result<R, ApiError>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction, &Budget) -> Result<R, ApiError> + Send + 'static,
    {
        let (st, space) = (Arc::clone(self), Arc::clone(space));
        tokio::task::spawn_blocking(move || {
            let _serial = st.spaces.serialize_reservations();
            let budget = st.budget_sync(&space)?;
            space.db.write_sync(|tx| f(tx, &budget).map(Tx::Commit))
        })
        .await
        .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    }

    /// Deletes a space for good (`api-v1.md` §4.4.1, steps 1 to 7): unregistering (the space is
    /// pending), presence check, positive act on the disk made durable, one transaction of
    /// `routing.db` (quota, consumed invitation, receipt), its identifiers freed, and only then
    /// its WebSockets closed with `4004`. Nothing else is touched: the other spaces, their
    /// connections and `public/` stay. No invitation is created (the relay keeps no token in
    /// clear, so it could not show one).
    ///
    /// Shared by `admin reset` (`receipt` = `None`: a reset by hand writes no receipt, §4.4.2)
    /// and `DELETE /v1/space`. The registry guard is the proof that the caller holds
    /// [`Spaces::lock_registry`]: nothing registers a device, a channel or a space between the
    /// caller's checks and the unregistering. It is **moved** into a task that does the whole
    /// deletion and is released when that task ends, not before. The caller only waits for the
    /// task: if it is cancelled meanwhile (a client that hangs up makes the server drop the
    /// request), the deletion goes on to its end, lock held.
    ///
    /// The WebSockets of the space stay open unless the space is gone for good
    /// ([`DeleteOutcome::Deleted`] or [`DeleteOutcome::DeletedWithoutReceipt`]): `4004` is never
    /// sent for a deletion that may not have happened. A task that fails (a panic outside the
    /// part [`Spaces::delete`] guards) is an unknown state: the space stays pending.
    pub async fn delete_space(
        &self,
        space: &Space,
        registry: RegistryGuard,
        receipt: Option<[u8; 32]>,
    ) -> DeleteOutcome {
        let (spaces, hub, clock) = (
            Arc::clone(&self.spaces),
            Arc::clone(&self.hub),
            Arc::clone(&self.clock),
        );
        let id = space.id;
        let task = tokio::spawn(async move {
            let _registry = registry;
            let devices = spaces.devices_of(&id);
            // 1. Unregistered first: no request reaches the space, nobody can log in.
            let Some(space) = spaces.unregister(&id) else {
                return DeleteOutcome::NotRegistered;
            };
            if let Some(pause) = spaces.take_deletion_pause() {
                pause.reached.add_permits(1);
                if let Ok(permit) = pause.resume.acquire().await {
                    permit.forget();
                }
            }
            let now = clock.now_ms();
            let blocking = Arc::clone(&spaces);
            let outcome =
                tokio::task::spawn_blocking(move || blocking.delete(&space, receipt, now))
                    .await
                    .unwrap_or(DeleteOutcome::Unknown);
            // 6. Only a space that is gone for good has its sockets closed.
            if matches!(
                outcome,
                DeleteOutcome::Deleted | DeleteOutcome::DeletedWithoutReceipt
            ) {
                hub.close_devices(&devices, close::SPACE_RESET, "space_reset")
                    .await;
            }
            outcome
        });
        task.await.unwrap_or(DeleteOutcome::Unknown)
    }

    /// Applies the effects of a committed transaction of `space`. Closing the sockets of a
    /// revoked device is done *before* returning, so the revoker's answer comes after the cut.
    pub async fn apply(&self, space: &Space, fx: Effects) {
        if !fx.unlink_blobs.is_empty() {
            let dir = space.dir.clone();
            let ids = fx.unlink_blobs;
            let _ = tokio::task::spawn_blocking(move || {
                for id in ids {
                    blobstore::remove(&dir, &id);
                }
            })
            .await;
        }
        for dev in &fx.close_ws {
            self.hub.close_device(dev, close::REVOKED, "revoked").await;
        }
        for w in fx.wakes {
            for t in w.targets {
                let delivered = self.hub.wake(&t.device, w.seq, t.ring);
                if !delivered && w.fcm {
                    self.push.notify(&t.device, t.urgency);
                }
            }
        }
        for ch in fx.pairing {
            self.pairing.notify(&ch);
        }
    }
}

/// What to do once a transaction has committed.
#[derive(Default)]
pub struct Effects {
    pub wakes: Vec<WakeEvent>,
    /// Devices whose WebSockets are closed with `4003`.
    pub close_ws: Vec<String>,
    /// Blob files to delete.
    pub unlink_blobs: Vec<[u8; 16]>,
    /// Pairing channels whose state or messages changed.
    pub pairing: Vec<String>,
}

impl Effects {
    pub fn merge(&mut self, other: Effects) {
        self.wakes.extend(other.wakes);
        self.close_ws.extend(other.close_ws);
        self.unlink_blobs.extend(other.unlink_blobs);
        self.pairing.extend(other.pairing);
    }
}

/// One doorbell: a new event at `seq` for some devices.
pub struct WakeEvent {
    pub seq: i64,
    /// Whether the event kind also goes through push (§4.10, column "Sonnette FCM").
    pub fcm: bool,
    pub targets: Vec<Target>,
}

/// A device to ring and how.
pub struct Target {
    pub device: String,
    /// `ring` of the `wake` message (the decision of §6.1 for an item, `false` otherwise).
    pub ring: bool,
    pub urgency: Urgency,
}

impl Target {
    pub fn quiet(device: &str) -> Self {
        Self {
            device: device.to_owned(),
            ring: false,
            urgency: Urgency::Normal,
        }
    }
}

/// Wake-ups for the long poll of the pairing channel.
#[derive(Default)]
pub struct PairingWatch {
    map: Mutex<HashMap<String, watch::Sender<u64>>>,
}

impl PairingWatch {
    /// A receiver that fires when the channel changes.
    pub fn subscribe(&self, channel_id: &str) -> watch::Receiver<u64> {
        let mut map = lock(&self.map);
        if map.len() > 1024 {
            map.retain(|_, tx| tx.receiver_count() > 0);
        }
        map.entry(channel_id.to_owned())
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    /// Announces a change of the channel.
    pub fn notify(&self, channel_id: &str) {
        if let Some(tx) = lock(&self.map).get(channel_id) {
            tx.send_modify(|v| *v = v.wrapping_add(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pairing_watch_wakes_a_subscriber() {
        let w = PairingWatch::default();
        let mut rx = w.subscribe("42");
        w.notify("42");
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
            .await
            .unwrap()
            .unwrap();
        // No subscriber, no panic.
        w.notify("nobody");
    }
}
