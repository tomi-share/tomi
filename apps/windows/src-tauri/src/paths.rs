//! Which local files the app may read, send, delete or reveal.
//!
//! The webview is not trusted: it must never be able to make Rust delete, upload or
//! reveal an arbitrary path just by naming it. So Rust keeps its own list of the files
//! it has itself vouched for, and only acts on those:
//!
//! - **staged**: files written by `stage_blob` (pasted screenshots) in the staging
//!   directory. They are the only files the app deletes.
//! - **user**: files the user designated through a channel Rust observes: drag and
//!   drop (`WindowEvent::DragDrop`), the file picker, "Envoyer vers". They may be sent,
//!   never deleted.
//! - **saved**: files written by "Enregistrer". They may be revealed in Explorer.
//!
//! Every check works on canonical paths and refuses any `..` component first, because
//! `Path::starts_with` is purely lexical (`staging\..\..\Documents\x` "starts with"
//! `staging`).

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct Inner {
    staged: HashSet<PathBuf>,
    user: HashSet<PathBuf>,
    saved: HashSet<PathBuf>,
}

pub struct PathRegistry {
    /// Canonical staging directory.
    staging_dir: PathBuf,
    inner: Mutex<Inner>,
}

/// Drops the `\\?\` prefix `canonicalize` adds on Windows (kept for UNC paths).
fn simplify(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let drive = rest.as_bytes();
        if drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    p
}

fn has_parent_component(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::ParentDir))
}

/// Canonical form of an existing path, or `None` when it contains `..` or does not
/// exist (nothing to act on).
pub fn resolve(p: &Path) -> Option<PathBuf> {
    if has_parent_component(p) {
        return None;
    }
    std::fs::canonicalize(p).ok().map(simplify)
}

