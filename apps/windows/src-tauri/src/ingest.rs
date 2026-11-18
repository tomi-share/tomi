//! Inputs that fill the popover from outside: dropped / picked / sent-to files, and
//! `tomi-share://` URLs. Nothing here sends anything: it only produces a prefill.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::deeplink::{self, DeepLinkAction};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::limits::{self, FileCandidate, Rejection};
use crate::model::Attachment;
use crate::paths::PathRegistry;
use crate::popover;
use crate::secret::SecretText;
use crate::state::{AppState, PendingPair, PendingSetup, PrefillPayload};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesResult {
    pub accepted: Vec<Attachment>,
    pub rejected: Vec<Rejection>,
}

/// What the limits need to know about a path: name, size, regular file or not.
fn candidate(p: &str) -> FileCandidate {
    let path = Path::new(p);
    let meta = std::fs::metadata(path).ok();
    FileCandidate {
        name: path
            .file_name()
            .and_then(|n| n.to_str())
            .map(files::sanitize_file_name)
            .unwrap_or_else(|| "fichier".to_string()),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        is_file: meta.is_some_and(|m| m.is_file()),
    }
}

/// Checks and describes local paths (blocking: reads metadata and builds thumbnails).
/// Does not decide whether a path may be used: see [`describe_known`] and
/// [`designate_and_describe`].
/// `existing_bytes` / `existing_count` is what the draft already holds.
pub fn describe_paths(paths: &[String], existing_bytes: u64, existing_count: usize) -> FilesResult {
    let candidates: Vec<FileCandidate> = paths.iter().map(|p| candidate(p)).collect();
    let check = limits::check_files(existing_bytes, existing_count, &candidates);
    let accepted = check
        .accepted
        .iter()
        .filter_map(|&i| files::describe(Path::new(&paths[i])).ok())
        .collect();
    FilesResult {
        accepted,
        rejected: check.rejected,
    }
}

fn unknown_path_rejection(p: &str) -> Rejection {
    Rejection {
        name: Path::new(p)
            .file_name()
            .and_then(|n| n.to_str())
            .map(files::sanitize_file_name)
            .unwrap_or_else(|| "fichier".to_string()),
        size: 0,
        reason: limits::RejectReason::NotAFile,
    }
}

/// Paths named by the webview (drag and drop). Only files Rust saw being dropped are
/// described; any other path is refused as if it were not a file.
pub fn describe_known(
    registry: &PathRegistry,
    paths: &[String],
    existing_bytes: u64,
    existing_count: usize,
) -> FilesResult {
    let (known, unknown): (Vec<String>, Vec<String>) = paths
        .iter()
        .cloned()
        .partition(|p| registry.is_sendable(Path::new(p)));
    let mut result = describe_paths(&known, existing_bytes, existing_count);
    result
        .rejected
        .extend(unknown.iter().map(|p| unknown_path_rejection(p)));
    result
}

/// Paths the user designated through a channel Rust controls (file picker, "Envoyer
/// vers"): they are recorded as sendable, then described.
pub fn designate_and_describe(
    registry: &PathRegistry,
    paths: &[String],
    existing_bytes: u64,
    existing_count: usize,
) -> FilesResult {
    for p in paths {
        registry.register_user_file(Path::new(p));
    }
    describe_paths(paths, existing_bytes, existing_count)
}

