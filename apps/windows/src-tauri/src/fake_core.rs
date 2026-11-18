//! In-memory stand-in for the real `tomi-core`: the demo mode (debug builds only, started
//! with `TOMI_DEMO=1`). No crypto, no network, nothing is kept between two runs.
//!
//! It follows the same contract as [`crate::real_core::RealCore`] so that the interface can
//! be tried without a relay: a pin is shared "between devices" only in the sense that the
//! item is re-emitted; an `afterFirstRead` item is consumed at once by its first read.
//!
//! Attachments are copied into a private directory (wiped at start) so that previews,
//! "Enregistrer", drag-out and cleanup of staged files behave like they will with the real
//! core. Demo content only: never put anything real in it.

use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
#[cfg(test)]
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::core_api::{
    clean_device_name, CoreError, CoreEvent, CoreStatus, DepartureOutcome, DepartureView,
    PairingOptions, PairingPeerView, PairingSession, PairingStage, PairingView, ProgressFn,
    SendOutcome, SendProgress, TomiCore,
};
use crate::files;
use crate::inactivity::{inactive_days, INACTIVE_AFTER_DAYS};
use crate::model::{
    now_ms, Delivery, DeliveryState, Device, DeviceRef, DeviceState, Item, ItemDraft, ReadPolicy,
    Transfer,
};
use crate::qr;
use crate::secret::SecretText;

mod departure;
pub use departure::RelayMode;
#[cfg(test)]
pub use departure::{Hold, Reply, Step};

/// The code the demo invitation shows (`7-crayon-lune` was never a valid code: `lune` is not
/// in the word list) and the verification code the demo "new device" shows, as in the
/// Android demo.
const DEMO_CODE: &str = "4821-crayon-renard";
const DEMO_LINK: &str = "https://relay.example.org/l/pair#4821-crayon-renard";
const DEMO_SAS: &str = "482 913";
/// The demo "Pixel" answers after this long, and the pairing ends this long after "Ajouter".
const DEMO_PEER_AFTER: Duration = Duration::from_millis(2500);
const DEMO_DONE_AFTER: Duration = Duration::from_millis(1200);
/// The code of a real invitation lives 120 s on the relay.
const DEMO_CODE_LIFE_MS: i64 = 120_000;

/// The demo invitation: a clock, not a protocol. Nothing is sent anywhere.
#[derive(Default)]
struct DemoInvitation {
    started: Option<Instant>,
    started_at_ms: i64,
    confirmed: Option<Instant>,
}

pub struct FakeCore {
    dir: PathBuf,
    items: Mutex<Vec<Item>>,
    tx: broadcast::Sender<CoreEvent>,
    /// Behind a lock because `rename_device` changes its name.
    this: Mutex<Device>,
    /// The other devices of the demo space; `revoke_device` removes one.
    others: Mutex<Vec<Device>>,
    /// Per-step pause while "uploading", to make progress visible.
    step_delay: Duration,
    invitation: Mutex<DemoInvitation>,
    /// The simulated relay and the departure state (see `departure.rs`).
    dep: Mutex<departure::Sim>,
    /// Moves the clock of the core (tests: the 15-minute window, the 10-minute verdict).
    skew_ms: AtomicI64,
    /// Test hook: the next attempt waits here (see `hold_next_attempt`).
    #[cfg(test)]
    held: Mutex<Option<Arc<departure::Hold>>>,
}

