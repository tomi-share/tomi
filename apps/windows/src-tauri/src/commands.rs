//! IPC commands exposed to the webviews. Thin wrappers: the logic lives in the modules
//! they call. Errors are [`AppError`] (a code the front end maps to French text).

use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use zeroize::Zeroizing;

use crate::actions::{self, blocking, DeleteScope, RevealedSecret, SaveResult};
use crate::core_api::{CoreStatus, PairingOptions, PairingView, RelayRevocation, SpaceState};
use crate::departure::{Cancelled, DepartureSnapshot};
use crate::detect::{self, ContentKind};
use crate::draft::{self, DraftInput};
use crate::error::{AppError, AppResult};
use crate::expiry::{self, AllExpiryRules};
use crate::files;
use crate::ingest::{self, FilesResult};
use crate::model::{now_ms, Attachment, AttachmentRemoval, Device, SourceFormat, Transfer};
#[cfg(any(test, debug_assertions))]
use crate::model::{Item, ReadPolicy, TextBody, UnreadableReason};
use crate::popover;
use crate::queue::JobView;
use crate::render::{self, RenderMode, Rendered};
use crate::settings::Settings;
use crate::shortcut;
use crate::state::{AppState, PendingPair, PendingSetup, PrefillPayload};
use crate::unlock;
use crate::view::{self, ItemFilter, ItemView};

// ---------------------------------------------------------------- feed

#[tauri::command]
pub async fn list_items(
    state: State<'_, AppState>,
    filter: Option<ItemFilter>,
    query: Option<String>,
) -> AppResult<Vec<ItemView>> {
    let items = state.core.list_items().await?;
    let filter = filter.unwrap_or_default();
    let query = query.unwrap_or_default();
    Ok(items
        .iter()
        .filter(|i| view::is_listed(i, filter, &query))
        .map(ItemView::from_item)
        .collect())
}

/// One item as the feed shows it (same redaction as `list_items`), for the detail screen:
/// the feed list may be filtered and not contain it.
#[tauri::command]
pub async fn item_view(state: State<'_, AppState>, id: String) -> AppResult<ItemView> {
    let item = state.core.get_item(&id).await?;
    Ok(ItemView::from_item(&item))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedItem {
    /// The mode actually shown: the one asked for if the item supports it, else "Aperçu".
    pub mode: RenderMode,
    /// The modes the item offers ("Markdown" only with a Markdown representation).
    pub modes: Vec<RenderMode>,
    pub rendered: Rendered,
}

/// Rendering of a text item. `mode` defaults to the remembered choice, then to the styled
/// preview. `dark` is the theme of the window, for the colours of the sandboxed document.
#[tauri::command]
pub async fn render_item(
    state: State<'_, AppState>,
    id: String,
    mode: Option<RenderMode>,
    dark: Option<bool>,
) -> AppResult<RenderedItem> {
    let item = state.core.get_item(&id).await?;
    if item.secret || item.unreadable {
        return Err(AppError::new("error.nothing_to_copy"));
    }
    let text = item
        .text
        .ok_or_else(|| AppError::new("error.nothing_to_copy"))?;
    let mode = render::resolve(
        &text,
        item.source_format,
        mode.or(state.settings().render_mode),
    );
    Ok(RenderedItem {
        mode,
        modes: render::available_modes(&text, item.source_format),
        rendered: render::render(&text, item.source_format, mode, dark.unwrap_or(false)),
    })
}

/// Largest file read into the code screen (same limit as Android, `CodeLimits.MAX_BYTES`).
const CODE_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Largest image sent to the image viewer as a `data:` URL; a bigger one is opened with the
/// system viewer instead.
const IMAGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The text of a code file, for the in-app code screen. Never for a hidden item: those are
/// opened with "Ouvrir" after "Afficher". Like "Ouvrir", it goes through a named decrypted copy
/// of the file (cleaned up with the others: `remove_open_copies`, `wipe_dir`), so the declared
/// size is checked BEFORE anything is downloaded or decrypted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeText {
    pub name: String,
    pub text: String,
}

#[tauri::command]
pub async fn read_code_file(app: AppHandle, id: String, slot: usize) -> AppResult<CodeText> {
    let item = app.state::<AppState>().core.get_item(&id).await?;
    if item.secret || item.unreadable {
        return Err(AppError::new("error.nothing_to_open"));
    }
    let (_, attachment) = item
        .attachment_by_slot(slot)
        .ok_or_else(|| AppError::new("error.nothing_to_open"))?;
    if attachment.mime.starts_with("image/") {
        return Err(AppError::new("error.nothing_to_open"));
    }
    if attachment.size > CODE_MAX_BYTES {
        return Err(AppError::new("error.code_too_large"));
    }
    let name = attachment.name.clone();
    let path = actions::attachment_path_of_slot(&app, &item, slot).await?;
    let bytes = blocking(move || -> AppResult<Vec<u8>> {
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let mut buf = Vec::new();
        // One byte more than the limit tells that the file is too big without reading it all.
        file.take(CODE_MAX_BYTES + 1).read_to_end(&mut buf)?;
        Ok(buf)
    })
    .await??;
    let text = decode_code(&bytes)?;
    Ok(CodeText { name, text })
}