/// Re-describes the attachments of a draft from the disk, whatever the front end
/// declared. Every path must be one Rust vouches for (staged, dropped, picked, sent to)
/// and must still be a regular file within the size limits.
pub fn validate_attachments(
    registry: &PathRegistry,
    declared: &[Attachment],
) -> AppResult<Vec<Attachment>> {
    let mut paths = Vec::with_capacity(declared.len());
    for a in declared {
        let path = a
            .path
            .as_deref()
            .ok_or_else(|| AppError::new("error.attachment_missing"))?;
        if !registry.is_sendable(Path::new(path)) {
            return Err(AppError::new("error.attachment_missing"));
        }
        paths.push(path);
    }
    let candidates: Vec<FileCandidate> = paths.iter().map(|p| candidate(p)).collect();
    let check = limits::check_files(0, 0, &candidates);
    if let Some(r) = check.rejected.into_iter().next() {
        return Err(match r.reason {
            limits::RejectReason::NotAFile => AppError::new("error.attachment_missing"),
            limits::RejectReason::TooLarge => AppError::with(
                "error.file_too_large",
                serde_json::json!({ "name": r.name }),
            ),
            limits::RejectReason::ItemTotalExceeded | limits::RejectReason::TooMany => {
                AppError::new("error.item_too_large")
            }
        });
    }
    paths
        .iter()
        .map(|p| {
            files::describe(Path::new(p)).map_err(|_| AppError::new("error.attachment_missing"))
        })
        .collect()
}
/// Stores the prefill, wakes the popover and tells the front end to pick it up.
fn deliver_prefill(app: &AppHandle, payload: PrefillPayload) {
    *app.state::<AppState>()
        .pending_prefill
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(payload);
    popover::show_popover(app);
    let _ = app.emit_to(popover::POPOVER, "prefill-available", ());
}

/// A pairing link: the code waits for the pairing screen, which shows it and waits for the
/// user to press "Rejoindre". Nothing is claimed, sent or started here
/// (`spec/pairing-v1.md` §4.4: a link never starts a pairing by itself).
fn deliver_pair_link(app: &AppHandle, code: SecretText, relay_url: Option<String>) {
    *app.state::<AppState>()
        .pending_pair
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(PendingPair { code, relay_url });
    popover::open_history(app, Some("devices"));
    // A window that is already open picks the code up now; one that opens picks it up when
    // the pairing screen is shown.
    let _ = app.emit_to(popover::HISTORY, "pair-link-available", ());
}

/// A creation link: the token and the relay wait for the creation screen, which shows the relay
/// in full and waits for the user to press "Créer". Nothing is sent here
/// (`spec/pairing-v1.md` §10).
fn deliver_setup_link(app: &AppHandle, token: SecretText, relay_url: String) {
    *app.state::<AppState>()
        .pending_setup
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(PendingSetup { token, relay_url });
    popover::open_history(app, Some("devices"));
    let _ = app.emit_to(popover::HISTORY, "setup-link-available", ());
}

/// A `tomi-share://` URL, from the OS (first instance) or forwarded by a second instance.
pub fn handle_deeplink_url(app: &AppHandle, raw: &str) {
    match deeplink::parse(raw) {
        Ok(DeepLinkAction::Open) => popover::show_popover(app),
        Ok(DeepLinkAction::OpenItem { id }) => popover::show_item(app, &id),
        Ok(DeepLinkAction::Pair { code, relay_url }) => deliver_pair_link(app, code, relay_url),
        Ok(DeepLinkAction::Create { token, relay_url }) => {
            deliver_setup_link(app, token, relay_url)
        }
        Ok(DeepLinkAction::Prefill(p)) => deliver_prefill(
            app,
            PrefillPayload {
                url: p.url,
                title: p.title,
                text: p.text,
                truncated: p.truncated,
                ..Default::default()
            },
        ),
        Err(e) => deliver_prefill(
            app,
            PrefillPayload {
                error: Some(e.code().to_string()),
                ..Default::default()
            },
        ),
    }
}

