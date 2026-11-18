//! Registry of the open WebSocket connections (`api-v1.md` §5): the doorbell. A connection
//! is a channel to the task that owns the socket; the hub never touches a socket itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::db::lock;

/// Close codes of §5.4.
pub mod close {
    pub const GOING_AWAY: u16 = 1001;
    pub const PROTOCOL_ERROR: u16 = 4000;
    pub const UNAUTHORIZED: u16 = 4001;
    pub const SESSION_EXPIRED: u16 = 4002;
    pub const REVOKED: u16 = 4003;
    pub const SPACE_RESET: u16 = 4004;
    pub const REPLACED: u16 = 4008;
    /// `try_again_later`: the space of the session is pending (§4.4.1, §5.4).
    pub const TRY_AGAIN_LATER: u16 = 1013;
}

/// Most connections kept open for one device (§5.4: the oldest one is replaced).
pub const MAX_CONNECTIONS_PER_DEVICE: usize = 3;

/// A message from the hub to the task that owns a connection.
#[derive(Debug)]
pub enum Down {
    /// `{"type":"wake","seq":N,"ring":b}`.
    Wake { seq: i64, ring: bool },
    /// Close with a code; `ack` fires once the close frame has been written.
    Close {
        code: u16,
        reason: &'static str,
        ack: Option<oneshot::Sender<()>>,
    },
}

struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<Down>,
}

/// Connections by device.
#[derive(Default)]
pub struct Hub {
    conns: Mutex<HashMap<String, Vec<Conn>>>,
    next_id: AtomicU64,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a connection of `device`. Beyond [`MAX_CONNECTIONS_PER_DEVICE`] the oldest
    /// one is closed with `4008 replaced`.
    pub fn register(&self, device: &str) -> (u64, mpsc::UnboundedReceiver<Down>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut map = lock(&self.conns);
        let list = map.entry(device.to_owned()).or_default();
        while list.len() >= MAX_CONNECTIONS_PER_DEVICE {
            let oldest = list.remove(0);
            let _ = oldest.tx.send(Down::Close {
                code: close::REPLACED,
                reason: "replaced",
                ack: None,
            });
        }
        list.push(Conn { id, tx });
        (id, rx)
    }

    /// Forgets a connection (its task ended).
    pub fn unregister(&self, device: &str, id: u64) {
        let mut map = lock(&self.conns);
        if let Some(list) = map.get_mut(device) {
            list.retain(|c| c.id != id);
            if list.is_empty() {
                map.remove(device);
            }
        }
    }

    /// An authenticated connection of `device` is open.
    pub fn has_connection(&self, device: &str) -> bool {
        lock(&self.conns).get(device).is_some_and(|l| !l.is_empty())
    }

    /// Number of open connections of `device`.
    pub fn connection_count(&self, device: &str) -> usize {
        lock(&self.conns).get(device).map_or(0, Vec::len)
    }

    /// Sends a `wake` to every connection of `device`. Returns whether one existed.
    pub fn wake(&self, device: &str, seq: i64, ring: bool) -> bool {
        let map = lock(&self.conns);
        let Some(list) = map.get(device) else {
            return false;
        };
        for c in list {
            let _ = c.tx.send(Down::Wake { seq, ring });
        }
        !list.is_empty()
    }

    /// Closes every connection of `device` and waits (briefly) until the close frames are
    /// written: a revoked device must be cut off *before* the revoker gets its answer.
    pub async fn close_device(&self, device: &str, code: u16, reason: &'static str) {
        let taken = lock(&self.conns).remove(device).unwrap_or_default();
        Self::close_list(taken, code, reason).await;
    }

    /// Closes every connection of the given devices (the reset of one space: the devices of
    /// the other spaces are not touched).
    pub async fn close_devices(&self, devices: &[String], code: u16, reason: &'static str) {
        let taken: Vec<Conn> = {
            let mut map = lock(&self.conns);
            devices
                .iter()
                .filter_map(|d| map.remove(d))
                .flatten()
                .collect()
        };
        Self::close_list(taken, code, reason).await;
    }

    /// Closes every connection (shutdown).
    pub async fn close_all(&self, code: u16, reason: &'static str) {
        let taken: Vec<Conn> = lock(&self.conns).drain().flat_map(|(_, v)| v).collect();
        Self::close_list(taken, code, reason).await;
    }

    async fn close_list(list: Vec<Conn>, code: u16, reason: &'static str) {
        let mut acks = Vec::new();
        for c in list {
            let (ack, wait) = oneshot::channel();
            if c.tx
                .send(Down::Close {
                    code,
                    reason,
                    ack: Some(ack),
                })
                .is_ok()
            {
                acks.push(wait);
            }
        }
        for wait in acks {
            let _ = tokio::time::timeout(Duration::from_secs(2), wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_wake_and_close() {
        let hub = Hub::new();
        let (id, mut rx) = hub.register("dev");
        assert!(hub.has_connection("dev"));
        assert!(hub.wake("dev", 5, true));
        assert!(matches!(
            rx.recv().await,
            Some(Down::Wake { seq: 5, ring: true })
        ));
        assert!(!hub.wake("other", 1, false));
        let h = std::sync::Arc::new(hub);
        let h2 = h.clone();
        let closer =
            tokio::spawn(async move { h2.close_device("dev", close::REVOKED, "revoked").await });
        match rx.recv().await {
            Some(Down::Close { code, ack, .. }) => {
                assert_eq!(code, 4003);
                if let Some(a) = ack {
                    let _ = a.send(());
                }
            }
            other => panic!("unexpected {other:?}"),
        }
        closer.await.unwrap();
        assert!(!h.has_connection("dev"));
        h.unregister("dev", id);
    }

    #[tokio::test]
    async fn closing_some_devices_leaves_the_others_connected() {
        let hub = std::sync::Arc::new(Hub::new());
        let (_a, mut rx_a) = hub.register("a");
        let (_b, mut rx_b) = hub.register("b");
        let (_c, mut rx_c) = hub.register("c");
        let h = hub.clone();
        let closer = tokio::spawn(async move {
            h.close_devices(
                &["a".to_owned(), "b".to_owned()],
                close::SPACE_RESET,
                "space_reset",
            )
            .await;
        });
        for rx in [&mut rx_a, &mut rx_b] {
            match rx.recv().await {
                Some(Down::Close { code, ack, .. }) => {
                    assert_eq!(code, close::SPACE_RESET);
                    if let Some(a) = ack {
                        let _ = a.send(());
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        closer.await.unwrap();
        assert!(!hub.has_connection("a") && !hub.has_connection("b"));
        assert!(hub.has_connection("c"));
        assert!(rx_c.try_recv().is_err(), "the third device heard nothing");
    }

    #[tokio::test]
    async fn fourth_connection_replaces_the_oldest() {
        let hub = Hub::new();
        let (_a, mut rx_a) = hub.register("d");
        let _b = hub.register("d");
        let _c = hub.register("d");
        let _d = hub.register("d");
        assert_eq!(hub.connection_count("d"), 3);
        match rx_a.recv().await {
            Some(Down::Close { code, .. }) => assert_eq!(code, close::REPLACED),
            other => panic!("unexpected {other:?}"),
        }
    }
}