impl FakeCore {
    pub fn new(dir: PathBuf) -> Self {
        // Demo content of an earlier run is not worth keeping on disk.
        let _ = std::fs::remove_dir_all(&dir);
        let (tx, _) = broadcast::channel(64);
        Self {
            dir,
            items: Mutex::new(Vec::new()),
            tx,
            this: Mutex::new(demo_device("dev-this", &hostname(), "windows", true)),
            others: Mutex::new(demo_others()),
            step_delay: Duration::from_millis(60),
            invitation: Mutex::new(DemoInvitation::default()),
            dep: Mutex::new(departure::Sim::default()),
            skew_ms: AtomicI64::new(0),
            #[cfg(test)]
            held: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub fn without_delay(mut self) -> Self {
        self.step_delay = Duration::ZERO;
        self
    }

    fn demo(&self) -> std::sync::MutexGuard<'_, DemoInvitation> {
        self.invitation.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Tells the windows, after `after`, that the demo invitation moved on.
    fn announce_pairing_in(&self, after: Duration) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            let _ = tx.send(CoreEvent::PairingChanged);
        });
    }

    /// The demo pairing screen, from the time elapsed since the demo "invitation" started.
    fn demo_view(&self) -> PairingView {
        view_of(&self.demo())
    }

    fn this_lock(&self) -> std::sync::MutexGuard<'_, Device> {
        self.this.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn others_lock(&self) -> std::sync::MutexGuard<'_, Vec<Device>> {
        self.others.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Item>> {
        self.items.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, e: CoreEvent) {
        let _ = self.tx.send(e);
    }

    fn is_erased_now(&self) -> bool {
        self.dep.lock().unwrap_or_else(|p| p.into_inner()).erased
    }

    fn purge_expired(&self) -> Vec<String> {
        let now = now_ms();
        let mut items = self.lock();
        let gone: Vec<String> = items
            .iter()
            .filter(|i| i.expires_at.is_some_and(|e| e <= now))
            .map(|i| i.id.clone())
            .collect();
        items.retain(|i| !gone.contains(&i.id));
        drop(items);
        for id in &gone {
            let _ = std::fs::remove_dir_all(self.dir.join(id));
            self.emit(CoreEvent::ItemRemoved(id.clone()));
        }
        gone
    }

    /// Simulates an item arriving from another device (demo and tests).
    pub fn inject_incoming(&self, mut item: Item) {
        item.outgoing = false;
        item.read = false;
        self.lock().push(item.clone());
        self.emit(CoreEvent::ItemAdded(item));
    }

    pub fn other_device(&self) -> DeviceRef {
        let others = self.others_lock();
        let d = &others[0];
        DeviceRef {
            id: d.id.clone(),
            name: d.name.clone(),
        }
    }

    /// Sets the pin of an item. `true` when something changed.
    fn set_pinned(&self, id: &str, pinned: bool) -> bool {
        let updated = {
            let mut items = self.lock();
            let Some(item) = items.iter_mut().find(|i| i.id == id) else {
                return false;
            };
            if item.pinned == pinned {
                return false;
            }
            item.pinned = pinned;
            item.pinned_at = pinned.then(now_ms);
            if pinned {
                // Pin and archive exclude each other: pinning an archived item unarchives it.
                item.archived = false;
                item.archived_at = None;
            }
            item.clone()
        };
        self.emit(CoreEvent::ItemUpdated(updated));
        true
    }

    /// Sets the archive of an item (`spec/archive-contract.md`). Archiving a pinned item
    /// unpins it, in the same step. Never touches `read`. `true` when something changed.
    fn set_archived(&self, id: &str, archived: bool) -> bool {
        let updated = {
            let mut items = self.lock();
            let Some(item) = items.iter_mut().find(|i| i.id == id) else {
                return false;
            };
            if item.archived == archived && !(archived && item.pinned) {
                return false;
            }
            item.archived = archived;
            item.archived_at = archived.then(now_ms);
            if archived {
                item.pinned = false;
                item.pinned_at = None;
            }
            item.clone()
        };
        self.emit(CoreEvent::ItemUpdated(updated));
        true
    }
}

fn demo_device(id: &str, name: &str, platform: &str, this: bool) -> Device {
    demo_device_seen(id, name, platform, this, now_ms())
}

