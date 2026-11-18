//! Item actions: copy, open, save, delete, secrets. Shared by the commands and the
//! toast buttons.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::clip_clears::PendingClear;
use crate::detect::{self, ContentKind};
use crate::error::{AppError, AppResult};
use crate::files;
use crate::model::{Item, ReadPolicy};
use crate::render::{self, RenderMode};
use crate::state::AppState;
use crate::view::{self, item_kind};
use crate::{clipboard, popover, tray, unlock};

/// Runs blocking Win32 / file work off the async threads.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| AppError::new("error.internal"))
}

async fn fetch(app: &AppHandle, id: &str) -> AppResult<Item> {
    let item = app.state::<AppState>().core.get_item(id).await?;
    Ok(item)
}

/// Reads an item for an action that shows, copies or opens its content. A hidden item asks
/// Windows Hello first (2-minute window, see `unlock`) and is read again afterwards: it may have
/// changed or gone while the prompt was on screen. Nothing of a hidden item is used before.
async fn fetch_for_use(app: &AppHandle, id: &str, owner: Option<isize>) -> AppResult<Item> {
    let item = fetch(app, id).await?;
    if !item.secret {
        return Ok(item);
    }
    unlock::unlock(app, owner).await?;
    fetch(app, id).await
}

/// Local paths of an item's files, ready to be opened, copied, saved or dragged out.
///
/// A file that came from the network gets the Mark-of-the-Web (NTFS stream
/// `Zone.Identifier`, `ZoneId=3`) before it is exposed, so that SmartScreen and Office
/// Protected View treat it like any download. Files this PC sent itself are local.
pub async fn attachment_paths(app: &AppHandle, item: &Item) -> AppResult<Vec<PathBuf>> {
    let state = app.state::<AppState>();
    let mut out = Vec::new();
    for position in 0..item.attachments.len() {
        // The core is asked for the SLOT of the file, not its position in the list.
        let slot = item
            .attachment_slot(position)
            .ok_or_else(|| AppError::new("error.nothing_to_open"))?;
        out.push(state.core.attachment_path(&item.id, slot).await?);
    }
    if !item.outgoing {
        let to_mark = out.clone();
        blocking(move || {
            for p in &to_mark {
                // Best effort: a file system without streams (FAT, network share) cannot
                // carry the mark, and the file is still usable.
                let _ = files::mark_of_the_web(p);
            }
        })
        .await?;
    }
    Ok(out)
}

/// Local path of ONE file of an item (the others are not fetched), with the same
/// Mark-of-the-Web rule as [`attachment_paths`]. `position` is its place in `item.attachments`;
/// the core is asked for its slot.
pub async fn attachment_path_at(
    app: &AppHandle,
    item: &Item,
    position: usize,
) -> AppResult<PathBuf> {
    let slot = item
        .attachment_slot(position)
        .ok_or_else(|| AppError::new("error.nothing_to_open"))?;
    let path = app
        .state::<AppState>()
        .core
        .attachment_path(&item.id, slot)
        .await?;
    if !item.outgoing {
        let to_mark = path.clone();
        blocking(move || {
            let _ = files::mark_of_the_web(&to_mark);
        })
        .await?;
    }
    Ok(path)
}

/// Marks an item read and refreshes the unread dot.
pub async fn mark_read(app: &AppHandle, id: &str) -> AppResult<()> {
    app.state::<AppState>().core.mark_read(id).await?;
    refresh_badge(app).await;
    Ok(())
}

/// Recomputes the unread count, updates the tray and tells the windows to refetch.
pub async fn refresh_badge(app: &AppHandle) {
    let state = app.state::<AppState>();
    let unread = state
        .core
        .list_items()
        .await
        .map(|items| view::unread_count(&items))
        .unwrap_or(0);
    let sending = !state.queue.active().is_empty();
    tray::update(app, unread, sending);
    let _ = app.emit("items-changed", unread);
}