impl PathRegistry {
    pub fn new(staging_dir: &Path) -> Self {
        let staging_dir = std::fs::canonicalize(staging_dir)
            .map(simplify)
            .unwrap_or_else(|_| staging_dir.to_path_buf());
        Self {
            staging_dir,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    fn inside_staging(&self, canonical: &Path) -> bool {
        canonical != self.staging_dir && canonical.starts_with(&self.staging_dir)
    }

    // ------------------------------------------------------------ staged

    /// Records a file `stage_blob` has just written. Refuses anything that is not a real
    /// file inside the staging directory.
    pub fn register_staged(&self, path: &Path) -> bool {
        let Some(canonical) = resolve(path) else {
            return false;
        };
        if !self.inside_staging(&canonical) || !canonical.is_file() {
            return false;
        }
        self.lock().staged.insert(canonical);
        true
    }

    /// Forgets a staged file and returns its canonical path when (and only when) it is
    /// one the app staged itself: this is the only path that may then be deleted.
    pub fn take_staged(&self, path: &Path) -> Option<PathBuf> {
        let canonical = resolve(path)?;
        if !self.inside_staging(&canonical) {
            return None;
        }
        self.lock().staged.remove(&canonical).then_some(canonical)
    }

    /// Deletes a staged file (and its now empty per-file folder). Returns whether
    /// something was removed. Paths the app did not stage are never touched.
    pub fn delete_staged(&self, path: &Path) -> bool {
        let Some(file) = self.take_staged(path) else {
            return false;
        };
        let removed = std::fs::remove_file(&file).is_ok();
        if let Some(parent) = file.parent() {
            if self.inside_staging(parent) {
                let _ = std::fs::remove_dir(parent);
            }
        }
        removed
    }

    // ------------------------------------------------------------ user

    /// Records a file the user designated (drop, picker, "Envoyer vers").
    pub fn register_user_file(&self, path: &Path) -> bool {
        let Some(canonical) = resolve(path) else {
            return false;
        };
        if !canonical.is_file() {
            return false;
        }
        self.lock().user.insert(canonical);
        true
    }

    pub fn forget_user_file(&self, path: &Path) {
        if let Some(canonical) = resolve(path) {
            self.lock().user.remove(&canonical);
        }
    }

    /// True when `path` may be described and sent: staged by the app or designated by
    /// the user through an observed channel.
    pub fn is_sendable(&self, path: &Path) -> bool {
        let Some(canonical) = resolve(path) else {
            return false;
        };
        let inner = self.lock();
        inner.staged.contains(&canonical) || inner.user.contains(&canonical)
    }

    // ------------------------------------------------------------ saved

    pub fn register_saved(&self, path: &Path) {
        if let Some(canonical) = resolve(path) {
            self.lock().saved.insert(canonical);
        }
    }

    /// True when `path` was written by "Enregistrer" and may be revealed in Explorer.
    pub fn is_revealable(&self, path: &Path) -> bool {
        resolve(path).is_some_and(|c| self.lock().saved.contains(&c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        staging: PathBuf,
        reg: PathRegistry,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).map(simplify).unwrap();
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let reg = PathRegistry::new(&staging);
        Env {
            _tmp: tmp,
            root,
            staging,
            reg,
        }
    }

    fn write(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"data").unwrap();
    }

    #[test]
    fn a_staged_file_is_deleted_with_its_folder() {
        let e = env();
        let f = e.staging.join("abc").join("shot.png");
        write(&f);
        assert!(e.reg.register_staged(&f));
        assert!(e.reg.delete_staged(&f));
        assert!(!f.exists());
        assert!(!f.parent().unwrap().exists());
        assert!(e.staging.exists());
    }

    #[test]
    fn a_file_inside_staging_that_the_app_did_not_stage_is_kept() {
        let e = env();
        let f = e.staging.join("planted.txt");
        write(&f);
        assert!(!e.reg.delete_staged(&f));
        assert!(f.exists());
    }

    #[test]
    fn dotdot_traversal_out_of_staging_never_deletes() {
        let e = env();
        let victim = e.root.join("Documents").join("x.txt");
        write(&victim);
        // Lexically "under staging", really outside.
        let sneaky = e
            .staging
            .join("sub")
            .join("..")
            .join("..")
            .join("Documents")
            .join("x.txt");
        assert!(
            sneaky.starts_with(&e.staging),
            "the lexical check is fooled"
        );
        assert!(!e.reg.register_staged(&sneaky));
        assert!(!e.reg.delete_staged(&sneaky));
        assert!(victim.exists());
    }

    #[test]
    fn registering_a_file_outside_staging_as_staged_is_refused() {
        let e = env();
        let outside = e.root.join("mine.bin");
        write(&outside);
        assert!(!e.reg.register_staged(&outside));
        assert!(!e.reg.delete_staged(&outside));
        assert!(outside.exists());
    }

    #[test]
    fn user_files_are_sendable_once_designated_and_never_deletable() {
        let e = env();
        let f = e.root.join("photo.jpg");
        write(&f);
        assert!(!e.reg.is_sendable(&f), "unknown paths are refused");
        assert!(e.reg.register_user_file(&f));
        assert!(e.reg.is_sendable(&f));
        assert!(!e.reg.delete_staged(&f));
        assert!(f.exists());
        e.reg.forget_user_file(&f);
        assert!(!e.reg.is_sendable(&f));
    }

    #[test]
    fn folders_and_missing_paths_cannot_be_designated() {
        let e = env();
        assert!(!e.reg.register_user_file(&e.root));
        assert!(!e.reg.register_user_file(&e.root.join("missing.bin")));
    }

    #[test]
    fn dotdot_paths_are_never_sendable_even_if_the_target_is_registered() {
        let e = env();
        let f = e.root.join("mine.bin");
        write(&f);
        assert!(e.reg.register_user_file(&f));
        let sneaky = e.staging.join("..").join("mine.bin");
        assert!(!e.reg.is_sendable(&sneaky));
    }

    #[test]
    fn only_saved_files_can_be_revealed() {
        let e = env();
        let saved = e.root.join("Tomi").join("a.pdf");
        write(&saved);
        let other = e.root.join("secret.txt");
        write(&other);
        assert!(!e.reg.is_revealable(&saved));
        e.reg.register_saved(&saved);
        assert!(e.reg.is_revealable(&saved));
        assert!(!e.reg.is_revealable(&other));
        assert!(!e
            .reg
            .is_revealable(&e.root.join("Tomi").join("..").join("a.pdf")));
    }

    #[test]
    fn simplify_strips_the_verbatim_prefix_of_drive_paths_only() {
        assert_eq!(
            simplify(PathBuf::from(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            simplify(PathBuf::from(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\?\UNC\srv\share")
        );
    }
}