/// Decodes the bytes of a code file: a UTF-8 BOM is dropped, an invalid sequence becomes
/// U+FFFD (a file in another encoding is shown, not refused). A file over the limit or with
/// a NUL byte in its first 8000 bytes is refused.
fn decode_code(bytes: &[u8]) -> AppResult<String> {
    if bytes.len() as u64 > CODE_MAX_BYTES {
        return Err(AppError::new("error.code_too_large"));
    }
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err(AppError::new("error.code_binary"));
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    Ok(String::from_utf8_lossy(body).into_owned())
}

/// One image of an item as a `data:` URL, for the in-app viewer. Never for a hidden item.
/// The type comes from the bytes, not from the declared MIME type, and only common rasters
/// are served (no SVG).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    pub name: String,
    pub data_url: String,
}

#[tauri::command]
pub async fn read_image(app: AppHandle, id: String, slot: usize) -> AppResult<ImageData> {
    let item = app.state::<AppState>().core.get_item(&id).await?;
    if item.secret || item.unreadable {
        return Err(AppError::new("error.nothing_to_open"));
    }
    let (_, attachment) = item
        .attachment_by_slot(slot)
        .ok_or_else(|| AppError::new("error.nothing_to_open"))?;
    if !attachment.mime.starts_with("image/") {
        return Err(AppError::new("error.nothing_to_open"));
    }
    if attachment.size > IMAGE_MAX_BYTES {
        return Err(AppError::new("error.image_too_large"));
    }
    let name = attachment.name.clone();
    let path = actions::attachment_path_of_slot(&app, &item, slot).await?;
    let data_url = blocking(move || -> AppResult<String> {
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let mut buf = Vec::new();
        file.take(IMAGE_MAX_BYTES + 1).read_to_end(&mut buf)?;
        image_data_url(&buf)
    })
    .await??;
    Ok(ImageData { name, data_url })
}

fn image_data_url(bytes: &[u8]) -> AppResult<String> {
    use base64::Engine;
    if bytes.len() as u64 > IMAGE_MAX_BYTES {
        return Err(AppError::new("error.image_too_large"));
    }
    let mime = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        Ok(image::ImageFormat::Gif) => "image/gif",
        Ok(image::ImageFormat::WebP) => "image/webp",
        Ok(image::ImageFormat::Bmp) => "image/bmp",
        _ => return Err(AppError::new("error.image")),
    };
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
pub async fn copy_item(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    mode: Option<RenderMode>,
) -> AppResult<()> {
    actions::copy_item(&app, &id, mode, owner_of(&window)).await
}

#[tauri::command]
pub async fn copy_secret(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
) -> AppResult<()> {
    actions::copy_secret(&app, &id, owner_of(&window)).await
}

/// Content of a hidden item (title, text, link, file names). The window that is about to
/// show it is excluded from screen capture first (it already is, permanently: see
/// `tauri.conf.json` and `popover::open_history`; this is the explicit guarantee at the
/// point of use).
#[tauri::command]
pub async fn reveal_secret(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
) -> AppResult<RevealedSecret> {
    window
        .set_content_protected(true)
        .map_err(|_| AppError::new("error.internal"))?;
    actions::reveal_secret(&app, &id, owner_of(&window)).await
}

/// Pins an item on all devices (`spec/pin-contract.md`). Named apart from
/// `set_pinned`, which keeps the popover open.
#[tauri::command]
pub async fn pin_item(state: State<'_, AppState>, id: String) -> AppResult<()> {
    Ok(state.core.pin(&id).await?)
}

#[tauri::command]
pub async fn unpin_item(state: State<'_, AppState>, id: String) -> AppResult<()> {
    Ok(state.core.unpin(&id).await?)
}

/// Archives an item on all devices (`spec/archive-contract.md`): it leaves "Récents" and
/// stays readable in "Archives" and in the search. Never marks the item read. The core
/// unpins an item it archives.
#[tauri::command]
pub async fn archive_item(state: State<'_, AppState>, id: String) -> AppResult<()> {
    Ok(state.core.archive(&id).await?)
}

/// Brings an archived item back to "Récents". The core makes the pin and the archive
/// exclusive: pinning an archived item unarchives it.
#[tauri::command]
pub async fn unarchive_item(state: State<'_, AppState>, id: String) -> AppResult<()> {
    Ok(state.core.unarchive(&id).await?)
}

#[tauri::command]
pub async fn open_item(app: AppHandle, window: tauri::WebviewWindow, id: String) -> AppResult<()> {
    actions::open_item(&app, &id, owner_of(&window)).await
}

#[tauri::command]
pub async fn save_item(app: AppHandle, id: String) -> AppResult<SaveResult> {
    actions::save_item(&app, &id).await
}

#[tauri::command]
pub async fn reveal_in_folder(app: AppHandle, path: String) -> AppResult<()> {
    actions::reveal_in_folder(&app, &path)
}

#[tauri::command]
pub async fn delete_item(app: AppHandle, id: String, scope: DeleteScope) -> AppResult<()> {
    actions::delete_item(&app, &id, scope).await
}

/// Removes ONE attachment of an item (by `slot`) from this PC or everywhere. No Windows Hello: like
/// deleting an item. The core refuses while the item is still queued; the page does not offer it then.
#[tauri::command]
pub async fn delete_attachment(
    app: AppHandle,
    id: String,
    slot: usize,
    everywhere: bool,
) -> AppResult<AttachmentRemoval> {
    actions::delete_attachment(&app, &id, slot, everywhere).await
}

