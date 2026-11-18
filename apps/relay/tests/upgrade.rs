//! Upgrading a relay that runs the single-space layout (0.1.x) to the multi-space one, and the
//! rules that keep a command which holds no lock from writing: `api-v1.md` §10.1.1, §8.7, §3.1.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::*;
use rusqlite::Connection;
use tomi_relay::admin::{self, Cmd};

/// The token the 0.1.1 relay of production had written to `install-token`.
const LEGACY_TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";

/// Every file under `dir`, with its content (the databases are compared as bytes).
fn contents(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&p).unwrap_or_default(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// The old per-space schema of 0.1.1 had the push bookkeeping inside the space database.
const OLD_PUSH_TABLES: &str = r"
CREATE TABLE push_health (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  last_success_at INTEGER, last_error_at INTEGER, last_error TEXT, state TEXT NOT NULL
);
CREATE TABLE push_events (at INTEGER NOT NULL, kind TEXT NOT NULL);
";

/// Reproduces the data directory of the real server, as 0.1.1 left it when it was stopped
/// without a clean checkpoint:
///
/// - `tomi.db` (schema v1, **no row in `space`**, no device) with `tomi.db-wal` and `tomi.db-shm`;
/// - an **empty** `blobs/`;
/// - `install-token` (a pending token), `public/`, `relay.lock`, `admin.sock`, `config.toml`,
///   `smoke.log`.
fn production_state(root: &Path) {
    let scratch = tempfile::tempdir().unwrap();
    let db = scratch.path().join("tomi.db");
    // Kept open while the files are copied: the log is not folded into the main file, so a
    // non-empty `-wal` and a `-shm` exist, as after a crash.
    let conn = Connection::open(&db).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    conn.execute_batch(tomi_relay::db::SCHEMA).unwrap();
    conn.execute_batch(OLD_PUSH_TABLES).unwrap();
    conn.execute(
        "INSERT INTO push_health (id, state) VALUES (1, 'unknown')",
        [],
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let from = scratch.path().join(format!("tomi.db{suffix}"));
        assert!(from.is_file(), "tomi.db{suffix} must exist for this test");
        std::fs::copy(&from, root.join(format!("tomi.db{suffix}"))).unwrap();
    }
    assert!(std::fs::metadata(root.join("tomi.db-wal")).unwrap().len() > 0);
    drop(conn);

    std::fs::create_dir(root.join("blobs")).unwrap();
    std::fs::write(root.join("install-token"), format!("{LEGACY_TOKEN}\n")).unwrap();
    std::fs::create_dir_all(root.join("public/.well-known")).unwrap();
    std::fs::write(
        root.join("public/.well-known/assetlinks.json"),
        b"[{\"relation\":[]}]",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("public/l")).unwrap();
    std::fs::write(root.join("public/l/index.html"), b"<p>pair</p>").unwrap();
    std::fs::write(root.join("relay.lock"), b"").unwrap();
    std::fs::write(root.join("admin.sock"), b"").unwrap();
    std::fs::write(root.join("config.toml"), b"listen = \"127.0.0.1:8080\"\n").unwrap();
    std::fs::write(root.join("smoke.log"), b"smoke ok\n").unwrap();
}

#[tokio::test]
async fn the_data_directory_of_the_running_0_1_1_relay_upgrades() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    production_state(&root);
    let kept: Vec<&str> = vec![
        "public/.well-known/assetlinks.json",
        "public/l/index.html",
        "config.toml",
        "smoke.log",
        "admin.sock",
        "relay.lock",
    ];
    let before = contents(&root);
    for k in &kept {
        assert!(before.contains_key(Path::new(k)), "{k}");
    }

    // The new relay starts, with the lock, as the service does.
    let env = Env::in_dir(dir, |_, o| o.hold_lock = true).await;

    // The old database is gone, with its log; the empty blob directory too; the token file is
    // imported and removed.
    for gone in ["tomi.db", "tomi.db-wal", "tomi.db-shm", "install-token"] {
        assert!(!root.join(gone).exists(), "{gone} should be gone");
    }
    assert!(!root.join("blobs").exists());
    assert!(!root.join("spaces.migrating").exists());
    // Nothing else was touched.
    let after = contents(&root);
    for k in &kept {
        assert_eq!(after.get(Path::new(k)), before.get(Path::new(k)), "{k}");
    }
    assert!(env.state.spaces.is_empty());
    assert_eq!(
        env.install_token, "",
        "the token of the file is kept: no new one"
    );

    // Nobody has a space yet, and the token that was in the file creates it.
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], false);
    let mut founder = Dev::new("PC");
    env.create_space_raw(&founder, LEGACY_TOKEN).await.ok(201);
    env.login(&mut founder, 1, 1).await;
    assert!(root
        .join("spaces")
        .join(&env.space_id)
        .join("relay.db")
        .is_file());

    // A retry after a lost 201 is told the space exists, and a restart does not change that.
    let env = env.restart().await;
    env.create_space_raw(&founder, LEGACY_TOKEN)
        .await
        .err(409, "space_exists");
    env.login_raw(&founder, 1, 1).await.ok(200);
}

