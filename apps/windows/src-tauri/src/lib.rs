mod account_auth;
mod actions;
mod clip_clears;
mod clipboard;
mod commands;
mod core_api;
mod deeplink;
mod departure;
mod departure_host;
mod detect;
mod draft;
mod error;
mod events;
mod expiry;
// The in-memory stand-in of the demo mode: tests and debug builds only, never a release.
#[cfg(any(test, debug_assertions))]
mod fake_core;
mod files;
mod inactivity;
mod ingest;
mod limits;
mod model;
mod notify;
mod pairing;
mod paths;
mod placement;
mod popover;
mod qr;
mod queue;
mod real_core;
mod render;
mod secret;
mod sendto;
mod settings;
mod shortcut;
mod state;
mod strings;
mod strong_auth;
mod tray;
mod unlock;
mod view;
mod wrapkey;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use tauri::{DragDropEvent, Manager, RunEvent, WindowEvent};

use crate::clip_clears::ClipboardClears;
use crate::core_api::TomiCore;
#[cfg(debug_assertions)]
use crate::fake_core::FakeCore;
use crate::paths::PathRegistry;
use crate::queue::SendQueue;
use crate::real_core::{RealCore, RealCoreConfig, DEFAULT_RELAY_URL};
use crate::settings::Settings;
use crate::state::AppState;

type SetupResult = Result<(), Box<dyn std::error::Error>>;

pub fn run() {
    let app = tauri::Builder::default()
        // Must be first: a second launch (SendTo, tomi-share:// URL, autostart twice)
        // is forwarded here and the plugin relays URLs to the deep-link plugin.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.len() <= 1 {
                popover::show_popover(app);
            } else {
                ingest::handle_args(app, args, true);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        popover::toggle_from_shortcut(app);
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            // Files dropped on a window: Rust sees them first-hand and records them, so
            // that `describe_files` / `send_draft` can tell them from a path the webview
            // merely claims.
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    for p in paths {
                        state.paths.register_user_file(p);
                    }
                }
            }
            // The pairing screen lives in the full window: closing it leaves the screen, which
            // abandons a code on show or codes being compared (`pairing-v1.md` §11.2). The
            // code on it must not stay valid for a window that is gone.
            if matches!(event, WindowEvent::Destroyed) && window.label() == popover::HISTORY {
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    state.core.pairing_leave();
                }
            }
        })
        .setup(|app| setup(app))
        .invoke_handler(tauri::generate_handler![
            commands::list_items,
            commands::render_item,
            commands::item_view,
            commands::read_code_file,
            commands::read_image,
            commands::copy_item,
            commands::copy_secret,
            commands::reveal_secret,
            commands::pin_item,
            commands::unpin_item,
            commands::archive_item,
            commands::unarchive_item,
            commands::open_item,
            commands::save_item,
            commands::reveal_in_folder,
            commands::delete_item,
            commands::mark_read,
            commands::consume_revealed,
            commands::start_drag,
            commands::detect_content,
            commands::expiry_rules,
            commands::describe_files,
            commands::pick_files,
            commands::stage_blob,
            commands::discard_staged,
            commands::send_draft,
            commands::cancel_job,
            commands::retry_job,
            commands::dismiss_job,
            commands::active_jobs,
            commands::take_prefill,
            commands::app_info,
            commands::enter_demo,
            commands::leave_demo,
            commands::devices,
            commands::rename_device,
            commands::revoke_device,
            commands::departure_state,
            commands::departure_open_leave,
            commands::departure_confirm_leave,
            commands::departure_confirm_delete,
            commands::departure_open_erase,
            commands::departure_set_erase_checked,
            commands::departure_confirm_erase,
            commands::departure_dismiss,
            commands::departure_retry,
            commands::departure_cancel,
            commands::departure_cancel_running,
            commands::departure_poke,
            #[cfg(debug_assertions)]
            commands::demo_departure,
            commands::sync_now,
            commands::transfers,
            commands::send_history,
            commands::answer_history,
            commands::cancel_transfer,
            commands::pairing_options,
            commands::pairing_state,
            commands::pairing_create_space,
            commands::pairing_invite,
            commands::pairing_join,
            commands::pairing_confirm,
            commands::pairing_cancel,
            commands::pairing_leave,
            commands::pairing_dismiss,
            commands::pairing_complete_word,
            commands::pairing_read_input,
            commands::take_pair_link,
            commands::take_setup_link,
            commands::set_shortcut,
            commands::set_notifications,
            commands::set_autostart,
            commands::set_render_mode,
            commands::choose_download_dir,
            commands::complete_onboarding,
            commands::set_pinned,
            commands::hide_popover,
            commands::open_window,
            commands::quit_app,
            #[cfg(debug_assertions)]
            commands::demo_receive,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Tomi application");

    app.run(|app, event| match event {
        // Closing the history window must not quit: Tomi lives in the tray.
        RunEvent::ExitRequested { api, code, .. } => {
            if code.is_none() {
                api.prevent_exit();
            }
        }
        // Quitting within 30 s of copying a secret: nobody would wipe it afterwards. The
        // decrypted copies of attachments do not outlive the session either.
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                if let Some(pending) = state.clip_clears.take_any() {
                    actions::wipe(&pending);
                }
                state.core.shutdown();
            }
            // Staged copies of pasted/dropped files are plaintext: do not leave them on
            // disk between sessions. They are also wiped at the next start, but a job that
            // failed keeps its files until then, so clear them here too.
            if let Ok(dir) = app.path().app_local_data_dir() {
                let _ = std::fs::remove_dir_all(dir.join("staging"));
            }
        }
        _ => {}
    });
}

