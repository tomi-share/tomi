//! Blob files on disk (`api-v1.md` §10): `<space_dir>/blobs/<2 hex>/<blob_id hex>`, where
//! `space_dir` is `<data_dir>/spaces/<space_id>`. A blob is never held in memory: parts are
//! copied from the request body to the file chunk by chunk.
//!
//! Every function takes the **directory of the space** (`Space::dir`): the files of two spaces
//! are never in the same tree.

use std::io;
use std::path::{Path, PathBuf};

/// Directory of the blob files of a space.
pub fn root(space_dir: &Path) -> PathBuf {
    space_dir.join("blobs")
}

/// File of a blob.
pub fn path(space_dir: &Path, blob_id: &[u8; 16]) -> PathBuf {
    let hex = hex::encode(blob_id);
    root(space_dir).join(&hex[..2]).join(hex)
}

/// Creates the directories `blobs/` and `blobs/<2 hex>/` of a blob file, **below a space
/// directory that must already exist**: the space directory itself is never created here, so an
/// upload that outlives the deletion of its space (`admin reset`) fails with `NotFound` instead
/// of leaving `spaces/<id>/blobs/` behind.
pub fn ensure_parent(file: &Path) -> io::Result<()> {
    fn make(dir: &Path) -> io::Result<()> {
        match std::fs::create_dir(dir) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            other => other,
        }
    }
    let Some(shard) = file.parent() else {
        return Ok(());
    };
    if let Some(blobs) = shard.parent() {
        make(blobs)?;
    }
    make(shard)
}

/// Removes a blob file; a missing file is fine.
pub fn remove(space_dir: &Path, blob_id: &[u8; 16]) {
    match std::fs::remove_file(path(space_dir, blob_id)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("blob file removal failed: {}", e.kind()),
    }
}

/// All blob ids that have a file (startup clean-up).
pub fn list(space_dir: &Path) -> Vec<[u8; 16]> {
    let mut out = Vec::new();
    let Ok(top) = std::fs::read_dir(root(space_dir)) else {
        return out;
    };
    for shard in top.flatten() {
        let Ok(files) = std::fs::read_dir(shard.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if let Some(id) = hex::decode(&name)
                .ok()
                .and_then(|b| <[u8; 16]>::try_from(b).ok())
            {
                out.push(id);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_listing() {
        let dir = tempfile::tempdir().unwrap();
        let id = [0xab; 16];
        let p = path(dir.path(), &id);
        assert!(p.ends_with(format!(
            "ab{}{}",
            std::path::MAIN_SEPARATOR,
            "ab".repeat(16)
        )));
        ensure_parent(&p).unwrap();
        ensure_parent(&p).unwrap();
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(list(dir.path()), vec![id]);
        remove(dir.path(), &id);
        remove(dir.path(), &id);
        assert!(list(dir.path()).is_empty());
    }

    #[test]
    fn the_space_directory_is_never_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let space = dir.path().join("gone");
        let p = path(&space, &[0xcd; 16]);
        let e = ensure_parent(&p).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(!space.exists());
    }
}
