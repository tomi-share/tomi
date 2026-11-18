//! Migration of the single-space layout (`<data_dir>/tomi.db` and `<data_dir>/blobs/`) to the
//! multi-space layout (`<data_dir>/spaces/<space_id>/relay.db` and `.../blobs/`).
//!
//! The relay 0.1.x in production has the old layout, so the migration is the simplest safe one:
//! renames inside the same directory, nothing is copied or rewritten, and an unclear state is
//! refused instead of guessed. `routing.db` is not migrated; it is created empty and the lookup
//! tables are rebuilt from the space databases (`spaces.rs`).
//!
//! Only a process that holds `relay.lock` migrates. The working directory is reserved with
//! `create_dir` (it fails if it exists), and the only one ever removed is the one the process
//! created.
//!
//! The new tree is first assembled under `spaces.migrating/` and switched into place by one
//! last rename to `spaces/`: `spaces/` existing means the migration is complete. If a step
//! fails, the renames already done are undone; if even that fails, `spaces.migrating/` stays and
//! the next start refuses to run until an operator has looked.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use rusqlite::{Connection, OpenFlags};

use crate::spaces::{SPACES_DIR, SPACE_DB_FILE};
use crate::wire::uuid_format;

/// Database file of the old layout.
pub const LEGACY_DB_FILE: &str = "tomi.db";
/// Staging directory of the migration.
const STAGING_DIR: &str = "spaces.migrating";

/// The data directory still has the intact single-space layout, not migrated yet. Seen by a
/// process that does not migrate (`admin status` without the lock): the directory is fine, it is
/// just older than this binary. NOT a refusal because of the data: the previous version reads it.
#[derive(Debug)]
pub struct LegacyLayout(PathBuf);

impl std::fmt::Display for LegacyLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} still has the single-space layout, not migrated yet: it is migrated when the \
             relay of this version starts (the previous version still reads it as it is)",
            self.0.display()
        )
    }
}

impl std::error::Error for LegacyLayout {}

fn has_files(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        if p.is_dir() {
            has_files(&p)
        } else {
            true
        }
    })
}

/// Sidecar files of an SQLite database (`-wal`, `-shm`).
fn sidecars(db: &Path) -> [PathBuf; 2] {
    let name = db
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    [
        db.with_file_name(format!("{name}-wal")),
        db.with_file_name(format!("{name}-shm")),
    ]
}

/// Migrates the data directory if it has the old layout. A directory that already has the new
/// layout, or neither, is left alone.
///
/// `may_migrate` says whether this process holds `relay.lock` (a serving relay, or an `admin`
/// command that writes). Without the lock the directory is only looked at: if it still has the
/// old layout, or a migration is under way or was interrupted, the call fails with a message that
/// says so, and nothing is touched. Only the intact old layout (`tomi.db` alone, no `spaces/`, no
/// `spaces.migrating/`) is a plain [`LegacyLayout`] (not a refusal because of the data: the previous
/// version still reads it); a migration under way or interrupted, or both layouts at once, are
/// refusals because of the data.
pub fn run(data_dir: &Path, may_migrate: bool) -> anyhow::Result<()> {
    let old_db = data_dir.join(LEGACY_DB_FILE);
    let old_blobs = data_dir.join("blobs");
    let spaces = data_dir.join(SPACES_DIR);
    let staging = data_dir.join(STAGING_DIR);

    if !may_migrate {
        let present = |p: &Path| std::fs::symlink_metadata(p).is_ok();
        if present(&staging) {
            bail!(
                "a migration of {} is under way or was interrupted ({} exists): do not start any \
                 version until an operator has looked",
                data_dir.display(),
                staging.display()
            );
        }
        if present(&old_db) {
            if present(&spaces) {
                bail!(
                    "both the old layout ({}) and the new one ({}) are present: refusing to guess \
                     which is current",
                    old_db.display(),
                    spaces.display()
                );
            }
            return Err(LegacyLayout(data_dir.to_path_buf()).into());
        }
        return Ok(());
    }

    if staging.exists() {
        bail!(
            "an earlier migration was interrupted ({} exists): the old files are in {} and {}; \
             move them back or finish by hand, then remove {}",
            staging.display(),
            data_dir.display(),
            staging.display(),
            staging.display()
        );
    }
    if !old_db.exists() {
        if has_files(&old_blobs) {
            bail!(
                "{} holds blob files but there is no {LEGACY_DB_FILE} to go with them: \
                 refusing to start",
                old_blobs.display()
            );
        }
        return Ok(());
    }
    if spaces.exists() {
        bail!(
            "both the old layout ({}) and the new one ({}) are present: refusing to guess which is \
             current",
            old_db.display(),
            spaces.display()
        );
    }

    // The working directory is reserved by creating it: `create_dir` fails if it exists, so two
    // processes never work in the same one, and the only staging directory this function ever
    // removes is the one it created.
    std::fs::create_dir(&staging).with_context(|| {
        format!(
            "cannot reserve {} (is another process migrating this directory?)",
            staging.display()
        )
    })?;
    let mut keep_staging = false;
    let outcome = migrate_reserved(data_dir, &staging, &mut keep_staging);
    if !keep_staging {
        // Empty after a success (renamed to `spaces/`: nothing to remove) or after a clean
        // failure (every rename undone).
        let _ = std::fs::remove_dir_all(&staging);
    }
    outcome
}

