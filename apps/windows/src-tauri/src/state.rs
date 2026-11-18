//! Shared application state, managed by Tauri.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;

use crate::clip_clears::ClipboardClears;
use crate::core_api::TomiCore;
use crate::departure::LeaveFlow;
#[cfg(debug_assertions)]
use crate::fake_core::FakeCore;
use crate::limits::Rejection;
use crate::model::Attachment;
use crate::paths::PathRegistry;
use crate::placement::Area;
use crate::queue::SendQueue;
use crate::secret::SecretText;
use crate::settings::Settings;
use crate::strong_auth::StrongAuth;
use crate::unlock::SecretGate;

/// Content to put in the popover (from a `tomi-share://` URL or the SendTo menu).
/// Never sent automatically: it only fills the compose card.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefillPayload {
    pub url: Option<String>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub files: Vec<Attachment>,
    pub rejected: Vec<Rejection>,
    pub truncated: bool,
    /// i18n key of a refusal ("lien refusé"), when the request was invalid.
    pub error: Option<String>,
}

/// A pairing link waiting for the pairing screen. The relay is NOT trusted: the screen puts it in
/// the "Relais" field and the verification screen shows it in full (`pairing-v1.md` §4.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPair {
    pub code: SecretText,
    /// https://host[:port], None for the `tomi-share://` form.
    pub relay_url: Option<String>,
}

/// A creation link waiting for the creation screen (same trust as [PendingPair]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSetup {
    pub token: SecretText,
    pub relay_url: String,
}

pub struct AppState {
    pub core: Arc<dyn TomiCore>,
    /// Windows Hello, for the actions that ask it outside the departure screens (revoking a
    /// device).
    pub auth: Arc<dyn StrongAuth>,
    /// The Windows Hello gate in front of hidden content (2-minute window).
    pub secret_gate: SecretGate,
    /// The departure screens and their rules (Windows Hello, anti-tap, back-off).
    pub departure: Arc<LeaveFlow>,
    /// Present while the fake core is in use (demo and debug helpers). Neither the field nor the
    /// fake core exist in a release build.
    #[cfg(debug_assertions)]
    pub fake: Option<Arc<FakeCore>>,
    pub queue: SendQueue,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    /// Local files the app vouches for (staged, dropped, picked, saved). The webview
    /// cannot make Rust delete, send or reveal anything else.
    pub paths: Arc<PathRegistry>,
    /// Pending clipboard wipes of copied secrets.
    pub clip_clears: Arc<ClipboardClears>,
    /// History offers already announced by a toast (one toast per offer).
    pub seen_offers: Mutex<HashSet<String>>,
    pub pending_prefill: Mutex<Option<PrefillPayload>>,
    /// The pairing code of a `tomi-share://pair#` or `https://<relay>/l/pair#` link, waiting for the
    /// pairing screen to pick it up (once). A secret, in memory only; nothing starts until the
    /// user presses "Rejoindre".
    pub pending_pair: Mutex<Option<PendingPair>>,
    /// The token of a creation link (`https://<relay>/create#<token>`), waiting for the creation
    /// screen to pick it up (once). A secret, in memory only; nothing is sent until "Créer".
    pub pending_setup: Mutex<Option<PendingSetup>>,
    /// The popover stays open when it loses focus.
    pub pinned: AtomicBool,
    /// While > 0 (file dialog, drag-out) losing focus does not hide the popover.
    pub suppress_blur: AtomicUsize,
    pub hidden_by_blur_at: Mutex<Option<Instant>>,
    pub tray_anchor: Mutex<Option<Area>>,
}

impl AppState {
    pub fn settings(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings().clone()
    }

    pub fn update_settings(
        &self,
        f: impl FnOnce(&mut Settings),
    ) -> crate::error::AppResult<Settings> {
        let mut s = self.settings();
        f(&mut s);
        s.save(&self.settings_path)?;
        Ok(s.clone())
    }
}