/// Starts the 30 s countdown that wipes a copied secret from the clipboard, and
/// remembers what is pending and for which item (see `clip_clears`). `item_id` is `None`
/// when the copy consumed the item (read once): the item is gone by design, and the value
/// must stay pasteable for the 30 s, so its removal must not wipe the clipboard.
fn schedule_clear(app: &AppHandle, item_id: Option<&str>, value: &str, seq: Option<u32>) {
    let clears = app.state::<AppState>().clip_clears.clone();
    let generation = clears.register(item_id, value, seq);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(clipboard::SECRET_CLEAR_DELAY).await;
        // Only the timer of the latest copy acts: an earlier one finds nothing.
        if let Some(pending) = clears.take_if_current(generation) {
            let _ = blocking(move || wipe(&pending)).await;
        }
    });
}

/// Wipes the clipboard if it still holds what the pending copy put there.
pub fn wipe(pending: &PendingClear) -> bool {
    clipboard::clear_if_still(pending.value(), pending.seq)
}

/// Same, telling "not ours" from "the clipboard is busy" (the departure tries again).
pub fn wipe_checked(pending: &PendingClear) -> clipboard::ClearResult {
    clipboard::clear_if_still_checked(pending.value(), pending.seq)
}

/// "Copier": copies what is displayed. A secret goes through the sensitive formats and
/// the 30 s wipe whatever it holds: a field, a link, text or files. A hidden item asks Windows
/// Hello first (`owner` is the window that asked, `None` from a toast: never for a hidden item,
/// the toast opens the popover instead).
pub async fn copy_item(
    app: &AppHandle,
    id: &str,
    mode: Option<RenderMode>,
    owner: Option<isize>,
) -> AppResult<()> {
    let item = fetch_for_use(app, id, owner).await?;
    if item.unreadable {
        return Err(AppError::new("error.unreadable"));
    }
    if item.secret {
        if let Some(value) = secret_value(&item) {
            return copy_secret_value(app, &item, value).await;
        }
    }
    let sensitive = item.secret;
    // What the wipe compares the clipboard with. Empty for files and images: only the
    // clipboard sequence number identifies those.
    let held: String;
    let seq = match item_kind(&item) {
        ContentKind::Image => {
            let path = attachment_paths(app, &item).await?.remove(0);
            held = String::new();
            blocking(move || clipboard::set_image(&path, sensitive)).await??
        }
        ContentKind::File | ContentKind::Files => {
            let paths: Vec<String> = attachment_paths(app, &item)
                .await?
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            held = String::new();
            blocking(move || clipboard::set_files(&paths, sensitive)).await??
        }
        ContentKind::Link => {
            let url = item.url.clone().unwrap_or_default();
            held = url.clone();
            blocking(move || clipboard::set_text(&url, sensitive)).await??
        }
        ContentKind::Text | ContentKind::RichText => {
            let text = item
                .text
                .clone()
                .ok_or_else(|| AppError::new("error.nothing_to_copy"))?;
            let preferred = mode.or(app.state::<AppState>().settings().render_mode);
            let mode = render::resolve(&text, item.source_format, preferred);
            let (plain, html) = render::copy_payload(&text, item.source_format, mode);
            held = plain.clone();
            blocking(move || match html {
                Some(h) => clipboard::set_rich(&plain, &h, sensitive),
                None => clipboard::set_text(&plain, sensitive),
            })
            .await??
        }
    };
    if sensitive {
        schedule_clear(app, clear_tie(&item), &held, seq);
    }
    // A read-once secret with files is consumed only by an explicit "Enregistrer": copying
    // its files must not erase them from under the clipboard (the CF_HDROP would dangle).
    if reveal_must_not_consume(&item) {
        return Ok(());
    }
    mark_read(app, id).await
}

/// What "Copier" puts on the clipboard for a hidden item: its text, else its link.
fn secret_value(item: &Item) -> Option<String> {
    item.text
        .as_ref()
        .map(|t| t.plain.clone())
        .filter(|p| !p.is_empty())
        .or_else(|| item.url.clone())
}