fn setup(app: &mut tauri::App) -> SetupResult {
    use tauri_plugin_deep_link::DeepLinkExt;

    let handle = app.handle().clone();
    let config_dir = app.path().app_config_dir()?;
    let data_dir = app.path().app_local_data_dir()?;

    let settings_path = config_dir.join("settings.json");
    let settings = Settings::load(&settings_path);

    // Staged files belong to jobs that only lived in memory: start clean.
    let staging_dir = data_dir.join("staging");
    let _ = std::fs::remove_dir_all(&staging_dir);
    std::fs::create_dir_all(&staging_dir)?;
    let paths = Arc::new(PathRegistry::new(&staging_dir));

    // The real core, unless this is a debug build started with `TOMI_DEMO=1`: the in-memory
    // stand-in, nothing is encrypted, stored or sent. A release build does not compile the
    // stand-in; its way into a demo is the real core's demo space, entered from the pairing
    // screen (`commands::enter_demo`).
    #[cfg(debug_assertions)]
    let (core, fake): (Arc<dyn TomiCore>, Option<Arc<FakeCore>>) =
        if std::env::var_os("TOMI_DEMO").is_some() {
            let fake = Arc::new(FakeCore::new(data_dir.join("fake-core")));
            for kind in ["link", "text", "markdown", "secret"] {
                if let Ok(item) = commands::demo_item(fake.other_device(), kind) {
                    fake.inject_incoming(item);
                }
            }
            (fake.clone(), Some(fake))
        } else {
            (open_real_core(app, &data_dir), None)
        };
    #[cfg(not(debug_assertions))]
    let core: Arc<dyn TomiCore> = open_real_core(app, &data_dir);
    let rx = core.subscribe();

    // The departure screens: the rules that protect an irreversible action (Windows Hello,
    // the 2 s anti-tap, the back-off) live in Rust, the webview only draws them.
    let auth = strong_auth::system();
    let departure = departure::LeaveFlow::new(
        core.clone(),
        auth.clone(),
        Arc::new(departure_host::TauriHost {
            app: handle.clone(),
        }),
        Arc::new(model::now_ms),
        Arc::new(|task| {
            tauri::async_runtime::spawn(task);
        }),
        Arc::new(departure::random_jitter),
    );

    let startup_core = core.clone();
    let queue = SendQueue::new(
        core.clone(),
        Arc::new(events::TauriSink {
            app: handle.clone(),
        }),
        queue::SEND_DELAY,
        paths.clone(),
    );

    app.manage(AppState {
        core,
        secret_gate: unlock::SecretGate::new(auth.clone()),
        auth,
        departure: departure.clone(),
        #[cfg(debug_assertions)]
        fake,
        queue,
        settings: Mutex::new(settings.clone()),
        settings_path,
        paths,
        clip_clears: Arc::new(ClipboardClears::default()),
        seen_offers: Mutex::new(HashSet::new()),
        pending_prefill: Mutex::new(None),
        pending_pair: Mutex::new(None),
        pending_setup: Mutex::new(None),
        pinned: AtomicBool::new(false),
        suppress_blur: AtomicUsize::new(0),
        hidden_by_blur_at: Mutex::new(None),
        tray_anchor: Mutex::new(None),
    });

    tray::build(&handle)?;

    // Popover: hide on focus loss (unless pinned).
    if let Some(w) = app.get_webview_window(popover::POPOVER) {
        let h = handle.clone();
        w.on_window_event(move |e| match e {
            WindowEvent::Focused(false) => popover::on_popover_blur(&h),
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                popover::hide_popover(&h);
            }
            _ => {}
        });
    }

    // Global shortcut. A conflict is not fatal: the user changes it in the settings.
    if let Err(e) = shortcut::rebind(&handle, None, &settings.shortcut) {
        eprintln!("tomi: global shortcut not registered ({})", e.code);
        let _ = shortcut::rebind(&handle, None, shortcut::DEFAULT_SHORTCUT);
    }

    // Deep links and command-line arguments of this very first launch.
    {
        let h = handle.clone();
        app.deep_link().on_open_url(move |event| {
            for url in event.urls() {
                ingest::handle_deeplink_url(&h, url.as_str());
            }
        });
        if let Ok(Some(urls)) = app.deep_link().get_current() {
            for url in urls {
                ingest::handle_deeplink_url(&handle, url.as_str());
            }
        }
        ingest::handle_args(&handle, std::env::args().collect(), true);
    }

    // System integration that must not touch a developer's machine in debug builds.
    if !cfg!(debug_assertions) {
        system_integration(&handle);
    }

    if !settings.onboarding_done {
        popover::open_history(&handle, Some("onboarding"));
    }

    // A departure that was waiting when the app last stopped is settled first (7.9.4, before
    // anything else uses the core: the settling of an interrupted pairing, `resume_pairing`, waits
    // for it); the back-off goes on from there if only the network or the server is missing.
    // Then, if a previous run erased the core and died before the app's own
    // cleanup ended (this PC is no longer paired, yet the persistent `belonged_to_space` proof
    // is still set), the cleanup is done now: it is idempotent (plan A6). A PC that was never
    // paired is left alone, and a pairing link that opened the app is never touched.
    let startup = handle.clone();
    tauri::async_runtime::spawn(async move {
        departure.resume_at_startup().await;
        startup_core.resume_pairing();
        let state = startup.state::<AppState>();
        departure_host::remember_membership(&state);
        if departure_host::needs_startup_cleanup(&state) {
            departure_host::finish_erasure(&startup, departure_host::Cleanup::AtStartup).await;
        }
    });
    tauri::async_runtime::spawn(events::run(handle.clone(), rx));
    tauri::async_runtime::spawn(events::purge_loop(handle.clone()));
    tauri::async_runtime::spawn(events::plaintext_janitor(handle.clone()));
    tauri::async_runtime::spawn(tray::watch_look(handle.clone()));
    let h = handle.clone();
    tauri::async_runtime::spawn(async move { actions::refresh_badge(&h).await });
    Ok(())
}

