//! Assembly: opening the relay (data directory, lock, database, start-up clean-up, push),
//! and serving it.

use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use rusqlite::params;

use crate::blobstore;
use crate::clock::{Clock, SystemClock};
use crate::config::{Config, Timing};
use crate::db::Db;
use crate::disk::{DiskProbe, RealDisk};
use crate::fcm::{FcmCredentials, FcmSender, HttpPost, UreqPoster};
use crate::hub::{close, Hub};
use crate::install;
use crate::push::{set_health, PushDispatcher, PushSender};
use crate::ratelimit::Limiter;
use crate::routes;
use crate::spaces::{data_refused, Space, Spaces};
use crate::state::{AppState, Inner, PairingWatch};

/// How push is wired.
#[derive(Clone, Default)]
pub enum PushChoice {
    /// FCM if `fcm_service_account_file` is configured, else disabled.
    #[default]
    FromConfig,
    /// No push.
    Disabled,
    /// A given sender (tests).
    Custom(Arc<dyn PushSender>),
}

/// What the caller can replace (tests, `admin`).
#[derive(Clone)]
pub struct Options {
    pub clock: Arc<dyn Clock>,
    pub disk: Arc<dyn DiskProbe>,
    pub timing: Timing,
    pub push: PushChoice,
    /// Start the periodic purge and compaction.
    pub background: bool,
    /// Take `<data_dir>/relay.lock` (a serving relay, or an `admin` command that writes).
    pub hold_lock: bool,
    /// Look, do not touch: for an `admin` command that only reads and holds no lock. Nothing is
    /// written to the data directory (no migration, no `routing.db` created, no row deleted, no
    /// directory created) and every write fails with `503`. Implies no `cleanup`.
    pub read_only: bool,
    /// Run the start-up clean-up (legacy token file, orphan blob files, partial uploads). Only
    /// safe when no other process serves the same data directory.
    pub cleanup: bool,
    /// HTTP client of the FCM sender (default: real HTTPS).
    pub fcm_http: Option<Arc<dyn HttpPost>>,
    /// Base URL of the FCM API (default: Google's).
    pub fcm_base: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            disk: Arc::new(RealDisk),
            timing: Timing::default(),
            push: PushChoice::FromConfig,
            background: true,
            hold_lock: true,
            read_only: false,
            cleanup: true,
            fcm_http: None,
            fcm_base: None,
        }
    }
}

