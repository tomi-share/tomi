//! The unified core ([`UnifiedCore`]): the real engine of this device and, on request, the demo
//! space of the apps beside it (feature `demo`, [`super::demo`]).
//!
//! An app opens its real core as usual. While this device is in no space it may *enter the
//! demo*: every call then goes to the engine of a fresh demo space (in memory, no network),
//! already joined, until the demo is left. The real engine is never closed nor touched
//! meanwhile: its database, its keys and its relay stay as they were. Leaving the demo drops
//! the demo space (its files are erased) and the calls go to the real engine again.
//!
//! Rules kept here, not by the apps:
//! * a demo is only entered from a real engine that is in no space and has no pairing left
//!   pending: a demo never runs beside a real space;
//! * an entered demo always starts joined, so the simulated pairing and the demo install token
//!   of [`super::demo`] are never reachable from a real core;
//! * a demo whose device left its space is over ([`UnifiedCore::settle_demo`]).
//!
//! Without the feature `demo` this is the real engine alone: `is_demo` is always `false`.

#[cfg(feature = "demo")]
use std::sync::RwLock;
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(feature = "demo")]
use super::demo::{DemoConfig, DemoWorld};
use super::engine::Engine;
#[cfg(feature = "demo")]
use crate::error::{CoreError, Result};

type Listener = Arc<dyn Fn() + Send + Sync>;

/// The real engine, and the demo entered from it (if any).
pub struct UnifiedCore {
    real: Arc<Engine>,
    /// What [`subscribe`](Self::subscribe) registered, subscribed again on every demo entered.
    listeners: Mutex<Vec<Listener>>,
    #[cfg(feature = "demo")]
    demo: RwLock<Option<Arc<DemoWorld>>>,
    /// One entry at a time: two demo spaces must never share the demo files directory.
    #[cfg(feature = "demo")]
    entering: Mutex<()>,
}

impl std::fmt::Debug for UnifiedCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedCore")
            .field("demo", &self.is_demo())
            .finish_non_exhaustive()
    }
}

impl UnifiedCore {
    pub fn new(real: Arc<Engine>) -> Self {
        Self {
            real,
            listeners: Mutex::new(Vec::new()),
            #[cfg(feature = "demo")]
            demo: RwLock::new(None),
            #[cfg(feature = "demo")]
            entering: Mutex::new(()),
        }
    }

    /// The engine every call goes to: the demo's while a demo is entered, the real one otherwise.
    pub fn engine(&self) -> Arc<Engine> {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            return world.engine();
        }
        self.real.clone()
    }

    /// The real engine, whatever is on screen.
    pub fn real(&self) -> Arc<Engine> {
        self.real.clone()
    }

    /// `true` while a demo entered with `enter_demo` is the active space.
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

    /// The demo entered from the real engine, if any.
    #[cfg(feature = "demo")]
    pub fn demo_world(&self) -> Option<Arc<DemoWorld>> {
        self.demo
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Registers `listener` on the real engine and on every demo, entered now or later.
    pub fn subscribe(&self, listener: Listener) {
        // Lock order everywhere: `listeners`, then `demo`.
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.real.subscribe(listener.clone());
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            world.engine().subscribe(listener.clone());
        }
        listeners.push(listener);
    }

    /// Enters the demo: builds a fresh demo space, already joined, and makes its engine the
    /// active one. Blocks while the demo history is written (well under a second): call it off
    /// the main thread. Already in the demo: nothing happens. `start_paired` of `config` is
    /// ignored (always joined). Errors: `InvalidInput` when the real engine is in a space or has
    /// a pairing left pending; what `DemoWorld::open` reports.
    #[cfg(feature = "demo")]
    pub fn enter_demo(&self, config: DemoConfig) -> Result<()> {
        let _entering = self.entering.lock().unwrap_or_else(PoisonError::into_inner);
        if self.demo_world().is_some() {
            return Ok(());
        }
        if self.real.is_provisioned() {
            return Err(CoreError::input("this device already belongs to a space"));
        }
        if self.real.has_pending_pairing()? {
            return Err(CoreError::input("a pairing is pending on this device"));
        }
        let world = DemoWorld::open(DemoConfig {
            start_paired: true,
            ..config
        })?;
        {
            let listeners = self
                .listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let engine = world.engine();
            for listener in listeners.iter() {
                engine.subscribe(listener.clone());
            }
            *self.demo.write().unwrap_or_else(PoisonError::into_inner) = Some(world);
        }
        self.fire();
        Ok(())
    }

    /// Leaves the demo: the demo space is dropped (its worker stops, its files are erased) and
    /// the calls go to the real engine again. `false` when no demo was entered.
    pub fn leave_demo(&self) -> bool {
        #[cfg(feature = "demo")]
        {
            let world = self
                .demo
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let Some(world) = world else {
                return false;
            };
            drop(world);
            self.fire();
            true
        }
        #[cfg(not(feature = "demo"))]
        {
            false
        }
    }

    /// After a departure: a demo whose device is no longer in its space is over. `true` when the
    /// demo was left.
    pub fn settle_demo(&self) -> bool {
        #[cfg(feature = "demo")]
        if let Some(world) = self.demo_world() {
            if !world.engine().is_provisioned() {
                drop(world);
                return self.leave_demo();
            }
        }
        false
    }

    /// Tells the app that everything it shows changed (the active engine is another one).
    #[cfg(feature = "demo")]
    fn fire(&self) {
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for listener in listeners {
            listener();
        }
    }
}