/// Opens the real core and starts its background work (change stream, doorbell, housekeeping),
/// which lives on Tauri's async runtime. The settling of an interrupted pairing is NOT started
/// here: it waits for the pending departure (`setup`, `TomiCore::resume_pairing`).
fn open_real_core(app: &tauri::App, data_dir: &std::path::Path) -> Arc<dyn TomiCore> {
    let real = RealCore::open(&RealCoreConfig {
        data_dir: data_dir.to_path_buf(),
        relay_url: relay_url(),
        app_version: app.package_info().version.to_string(),
    });
    real.start();
    real
}

/// The relay of a PC that has none yet: the default compiled into the build
/// (`TOMI_DEFAULT_RELAY_URL` at build time, the installer of the owner), else none. A release
/// build without it asks for the relay (link, QR code, creation link or the "Relais" field). A
/// debug build may point `TOMI_RELAY_URL` at a local one. A PC that belongs to a space keeps the
/// relay of its space, whatever this says.
fn relay_url() -> Option<String> {
    if cfg!(debug_assertions) {
        if let Ok(url) = std::env::var("TOMI_RELAY_URL") {
            return Some(url).filter(|u| !u.trim().is_empty());
        }
    }
    DEFAULT_RELAY_URL
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
}

/// Autostart (on by default, once) and the "Envoyer vers" shortcut.
fn system_integration(app: &tauri::AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let state = app.state::<AppState>();

    let snapshot = state.settings_snapshot();
    if !snapshot.autostart_initialised {
        if snapshot.autostart {
            let _ = app.autolaunch().enable();
        }
        let _ = state.update_settings(|s| s.autostart_initialised = true);
    }

    if let (Ok(exe), Some(dir)) = (std::env::current_exe(), sendto::sendto_dir()) {
        let exe_str = exe.to_string_lossy().into_owned();
        // Written on first launch, when the app moved, or when the executable is a new file
        // (update, reinstall after an uninstall which removed the shortcut): NOT at every launch,
        // so that someone who deleted « Envoyer vers › Tomi » on purpose does not get it back.
        let stamp = sendto::exe_stamp(&exe);
        let remember = match sendto::decide(
            snapshot.sendto_exe.as_deref(),
            snapshot.sendto_stamp,
            &exe_str,
            stamp,
        ) {
            sendto::Action::Nothing => false,
            sendto::Action::Remember => true,
            sendto::Action::Write => sendto::write_shortcut(&exe, &dir).is_ok(),
        };
        if remember {
            let _ = state.update_settings(|s| {
                s.sendto_exe = Some(exe_str);
                s.sendto_stamp = stamp;
            });
        }
    }
}