/// The item a sensitive copy is tied to, so that its removal wipes the clipboard. An item
/// read once that is not ours is erased by the very copy that follows: no tie then. But a
/// read-once item with files is NOT consumed by the copy (only "Enregistrer" consumes it),
/// so it keeps its tie like any other.
fn clear_tie(item: &Item) -> Option<&str> {
    let consumed = item.read_policy == ReadPolicy::AfterFirstRead
        && !item.outgoing
        && !reveal_must_not_consume(item);
    (!consumed).then_some(item.id.as_str())
}

/// Copies the value of a hidden item: sensitive clipboard formats, wiped after 30 s if
/// unchanged. The value never leaves Rust.
async fn copy_secret_value(app: &AppHandle, item: &Item, value: String) -> AppResult<()> {
    let v = value.clone();
    let seq = blocking(move || clipboard::set_text(&v, true)).await??;
    schedule_clear(app, clear_tie(item), &value, seq);
    mark_read(app, &item.id).await
}

/// "Copier" on a hidden item (Windows Hello first).
pub async fn copy_secret(app: &AppHandle, id: &str, owner: Option<isize>) -> AppResult<()> {
    let item = fetch_for_use(app, id, owner).await?;
    if !item.secret {
        return Err(AppError::new("error.not_secret"));
    }
    let value = secret_value(&item).ok_or_else(|| AppError::new("error.nothing_to_copy"))?;
    copy_secret_value(app, &item, value).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedFile {
    pub name: String,
    pub size: u64,
}

/// What an explicit "Afficher" shows of a hidden item. No `Debug`: it holds the secret.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedSecret {
    pub title: Option<String>,
    pub text: Option<String>,
    pub url: Option<String>,
    pub attachments: Vec<RevealedFile>,
    /// The item is read once and nothing consumed it yet: the front calls `consume_revealed`
    /// once the value is really on screen (never before: a value thrown away must not cost the
    /// item).
    pub consume_after_show: bool,
}

impl RevealedSecret {
    pub fn of(item: &Item) -> Self {
        Self {
            consume_after_show: reveal_read(item) == RevealRead::ConsumeAfterShow,
            title: item.title.clone(),
            text: item.text.as_ref().map(|t| t.plain.clone()),
            url: item.url.clone(),
            attachments: item
                .attachments
                .iter()
                .map(|a| RevealedFile {
                    name: a.name.clone(),
                    size: a.size,
                })
                .collect(),
        }
    }
}

/// True for a hidden item that a read would erase everywhere (read once, not ours) and
/// whose files must survive the reveal so the user can still open or save them. For such
/// an item, showing it must NOT consume it: only an explicit "Enregistrer" (which copies
/// the files before it consumes) or a text-only consume may. Otherwise the core erases the
/// item and its files the instant it is shown, and "Ouvrir"/"Enregistrer" hit `NotFound`.
fn reveal_must_not_consume(item: &Item) -> bool {
    item.read_policy == ReadPolicy::AfterFirstRead && !item.outgoing && !item.attachments.is_empty()
}

/// What "Afficher" does about the read of a hidden item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevealRead {
    /// Marks it read at once (nothing is destroyed by it).
    MarkNow,
    /// Read once, no files: the front consumes it (`consume_revealed`) AFTER the value is on
    /// screen. Consuming here would destroy the item even when the window lost the focus in the
    /// meantime and the value was thrown away.
    ConsumeAfterShow,
    /// Read once with files: showing consumes nothing; only "Enregistrer" does.
    Keep,
}

fn reveal_read(item: &Item) -> RevealRead {
    if item.read_policy != ReadPolicy::AfterFirstRead || item.outgoing {
        RevealRead::MarkNow
    } else if reveal_must_not_consume(item) {
        RevealRead::Keep
    } else {
        RevealRead::ConsumeAfterShow
    }
}

