//! Reacts to the core's item stream: refreshes the unread dot and shows toasts.
//! Items arriving close together are grouped into one toast (no previews).

use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::{error::RecvError, Receiver};

use crate::core_api::{CoreEvent, TomiCore};
use crate::model::{Item, TransferPhase};
use crate::queue::{JobSink, JobState, JobView};
use crate::relay_notices::{self, RelayNotices};
use crate::state::AppState;
use crate::{actions, notify, popover};

/// Items received within this window share one toast.
const BURST_WINDOW: Duration = Duration::from_millis(700);
/// Expired items vanish from the feed without waiting for the user to act.
const PURGE_EVERY: Duration = Duration::from_secs(60);
/// How often the janitor looks at whether the user is still around.
const JANITOR_EVERY: Duration = Duration::from_secs(15);
/// Nobody looked at Tomi for this long: the decrypted copies of attachments go. Long
/// enough for a program that was just asked to open one to have read it.
const PLAINTEXT_GRACE: Duration = Duration::from_secs(60);

/// Forwards queue changes to the windows and keeps the tray tooltip in sync.
pub struct TauriSink {
    pub app: AppHandle,
}

impl JobSink for TauriSink {
    fn job_changed(&self, job: &JobView) {
        let _ = self.app.emit("job-changed", job);
        // Progress ticks do not change the tooltip.
        let tick = matches!(job.state, JobState::Sending { sent_bytes, .. } if sent_bytes > 0);
        if !tick {
            let app = self.app.clone();
            tauri::async_runtime::spawn(async move { actions::refresh_badge(&app).await });
        }
    }
}

/// The toast of an item that is gone (deleted, expired) or archived goes too, on screen or in the
/// notification centre: it must not keep announcing it.
async fn toast_withdrawn(id: &str) {
    let id = id.to_string();
    let _ = actions::blocking(move || notify::remove(&id)).await;
}

/// An item is gone (tombstone from another device, deletion here, expiry): the clipboard
/// must not keep the value of a secret that no longer exists, nor its toast stay.
async fn item_removed(app: &AppHandle, id: &str) {
    toast_withdrawn(id).await;
    let pending = app.state::<AppState>().clip_clears.take_for_item(id);
    if let Some(pending) = pending {
        let _ = actions::blocking(move || actions::wipe(&pending)).await;
    }
}

/// An item changed: once archived it is "done with", and a toast still announcing it goes.
async fn item_updated(item: &Item) {
    if item.archived {
        toast_withdrawn(&item.id).await;
    }
}

/// The event channel lagged (more changes than the buffer held): some `ItemRemoved` were
/// dropped. Reconcile the pending clipboard clear against the current feed, so a secret
/// whose item vanished in the flood is still wiped.
async fn reconcile_after_lag(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(items) = state.core.list_items().await else {
        return;
    };
    let present: std::collections::HashSet<String> = items.into_iter().map(|i| i.id).collect();
    let pending = state
        .clip_clears
        .take_if_item_absent(|id| present.contains(id));
    if let Some(pending) = pending {
        let _ = actions::blocking(move || actions::wipe(&pending)).await;
    }
}

/// A new arrival worth a toast: received from another device, not part of a history
/// transfer (hundreds of old items are not news).
fn is_news(item: &Item) -> bool {
    !item.outgoing && !item.imported
}

pub async fn run(app: AppHandle, mut rx: Receiver<CoreEvent>) {
    remember_devices(&app).await;
    loop {
        let mut batch: Vec<Item> = Vec::new();
        match rx.recv().await {
            Ok(CoreEvent::ItemAdded(item)) => {
                if is_news(&item) {
                    batch.push(item);
                }
                collect_burst(&app, &mut rx, &mut batch).await;
            }
            Ok(CoreEvent::ItemRemoved(id)) => item_removed(&app, &id).await,
            Ok(CoreEvent::Changed) => space_changed(&app).await,
            Ok(CoreEvent::PairingChanged) => pairing_changed(&app),
            Ok(CoreEvent::DepartureChanged) => departure_changed(&app),
            Ok(CoreEvent::ItemUpdated(item)) => item_updated(&item).await,
            // Dropped events may include an `ItemRemoved`: reconcile the clipboard clear. A
            // pairing screen that missed its change reads the pairing again.
            Err(RecvError::Lagged(_)) => {
                pairing_changed(&app);
                departure_changed(&app);
                reconcile_after_lag(&app).await;
            }
            Err(RecvError::Closed) => return,
        }
        actions::refresh_badge(&app).await;
        if !batch.is_empty() {
            toast(&app, batch);
        }
    }
}