/// The migration proper, once `staging` is reserved. `keep_staging` is set when the staging
/// directory must stay for a human (a failure that could not be undone).
fn migrate_reserved(
    data_dir: &Path,
    staging: &Path,
    keep_staging: &mut bool,
) -> anyhow::Result<()> {
    let old_db = data_dir.join(LEGACY_DB_FILE);
    let old_blobs = data_dir.join("blobs");
    let spaces = data_dir.join(SPACES_DIR);

    // Another process may have finished between the checks and the reservation.
    if !old_db.exists() || spaces.exists() {
        return Ok(());
    }

    // 1. The old database: flush its log, read the space id. (Never created by this open.)
    let space_id = {
        let conn = Connection::open_with_flags(
            &old_db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("cannot open {}", old_db.display()))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != 1 {
            bail!(
                "{} has schema version {version}, expected 1: refusing to migrate",
                old_db.display()
            );
        }
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .context("cannot checkpoint the old database")?;
        let id: Option<Vec<u8>> = conn
            .query_row("SELECT space_id FROM space WHERE id = 1", [], |r| r.get(0))
            .ok();
        match id {
            Some(raw) => <[u8; 16]>::try_from(raw.as_slice()).map_err(|_| {
                anyhow::anyhow!("the space id of {} is malformed", old_db.display())
            })?,
            None => {
                // No space was ever created here: nothing to keep.
                let devices: i64 = conn
                    .query_row("SELECT count(*) FROM devices", [], |r| r.get(0))
                    .unwrap_or(1);
                drop(conn);
                if devices != 0 || has_files(&old_blobs) {
                    bail!(
                        "{} has no space but has devices or blob files: refusing to start",
                        old_db.display()
                    );
                }
                for f in std::iter::once(old_db.clone()).chain(sidecars(&old_db)) {
                    match std::fs::remove_file(&f) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(e).with_context(|| format!("cannot remove {}", f.display()))
                        }
                    }
                }
                // Only an empty `blobs/` tree: no file in it (`has_files`), nothing else is touched.
                let _ = std::fs::remove_dir_all(&old_blobs);
                tracing::info!("migration: the old database held no space; it was removed");
                return Ok(());
            }
        }
    };

    // 2. Assemble the new tree under the staging name.
    let name = uuid_format(&space_id);
    let target_dir = staging.join(&name);
    let target_db = target_dir.join(SPACE_DB_FILE);
    let target_blobs = target_dir.join("blobs");
    std::fs::create_dir(&target_dir)
        .with_context(|| format!("cannot create {}", target_dir.display()))?;

    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let moved = (|| -> anyhow::Result<()> {
        let mut pairs = vec![(old_db.clone(), target_db.clone())];
        for (from, to) in sidecars(&old_db).into_iter().zip(sidecars(&target_db)) {
            if from.exists() {
                pairs.push((from, to));
            }
        }
        for (from, to) in pairs {
            std::fs::rename(&from, &to)
                .with_context(|| format!("cannot move {}", from.display()))?;
            done.push((from, to));
        }
        if old_blobs.exists() {
            std::fs::rename(&old_blobs, &target_blobs)
                .with_context(|| format!("cannot move {}", old_blobs.display()))?;
            done.push((old_blobs.clone(), target_blobs.clone()));
        } else {
            std::fs::create_dir_all(&target_blobs)?;
        }
        // 3. The switch: from here on `spaces/` exists and the migration is over.
        std::fs::rename(staging, &spaces).with_context(|| {
            format!(
                "cannot rename {} to {}",
                staging.display(),
                spaces.display()
            )
        })
    })();

    if let Err(e) = moved {
        let mut undone = true;
        for (from, to) in done.iter().rev() {
            if std::fs::rename(to, from).is_err() {
                undone = false;
            }
        }
        if undone {
            return Err(e.context("the migration failed; the old layout was restored"));
        }
        *keep_staging = true;
        return Err(e.context(format!(
            "the migration failed and could not be undone: see {}",
            staging.display()
        )));
    }
    tracing::info!(
        space = &name[..8],
        "migration: single-space layout moved to spaces/"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_db(path: &Path, space_id: Option<[u8; 16]>) {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        if let Some(id) = space_id {
            conn.execute(
                "INSERT INTO space (id, space_id, created_at, key_epoch) VALUES (1, ?1, 7, 1)",
                [id.to_vec()],
            )
            .unwrap();
        }
    }

    #[test]
    fn nothing_to_migrate_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), true).unwrap();
        std::fs::create_dir_all(dir.path().join("blobs")).unwrap();
        run(dir.path(), true).unwrap();
        assert!(!dir.path().join(SPACES_DIR).exists());
    }

    #[test]
    fn the_old_layout_moves_into_the_space_directory() {
        let dir = tempfile::tempdir().unwrap();
        let id = [0x42; 16];
        legacy_db(&dir.path().join(LEGACY_DB_FILE), Some(id));
        let blob = dir.path().join("blobs").join("ab");
        std::fs::create_dir_all(&blob).unwrap();
        std::fs::write(blob.join("ab".repeat(16)), b"x").unwrap();

        run(dir.path(), true).unwrap();

        let target = dir.path().join(SPACES_DIR).join(uuid_format(&id));
        assert!(target.join(SPACE_DB_FILE).is_file());
        assert!(target.join("blobs/ab").join("ab".repeat(16)).is_file());
        assert!(!dir.path().join(LEGACY_DB_FILE).exists());
        assert!(!dir.path().join("blobs").exists());
        assert!(!dir.path().join(STAGING_DIR).exists());
        // Idempotent: the new layout is left alone.
        run(dir.path(), true).unwrap();
    }

    #[test]
    fn a_database_without_blobs_gets_an_empty_blob_directory() {
        let dir = tempfile::tempdir().unwrap();
        let id = [0x43; 16];
        legacy_db(&dir.path().join(LEGACY_DB_FILE), Some(id));
        run(dir.path(), true).unwrap();
        let target = dir.path().join(SPACES_DIR).join(uuid_format(&id));
        assert!(target.join("blobs").is_dir());
    }

    #[test]
    fn both_layouts_at_once_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        legacy_db(&dir.path().join(LEGACY_DB_FILE), Some([1; 16]));
        std::fs::create_dir_all(dir.path().join(SPACES_DIR)).unwrap();
        let err = run(dir.path(), true).unwrap_err().to_string();
        assert!(err.contains("both"), "{err}");
        assert!(
            dir.path().join(LEGACY_DB_FILE).exists(),
            "nothing was touched"
        );
    }

    #[test]
    fn an_interrupted_migration_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(STAGING_DIR)).unwrap();
        let err = run(dir.path(), true).unwrap_err().to_string();
        assert!(err.contains("interrupted"), "{err}");
    }

    #[test]
    fn blobs_without_a_database_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let blob = dir.path().join("blobs").join("ab");
        std::fs::create_dir_all(&blob).unwrap();
        std::fs::write(blob.join("ab".repeat(16)), b"x").unwrap();
        let err = run(dir.path(), true).unwrap_err().to_string();
        assert!(err.contains("blob files"), "{err}");
    }

    #[test]
    fn an_old_database_without_a_space_is_dropped_but_one_with_devices_is_not() {
        let dir = tempfile::tempdir().unwrap();
        legacy_db(&dir.path().join(LEGACY_DB_FILE), None);
        run(dir.path(), true).unwrap();
        assert!(!dir.path().join(LEGACY_DB_FILE).exists());

        let conn_path = dir.path().join(LEGACY_DB_FILE);
        legacy_db(&conn_path, None);
        {
            let conn = Connection::open(&conn_path).unwrap();
            conn.execute(
                "INSERT INTO devices (device_id, name, platform, issuer_device_id, joined_seq, registered_at)
                 VALUES ('d', 'n', 'p', 'd', 0, 0)",
                [],
            )
            .unwrap();
        }
        assert!(run(dir.path(), true).is_err());
        assert!(conn_path.exists());
    }

    #[test]
    fn without_the_lock_nothing_is_migrated_and_nothing_is_touched() {
        let dir = tempfile::tempdir().unwrap();
        let id = [0x44; 16];
        legacy_db(&dir.path().join(LEGACY_DB_FILE), Some(id));
        let err = run(dir.path(), false).unwrap_err();
        // The intact old layout: a plain `LegacyLayout`, not a refusal because of the data.
        assert!(err.is::<LegacyLayout>(), "{err:#}");
        assert!(err.to_string().contains("not migrated yet"), "{err:#}");
        assert!(dir.path().join(LEGACY_DB_FILE).is_file());
        assert!(!dir.path().join(SPACES_DIR).exists());
        assert!(!dir.path().join(STAGING_DIR).exists());
        // An interrupted migration is not for a reader to judge either.
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join(STAGING_DIR)).unwrap();
        let err = run(other.path(), false).unwrap_err();
        assert!(!err.is::<LegacyLayout>(), "{err:#}");
        assert!(other.path().join(STAGING_DIR).is_dir(), "left alone");
        // The new layout, or nothing, is fine.
        run(tempfile::tempdir().unwrap().path(), false).unwrap();
        run(dir.path(), true).unwrap();
        run(dir.path(), false).unwrap();
    }

    #[test]
    fn a_foreign_staging_directory_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        legacy_db(&dir.path().join(LEGACY_DB_FILE), Some([0x45; 16]));
        // Someone else is working in the staging directory.
        let staging = dir.path().join(STAGING_DIR);
        std::fs::create_dir_all(staging.join("theirs")).unwrap();
        std::fs::write(staging.join("theirs").join("file"), b"x").unwrap();
        assert!(run(dir.path(), true).is_err());
        assert!(staging.join("theirs").join("file").is_file());
        assert!(dir.path().join(LEGACY_DB_FILE).is_file());
    }

    #[test]
    fn two_processes_migrating_at_once_lose_nothing() {
        for _ in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let id = [0x46; 16];
            legacy_db(&dir.path().join(LEGACY_DB_FILE), Some(id));
            let blob = dir.path().join("blobs").join("ab");
            std::fs::create_dir_all(&blob).unwrap();
            std::fs::write(blob.join("ab".repeat(16)), b"precious").unwrap();

            let results: Vec<anyhow::Result<()>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..2).map(|_| s.spawn(|| run(dir.path(), true))).collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(results.iter().any(Result::is_ok), "{results:?}");
            let target = dir.path().join(SPACES_DIR).join(uuid_format(&id));
            assert!(target.join(SPACE_DB_FILE).is_file());
            assert_eq!(
                std::fs::read(target.join("blobs/ab").join("ab".repeat(16))).unwrap(),
                b"precious"
            );
            assert!(!dir.path().join(LEGACY_DB_FILE).exists(), "no stray file");
            assert!(!dir.path().join(STAGING_DIR).exists());
        }
    }

    #[test]
    fn an_unknown_schema_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(LEGACY_DB_FILE);
        legacy_db(&p, Some([1; 16]));
        Connection::open(&p)
            .unwrap()
            .pragma_update(None, "user_version", 9)
            .unwrap();
        assert!(run(dir.path(), true)
            .unwrap_err()
            .to_string()
            .contains("version 9"));
    }
}