/// Command-line arguments of this or a second instance: `tomi-share://` URLs and file
/// paths (what "Envoyer vers" passes). `args[0]` is the executable.
///
/// URLs are skipped when `skip_urls` is set: the single-instance plugin has already
/// forwarded them to the deep-link plugin.
pub fn handle_args(app: &AppHandle, args: Vec<String>, skip_urls: bool) {
    let (urls, paths) = split_args(args);
    if !skip_urls {
        for url in &urls {
            handle_deeplink_url(app, url);
        }
    }
    // A `tomi-share:` URL wins: it is one request, and the file paths next to it would
    // be a second, unrelated one overwriting its prefill.
    if !urls.is_empty() || paths.is_empty() {
        return;
    }
    let registry = app.state::<AppState>().paths.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = tauri::async_runtime::spawn_blocking(move || {
            designate_and_describe(&registry, &paths, 0, 0)
        })
        .await
        .unwrap_or_default();
        deliver_prefill(
            &app,
            PrefillPayload {
                files: result.accepted,
                rejected: result.rejected,
                ..Default::default()
            },
        );
    });
}

/// Separates the `tomi-share:` URLs from the file paths of a command line. `args[0]` is the
/// executable; options (`--autostart`) and arguments that are neither a URL nor an
/// existing path are ignored.
fn split_args(args: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut urls = Vec::new();
    let mut paths = Vec::new();
    for arg in args.into_iter().skip(1) {
        if arg.starts_with("--") {
            continue;
        }
        if arg.to_ascii_lowercase().starts_with("tomi-share:") {
            urls.push(arg);
        } else if PathBuf::from(&arg).exists() {
            paths.push(arg);
        }
    }
    (urls, paths)
}