async fn collect_burst(app: &AppHandle, rx: &mut Receiver<CoreEvent>, batch: &mut Vec<Item>) {
    while let Ok(next) = tokio::time::timeout(BURST_WINDOW, rx.recv()).await {
        match next {
            Ok(CoreEvent::ItemAdded(item)) if is_news(&item) => batch.push(item),
            // A removal inside a burst is still a removal.
            Ok(CoreEvent::ItemRemoved(id)) => item_removed(app, &id).await,
            Ok(CoreEvent::Changed) => space_changed(app).await,
            Ok(CoreEvent::PairingChanged) => pairing_changed(app),
            Ok(CoreEvent::DepartureChanged) => departure_changed(app),
            Ok(CoreEvent::ItemUpdated(item)) => item_updated(&item).await,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => {
                pairing_changed(app);
                departure_changed(app);
                reconcile_after_lag(app).await;
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// The pairing screen has something new: the windows read the pairing again. The view itself
/// is not in the event (it holds the code and the verification code): the window asks for it.
fn pairing_changed(app: &AppHandle) {
    // A PC that was just provisioned (joined, or created the space) remembers that it belongs to
    // a space: the proof the start-up cleanup of a departure that died half-way relies on.
    crate::departure_host::remember_membership(&app.state::<AppState>());
    let _ = app.emit("pairing-changed", ());
}

/// The departure marker appeared (or went) outside the departure flow's own calls: the banner
/// reads the departure again.
fn departure_changed(app: &AppHandle) {
    let _ = app.emit("departure-changed", ());
}

/// The devices, a history transfer or the state of the space changed: the windows refetch
/// them, and a history offered by another device is announced once.
async fn space_changed(app: &AppHandle) {
    let _ = app.emit("space-changed", ());
    let state = app.state::<AppState>();
    announce_new_devices(app, state.core.as_ref()).await;
    tell_relay_revocations(app, state.core.as_ref(), &state.relay_notices);
    if !may_notify(state.core.as_ref()) {
        return;
    }
    let Ok(transfers) = state.core.transfers().await else {
        return;
    };
    let fresh: Vec<_> = {
        let mut seen = state.seen_offers.lock().unwrap_or_else(|p| p.into_inner());
        transfers
            .iter()
            .filter(|t| t.phase == TransferPhase::Offered)
            .filter(|t| seen.insert(t.id.clone()))
            .map(|t| t.peer.name.clone())
            .collect()
    };
    for peer in fresh {
        let handle = app.clone();
        notify::show(notify::offer_toast(&peer), None, move |_| {
            popover::open_history(&handle, Some("devices"));
        });
    }
}

/// Devices the relay removed on its own authority: taken from the core (once) after a change of
/// the space, never on a timer, and handed to the webview that is on screen as a plain notice.
/// Nothing in the demo space nor on a PC that has no space; a failed read leaves the core's list
/// as it was, so the next change tries again.
fn tell_relay_revocations(app: &AppHandle, core: &dyn TomiCore, notices: &RelayNotices) {
    if !may_notify(core) || core.is_demo() {
        return;
    }
    if relay_notices::collect(core, notices).unwrap_or(false) {
        let _ = app.emit("relay-revocations", ());
    }
}

/// The devices of the space as last seen, to tell a newcomer from a device that was already
/// there. `None` until a first look: that first look only records (a PC that has just joined
/// finds the whole space "new"). Lost with the process: a device that joined while Tomi was
/// closed is not announced.
static KNOWN_DEVICES: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

/// Names of the other devices that are not in `known`, which is then brought up to date. The
/// first call (`known` is `None`) only records.
fn arrivals(
    known: &mut Option<std::collections::HashSet<String>>,
    devices: &[crate::model::Device],
) -> Vec<String> {
    let ids = devices.iter().map(|d| d.id.clone()).collect();
    let fresh = match known.as_ref() {
        None => Vec::new(),
        Some(seen) => devices
            .iter()
            .filter(|d| !d.this_device && !seen.contains(&d.id))
            .map(|d| d.name.clone())
            .collect(),
    };
    *known = Some(ids);
    fresh
}

/// Another device joined the space: a toast with its name (no content), so that a device the
/// user did not add shows up at once. Nothing in the demo space nor on a PC that has no space.
async fn announce_new_devices(app: &AppHandle, core: &dyn TomiCore) {
    if !may_notify(core) || core.is_demo() {
        *KNOWN_DEVICES.lock().unwrap_or_else(|p| p.into_inner()) = None;
        return;
    }
    let Ok(devices) = core.devices().await else {
        return;
    };
    let fresh = arrivals(
        &mut KNOWN_DEVICES.lock().unwrap_or_else(|p| p.into_inner()),
        &devices,
    );
    for name in fresh {
        let handle = app.clone();
        notify::show(notify::new_device_toast(&name), None, move |_| {
            popover::open_history(&handle, Some("devices"));
        });
    }
}

/// Records the devices of the space at start-up, without announcing anything.
async fn remember_devices(app: &AppHandle) {
    let core = app.state::<AppState>().core.clone();
    if may_notify(core.as_ref()) && !core.is_demo() {
        if let Ok(devices) = core.devices().await {
            *KNOWN_DEVICES.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(devices.into_iter().map(|d| d.id).collect());
        }
    }
}

/// Periodic refresh so that expired items disappear.
pub async fn purge_loop(app: AppHandle) {
    let mut tick = tokio::time::interval(PURGE_EVERY);
    loop {
        tick.tick().await;
        actions::refresh_badge(&app).await;
    }
}

/// The decrypted copies of attachments (opened, dragged out, copied as files) are kept
/// on disk only while someone is around: once no Tomi window has been in use for a
/// minute, the core erases them. Files a program still holds open stay until the next
/// round; everything goes when Tomi quits. Does nothing when no copy was made.
pub async fn plaintext_janitor(app: AppHandle) {
    let mut tick = tokio::time::interval(JANITOR_EVERY);
    let mut idle_since = Instant::now();
    loop {
        tick.tick().await;
        if popover::is_user_active(&app) {
            idle_since = Instant::now();
            continue;
        }
        if idle_since.elapsed() >= PLAINTEXT_GRACE {
            let core = app.state::<AppState>().core.clone();
            let _ = core.purge_plaintext_cache().await;
        }
    }
}

/// Nothing is announced by a device that is no longer in a space: a burst that was still being
/// collected (700 ms) or a bridge pass that was still running when the erasure happened would
/// otherwise show up to 80 characters of an item after the clean-up.
fn may_notify(core: &dyn TomiCore) -> bool {
    core.status().paired
}

fn toast(app: &AppHandle, batch: Vec<Item>) {
    let state = app.state::<AppState>();
    if !may_notify(state.core.as_ref()) || !state.settings().notifications {
        return;
    }
    // The user is looking at the popover: the item shows up there.
    let looking = popover::is_popover_visible(app)
        && app
            .get_webview_window(popover::POPOVER)
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
    if looking {
        return;
    }
    let handle = app.clone();
    if let [item] = batch.as_slice() {
        let id = item.id.clone();
        // Tagged with the id of the item: taken back when the item is deleted, expired or archived.
        notify::show(notify::single_toast(item), Some(&item.id), move |action| {
            let (app, id) = (handle.clone(), id.clone());
            tauri::async_runtime::spawn(async move {
                actions::run_toast_action(&app, &id, action.as_deref()).await;
            });
        });
    } else {
        notify::show(notify::group_toast(batch.len()), None, move |_| {
            popover::show_popover(&handle);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_core::{FakeCore, Reply, Step};
    use crate::model::{Device, DeviceState};

    fn device(id: &str, name: &str, this: bool) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            platform: "android".into(),
            last_seen: None,
            this_device: this,
            state: DeviceState::Verified,
            inactive_days: None,
        }
    }

    #[test]
    fn a_newcomer_is_announced_once_and_the_first_look_only_records() {
        let mut known = None;
        let space = vec![device("a", "PC", true), device("b", "Pixel", false)];
        assert!(arrivals(&mut known, &space).is_empty(), "first look");
        assert!(arrivals(&mut known, &space).is_empty(), "nothing new");

        let mut grown = space.clone();
        grown.push(device("c", "iPhone", false));
        assert_eq!(arrivals(&mut known, &grown), vec!["iPhone".to_string()]);
        assert!(arrivals(&mut known, &grown).is_empty(), "only once");

        // A revoked device leaves the list; it is not an arrival when it goes, nor is this PC.
        assert!(arrivals(&mut known, &space).is_empty());
        let mut again = space.clone();
        again.push(device("c", "iPhone", false));
        assert_eq!(arrivals(&mut known, &again), vec!["iPhone".to_string()]);
    }

    #[test]
    fn this_pc_is_never_an_arrival() {
        let mut known = Some(std::collections::HashSet::new());
        let space = vec![device("a", "PC", true)];
        assert!(arrivals(&mut known, &space).is_empty());
    }

    #[tokio::test]
    async fn nothing_is_announced_once_the_device_is_erased() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().join("store")).without_delay();
        assert!(may_notify(&core), "a paired device announces");
        core.script(Step::LeaveMe, Reply::Ok);
        core.leave_space().await.unwrap();
        assert!(!core.status().paired);
        assert!(
            !may_notify(&core),
            "a toast that was still on its way after the erasure is dropped"
        );
    }
}