/// Whether the relay takes "remove this file everywhere" (otherwise only "from this PC" is offered).
#[tauri::command]
pub async fn attachment_removal_everywhere(app: AppHandle) -> bool {
    app.state::<AppState>()
        .core
        .supports_attachment_removal()
        .await
}

/// Marks an item read (a tap on a card). Never consumes a hidden item read once: that is
/// `consume_revealed`.
#[tauri::command]
pub async fn mark_read(app: AppHandle, id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    actions::mark_read_checked(state.core.as_ref(), &id).await?;
    actions::refresh_badge(&app).await;
    Ok(())
}

/// Consumes a hidden item read once, after the front has really displayed its value. Refused
/// unless the Windows Hello window is open.
#[tauri::command]
pub async fn consume_revealed(app: AppHandle, id: String) -> AppResult<()> {
    actions::consume_revealed(&app, &id).await
}

/// Drags the files of an item out of the popover (Explorer, mail...).
#[tauri::command]
pub async fn start_drag(app: AppHandle, window: tauri::WebviewWindow, id: String) -> AppResult<()> {
    let state = app.state::<AppState>();
    let item = state.core.get_item(&id).await?;
    if item.secret || item.unreadable || item.attachments.is_empty() {
        return Err(AppError::new("error.nothing_to_save"));
    }
    // Marks received files with the Mark-of-the-Web before they leave the app.
    let paths = actions::attachment_paths(&app, &item).await?;
    state.suppress_blur.fetch_add(1, Ordering::Relaxed);
    let app2 = app.clone();
    let result = app.run_on_main_thread(move || {
        let icon = drag::Image::Raw(include_bytes!("../icons/tomi-32.png").to_vec());
        let _ = drag::start_drag(
            &window,
            drag::DragItem::Files(paths),
            icon,
            |_, _| {},
            drag::Options::default(),
        );
        let state = app2.state::<AppState>();
        state.suppress_blur.fetch_sub(1, Ordering::Relaxed);
    });
    if result.is_err() {
        state.suppress_blur.fetch_sub(1, Ordering::Relaxed);
        return Err(AppError::new("error.internal"));
    }
    Ok(())
}

// ---------------------------------------------------------------- compose

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub kind: ContentKind,
    pub source_format: Option<SourceFormat>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub looks_like_secret: bool,
}

/// Classifies pasted content (link, text, rich text...) and flags secret-looking text.
#[tauri::command]
pub async fn detect_content(
    plain: String,
    html: Option<String>,
    file_mimes: Option<Vec<String>>,
) -> AppResult<Detected> {
    let mimes = file_mimes.unwrap_or_default();
    let refs: Vec<&str> = mimes.iter().map(String::as_str).collect();
    let d = detect::detect(&plain, html.as_deref(), &refs);
    Ok(Detected {
        kind: d.kind,
        source_format: d.source_format,
        url: d.url,
        title: d.title,
        looks_like_secret: detect::looks_like_secret(&plain),
    })
}

#[tauri::command]
pub async fn expiry_rules() -> AppResult<AllExpiryRules> {
    Ok(expiry::all_rules())
}

/// Checks and describes files dropped on the window. Only paths Rust saw being dropped
/// (`WindowEvent::DragDrop`, see `lib.rs`) are accepted: the webview cannot name an
/// arbitrary file of the disk.
#[tauri::command]
pub async fn describe_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
    existing_bytes: Option<u64>,
    existing_count: Option<usize>,
) -> AppResult<FilesResult> {
    let registry = state.paths.clone();
    blocking(move || {
        ingest::describe_known(
            &registry,
            &paths,
            existing_bytes.unwrap_or(0),
            existing_count.unwrap_or(0),
        )
    })
    .await
}

/// File picker. Losing focus to the dialog must not hide the popover.
#[tauri::command]
pub async fn pick_files(
    app: AppHandle,
    existing_bytes: Option<u64>,
    existing_count: Option<usize>,
) -> AppResult<FilesResult> {
    use tauri_plugin_dialog::DialogExt;
    let state = app.state::<AppState>();
    state.suppress_blur.fetch_add(1, Ordering::Relaxed);
    let app2 = app.clone();
    let picked = blocking(move || app2.dialog().file().blocking_pick_files()).await;
    state.suppress_blur.fetch_sub(1, Ordering::Relaxed);
    let picked = picked?.unwrap_or_default();
    let paths: Vec<String> = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if let Some(w) = app.get_webview_window(popover::POPOVER) {
        let _ = w.set_focus();
    }
    if paths.is_empty() {
        return Ok(FilesResult::default());
    }
    // The picker is Rust's own channel: what it returns is recorded as sendable.
    let registry = state.paths.clone();
    blocking(move || {
        ingest::designate_and_describe(
            &registry,
            &paths,
            existing_bytes.unwrap_or(0),
            existing_count.unwrap_or(0),
        )
    })
    .await
}