/// Opens the relay: directories, lock, `routing.db` and the spaces, start-up clean-up, push
/// dispatcher.
pub async fn open(cfg: Config, opts: Options) -> anyhow::Result<AppState> {
    cfg.validate()?;
    let trusted = cfg.trusted_nets()?;
    if opts.read_only && opts.hold_lock {
        anyhow::bail!("a read-only open holds no lock");
    }
    // Before anything is created in the data directory (§8.6): a volume that is not mounted
    // must not turn into a new, empty relay.
    if cfg.require_existing_data_dir && looks_empty(&cfg.data_dir) {
        return Err(data_refused(anyhow::anyhow!(
            "data_dir looks empty (volume not mounted?): refusing to start; set require_existing_data_dir = false for a first installation"
        )));
    }
    if !opts.read_only {
        std::fs::create_dir_all(&cfg.data_dir).map_err(|e| {
            data_refused(anyhow::Error::new(e).context(format!(
                "cannot create the data directory {}",
                cfg.data_dir.display()
            )))
        })?;
    }

    let lock_file = if opts.hold_lock {
        // A lock file that cannot be opened is the data's fault (`78`); only a lock that is
        // really held by another process is an ordinary failure (`1`).
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(cfg.path("relay.lock"))
            .map_err(|e| data_refused(anyhow::Error::new(e).context("cannot open relay.lock")))?;
        f.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => {
                anyhow::anyhow!("another tomi-relay is using {}", cfg.data_dir.display())
            }
            std::fs::TryLockError::Error(e) => {
                data_refused(anyhow::Error::new(e).context("cannot lock relay.lock"))
            }
        })?;
        Some(f)
    } else {
        None
    };

    // The layout of a single-space relay is migrated here, before anything is opened (only by a
    // process that may write: a read-only one refuses an unmigrated directory).
    let spaces = Arc::new(if opts.read_only {
        Spaces::open_read_only(&cfg.data_dir)?
    } else {
        Spaces::open(&cfg.data_dir)?
    });
    let now = opts.clock.now_ms();
    if opts.cleanup && !opts.read_only {
        startup_cleanup(&cfg, &spaces, now).map_err(data_refused)?;
    }

    let hub = Arc::new(Hub::new());
    let push = match opts.push.clone() {
        PushChoice::Disabled => PushDispatcher::disabled(),
        PushChoice::Custom(sender) => PushDispatcher::spawn(
            sender,
            spaces.clone(),
            hub.clone(),
            opts.clock.clone(),
            opts.timing.push_min_interval,
        ),
        PushChoice::FromConfig => match &cfg.fcm_service_account_file {
            None => PushDispatcher::disabled(),
            Some(file) => match FcmCredentials::load(file, cfg.fcm_project_id.as_deref()) {
                Ok(creds) => {
                    let http: Arc<dyn HttpPost> = opts
                        .fcm_http
                        .clone()
                        .unwrap_or_else(|| Arc::new(UreqPoster::default()));
                    let sender = Arc::new(FcmSender::new(
                        creds,
                        http,
                        opts.clock.clone(),
                        opts.fcm_base.clone(),
                    ));
                    write_health(spaces.routing(), now, "unknown", None);
                    PushDispatcher::spawn(
                        sender,
                        spaces.clone(),
                        hub.clone(),
                        opts.clock.clone(),
                        opts.timing.push_min_interval,
                    )
                }
                Err(e) => {
                    // The relay keeps serving; only the push doorbell is out (§6.3).
                    tracing::error!(
                        "push: the FCM service account is unusable ({e}); push is disabled"
                    );
                    write_health(
                        spaces.routing(),
                        now,
                        "auth_error",
                        Some("invalid_service_account"),
                    );
                    PushDispatcher::disabled()
                }
            },
        },
    };

    let limiter = Limiter::new(opts.clock.clone());
    // The panel, if asked for: its secret path is created at the first start with it on. A
    // read-only open (an `admin` command that only looks) never serves it.
    let admin_web = if cfg.admin_web && !opts.read_only {
        Some(crate::admin_web::AdminWeb::open(
            &cfg,
            spaces.routing(),
            now,
        )?)
    } else {
        None
    };
    let state: AppState = Arc::new(Inner {
        cfg,
        timing: opts.timing,
        clock: opts.clock,
        spaces,
        hub,
        limiter,
        disk: opts.disk,
        push,
        trusted,
        pairing: PairingWatch::default(),
        admin_web,
        blob_busy: Mutex::new(HashSet::new()),
        lock_file: Mutex::new(lock_file),
    });
    if opts.background {
        crate::maintenance::spawn(state.clone());
    }
    Ok(state)
}

/// Whether `data_dir` holds nothing of a relay: neither `routing.db`, nor `spaces/`, nor the
/// layout of the one-space relay (`tomi.db`, `blobs/`, `install-token`, §10.1.1). A directory
/// that does not exist or cannot be read looks empty too.
fn looks_empty(data_dir: &std::path::Path) -> bool {
    let present = |name: &str| std::fs::symlink_metadata(data_dir.join(name)).is_ok();
    ![
        crate::spaces::ROUTING_FILE,
        crate::spaces::SPACES_DIR,
        "tomi.db",
        "blobs",
        "install-token",
    ]
    .iter()
    .any(|n| present(n))
}

fn write_health(routing: &Db, now: i64, state: &str, error: Option<&str>) {
    let (state, error) = (state.to_owned(), error.map(str::to_owned));
    let r = routing.write_sync(|tx| {
        set_health(tx, now, &state, error.as_deref(), false)?;
        Ok(crate::db::Tx::Commit(()))
    });
    if r.is_err() {
        tracing::warn!("cannot record the push health");
    }
}