/// Content of a hidden item, for the explicit "Afficher" action. Counts as a read, except for
/// an item read once: that one is NOT consumed here but by `consume_revealed`, which the front
/// calls once the value is displayed (the card then keeps what is on screen until it is masked
/// again). A read-once item that carries files is never consumed by showing it, so its files
/// stay available for "Ouvrir" and "Enregistrer"; the save is then the single consuming action.
pub async fn reveal_secret(
    app: &AppHandle,
    id: &str,
    owner: Option<isize>,
) -> AppResult<RevealedSecret> {
    let item = fetch_for_use(app, id, owner).await?;
    if !item.secret {
        return Err(AppError::new("error.not_secret"));
    }
    let state = app.state::<AppState>();
    let revealed = reveal_with(state.core.as_ref(), &item).await?;
    if reveal_read(&item) == RevealRead::MarkNow {
        refresh_badge(app).await;
    }
    Ok(revealed)
}

/// What is shown of an item, and the read it counts for (see `RevealRead`).
async fn reveal_with(
    core: &dyn crate::core_api::TomiCore,
    item: &Item,
) -> AppResult<RevealedSecret> {
    let revealed = RevealedSecret::of(item);
    if reveal_read(item) == RevealRead::MarkNow {
        core.mark_read(&item.id).await?;
    }
    Ok(revealed)
}

/// Checks that a hidden item may be consumed by `consume_revealed`, then consumes it. The
/// Windows Hello window must be open (the value was shown through it), the item hidden, read
/// once, not ours and without files: nothing else is consumed this way.
pub async fn consume_revealed_with(
    core: &dyn crate::core_api::TomiCore,
    gate: &unlock::SecretGate,
    id: &str,
) -> AppResult<()> {
    if !gate.is_open() {
        return Err(AppError::new("error.auth_required"));
    }
    let item = core.get_item(id).await?;
    if !item.secret {
        return Err(AppError::new("error.not_secret"));
    }
    if reveal_read(&item) != RevealRead::ConsumeAfterShow {
        return Err(AppError::new("error.not_consumable"));
    }
    core.mark_read(id).await?;
    Ok(())
}

/// Consumes a read-once hidden item whose value the front has just displayed.
pub async fn consume_revealed(app: &AppHandle, id: &str) -> AppResult<()> {
    let state = app.state::<AppState>();
    consume_revealed_with(state.core.as_ref(), &state.secret_gate, id).await?;
    refresh_badge(app).await;
    Ok(())
}

/// The generic "mark read" of a tap on a card. A hidden item read once is out of its reach:
/// reading it erases it everywhere, and only `consume_revealed` (behind Windows Hello, once the
/// value is shown) or an explicit copy / save may do that.
pub async fn mark_read_checked(core: &dyn crate::core_api::TomiCore, id: &str) -> AppResult<()> {
    let item = core.get_item(id).await?;
    if item.secret && item.read_policy == ReadPolicy::AfterFirstRead && !item.outgoing {
        return Err(AppError::new("error.not_consumable"));
    }
    core.mark_read(id).await?;
    Ok(())
}

/// "Ouvrir": a link in the browser, a file with its default application. The files of a hidden
/// item are decrypted to open them: Windows Hello first.
pub async fn open_item(app: &AppHandle, id: &str, owner: Option<isize>) -> AppResult<()> {
    let item = fetch_for_use(app, id, owner).await?;
    if item.unreadable {
        return Err(AppError::new("error.unreadable"));
    }
    match (&item.url, item.attachments.is_empty()) {
        (Some(url), true) if !item.secret => {
            // Only http(s): a stored value is data, not a command.
            if detect::single_url(url).is_none() {
                return Err(AppError::new("error.invalid_url"));
            }
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|_| AppError::new("error.open"))?;
        }
        (_, false) => {
            let path = attachment_paths(app, &item).await?.remove(0);
            let name = &item.attachments[0].name;
            if files::is_executable_name(name) || files::is_executable_name(&path_name(&path)) {
                // A received program or script is never launched by a click inside
                // Tomi: show it in its folder and let the user decide.
                reveal_path(app, &path)?;
            } else {
                app.opener()
                    .open_path(path.to_string_lossy(), None::<&str>)
                    .map_err(|_| AppError::new("error.open"))?;
            }
        }
        _ => return Err(AppError::new("error.nothing_to_open")),
    }
    // A read-once secret with files is consumed only by an explicit "Enregistrer", never
    // by opening it: otherwise the core erases the file the instant the viewer launches.
    if reveal_must_not_consume(&item) {
        return Ok(());
    }
    mark_read(app, id).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub dir: String,
    pub paths: Vec<String>,
}