/// Writes pasted bytes (e.g. a screenshot) into the staging directory and records the
/// file as staged: the only kind of file the app will ever delete.
pub fn stage_bytes(registry: &PathRegistry, name: &str, bytes: &[u8]) -> AppResult<Attachment> {
    if bytes.len() as u64 > limits::MAX_FILE_BYTES {
        return Err(AppError::with(
            "error.file_too_large",
            serde_json::json!({ "name": name }),
        ));
    }
    let dir = registry
        .staging_dir()
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let path = files::unique_path(&dir, name);
    std::fs::write(&path, bytes)?;
    if !registry.register_staged(&path) {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        return Err(AppError::new("error.internal"));
    }
    Ok(files::describe(&path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_paths_refuses_per_file_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.txt");
        std::fs::write(&ok, b"hello").unwrap();
        let big = dir.path().join("big.bin");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(limits::MAX_FILE_BYTES + 1).unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();

        let paths: Vec<String> = [&ok, &big, &folder]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let r = describe_paths(&paths, 0, 0);
        assert_eq!(r.accepted.len(), 1);
        assert_eq!(r.accepted[0].name, "ok.txt");
        assert_eq!(r.rejected.len(), 2);
        assert_eq!(r.rejected[0].name, "big.bin");
        assert_eq!(r.rejected[0].reason, limits::RejectReason::TooLarge);
        assert_eq!(r.rejected[1].reason, limits::RejectReason::NotAFile);
    }

    #[test]
    fn describe_paths_ignores_missing_files_as_not_files() {
        let r = describe_paths(&["C:/definitely/missing.bin".to_string()], 0, 0);
        assert!(r.accepted.is_empty());
        assert_eq!(r.rejected[0].reason, limits::RejectReason::NotAFile);
    }

    fn registry_in(dir: &Path) -> PathRegistry {
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        PathRegistry::new(&staging)
    }

    #[test]
    fn stage_bytes_writes_a_sanitised_copy_and_enforces_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_in(dir.path());
        let a = stage_bytes(&reg, "..\\..\\evil.png", b"not really a png").unwrap();
        let path = PathBuf::from(a.path.unwrap());
        assert!(path.starts_with(reg.staging_dir()));
        assert_eq!(path.file_name().unwrap(), "evil.png");
        assert_eq!(a.size, 16);
        // What was staged is known to the registry: it can be sent and deleted.
        assert!(reg.is_sendable(&path));
        assert!(reg.delete_staged(&path));

        let too_big = vec![0u8; limits::MAX_FILE_BYTES as usize + 1];
        assert_eq!(
            stage_bytes(&reg, "x.bin", &too_big).unwrap_err().code,
            "error.file_too_large"
        );
    }

    #[test]
    fn only_files_rust_saw_being_dropped_are_described() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_in(dir.path());
        let dropped = dir.path().join("dropped.txt");
        let other = dir.path().join("passwd.txt");
        std::fs::write(&dropped, b"x").unwrap();
        std::fs::write(&other, b"y").unwrap();
        assert!(reg.register_user_file(&dropped));

        let paths: Vec<String> = [&dropped, &other]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let r = describe_known(&reg, &paths, 0, 0);
        assert_eq!(r.accepted.len(), 1);
        assert_eq!(r.accepted[0].name, "dropped.txt");
        assert_eq!(r.rejected.len(), 1);
        assert_eq!(r.rejected[0].name, "passwd.txt");
    }

    #[test]
    fn picked_files_become_known() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_in(dir.path());
        let f = dir.path().join("picked.txt");
        std::fs::write(&f, b"x").unwrap();
        let p = vec![f.to_string_lossy().into_owned()];
        assert!(!reg.is_sendable(&f));
        let r = designate_and_describe(&reg, &p, 0, 0);
        assert_eq!(r.accepted.len(), 1);
        assert!(reg.is_sendable(&f));
    }

    #[test]
    fn send_validation_refuses_unknown_paths_folders_and_vanished_files() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_in(dir.path());
        let known = dir.path().join("known.txt");
        std::fs::write(&known, b"abc").unwrap();
        reg.register_user_file(&known);
        let unknown = dir.path().join("unknown.txt");
        std::fs::write(&unknown, b"abc").unwrap();

        let att = |p: &Path| Attachment {
            name: "x".into(),
            mime: "text/plain".into(),
            size: 1,
            path: Some(p.to_string_lossy().into_owned()),
            thumbnail: None,
            available: true,
            slot: None,
        };
        // Sizes and names come from the disk, not from what the front end claims.
        let ok = validate_attachments(&reg, &[att(&known)]).unwrap();
        assert_eq!(ok[0].name, "known.txt");
        assert_eq!(ok[0].size, 3);

        let code = |a: &[Attachment]| validate_attachments(&reg, a).unwrap_err().code;
        assert_eq!(code(&[att(&unknown)]), "error.attachment_missing");
        assert_eq!(code(&[att(dir.path())]), "error.attachment_missing");
        std::fs::remove_file(&known).unwrap();
        assert_eq!(code(&[att(&known)]), "error.attachment_missing");
        let mut no_path = att(&known);
        no_path.path = None;
        assert_eq!(code(&[no_path]), "error.attachment_missing");
    }

    #[test]
    fn send_validation_enforces_the_size_limits_on_the_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry_in(dir.path());
        let big = dir.path().join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(limits::MAX_FILE_BYTES + 1)
            .unwrap();
        reg.register_user_file(&big);
        let declared = Attachment {
            name: "big.bin".into(),
            mime: "application/octet-stream".into(),
            size: 1, // the front end lies
            path: Some(big.to_string_lossy().into_owned()),
            thumbnail: None,
            available: true,
            slot: None,
        };
        assert_eq!(
            validate_attachments(&reg, &[declared]).unwrap_err().code,
            "error.file_too_large"
        );
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn split_args_separates_urls_from_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let f = f.to_string_lossy().into_owned();

        let (urls, paths) = split_args(args(&[
            "tomi.exe",
            "--autostart",
            "TOMI-SHARE://new?url=https%3A%2F%2Fa.org",
            &f,
            "C:/definitely/missing.bin",
        ]));
        assert_eq!(urls, vec!["TOMI-SHARE://new?url=https%3A%2F%2Fa.org"]);
        assert_eq!(paths, vec![f]);

        // The executable itself is never treated as an argument.
        let (urls, paths) = split_args(args(&["tomi.exe"]));
        assert!(urls.is_empty() && paths.is_empty());
    }
}