fn demo_device_seen(id: &str, name: &str, platform: &str, this: bool, seen: i64) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        platform: platform.into(),
        last_seen: Some(seen),
        this_device: this,
        state: DeviceState::Verified,
        inactive_days: inactive_days(Some(seen), this, now_ms(), INACTIVE_AFTER_DAYS),
    }
}

/// The other devices of the demo space. "PC-Maison" has not shown up for 34 days, to try the
/// hint that suggests removing it.
fn demo_others() -> Vec<Device> {
    vec![
        demo_device("dev-pixel", "Pixel", "android", false),
        demo_device_seen(
            "dev-home",
            "PC-Maison",
            "windows",
            false,
            now_ms() - 34 * 86_400_000,
        ),
    ]
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Ce PC".to_string())
}

/// The demo pairing screen at this moment of the script: the code, then a "Pixel 9" that shows
/// the verification code, then (once the user pressed "Ajouter") done.
fn view_of(d: &DemoInvitation) -> PairingView {
    let Some(started) = d.started else {
        return PairingView::None;
    };
    let pixel = PairingPeerView {
        name: "Pixel 9".into(),
        platform: "android".into(),
    };
    if let Some(confirmed) = d.confirmed {
        let stage = if confirmed.elapsed() >= DEMO_DONE_AFTER {
            PairingStage::Done
        } else {
            PairingStage::Provisioning
        };
        let mut s = PairingSession::new(stage);
        s.peer = Some(pixel);
        return PairingView::Invite(s);
    }
    if started.elapsed() >= DEMO_PEER_AFTER {
        let mut s = PairingSession::new(PairingStage::Verify);
        s.sas = Some(SecretText::new(DEMO_SAS));
        s.peer = Some(pixel);
        return PairingView::Invite(s);
    }
    let mut s = PairingSession::new(PairingStage::Waiting);
    s.code = Some(SecretText::new(DEMO_CODE));
    s.link = Some(SecretText::new(DEMO_LINK));
    s.qr = qr::encode(DEMO_LINK);
    s.expires_at = Some(d.started_at_ms + DEMO_CODE_LIFE_MS);
    PairingView::Invite(s)
}

#[async_trait]
impl TomiCore for FakeCore {
    fn status(&self) -> CoreStatus {
        let sim = self.dep.lock().unwrap_or_else(|p| p.into_inner());
        CoreStatus {
            paired: !sim.erased,
            space: sim.space_state(),
            unavailable: None,
        }
    }

    fn this_device(&self) -> Device {
        self.this_lock().clone()
    }

    async fn send_item(
        &self,
        draft: ItemDraft,
        progress: ProgressFn,
    ) -> Result<SendOutcome, CoreError> {
        if self.is_erased_now() {
            return Err(CoreError::NotPaired);
        }
        let this = self.this_device();
        let id = draft
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let item_dir = self.dir.join(&id);
        let total = draft.total_attachment_bytes().max(1);

        // "Upload": copy the files while reporting progress.
        let mut attachments = Vec::new();
        let mut sent = 0u64;
        progress(SendProgress {
            sent_bytes: 0,
            total_bytes: total,
        });
        for a in &draft.attachments {
            let Some(src) = a.path.as_deref() else {
                return Err(CoreError::Rejected("attachment without path".into()));
            };
            std::fs::create_dir_all(&item_dir).map_err(|e| CoreError::Other(e.to_string()))?;
            let dest = files::unique_path(&item_dir, &a.name);
            std::fs::copy(src, &dest).map_err(|e| CoreError::Other(e.to_string()))?;
            sent += a.size;
            tokio::time::sleep(self.step_delay).await;
            progress(SendProgress {
                sent_bytes: sent.min(total),
                total_bytes: total,
            });
            let mut stored = a.clone();
            stored.path = Some(dest.to_string_lossy().into_owned());
            attachments.push(stored);
        }

        let item = Item {
            id: id.clone(),
            title: draft.title,
            text: draft.text,
            url: draft.url,
            attachments,
            secret: draft.secret,
            source_device: DeviceRef {
                id: this.id.clone(),
                name: this.name.clone(),
            },
            created_at: now_ms(),
            expires_at: draft.expires_at,
            read_policy: draft.read_policy,
            read: true,
            outgoing: true,
            imported: false,
            delivery: self
                .others_lock()
                .iter()
                .map(|d| Delivery {
                    device: DeviceRef {
                        id: d.id.clone(),
                        name: d.name.clone(),
                    },
                    state: DeliveryState::Delivered,
                })
                .collect(),
            source_format: draft.source_format,
            unreadable: false,
            unreadable_reason: None,
            pinned: false,
            pinned_at: None,
            archived: false,
            archived_at: None,
        };
        self.lock().push(item.clone());
        self.emit(CoreEvent::ItemAdded(item));
        Ok(SendOutcome {
            item_id: id,
            queued: false,
        })
    }