pub fn download_dir(app: &AppHandle) -> AppResult<PathBuf> {
    if let Some(d) = app.state::<AppState>().settings().download_dir.clone() {
        return Ok(PathBuf::from(d));
    }
    app.path()
        .download_dir()
        .map(|d| d.join("Tomi"))
        .map_err(|_| AppError::new("error.download_dir"))
}

/// "Enregistrer": copies the item's files into the fixed folder, no dialog. Never for a hidden
/// item: that would leave a permanent clear copy in Downloads (golden rule 3). It stays
/// openable ("Ouvrir": a short-lived copy, wiped with the others).
pub async fn save_item(app: &AppHandle, id: &str) -> AppResult<SaveResult> {
    let item = fetch(app, id).await?;
    if item.secret {
        return Err(AppError::new("error.secret_no_save"));
    }
    if item.attachments.is_empty() {
        return Err(AppError::new("error.nothing_to_save"));
    }
    let dir = download_dir(app)?;
    let sources = attachment_paths(app, &item).await?;
    let names: Vec<String> = item.attachments.iter().map(|a| a.name.clone()).collect();
    let dir2 = dir.clone();
    let registry = app.state::<AppState>().paths.clone();
    let from_network = !item.outgoing;
    let paths = blocking(move || -> std::io::Result<Vec<String>> {
        std::fs::create_dir_all(&dir2)?;
        let mut out = Vec::new();
        for (src, name) in sources.iter().zip(names.iter()) {
            let dest = files::unique_path(&dir2, name);
            std::fs::copy(src, &dest)?;
            // The opened copy is read-only (see `real_core::named_copy`) and the copy inherits
            // the attribute: the file saved for the user must be a normal, editable file.
            let _ = files::set_readonly(&dest, false);
            if from_network {
                // The copy may or may not carry the stream of the cache file: write it.
                let _ = files::mark_of_the_web(&dest);
            }
            // The only paths `reveal_in_folder` will accept.
            registry.register_saved(&dest);
            out.push(dest.to_string_lossy().into_owned());
        }
        Ok(out)
    })
    .await??;
    mark_read(app, id).await?;
    Ok(SaveResult {
        dir: dir.to_string_lossy().into_owned(),
        paths,
    })
}

fn path_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Shows a file in Explorer. The caller has already established that the path is one
/// the app knows (a cache file of an item, a file it saved).
fn reveal_path(app: &AppHandle, path: &Path) -> AppResult<()> {
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|_| AppError::new("error.open"))
}