/// Stores pasted bytes (a screenshot) in the staging directory. The raw request body
/// is the file; the name travels in the `x-name` header (percent-encoded).
#[tauri::command]
pub async fn stage_blob(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> AppResult<Attachment> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::new("error.internal"));
    };
    let name = request
        .headers()
        .get("x-name")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .map(|n| files::sanitize_file_name(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "capture.png".to_string());
    let registry = state.paths.clone();
    let bytes = bytes.clone();
    blocking(move || ingest::stage_bytes(&registry, &name, &bytes)).await?
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Removes a staged file the user took out of the composer. Only a file `stage_blob`
/// wrote is ever deleted: any other path, `..` tricks included, is ignored.
#[tauri::command]
pub async fn discard_staged(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let registry = state.paths.clone();
    blocking(move || {
        registry.delete_staged(std::path::Path::new(&path));
    })
    .await
}

/// Validates the composed item and queues it (sent after the cancellation window).
#[tauri::command]
pub async fn send_draft(state: State<'_, AppState>, input: DraftInput) -> AppResult<JobView> {
    // Said now, not after the cancellation window: this PC cannot send yet.
    let status = state.core.status();
    if let Some(code) = status.unavailable {
        return Err(AppError::with(
            "error.storage",
            serde_json::json!({ "code": code }),
        ));
    }
    if !status.paired {
        return Err(AppError::new("error.not_paired"));
    }
    if status.space != SpaceState::Active {
        return Err(AppError::new("error.blocked"));
    }
    let mut input = input;
    // The front end is not trusted for paths, sizes, names or mime types: every path must
    // be one Rust vouches for (staged, dropped, picked, sent to), is re-read from the
    // disk, must still be a regular file and is checked against the limits.
    let declared = std::mem::take(&mut input.attachments);
    let registry = state.paths.clone();
    input.attachments =
        blocking(move || ingest::validate_attachments(&registry, &declared)).await??;
    let draft = draft::build_draft(input, now_ms())?;
    Ok(state.queue.enqueue(draft))
}

#[tauri::command]
pub async fn cancel_job(state: State<'_, AppState>, id: String) -> AppResult<bool> {
    Ok(state.queue.cancel(&id))
}

#[tauri::command]
pub async fn retry_job(state: State<'_, AppState>, id: String) -> AppResult<bool> {
    Ok(state.queue.retry(&id))
}

#[tauri::command]
pub async fn dismiss_job(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.queue.dismiss(&id);
    Ok(())
}

#[tauri::command]
pub async fn active_jobs(state: State<'_, AppState>) -> AppResult<Vec<JobView>> {
    Ok(state.queue.active())
}

/// Content waiting to fill the composer (deep link, "Envoyer vers"). Consumed once.
#[tauri::command]
pub async fn take_prefill(state: State<'_, AppState>) -> AppResult<Option<PrefillPayload>> {
    Ok(state
        .pending_prefill
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take())
}

// ---------------------------------------------------------------- app / settings

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    /// The demo core is in use (debug builds only, the field does not exist in a release):
    /// nothing is encrypted or sent.
    #[cfg(debug_assertions)]
    pub demo: bool,
    /// Paired or not, blocked or not, store usable or not.
    pub status: CoreStatus,
    pub this_device: Device,
    pub settings: Settings,
    pub shortcut_registered: bool,
    pub unread: usize,
    pub download_dir: Option<String>,
    /// The demo space of the real core is the active space ("Quitter la démo" is offered). Not
    /// `demo`, which is the stand-in core of `TOMI_DEMO=1`.
    pub demo_entered: bool,
    /// The pairing screen may offer to enter the demo space.
    pub can_enter_demo: bool,
    /// The key rotation after a revocation keeps failing (`device_set_mismatch`) and is retried
    /// every hour: a removed device may still hold the current key. A stored flag, read with the
    /// status after a sync or a change, never polled.
    pub rotation_stalled: bool,
}

#[tauri::command]
pub async fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppResult<AppInfo> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    let settings = state.settings_snapshot();
    let registered = settings
        .shortcut
        .parse::<Shortcut>()
        .map(|s| app.global_shortcut().is_registered(s))
        .unwrap_or(false);
    let unread = state
        .core
        .list_items()
        .await
        .map(|i| view::unread_count(&i))
        .unwrap_or(0);
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        #[cfg(debug_assertions)]
        demo: state.fake.is_some(),
        status: state.core.status(),
        this_device: state.core.this_device(),
        shortcut_registered: registered,
        unread,
        download_dir: actions::download_dir(&app)
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        demo_entered: state.core.is_demo(),
        can_enter_demo: state.core.can_enter_demo(),
        rotation_stalled: state.core.rotation_stalled(),
        settings,
    })
}

/// The devices the relay removed on its own authority that the user was not told about yet,
/// once: a second call returns nothing, so only one window shows each notice. The webview calls
/// it only while it is visible.
#[tauri::command]
pub fn take_relay_notices(state: State<'_, AppState>) -> Vec<RelayRevocation> {
    crate::events::relay_notices_for_a_visible_window(state.core.as_ref(), &state.relay_notices)
}

/// "Essayer avec des données de démonstration": enters the demo space of the real core (fictitious
/// data, nothing is sent), then shows its feed.
#[tauri::command]
pub async fn enter_demo(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    // Only a PC that belongs to no space, with no pairing on screen, may enter the demo (the
    // button is hidden otherwise, and the core refuses a provisioned device too): checked here
    // as well, so that the webview cannot ask for it. The demo has a fixed verification code
    // (482913): a real PC must never be put in it.
    if !state.core.is_demo() && !state.core.can_enter_demo() {
        return Err(AppError::new("error.pairing_already_paired"));
    }
    state.core.enter_demo().await?;
    popover::open_history(&app, Some("history"));
    Ok(())
}