#[cfg(all(test, feature = "demo"))]
mod tests {
    use super::*;
    use crate::new_space_id;
    use crate::runtime::engine::{Clock, EngineConfig, SystemClock};
    use crate::runtime::mock::MockRelay;
    use crate::runtime::store::Store;
    use crate::runtime::DepartureOutcome;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A real engine in no space, on a relay double it never reaches. The relay is returned so
    /// that it lives as long as the engine.
    fn real_engine(dir: &Path) -> (Arc<Engine>, MockRelay) {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let relay = MockRelay::new(&new_space_id().unwrap(), clock.clone());
        let store = Store::open(&dir.join("tomi.db"), &dir.join("files"), &[7u8; 32]).unwrap();
        let engine = Engine::open(
            EngineConfig {
                app_version: "0.1.0".into(),
                platform: "ios".into(),
                auto_import: false,
                default_relay_url: None,
            },
            store,
            relay.transport(),
            clock,
        )
        .unwrap();
        (Arc::new(engine), relay)
    }

    fn demo_items() -> usize {
        crate::runtime::demo::expected_feed().items
    }

    fn demo_config(dir: &Path) -> DemoConfig {
        DemoConfig {
            files_dir: dir.join("demo"),
            assets_dir: None,
            platform: "ios".into(),
            app_version: "0.1.0".into(),
            device_name: "iPhone".into(),
            // Ignored: an entered demo always starts joined.
            start_paired: false,
        }
    }

    #[test]
    fn the_demo_is_entered_and_left_without_touching_the_real_engine() {
        let dir = tempfile::tempdir().unwrap();
        let (real, _relay) = real_engine(dir.path());
        let core = UnifiedCore::new(real.clone());
        assert!(!core.is_demo());
        assert!(Arc::ptr_eq(&core.engine(), &real));

        core.enter_demo(demo_config(dir.path())).unwrap();
        assert!(core.is_demo());
        assert!(!Arc::ptr_eq(&core.engine(), &real));
        assert!(core.engine().is_provisioned());
        assert_eq!(core.engine().list_items().unwrap().len(), demo_items());
        assert!(!real.is_provisioned(), "the real engine joined nothing");
        // A second entry changes nothing.
        core.enter_demo(demo_config(dir.path())).unwrap();
        assert_eq!(core.engine().list_items().unwrap().len(), demo_items());

        assert!(core.leave_demo());
        assert!(!core.is_demo());
        assert!(Arc::ptr_eq(&core.engine(), &real));
        assert!(!core.leave_demo(), "nothing left to leave");
        assert!(
            !dir.path().join("demo/this").exists(),
            "the demo files are erased"
        );

        // And again: a fresh demo each time.
        core.enter_demo(demo_config(dir.path())).unwrap();
        assert_eq!(core.engine().list_items().unwrap().len(), demo_items());
    }

    #[test]
    fn the_app_is_told_at_the_entry_and_at_the_exit() {
        let dir = tempfile::tempdir().unwrap();
        let (real, _relay) = real_engine(dir.path());
        let core = UnifiedCore::new(real);
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        core.subscribe(Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        }));
        core.enter_demo(demo_config(dir.path())).unwrap();
        let after_entry = count.load(Ordering::SeqCst);
        assert!(after_entry > 0);
        // A change inside the demo reaches the same listener.
        let id = core.engine().list_items().unwrap()[0].id.clone();
        core.engine().archive(&id).unwrap();
        let after_change = count.load(Ordering::SeqCst);
        assert!(after_change > after_entry);
        core.leave_demo();
        assert!(count.load(Ordering::SeqCst) > after_change);
    }

    #[test]
    fn a_departure_inside_the_demo_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let (real, _relay) = real_engine(dir.path());
        let core = UnifiedCore::new(real.clone());
        core.enter_demo(demo_config(dir.path())).unwrap();
        assert!(
            !core.settle_demo(),
            "still in the demo space: nothing to settle"
        );
        assert_eq!(
            core.engine().leave_space().unwrap(),
            DepartureOutcome::Erased
        );
        assert!(core.settle_demo());
        assert!(!core.is_demo());
        assert!(Arc::ptr_eq(&core.engine(), &real));
    }
}