/// "Afficher le dossier" for a file written by "Enregistrer". Any other path coming from
/// the webview is refused.
pub fn reveal_in_folder(app: &AppHandle, path: &str) -> AppResult<()> {
    let path = Path::new(path);
    if !app.state::<AppState>().paths.is_revealable(path) {
        return Err(AppError::new("error.open"));
    }
    reveal_path(app, path)
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeleteScope {
    Local,
    Everywhere,
}

pub async fn delete_item(app: &AppHandle, id: &str, scope: DeleteScope) -> AppResult<()> {
    let state = app.state::<AppState>();
    match scope {
        DeleteScope::Local => state.core.delete_local(id).await?,
        DeleteScope::Everywhere => state.core.delete_everywhere(id).await?,
    }
    refresh_badge(app).await;
    Ok(())
}

/// Default action of a toast button / tap. See `notify::tap_action`.
pub async fn run_toast_action(app: &AppHandle, id: &str, action: Option<&str>) {
    use crate::notify::{tap_action, TapAction};
    let Ok(item) = fetch(app, id).await else {
        return;
    };
    // A hidden item is never copied nor opened from a toast (no window to own Windows Hello,
    // and one click must not hand over a secret): the popover shows it, where "Afficher" and
    // "Copier" ask Windows Hello.
    if item.secret {
        popover::show_item(app, id);
        return;
    }
    let result = match action {
        Some("copy") => copy_item(app, id, None, None).await,
        Some("open") => match item_kind(&item) {
            ContentKind::Link | ContentKind::File => open_item(app, id, None).await,
            _ => {
                popover::show_item(app, id);
                Ok(())
            }
        },
        _ => match tap_action(&item) {
            TapAction::OpenLink | TapAction::OpenFile => open_item(app, id, None).await,
            TapAction::CopyText => copy_item(app, id, None, None).await,
            TapAction::ShowInPopover => {
                popover::show_item(app, id);
                Ok(())
            }
        },
    };
    if let Err(e) = result {
        let _ = app.emit("notice", &e);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::core_api::TomiCore;
    use crate::fake_core::FakeCore;
    use crate::model::{Attachment, ItemDraft, TextBody};
    use crate::strong_auth::{AuthOutcome, ScriptedAuth};
    use crate::unlock::SecretGate;

    fn draft(secret: bool, policy: ReadPolicy) -> ItemDraft {
        ItemDraft {
            id: None,
            title: None,
            text: Some(TextBody {
                plain: "mot de passe".into(),
                html: None,
                markdown: None,
            }),
            url: None,
            attachments: vec![],
            secret,
            expires_at: None,
            read_policy: policy,
            source_format: None,
        }
    }

    /// An item received from another device (the core never consumes an item for its sender).
    async fn incoming(core: &FakeCore, id: &str, d: ItemDraft) {
        let out = core.send_item(d, Arc::new(|_| {})).await.unwrap();
        let mut item = core.get_item(&out.item_id).await.unwrap();
        item.id = id.into();
        core.inject_incoming(item);
    }

    /// An item sent from this PC.
    async fn outgoing(core: &FakeCore, d: ItemDraft) -> Item {
        let out = core.send_item(d, Arc::new(|_| {})).await.unwrap();
        core.get_item(&out.item_id).await.unwrap()
    }

    async fn open_gate() -> SecretGate {
        let gate = SecretGate::new(ScriptedAuth::new(&[AuthOutcome::Verified]));
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert!(gate.is_open());
        gate
    }

    fn core() -> (FakeCore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (FakeCore::new(dir.path().to_path_buf()).without_delay(), dir)
    }

    #[tokio::test]
    async fn revealing_a_read_once_secret_does_not_consume_it() {
        let (core, _dir) = core();
        incoming(&core, "s1", draft(true, ReadPolicy::AfterFirstRead)).await;
        let item = core.get_item("s1").await.unwrap();
        assert_eq!(reveal_read(&item), RevealRead::ConsumeAfterShow);

        let shown = reveal_with(&core, &item).await.unwrap();

        // The value is returned and the front is told to consume it once displayed; the item
        // is still there and unread.
        assert!(shown.consume_after_show);
        assert_eq!(shown.text.as_deref(), Some("mot de passe"));
        assert!(!core.get_item("s1").await.unwrap().read);
    }

    #[tokio::test]
    async fn revealing_other_hidden_items_counts_as_a_read_at_once() {
        let (core, _dir) = core();
        incoming(&core, "n1", draft(true, ReadPolicy::Normal)).await;
        let normal = core.get_item("n1").await.unwrap();
        assert_eq!(reveal_read(&normal), RevealRead::MarkNow);
        let shown = reveal_with(&core, &normal).await.unwrap();
        assert!(!shown.consume_after_show);
        assert!(core.get_item("n1").await.unwrap().read);

        // Ours: the core never consumes it for its sender.
        let mine = outgoing(&core, draft(true, ReadPolicy::AfterFirstRead)).await;
        assert_eq!(reveal_read(&mine), RevealRead::MarkNow);
    }

    #[tokio::test]
    async fn a_read_once_secret_with_files_is_kept_by_showing_it() {
        let (core, _dir) = core();
        incoming(&core, "s1", draft(true, ReadPolicy::AfterFirstRead)).await;
        let mut item = core.get_item("s1").await.unwrap();
        item.attachments = vec![Attachment {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            size: 1,
            path: None,
            thumbnail: None,
            available: true,
            slot: None,
        }];
        assert_eq!(reveal_read(&item), RevealRead::Keep);
        assert!(!RevealedSecret::of(&item).consume_after_show);
        reveal_with(&core, &item).await.unwrap();
        assert!(!core.get_item("s1").await.unwrap().read);
    }

    #[tokio::test]
    async fn consume_revealed_consumes_a_shown_read_once_secret() {
        let (core, _dir) = core();
        incoming(&core, "s1", draft(true, ReadPolicy::AfterFirstRead)).await;
        let gate = open_gate().await;
        consume_revealed_with(&core, &gate, "s1").await.unwrap();
        assert!(core.get_item("s1").await.is_err());
    }

    #[tokio::test]
    async fn consume_revealed_is_refused_when_the_gate_is_closed() {
        let (core, _dir) = core();
        incoming(&core, "s1", draft(true, ReadPolicy::AfterFirstRead)).await;
        let gate = open_gate().await;
        gate.close();
        let err = consume_revealed_with(&core, &gate, "s1").await.unwrap_err();
        assert_eq!(err.code, "error.auth_required");
        assert!(core.get_item("s1").await.is_ok());
        // Never opened at all.
        let never = SecretGate::new(ScriptedAuth::new(&[]));
        let err = consume_revealed_with(&core, &never, "s1")
            .await
            .unwrap_err();
        assert_eq!(err.code, "error.auth_required");
        assert!(core.get_item("s1").await.is_ok());
    }

    #[tokio::test]
    async fn consume_revealed_refuses_what_is_not_a_hidden_read_once_item() {
        let (core, _dir) = core();
        incoming(&core, "plain", draft(false, ReadPolicy::AfterFirstRead)).await;
        incoming(&core, "normal", draft(true, ReadPolicy::Normal)).await;
        let mine = outgoing(&core, draft(true, ReadPolicy::AfterFirstRead)).await;
        let gate = open_gate().await;

        let code = |r: AppResult<()>| r.unwrap_err().code;
        // Not hidden.
        assert_eq!(
            code(consume_revealed_with(&core, &gate, "plain").await),
            "error.not_secret"
        );
        // Hidden but not read once.
        assert_eq!(
            code(consume_revealed_with(&core, &gate, "normal").await),
            "error.not_consumable"
        );
        // Read once but ours.
        assert_eq!(
            code(consume_revealed_with(&core, &gate, &mine.id).await),
            "error.not_consumable"
        );
        for id in ["plain", "normal", mine.id.as_str()] {
            assert!(core.get_item(id).await.is_ok(), "{id} must still be there");
        }
    }

    #[tokio::test]
    async fn the_generic_mark_read_cannot_consume_a_hidden_read_once_item() {
        let (core, _dir) = core();
        incoming(&core, "s1", draft(true, ReadPolicy::AfterFirstRead)).await;
        incoming(&core, "n1", draft(false, ReadPolicy::Normal)).await;
        let err = mark_read_checked(&core, "s1").await.unwrap_err();
        assert_eq!(err.code, "error.not_consumable");
        assert!(core.get_item("s1").await.is_ok());
        // An ordinary item is marked read as before.
        mark_read_checked(&core, "n1").await.unwrap();
        assert!(core.get_item("n1").await.unwrap().read);
    }
}