/// "Quitter la démo": the demo space is dropped with its files, then the same way back as after
/// "Se déconnecter" (the welcome screen).
#[tauri::command]
pub async fn leave_demo(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    if state.core.leave_demo().await {
        crate::departure_host::finish_erasure(&app, crate::departure_host::Cleanup::AfterErasure)
            .await;
    }
    Ok(())
}

#[tauri::command]
pub async fn devices(state: State<'_, AppState>) -> AppResult<Vec<Device>> {
    Ok(state.core.devices().await?)
}

/// Renames a device of the space, this PC included. The core checks the name (blanks trimmed,
/// 1 to 64 characters, no control character) and the relay too: a refusal comes back as
/// `error.device_name_invalid`. The windows learn of the new name through `space-changed`.
#[tauri::command]
pub async fn rename_device(state: State<'_, AppState>, id: String, name: String) -> AppResult<()> {
    Ok(state.core.rename_device(&id, &name).await?)
}

/// Removes a device from the space and renews the space key. Never this PC
/// (`error.revoke_self`). The confirmation is asked by the window; then Windows Hello, in Rust,
/// before the core is called (the webview cannot skip it, `error.auth_*` otherwise); the key
/// renewal itself is the core's. This PC is refused before anything is asked.
#[tauri::command]
pub async fn revoke_device(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<()> {
    if state.core.this_device().id == id {
        return Err(AppError::new("error.revoke_self"));
    }
    unlock::confirm_revoke(&app, owner_of(&window)).await?;
    Ok(state.core.revoke_device(&id).await?)
}

// ---------------------------------------------------------------- departure

// Leaving the space, deleting it, erasing without proof. The rules (Windows Hello before the
// irreversible ones, the 2 s anti-tap, the back-off) are in `departure.rs`; these commands carry
// the clicks. The window reads `departure_state` and is told by the `departure-changed` event.

/// The raw handle of the window that asks, so that Windows Hello draws its prompt over it.
fn owner_of(window: &tauri::WebviewWindow) -> Option<isize> {
    #[cfg(windows)]
    {
        window.hwnd().ok().map(|h| h.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// Everything the windows need to draw the banner and the dialogs.
#[tauri::command]
pub async fn departure_state(state: State<'_, AppState>) -> AppResult<DepartureSnapshot> {
    Ok(state.departure.snapshot())
}

/// Menu entry "Se déconnecter": opens its confirmation, in the window that asked.
#[tauri::command]
pub async fn departure_open_leave(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<DepartureSnapshot> {
    state.departure.note_origin(window.label());
    state.departure.open_leave()
}

/// Confirms "Se déconnecter". One attempt: it ends within 520 s at worst on a bad network (a
/// retry after a lost answer included); "Annuler" (`departure_cancel_running`) does not wait for it.
#[tauri::command]
pub async fn departure_confirm_leave(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<DepartureSnapshot> {
    let flow = state.departure.clone();
    flow.note_origin(window.label());
    flow.confirm_leave().await
}

/// Confirms "Supprimer l'espace": armed after 2 s, then Windows Hello, then the core (an attempt
/// of up to 1060 s on a bad network, which "Annuler" does not wait for either).
#[tauri::command]
pub async fn departure_confirm_delete(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<DepartureSnapshot> {
    let flow = state.departure.clone();
    flow.note_origin(window.label());
    flow.confirm_delete(owner_of(&window)).await
}

/// Opens "Effacer quand même" (banner, or the refusal "relay too old").
#[tauri::command]
pub async fn departure_open_erase(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<DepartureSnapshot> {
    let flow = state.departure.clone();
    flow.note_origin(window.label());
    flow.open_erase().await
}

#[tauri::command]
pub async fn departure_set_erase_checked(
    state: State<'_, AppState>,
    checked: bool,
) -> AppResult<DepartureSnapshot> {
    Ok(state.departure.set_erase_checked(checked))
}

/// Confirms "Effacer quand même": checkbox, 2 s, allowed by the core, Windows Hello.
#[tauri::command]
pub async fn departure_confirm_erase(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<DepartureSnapshot> {
    let flow = state.departure.clone();
    flow.confirm_erase(owner_of(&window)).await
}

/// Closes the dialog (ignored while something is under way).
#[tauri::command]
pub async fn departure_dismiss(state: State<'_, AppState>) -> AppResult<DepartureSnapshot> {
    Ok(state.departure.dismiss())
}

/// "Réessayer" on the banner. For a pending deletion of the space, Windows Hello first, again.
#[tauri::command]
pub async fn departure_retry(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<DepartureSnapshot> {
    let flow = state.departure.clone();
    flow.note_origin(window.label());
    flow.retry(owner_of(&window)).await
}

/// "Annuler" on the banner: immediate, never erases, available while an attempt runs.
#[tauri::command]
pub async fn departure_cancel(state: State<'_, AppState>) -> AppResult<Cancelled> {
    let flow = state.departure.clone();
    flow.cancel().await
}

/// "Annuler" in a dialog whose attempt is running: the dialog closes at once and the departure
/// is cancelled in the core, which does not wait for the attempt.
#[tauri::command]
pub async fn departure_cancel_running(state: State<'_, AppState>) -> AppResult<Cancelled> {
    let flow = state.departure.clone();
    flow.cancel_running().await
}

/// A window came back (popover shown, full window focused): a pending departure is looked at
/// again, at most once a minute.
#[tauri::command]
pub async fn departure_poke(state: State<'_, AppState>) -> AppResult<()> {
    let flow = state.departure.clone();
    flow.poke().await;
    Ok(())
}

/// Demo only (debug builds: not compiled in a release): what the simulated relay answers to the
/// departure ("normal", "offline", "server", "reset", "oldRelay"), or "solo" to leave this PC as
/// the only device of the space.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn demo_departure(state: State<'_, AppState>, mode: String) -> AppResult<()> {
    let fake = state
        .fake
        .as_ref()
        .ok_or_else(|| AppError::new("error.not_demo"))?;
    if mode == "solo" {
        fake.simulate_last_device();
        return Ok(());
    }
    let mode = mode
        .parse::<crate::fake_core::RelayMode>()
        .map_err(|_| AppError::new("error.not_demo"))?;
    fake.simulate_relay(mode);
    Ok(())
}

/// Fetches what waits on the relay (the popover was just opened). Never an error for
/// being offline; a device that is not paired, or whose space is blocked, has nothing to
/// fetch and the status banner already explains the situation, so it is not surfaced here.
#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> AppResult<()> {
    use crate::core_api::CoreError;
    match state.core.sync().await {
        Ok(()) | Err(CoreError::NotPaired) | Err(CoreError::Blocked(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------- history transfer

/// History transfers, oldest first. Not paired: none.
#[tauri::command]
pub async fn transfers(state: State<'_, AppState>) -> AppResult<Vec<Transfer>> {
    match state.core.transfers().await {
        Err(crate::core_api::CoreError::NotPaired) => Ok(Vec::new()),
        other => Ok(other?),
    }
}

/// Sends the whole history to a device of the space, sealed for that device only.
/// Secrets and files included: one yes or no, no choice per type (decision D2).
#[tauri::command]
pub async fn send_history(state: State<'_, AppState>, device_id: String) -> AppResult<()> {
    Ok(state.core.send_history(&device_id).await?)
}

/// The answer to a history another device offers: yes or no.
#[tauri::command]
pub async fn answer_history(
    state: State<'_, AppState>,
    transfer_id: String,
    accept: bool,
) -> AppResult<()> {
    Ok(state.core.answer_history(&transfer_id, accept).await?)
}

#[tauri::command]
pub async fn cancel_transfer(state: State<'_, AppState>, transfer_id: String) -> AppResult<()> {
    Ok(state.core.cancel_transfer(&transfer_id).await?)
}

// ---------------------------------------------------------------- pairing

// The pairing runs in the core (`spec/pairing-v1.md`); these commands start it, read the
// screen and carry the decisions of the user. The screen is read with `pairing_state` and
// announced by the `pairing-changed` event: no command waits on the other device.

/// What this PC can do: invite a device (paired), create the space or join one (not paired).
/// Asks the relay when this PC is not paired, so it can take a moment.
#[tauri::command]
pub async fn pairing_options(state: State<'_, AppState>) -> AppResult<PairingOptions> {
    Ok(state.core.pairing_options().await?)
}

/// The pairing screen as it is now. No network.
#[tauri::command]
pub async fn pairing_state(state: State<'_, AppState>) -> AppResult<PairingView> {
    Ok(state.core.pairing_view())
}

/// First device: creates the space. The install token is used once and kept nowhere.
#[tauri::command]
pub async fn pairing_create_space(
    state: State<'_, AppState>,
    install_token: String,
    device_name: Option<String>,
    relay_url: Option<String>,
) -> AppResult<()> {
    let token = Zeroizing::new(install_token);
    Ok(state
        .core
        .pairing_create_space_at(&token, device_name.as_deref(), relay_url.as_deref())
        .await?)
}

/// A member shows a code and a QR code for a new device.
#[tauri::command]
pub async fn pairing_invite(state: State<'_, AppState>) -> AppResult<PairingView> {
    Ok(state.core.pairing_invite().await?)
}

/// A new device joins with a typed code, a pasted link or a `tomi-share://pair#` link.
#[tauri::command]
pub async fn pairing_join(
    state: State<'_, AppState>,
    code_or_link: String,
    device_name: Option<String>,
    relay_url: Option<String>,
) -> AppResult<PairingView> {
    let input = Zeroizing::new(code_or_link);
    Ok(state
        .core
        .pairing_join_at(&input, device_name.as_deref(), relay_url.as_deref())
        .await?)
}

/// What a text typed or pasted in the pairing or the creation form is, and the relay it names
/// (https://host) with its authority. Local only; neither the code nor the token comes back.
#[tauri::command]
pub async fn pairing_read_input(text: String) -> AppResult<crate::pairing::InputRead> {
    let text = Zeroizing::new(text);
    Ok(crate::pairing::read_input(&text))
}

/// The user compared the verification codes and agrees. `import_history` answers the history
/// question of the new device (the inviter ignores it). The inviter hands the space key to the new
/// device, so a fresh Windows Hello check (Rust side, never remembered) comes first, like a
/// revocation. A PC that is joining a space is not asked: it gives nothing away.
#[tauri::command]
pub async fn pairing_confirm(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    import_history: bool,
) -> AppResult<()> {
    if matches!(state.core.pairing_view(), PairingView::Invite(_)) {
        unlock::confirm_add_device(&app, owner_of(&window)).await?;
    }
    Ok(state.core.pairing_confirm(import_history)?)
}

/// The user refuses, cancels or leaves the screen.
#[tauri::command]
pub async fn pairing_cancel(state: State<'_, AppState>) -> AppResult<()> {
    state.core.pairing_cancel();
    Ok(())
}

/// The pairing screen was left (another tab): a code on screen, or codes being compared, are
/// abandoned; a pairing that has moved on to adding the device goes on.
#[tauri::command]
pub async fn pairing_leave(state: State<'_, AppState>) -> AppResult<()> {
    state.core.pairing_leave();
    Ok(())
}

/// Forgets a finished pairing and the notice of an interrupted one that was settled.
#[tauri::command]
pub async fn pairing_dismiss(state: State<'_, AppState>) -> AppResult<()> {
    state.core.pairing_dismiss();
    Ok(())
}

/// The word of the code list that the first letters typed so far name (four are enough), to
/// complete a code while it is typed. The list is public; nothing is asked of the relay.
#[tauri::command]
pub async fn pairing_complete_word(prefix: String) -> AppResult<Option<String>> {
    Ok(crate::pairing::complete_word(&prefix))
}

/// The code of a `tomi-share://pair#` link that opened the app, once. It only fills the pairing
/// screen: joining is the user's click.
#[tauri::command]
pub async fn take_pair_link(state: State<'_, AppState>) -> AppResult<Option<PendingPair>> {
    Ok(state
        .pending_pair
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take())
}

/// The token and the relay of a creation link that opened the app, once. They only fill the
/// creation screen: creating the space is the user's click.
#[tauri::command]
pub async fn take_setup_link(state: State<'_, AppState>) -> AppResult<Option<PendingSetup>> {
    Ok(state
        .pending_setup
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take())
}

/// Rebinds the global shortcut. A conflict keeps the previous one.
#[tauri::command]
pub async fn set_shortcut(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
) -> AppResult<String> {
    let old = state.settings_snapshot().shortcut;
    let normalized = shortcut::rebind(&app, Some(&old), &spec)?;
    state.update_settings(|s| s.shortcut = normalized.clone())?;
    Ok(normalized)
}

#[tauri::command]
pub async fn set_notifications(state: State<'_, AppState>, enabled: bool) -> AppResult<Settings> {
    state.update_settings(|s| s.notifications = enabled)
}

#[tauri::command]
pub async fn set_autostart(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> AppResult<Settings> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|_| AppError::new("error.autostart"))?;
    state.update_settings(|s| {
        s.autostart = enabled;
        s.autostart_initialised = true;
    })
}

#[tauri::command]
pub async fn set_render_mode(
    state: State<'_, AppState>,
    mode: Option<RenderMode>,
) -> AppResult<Settings> {
    state.update_settings(|s| s.render_mode = mode)
}

/// Folder picker for "Enregistrer". `None` resets to Downloads\Tomi.
#[tauri::command]
pub async fn choose_download_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    reset: Option<bool>,
) -> AppResult<Settings> {
    use tauri_plugin_dialog::DialogExt;
    if reset.unwrap_or(false) {
        return state.update_settings(|s| s.download_dir = None);
    }
    let app2 = app.clone();
    let picked = blocking(move || app2.dialog().file().blocking_pick_folder()).await?;
    match picked.and_then(|p| p.into_path().ok()) {
        Some(p) => {
            let dir = p.to_string_lossy().into_owned();
            state.update_settings(|s| s.download_dir = Some(dir))
        }
        None => Ok(state.settings_snapshot()),
    }
}

#[tauri::command]
pub async fn complete_onboarding(state: State<'_, AppState>) -> AppResult<Settings> {
    state.update_settings(|s| s.onboarding_done = true)
}

// ---------------------------------------------------------------- windows

#[tauri::command]
pub async fn set_pinned(state: State<'_, AppState>, pinned: bool) -> AppResult<()> {
    state.pinned.store(pinned, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn hide_popover(app: AppHandle) -> AppResult<()> {
    popover::hide_popover(&app);
    Ok(())
}

#[tauri::command]
pub async fn open_window(app: AppHandle, tab: Option<String>) -> AppResult<()> {
    popover::open_history(&app, tab.as_deref());
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) -> AppResult<()> {
    app.exit(0);
    Ok(())
}

// ---------------------------------------------------------------- demo (fake core only)

/// Simulates an item arriving from another device. Only meaningful with the fake core; debug
/// builds only (not compiled in a release).
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn demo_receive(state: State<'_, AppState>, kind: String) -> AppResult<()> {
    let fake = state
        .fake
        .as_ref()
        .ok_or_else(|| AppError::new("error.not_demo"))?;
    let item = demo_item(fake.other_device(), &kind)?;
    fake.inject_incoming(item);
    Ok(())
}

#[cfg(any(test, debug_assertions))]
pub fn demo_item(source: crate::model::DeviceRef, kind: &str) -> AppResult<Item> {
    let now = now_ms();
    let mut item = Item {
        id: uuid::Uuid::new_v4().to_string(),
        title: None,
        text: None,
        url: None,
        attachments: vec![],
        secret: false,
        source_device: source,
        created_at: now,
        // Nothing expires by default (owner decision). Some demo items below show an
        // explicit choice.
        expires_at: None,
        read_policy: ReadPolicy::Normal,
        read: false,
        outgoing: false,
        imported: false,
        delivery: vec![],
        source_format: None,
        unreadable: false,
        unreadable_reason: None,
        pinned: false,
        pinned_at: None,
        archived: false,
        archived_at: None,
    };
    let text = |plain: &str, md: Option<&str>| TextBody {
        plain: plain.to_string(),
        html: None,
        markdown: md.map(str::to_string),
    };
    match kind {
        "link" => {
            item.url = Some("https://tauri.app/fr/".into());
            item.title = Some("Tauri 2".into());
        }
        "text" => item.text = Some(text("42 rue des Lilas, 12000 Rodez", None)),
        "markdown" => {
            let md = "# Courses\n\n- **Pain**\n- Lait\n- `Café`\n\n> Penser au *ticket* de caisse.";
            item.text = Some(text(md, Some(md)));
            item.source_format = Some(SourceFormat::Markdown);
            // An explicit duration, to show what the countdown looks like.
            item.expires_at = Some(now + 7 * 24 * 3_600_000);
        }
        "secret" => {
            // Hidden: the title is part of the secret, like the text.
            item.secret = true;
            item.title = Some("Wi-Fi invités".into());
            item.text = Some(text("correct-horse-battery", None));
        }
        // An item that cannot be read (kept, never deleted): nothing of it is known but its
        // sender and the reason, which picks the sentence of the card and of the toast.
        "unreadable_update" => {
            item.unreadable = true;
            item.unreadable_reason = Some(UnreadableReason::UpdateRequired);
        }
        "unreadable_other" => {
            item.unreadable = true;
            item.unreadable_reason = Some(UnreadableReason::Other);
        }
        _ => return Err(AppError::new("error.not_demo")),
    }
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceRef;

    #[test]
    fn percent_decode_handles_utf8_and_stray_percent() {
        assert_eq!(percent_decode("caf%C3%A9.png"), "café.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
    }

    #[test]
    fn code_files_are_decoded_with_the_limits_of_the_code_screen() {
        assert_eq!(decode_code(b"fn main() {}").unwrap(), "fn main() {}");
        // A UTF-8 byte order mark is dropped, a bad sequence is replaced, not refused.
        assert_eq!(decode_code(b"\xEF\xBB\xBFabc").unwrap(), "abc");
        assert_eq!(decode_code(b"a\xFFb").unwrap(), "a\u{FFFD}b");
        assert_eq!(decode_code(b"a\0b").unwrap_err().code, "error.code_binary");
        let big = vec![b'a'; CODE_MAX_BYTES as usize + 1];
        assert_eq!(decode_code(&big).unwrap_err().code, "error.code_too_large");
        assert!(decode_code(&big[..CODE_MAX_BYTES as usize]).is_ok());
    }

    #[test]
    fn the_image_type_comes_from_the_bytes_and_svg_is_refused() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let url = image_data_url(&png).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'></svg>";
        assert_eq!(image_data_url(svg).unwrap_err().code, "error.image");
        assert_eq!(
            image_data_url(b"not an image").unwrap_err().code,
            "error.image"
        );
    }

    #[test]
    fn demo_items_cover_each_kind_and_reject_unknown() {
        let src = DeviceRef {
            id: "d".into(),
            name: "Pixel".into(),
        };
        for k in [
            "link",
            "text",
            "markdown",
            "secret",
            "unreadable_update",
            "unreadable_other",
        ] {
            assert!(demo_item(src.clone(), k).is_ok(), "{k}");
        }
        assert!(demo_item(src, "bogus").is_err());
    }

    #[test]
    fn the_demo_offers_both_unreadable_reasons() {
        let src = DeviceRef {
            id: "d".into(),
            name: "Pixel".into(),
        };
        let update = demo_item(src.clone(), "unreadable_update").unwrap();
        assert!(update.unreadable && update.text.is_none() && update.url.is_none());
        assert_eq!(
            update.unreadable_reason,
            Some(UnreadableReason::UpdateRequired)
        );
        let other = demo_item(src, "unreadable_other").unwrap();
        assert!(other.unreadable);
        assert_eq!(other.unreadable_reason, Some(UnreadableReason::Other));
    }

    #[test]
    fn demo_items_do_not_expire_by_default_secrets_included() {
        let src = DeviceRef {
            id: "d".into(),
            name: "Pixel".into(),
        };
        for k in ["link", "text", "secret"] {
            let item = demo_item(src.clone(), k).unwrap();
            assert_eq!(item.expires_at, None, "{k}");
            assert_eq!(item.read_policy, ReadPolicy::Normal, "{k}");
        }
        // One example shows an explicit duration.
        assert!(demo_item(src, "markdown").unwrap().expires_at.is_some());
    }
}