/// The installation token of a relay that has neither a space nor a pending invitation (a
/// fresh install): an invitation is created and its token returned, so that the caller can show
/// it. `None` when the relay already has a space or an invitation (`api-v1.md` §3.1).
///
/// The token is shown exactly once; the relay keeps only its hash. [`announce_invitation`] calls it
/// and logs the token; the `admin` commands never do.
pub fn first_invitation(st: &AppState) -> anyhow::Result<Option<String>> {
    if !st.spaces.is_empty() || !install::list(st.spaces.routing())?.is_empty() {
        return Ok(None);
    }
    Ok(Some(install::create(st.spaces.routing(), st.now())?))
}

/// What `tomi-relay` does about invitations once it is open and about to serve: on a fresh install
/// it creates the first invitation and logs its token, the one documented exception to "no token
/// in the logs" (`api-v1.md` §3.1); otherwise it logs no token. The relay keeps only the hash of
/// a token, so a token is shown here once; `admin invite --new` makes another. Returns the token
/// if one was created.
pub fn announce_invitation(st: &AppState) -> anyhow::Result<Option<String>> {
    let token = first_invitation(st)?;
    match &token {
        Some(t) => tracing::warn!("install token: {t}"),
        None if st.spaces.is_empty() => tracing::info!(
            "no space yet: `tomi-relay admin invite --new` creates an installation token"
        ),
        None => {}
    }
    Ok(token)
}

/// Start-up clean-up (§3.1, §8.3): the legacy installation token file, and, in every space,
/// blob files without a row and partial uploads whose file does not match the recorded offset.
fn startup_cleanup(cfg: &Config, spaces: &Spaces, now: i64) -> anyhow::Result<()> {
    // The older layout kept the token in clear in a file: it becomes an invitation (so the token
    // stays valid) unless a space exists, and the file goes.
    if let Some(token) = install::take_legacy_file(&cfg.data_dir) {
        if spaces.is_empty() {
            install::insert(spaces.routing(), &install::hash(&token), now)?;
        }
    }
    for space in spaces.list() {
        cleanup_space(&space)?;
    }
    Ok(())
}

fn cleanup_space(space: &Space) -> anyhow::Result<()> {
    let (db, dir) = (&space.db, &space.dir);
    let rows: Vec<(Vec<u8>, i64, i64)> = db.read_sync(|tx| {
        let mut st = tx.prepare("SELECT blob_id, complete, upload_offset FROM blobs")?;
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    })?;
    let known: HashSet<[u8; 16]> = rows
        .iter()
        .filter_map(|(id, _, _)| <[u8; 16]>::try_from(id.as_slice()).ok())
        .collect();
    let mut removed = 0usize;
    for id in blobstore::list(dir) {
        if !known.contains(&id) {
            blobstore::remove(dir, &id);
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(
            space = space.short_id(),
            "start-up: {removed} orphan blob files removed"
        );
    }
    for (id, complete, offset) in rows {
        let Ok(id) = <[u8; 16]>::try_from(id.as_slice()) else {
            continue;
        };
        if complete != 0 {
            continue;
        }
        let file = blobstore::path(dir, &id);
        let len =
            std::fs::metadata(&file).map_or(0, |m| i64::try_from(m.len()).unwrap_or(i64::MAX));
        if len > offset {
            // A part was being written when the process stopped: cut it off.
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&file) {
                let _ = f.set_len(u64::try_from(offset).unwrap_or(0));
            }
        } else if len < offset {
            db.write_sync(|tx| {
                tx.execute(
                    "UPDATE blobs SET upload_offset = ?2 WHERE blob_id = ?1",
                    params![id.to_vec(), len],
                )?;
                Ok(crate::db::Tx::Commit(()))
            })?;
        }
    }
    Ok(())
}
/// Serves the API on `listener` until `shutdown` completes. The WebSocket connections are
/// closed with `1001 going_away` first (§5.4).
pub async fn serve(
    state: AppState,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = routes::router(state.clone());
    let hub = state.hub.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        hub.close_all(close::GOING_AWAY, "going_away").await;
    })
    .await
}