    async fn list_items(&self) -> Result<Vec<Item>, CoreError> {
        self.purge_expired();
        let mut v = self.lock().clone();
        v.sort_by_key(|i| std::cmp::Reverse(i.created_at));
        Ok(v)
    }

    async fn get_item(&self, id: &str) -> Result<Item, CoreError> {
        self.purge_expired();
        self.lock()
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }

    fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }

    async fn delete_local(&self, id: &str) -> Result<(), CoreError> {
        let removed = {
            let mut items = self.lock();
            let before = items.len();
            items.retain(|i| i.id != id);
            before != items.len()
        };
        if !removed {
            return Err(CoreError::NotFound(id.to_string()));
        }
        let _ = std::fs::remove_dir_all(self.dir.join(id));
        self.emit(CoreEvent::ItemRemoved(id.to_string()));
        Ok(())
    }

    async fn delete_everywhere(&self, id: &str) -> Result<(), CoreError> {
        // A real core would also publish a tombstone to the relay and the other devices.
        self.delete_local(id).await
    }

    async fn mark_read(&self, id: &str) -> Result<(), CoreError> {
        let (updated, consumed) = {
            let mut items = self.lock();
            let item = items
                .iter_mut()
                .find(|i| i.id == id)
                .ok_or_else(|| CoreError::NotFound(id.to_string()))?;
            // An item sent from here is never consumed by its sender (as in the core).
            let consumed = item.read_policy == ReadPolicy::AfterFirstRead && !item.outgoing;
            item.read = true;
            (item.clone(), consumed)
        };
        if consumed {
            // Like the core: read once, then gone everywhere.
            return self.delete_local(id).await;
        }
        self.emit(CoreEvent::ItemUpdated(updated));
        Ok(())
    }

    async fn pin(&self, id: &str) -> Result<(), CoreError> {
        self.set_pinned(id, true);
        Ok(())
    }

    async fn unpin(&self, id: &str) -> Result<(), CoreError> {
        self.set_pinned(id, false);
        Ok(())
    }

    async fn archive(&self, id: &str) -> Result<(), CoreError> {
        self.set_archived(id, true);
        Ok(())
    }

    async fn unarchive(&self, id: &str) -> Result<(), CoreError> {
        self.set_archived(id, false);
        Ok(())
    }

    async fn devices(&self) -> Result<Vec<Device>, CoreError> {
        if self.is_erased_now() {
            return Err(CoreError::NotPaired);
        }
        let mut all = vec![self.this_device()];
        all.extend(self.others_lock().clone());
        Ok(all)
    }

    async fn rename_device(&self, id: &str, name: &str) -> Result<(), CoreError> {
        let name =
            clean_device_name(name).ok_or_else(|| CoreError::Rejected("name_invalid".into()))?;
        let renamed = {
            let mut this = self.this_lock();
            if this.id == id {
                this.name = name;
                true
            } else {
                drop(this);
                let mut others = self.others_lock();
                match others.iter_mut().find(|d| d.id == id) {
                    Some(d) => {
                        d.name = name;
                        true
                    }
                    None => false,
                }
            }
        };
        if !renamed {
            return Err(CoreError::Rejected("unknown_device".into()));
        }
        self.emit(CoreEvent::Changed);
        Ok(())
    }

    async fn revoke_device(&self, id: &str) -> Result<(), CoreError> {
        if self.this_lock().id == id {
            return Err(CoreError::Rejected("revoke_self".into()));
        }
        {
            let mut others = self.others_lock();
            let before = others.len();
            others.retain(|d| d.id != id);
            if others.len() == before {
                return Err(CoreError::Rejected("unknown_device".into()));
            }
        }
        self.emit(CoreEvent::Changed);
        Ok(())
    }

    async fn attachment_path(&self, id: &str, slot: usize) -> Result<PathBuf, CoreError> {
        let item = self.get_item(id).await?;
        item.attachments
            .iter()
            .enumerate()
            .find(|(position, a)| a.slot.map_or(*position, |s| s as usize) == slot)
            .and_then(|(_, a)| a.path.clone())
            .map(PathBuf::from)
            .ok_or_else(|| CoreError::NotFound(format!("{id}#{slot}")))
    }

    async fn transfers(&self) -> Result<Vec<Transfer>, CoreError> {
        Ok(Vec::new())
    }

    async fn send_history(&self, _device_id: &str) -> Result<(), CoreError> {
        Err(CoreError::Rejected("demo".into()))
    }

    async fn answer_history(&self, _transfer_id: &str, _accept: bool) -> Result<(), CoreError> {
        Err(CoreError::Rejected("demo".into()))
    }

    async fn cancel_transfer(&self, _transfer_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    // The demo PC is always "paired": it can only invite. The invitation is a short script
    // (a code, then a "Pixel 9" that shows the verification code, then done) so that the
    // screens can be tried; nothing is encrypted, sent or kept.

    async fn pairing_options(&self) -> Result<PairingOptions, CoreError> {
        Ok(PairingOptions {
            paired: !self.is_erased_now(),
            device_name: self.this_device().name,
            space_initialized: None,
            pending: false,
            // The demo PC is paired: it shows no relay field.
            relay_authority: None,
            resumed: None,
        })
    }

    fn pairing_view(&self) -> PairingView {
        self.demo_view()
    }

    async fn pairing_create_space_at(
        &self,
        _install_token: &str,
        _device_name: Option<&str>,
        _relay_url: Option<&str>,
    ) -> Result<(), CoreError> {
        // After an erasure the demo PC can pair again; otherwise it already has its space.
        if self.dep_rejoin() {
            return Ok(());
        }
        Err(CoreError::Rejected("demo".into()))
    }

    async fn pairing_invite(&self) -> Result<PairingView, CoreError> {
        {
            let mut d = self.demo();
            if matches!(view_of(&d), PairingView::Invite(s) if !s.stage.is_over()) {
                return Err(CoreError::Pairing("in_progress".into()));
            }
            *d = DemoInvitation {
                started: Some(Instant::now()),
                started_at_ms: now_ms(),
                confirmed: None,
            };
        }
        self.announce_pairing_in(DEMO_PEER_AFTER);
        Ok(self.demo_view())
    }

    async fn pairing_join_at(
        &self,
        _code_or_link: &str,
        _device_name: Option<&str>,
        _relay_url: Option<&str>,
    ) -> Result<PairingView, CoreError> {
        if self.dep_rejoin() {
            return Ok(PairingView::None);
        }
        Err(CoreError::Rejected("demo".into()))
    }

    fn pairing_confirm(&self, _import_history: bool) -> Result<(), CoreError> {
        let verifying = matches!(
            self.demo_view(),
            PairingView::Invite(s) if s.stage == PairingStage::Verify
        );
        if !verifying {
            return Err(CoreError::Pairing("not_ready".into()));
        }
        self.demo().confirmed = Some(Instant::now());
        let _ = self.tx.send(CoreEvent::PairingChanged);
        self.announce_pairing_in(DEMO_DONE_AFTER);
        Ok(())
    }

    fn pairing_cancel(&self) {
        *self.demo() = DemoInvitation::default();
        let _ = self.tx.send(CoreEvent::PairingChanged);
    }

    fn pairing_leave(&self) {
        // Once "Ajouter" was pressed the demo goes on by itself, as the real pairing does.
        if !matches!(self.demo_view(), PairingView::Invite(s) if s.stage == PairingStage::Provisioning)
        {
            self.pairing_cancel();
        }
    }

    fn pairing_dismiss(&self) {
        if matches!(self.demo_view(), PairingView::Invite(s) if s.stage.is_over()) {
            *self.demo() = DemoInvitation::default();
            let _ = self.tx.send(CoreEvent::PairingChanged);
        }
    }

    async fn sync(&self) -> Result<(), CoreError> {
        Ok(())
    }

    async fn purge_plaintext_cache(&self) -> Result<(), CoreError> {
        Ok(())
    }

    fn departure_available(&self) -> bool {
        self.dep_available()
    }

    async fn leave_space(&self) -> Result<DepartureOutcome, CoreError> {
        self.dep_leave_space().await
    }

    async fn delete_space(&self) -> Result<DepartureOutcome, CoreError> {
        self.dep_delete_space().await
    }

    async fn retry_departure(&self) -> Result<DepartureOutcome, CoreError> {
        self.dep_retry().await
    }

    async fn continue_departure(&self) -> Result<DepartureOutcome, CoreError> {
        self.dep_continue().await
    }

    async fn cancel_departure(&self) -> Result<bool, CoreError> {
        self.dep_cancel()
    }

    fn departure(&self) -> Option<DepartureView> {
        self.dep_view()
    }

    fn can_erase_without_proof(&self) -> bool {
        self.dep_can_erase_without_proof()
    }

    async fn erase_without_proof(&self) -> Result<(), CoreError> {
        self.dep_erase_without_proof()
    }

    fn shutdown(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, TextBody};
    use std::sync::{Arc, Mutex as StdMutex};

    pub fn text_draft(text: &str) -> ItemDraft {
        ItemDraft {
            id: None,
            title: None,
            text: Some(TextBody {
                plain: text.into(),
                html: None,
                markdown: None,
            }),
            url: None,
            attachments: vec![],
            secret: false,
            // Nothing expires by default.
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            source_format: None,
        }
    }

    fn noop() -> ProgressFn {
        Arc::new(|_| {})
    }

    async fn send(core: &FakeCore, draft: ItemDraft) -> Item {
        let out = core.send_item(draft, noop()).await.unwrap();
        core.get_item(&out.item_id).await.unwrap()
    }

    #[tokio::test]
    async fn send_then_list_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let a = send(&core, text_draft("a")).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        let b = send(&core, text_draft("b")).await;
        let ids: Vec<_> = core
            .list_items()
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec![b.id, a.id]);
    }

    #[tokio::test]
    async fn attachments_are_copied_and_progress_reaches_total() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, vec![1u8; 1000]).unwrap();
        let core = FakeCore::new(dir.path().join("store")).without_delay();
        let mut d = text_draft("x");
        d.attachments = vec![Attachment {
            name: "src.bin".into(),
            mime: "application/octet-stream".into(),
            size: 1000,
            path: Some(src.to_string_lossy().into_owned()),
            thumbnail: None,
            available: true,
            slot: None,
        }];
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let seen2 = seen.clone();
        let out = core
            .send_item(
                d,
                Arc::new(move |p| seen2.lock().unwrap().push(p.sent_bytes)),
            )
            .await
            .unwrap();
        assert_eq!(*seen.lock().unwrap().last().unwrap(), 1000);
        let stored = core.attachment_path(&out.item_id, 0).await.unwrap();
        assert_eq!(std::fs::read(stored).unwrap().len(), 1000);
        // Deleting removes the copy.
        core.delete_local(&out.item_id).await.unwrap();
        assert!(core.get_item(&out.item_id).await.is_err());
    }

    #[tokio::test]
    async fn expired_items_are_not_listed() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut d = text_draft("old");
        d.expires_at = Some(now_ms() - 1);
        core.send_item(d, noop()).await.unwrap();
        assert!(core.list_items().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn incoming_items_are_unread_and_stream_to_subscribers() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut rx = core.subscribe();
        let mut item = send(&core, text_draft("hi")).await;
        let _ = rx.recv().await.unwrap();
        item.id = "incoming-1".into();
        core.inject_incoming(item);
        match rx.recv().await.unwrap() {
            CoreEvent::ItemAdded(i) => {
                assert_eq!(i.id, "incoming-1");
                assert!(!i.read && !i.outgoing);
            }
            other => panic!("unexpected {other:?}"),
        }
        core.mark_read("incoming-1").await.unwrap();
        assert!(core.get_item("incoming-1").await.unwrap().read);
    }

    #[tokio::test]
    async fn an_after_first_read_item_is_consumed_by_its_first_read() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut d = text_draft("une fois");
        d.secret = true;
        d.read_policy = ReadPolicy::AfterFirstRead;
        let mut incoming = send(&core, d).await;
        incoming.id = "s1".into();
        core.inject_incoming(incoming);
        let mut rx = core.subscribe();

        core.mark_read("s1").await.unwrap();

        // Like the core: erased at once, no grace period.
        assert!(core.get_item("s1").await.is_err());
        match rx.recv().await.unwrap() {
            CoreEvent::ItemRemoved(id) => assert_eq!(id, "s1"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_item_sent_from_here_is_not_consumed_by_its_sender() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut d = text_draft("moi");
        d.secret = true;
        d.read_policy = ReadPolicy::AfterFirstRead;
        let out = core.send_item(d, noop()).await.unwrap();
        core.mark_read(&out.item_id).await.unwrap();
        assert!(core.get_item(&out.item_id).await.is_ok());
    }

    #[tokio::test]
    async fn pin_and_unpin_are_idempotent_and_re_emit_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let out = core.send_item(text_draft("a"), noop()).await.unwrap();
        let mut rx = core.subscribe();

        core.pin(&out.item_id).await.unwrap();
        let pinned = core.get_item(&out.item_id).await.unwrap();
        assert!(pinned.pinned);
        let at = pinned.pinned_at.expect("pin time");
        match rx.recv().await.unwrap() {
            CoreEvent::ItemUpdated(i) => assert!(i.pinned && i.pinned_at == Some(at)),
            other => panic!("unexpected {other:?}"),
        }

        // Already pinned: nothing is emitted and the time does not move.
        core.pin(&out.item_id).await.unwrap();
        assert_eq!(
            core.get_item(&out.item_id).await.unwrap().pinned_at,
            Some(at)
        );
        assert!(rx.try_recv().is_err());

        core.unpin(&out.item_id).await.unwrap();
        let unpinned = core.get_item(&out.item_id).await.unwrap();
        assert!(!unpinned.pinned && unpinned.pinned_at.is_none());
    }

    #[tokio::test]
    async fn archive_and_unarchive_are_idempotent_and_re_emit_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let out = core.send_item(text_draft("a"), noop()).await.unwrap();
        let read_before = core.get_item(&out.item_id).await.unwrap().read;
        let mut rx = core.subscribe();

        core.archive(&out.item_id).await.unwrap();
        let archived = core.get_item(&out.item_id).await.unwrap();
        assert!(archived.archived && !archived.pinned);
        assert_eq!(archived.read, read_before, "archiving never marks read");
        let at = archived.archived_at.expect("archive time");
        match rx.recv().await.unwrap() {
            CoreEvent::ItemUpdated(i) => assert!(i.archived && i.archived_at == Some(at)),
            other => panic!("unexpected {other:?}"),
        }

        // Already archived: nothing is emitted and the time does not move.
        core.archive(&out.item_id).await.unwrap();
        assert_eq!(
            core.get_item(&out.item_id).await.unwrap().archived_at,
            Some(at)
        );
        assert!(rx.try_recv().is_err());

        core.unarchive(&out.item_id).await.unwrap();
        let back = core.get_item(&out.item_id).await.unwrap();
        assert!(!back.archived && back.archived_at.is_none());
        assert!(matches!(
            rx.recv().await.unwrap(),
            CoreEvent::ItemUpdated(i) if !i.archived
        ));
        // Not archived: nothing to do, nothing emitted.
        core.unarchive(&out.item_id).await.unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn archive_and_pin_exclude_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let out = core.send_item(text_draft("a"), noop()).await.unwrap();

        core.pin(&out.item_id).await.unwrap();
        core.archive(&out.item_id).await.unwrap();
        let archived = core.get_item(&out.item_id).await.unwrap();
        assert!(archived.archived && !archived.pinned && archived.pinned_at.is_none());

        core.pin(&out.item_id).await.unwrap();
        let pinned = core.get_item(&out.item_id).await.unwrap();
        assert!(pinned.pinned && !pinned.archived && pinned.archived_at.is_none());
    }

    #[tokio::test]
    async fn archiving_an_unknown_id_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut rx = core.subscribe();
        core.archive("nope").await.unwrap();
        core.unarchive("nope").await.unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pinning_an_unknown_id_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        core.pin("nope").await.unwrap();
        core.unpin("nope").await.unwrap();
    }

    fn rejected(code: &str) -> CoreError {
        CoreError::Rejected(code.into())
    }

    async fn names(core: &FakeCore) -> Vec<String> {
        core.devices()
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect()
    }

    #[tokio::test]
    async fn a_device_is_renamed_with_a_trimmed_valid_name() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut rx = core.subscribe();

        core.rename_device("dev-pixel", "  Pixel de Sam  ")
            .await
            .unwrap();
        assert!(names(&core).await.contains(&"Pixel de Sam".to_string()));
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Changed)));

        // This PC can be renamed too, as on Android; the pairing screen follows.
        core.rename_device("dev-this", "Bureau").await.unwrap();
        assert_eq!(core.this_device().name, "Bureau");
        assert_eq!(core.pairing_options().await.unwrap().device_name, "Bureau");
        // A name of 64 characters is the longest.
        core.rename_device("dev-home", &"é".repeat(64))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_invalid_name_or_an_unknown_device_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let before = names(&core).await;
        let mut rx = core.subscribe();

        for bad in ["", "   ", "a\u{7}b", "a\nb", &"x".repeat(65)] {
            assert_eq!(
                core.rename_device("dev-pixel", bad).await.unwrap_err(),
                rejected("name_invalid"),
                "{bad:?}"
            );
        }
        assert_eq!(
            core.rename_device("dev-gone", "Nom").await.unwrap_err(),
            rejected("unknown_device")
        );
        assert_eq!(names(&core).await, before);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn another_device_is_revoked_and_gone_from_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut rx = core.subscribe();

        core.revoke_device("dev-pixel").await.unwrap();
        let ids: Vec<String> = core
            .devices()
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(ids, vec!["dev-this".to_string(), "dev-home".to_string()]);
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Changed)));

        // Already gone: a refusal, not a panic and not another event.
        assert_eq!(
            core.revoke_device("dev-pixel").await.unwrap_err(),
            rejected("unknown_device")
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn this_pc_cannot_be_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().to_path_buf()).without_delay();
        let mut rx = core.subscribe();

        assert_eq!(
            core.revoke_device(&core.this_device().id)
                .await
                .unwrap_err(),
            rejected("revoke_self")
        );
        assert_eq!(core.devices().await.unwrap().len(), 3);
        assert!(rx.try_recv().is_err());
    }
}