#[tokio::test]
async fn a_command_that_holds_no_lock_does_not_migrate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    production_state(&root);
    let before = contents(&root);

    let err = open(
        Config::for_tests(&root),
        Options {
            read_only: true,
            hold_lock: false,
            background: false,
            cleanup: false,
            push: PushChoice::Disabled,
            ..Options::default()
        },
    )
    .await
    .err()
    .unwrap();
    let text = format!("{err:#}");
    assert!(text.contains("not migrated yet"), "{text}");
    assert!(!text.contains("restart"), "{text}");
    // The intact old layout is not a refusal because of the data: the previous version reads it
    // (R2), so `admin status` does not exit `78` and a rollback is not blocked.
    assert_eq!(tomi_relay::spaces::start_exit_code(&err), 1);
    assert!(root.join("tomi.db").is_file());
    assert!(!root.join("spaces").exists());
    assert!(!root.join("spaces.migrating").exists());
    assert!(!root.join("routing.db").exists());
    // (the copy of the database may have gained a shm/wal sidecar from the failed check: the
    // content of every file we put there is still the same)
    let after = contents(&root);
    for (k, v) in &before {
        assert_eq!(after.get(k), Some(v), "{}", k.display());
    }
}

/// R2: what is NOT the intact old layout stays a refusal because of the data (`78`) for a command
/// that holds no lock: both layouts at once, or a migration under way / interrupted.
#[tokio::test]
async fn a_read_only_command_refuses_with_the_data_exit_code_when_the_layouts_mix() {
    for leftover in ["spaces", "spaces.migrating"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        production_state(&root);
        std::fs::create_dir(root.join(leftover)).unwrap();
        let err = open(Config::for_tests(&root), read_only_options())
            .await
            .err()
            .unwrap();
        assert_eq!(
            tomi_relay::spaces::start_exit_code(&err),
            tomi_relay::spaces::EXIT_DATA_REFUSED,
            "{leftover}: {err:#}"
        );
        assert!(root.join("tomi.db").is_file());
    }
}

fn read_only_options() -> Options {
    Options {
        read_only: true,
        hold_lock: false,
        background: false,
        cleanup: false,
        push: PushChoice::Disabled,
        ..Options::default()
    }
}

#[tokio::test]
async fn a_read_only_command_creates_nothing() {
    // A data directory that does not exist: it is not created, nor is routing.db.
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("never-created");
    let st = open(Config::for_tests(&missing), read_only_options())
        .await
        .unwrap();
    let out = admin::run(
        &st,
        Cmd::Status {
            json: true,
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(out.stdout.contains("\"spaces\":0"), "{}", out.stdout);
    let out = admin::run(
        &st,
        Cmd::Invite {
            rotate: false,
            new: false,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    // A write is refused, not performed.
    let out = admin::run(
        &st,
        Cmd::Invite {
            rotate: false,
            new: true,
        },
    )
    .await;
    assert_ne!(out.exit, 0);
    assert!(!missing.exists());

    // An empty directory stays empty.
    let empty = tempfile::tempdir().unwrap();
    let st = open(Config::for_tests(empty.path()), read_only_options())
        .await
        .unwrap();
    admin::run(&st, Cmd::Spaces { json: false }).await;
    assert_eq!(std::fs::read_dir(empty.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_read_only_command_changes_no_database() {
    let env = Env::new().await;
    let (_devs, _) = env.space_of(2).await;
    // A quota override of a space that is gone: the relay forgets it when it starts, a command
    // that only reads must not.
    let ghost = new_uuid();
    env.state
        .spaces
        .routing()
        .write_sync(|tx| {
            tx.execute(
                "INSERT INTO space_quotas (space_id, max_bytes) VALUES (?1, 5)",
                [&ghost],
            )?;
            Ok(tomi_relay::db::Tx::Commit(()))
        })
        .unwrap();
    env.state.spaces.close_all();
    let root = env.data_dir().to_path_buf();
    let databases = |root: &Path| -> BTreeMap<PathBuf, Vec<u8>> {
        contents(root)
            .into_iter()
            .filter(|(k, _)| {
                let n = k.file_name().unwrap().to_string_lossy().into_owned();
                n == "routing.db" || n == "relay.db"
            })
            .collect()
    };
    let before = databases(&root);
    assert_eq!(before.len(), 2);

    let st = open(Config::for_tests(&root), read_only_options())
        .await
        .unwrap();
    for cmd in [
        Cmd::Status {
            json: false,
            space: None,
        },
        Cmd::Spaces { json: true },
        Cmd::Devices {
            json: false,
            space: None,
        },
        Cmd::Invite {
            rotate: false,
            new: false,
        },
    ] {
        let out = admin::run(&st, cmd).await;
        assert_eq!(out.exit, 0, "{}", out.stderr);
    }
    let out = admin::run(
        &st,
        Cmd::Status {
            json: true,
            space: None,
        },
    )
    .await;
    assert!(out.stdout.contains("\"spaces\":1"), "{}", out.stdout);
    st.spaces.close_all();
    drop(st);

    assert_eq!(databases(&root), before, "not a byte changed");
    let routing = Connection::open_with_flags(
        root.join("routing.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let n: i64 = routing
        .query_row("SELECT count(*) FROM space_quotas", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "the dangling override is still there");
    let mode: String = routing
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");

    // The relay itself keeps it at its next start too (`api-v1.md` §10.1): a start cannot tell
    // a deleted space from a volume that is not mounted, and the row is harmless.
    drop(routing);
    let env = env.restart().await;
    let n: i64 = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| Ok(tx.query_row("SELECT count(*) FROM space_quotas", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn a_deleted_space_leaves_nothing_and_an_upload_cannot_bring_it_back() {
    let (a, b) = {
        let a = Env::new().await;
        let b = a.another_space().await;
        (a, b)
    };
    let (da, _) = a.space_of(1).await;
    b.space_of(1).await;
    let blob = a.upload_blob(&da[0], &[7u8; 300], 300).await;
    // An upload in flight holds the space it started in.
    let held = a.space();
    let dir = held.dir.clone();
    let path = tomi_relay::blobstore::path(&dir, &uuid_bytes(&blob));
    assert!(path.is_file());

    let out = admin::run(
        &a.state,
        Cmd::Reset {
            confirm: Some(a.space_id[..8].to_owned()),
            space: Some(a.space_id[..8].to_owned()),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(!dir.exists());
    let leftovers: Vec<String> = std::fs::read_dir(a.data_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("spaces.deleted-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // The part that was about to be written finds no directory, and makes none.
    let late = uuid_bytes(&new_uuid());
    let e = tomi_relay::blobstore::ensure_parent(&tomi_relay::blobstore::path(&dir, &late))
        .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    assert!(!dir.exists(), "spaces/<id> was not brought back");
    drop(held);

    // The other space is untouched, and a crash leftover is cleaned at the next start.
    let debris = a.data_dir().join("spaces.deleted-crash-0000");
    std::fs::create_dir_all(debris.join("blobs")).unwrap();
    std::fs::write(debris.join("blobs/x"), b"x").unwrap();
    let b_id = b.space_id.clone();
    drop(b);
    let a = a.restart().await;
    assert!(!debris.exists());
    assert!(a.state.spaces.get(&uuid_bytes(&b_id)).is_some());
}
