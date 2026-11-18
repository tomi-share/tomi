//! Local administration (`api-v1.md` §8.7): `tomi-relay admin ...`. No administration API is
//! exposed on the network. A running relay listens on a Unix socket (`<data_dir>/admin.sock`,
//! mode 0600); without a socket the command works on the databases directly.
//!
//! The same [`run`] serves both paths, so the output and the effects are identical.
//!
//! The relay hosts several spaces. The commands that look at or change one space take
//! `--space <id or prefix of 8 characters>`; without it, a relay that has exactly one space
//! uses that one, and a relay that has several refuses (exit 2) and lists them.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::fcm::FcmCredentials;
use crate::install;
use crate::revoke::{revoke_device, Via};
use crate::spaces::{DeleteOutcome, Find, Space};
use crate::state::{AppState, Effects};
use crate::store::{self, members};
use crate::wire::{sha256, ts_format};

/// An administration command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// Without `space`: the state of the relay. With it: the state of that space.
    Status {
        json: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    Devices {
        json: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    /// The spaces of the relay.
    Spaces { json: bool },
    /// Sets the quota of ONE space: a number of bytes, `none` (no limit) or `default` (back to the
    /// default quota of the relay: the web panel's, else `max_space_bytes`, else no limit).
    Quota {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
        value: String,
    },
    /// Lists the pending invitations; `new` adds one and shows its token; `rotate` replaces
    /// all of them by one.
    Invite {
        #[serde(default)]
        rotate: bool,
        #[serde(default)]
        new: bool,
    },
    Revoke {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    Reset {
        confirm: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    /// The web administration panel (`admin_web.rs`): its URL, its enrolment, its passkey.
    Web { action: WebAction },
}

/// What `tomi-relay admin web ...` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebAction {
    /// Shows the URL of the panel and whether a passkey is registered.
    Show,
    /// Generates a new secret path (the old one and every session stop working).
    RotatePath,
    /// Creates a one-time enrolment code and shows it.
    Enroll,
    /// Deletes the passkey and the sessions. The enrolment reopens once `enroll` makes a new code.
    Reset { confirm: Option<String> },
}

/// What a command printed and its exit code: 0 success, 1 error, 2 refusal or bad usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub(crate) fn ok(stdout: impl Into<String>) -> Self {
        Self {
            exit: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    pub(crate) fn fail(exit: i32, stderr: impl Into<String>) -> Self {
        Self {
            exit,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
    fn from_error(e: &ApiError) -> Self {
        Self::fail(1, format!("error: {}", e.code.as_str()))
    }
}

/// Does the command change anything (it then needs the relay stopped, or its socket)?
pub fn needs_exclusive_access(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Status { .. } | Cmd::Devices { .. } | Cmd::Spaces { .. } => false,
        Cmd::Invite { rotate, new } => *rotate || *new,
        Cmd::Web { action } => !matches!(action, WebAction::Show),
        Cmd::Quota { .. } | Cmd::Revoke { .. } | Cmd::Reset { .. } => true,
    }
}

/// Runs a command against the relay state.
pub async fn run(st: &AppState, cmd: Cmd) -> Output {
    match cmd {
        Cmd::Status { json, space } => match space {
            None => status_relay(st, json).await,
            Some(s) => match pick_space(st, Some(&s)) {
                Ok(space) => status_space(st, &space, json).await,
                Err(out) => out,
            },
        },
        Cmd::Devices { json, space } => devices(st, space.as_deref(), json).await,
        Cmd::Spaces { json } => spaces(st, json).await,
        Cmd::Quota { space, value } => quota(st, space.as_deref(), &value),
        Cmd::Invite { rotate, new } => invite(st, rotate, new),
        Cmd::Revoke { target, space } => revoke(st, space.as_deref(), &target).await,
        Cmd::Reset { confirm, space } => reset(st, space.as_deref(), confirm.as_deref()).await,
        Cmd::Web { action } => crate::admin_web::command(st, action).await,
    }
}

/// The space a command is about: the one asked for, or the only one there is. Shared with the web
/// panel, which only ever names a space by the prefix it listed.
pub(crate) fn pick_space(st: &AppState, requested: Option<&str>) -> Result<Arc<Space>, Output> {
    match requested {
        Some(prefix) => st.spaces.find(prefix).map_err(|why| match why {
            Find::Unusable => Output::fail(
                2,
                "give a space id, or a prefix of at least 8 hexadecimal characters \
                 (see `tomi-relay admin spaces`)",
            ),
            Find::NotFound => Output::fail(1, "no such space"),
            Find::Ambiguous(ids) => {
                Output::fail(2, format!("the prefix is ambiguous:\n{}", ids.join("\n")))
            }
        }),
        None => {
            let mut all = st.spaces.list();
            match all.len() {
                0 => Err(Output::fail(1, "no space exists")),
                1 => Ok(all.remove(0)),
                _ => Err(Output::fail(
                    2,
                    format!(
                        "this relay has several spaces: say which one with --space <id or \
                         8-character prefix>\n{}",
                        all.iter()
                            .map(|s| s.text_id())
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                )),
            }
        }
    }
}

/// A recorded push-health row: `(state, last_success_at_ms, last_error_at_ms, error_class)`.
type HealthRow = (String, Option<i64>, Option<i64>, Option<String>);

/// Health of the push doorbell: `disabled`, `ok`, `auth_error`, `degraded` or `unknown`.
fn fcm_health(st: &AppState, recorded: Option<HealthRow>) -> Value {
    let Some(file) = &st.cfg.fcm_service_account_file else {
        return json!({ "state": "disabled" });
    };
    // A service account that cannot be loaded is an authentication problem, whatever the
    // database remembers.
    if let Err(e) = FcmCredentials::load(file, st.cfg.fcm_project_id.as_deref()) {
        tracing::debug!("admin: service account check failed ({e})");
        return json!({ "state": "auth_error", "last_error": { "class": "invalid_service_account" } });
    }
    match recorded {
        None => json!({ "state": "unknown" }),
        Some((state, ok_at, err_at, err)) => {
            let mut v = json!({ "state": state });
            if let Some(t) = ok_at {
                v["last_success_at"] = json!(ts_format(t));
            }
            if let (Some(t), Some(class)) = (err_at, err) {
                v["last_error"] = json!({ "class": class, "at": ts_format(t) });
            }
            v
        }
    }
}

/// The FCM health of the relay (`routing.db`) and the dead tokens of the last 7 days.
async fn fcm_state(st: &AppState) -> Result<Value, ApiError> {
    let seven_days = st.now() - 7 * 86_400_000;
    let (health, unregistered) = st
        .spaces
        .routing()
        .read(move |tx| {
            let health = tx
                .query_row(
                    "SELECT state, last_success_at, last_error_at, last_error FROM push_health WHERE id = 1",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .ok();
            let unregistered: i64 = tx.query_row(
                "SELECT count(*) FROM push_events WHERE kind = 'unregistered' AND at >= ?1",
                rusqlite::params![seven_days],
                |r| r.get(0),
            )?;
            Ok((health, unregistered))
        })
        .await?;
    let mut fcm = fcm_health(st, health);
    fcm["unregistered_tokens_7d"] = json!(unregistered);
    Ok(fcm)
}

/// The state of the relay as a whole (`status`, and the web panel): the version, the number of
/// spaces, the total usage, the free disk, the push health, the pending invitations. Metadata
/// only.
pub(crate) async fn relay_status(st: &AppState) -> Result<Value, ApiError> {
    let all = st.spaces.list();
    let mut used = 0u64;
    for s in &all {
        used = used.saturating_add(s.db.read(|tx| store::used_bytes(tx)).await?);
    }
    let fcm = fcm_state(st).await?;
    let invitations = install::list(st.spaces.routing())?.len();
    Ok(json!({
        "relay_version": env!("CARGO_PKG_VERSION"),
        "spaces": all.len(),
        // Pending spaces (§4.4.1) are counted apart, and count for `space_initialized` exactly
        // as in `GET /v1/hello`.
        "pending_spaces": st.spaces.pending_count(),
        "space_initialized": !st.spaces.is_empty(),
        "quota": { "used_bytes": used, "max_bytes": st.cfg.max_relay_bytes },
        "default_space_quota": default_quota_json(st),
        "disk_free_bytes": st.disk_free(),
        "fcm": fcm,
        "invitations_pending": invitations,
    }))
}

/// The default quota of a space that has none of its own, as the panel and `status` show it:
/// `bytes` (`null`: no limit per space), `source` (`panel`, `config`, or `none` when nothing is
/// set: no limit), and the bounds of what the panel accepts. A number of bytes and a word: nothing
/// about a space.
pub(crate) fn default_quota_json(st: &AppState) -> Value {
    let (bytes, source) = st.spaces.default_space_quota(&st.cfg);
    json!({
        "bytes": bytes,
        "source": source,
        "min_bytes": crate::config::MIN_DEFAULT_SPACE_QUOTA_BYTES,
        "max_bytes": crate::config::MAX_DEFAULT_SPACE_QUOTA_BYTES,
        "relay_max_bytes": st.cfg.max_relay_bytes,
    })
}

/// Reads the `default_bytes` of a request of the web panel: an integer number of bytes between
/// [`MIN_DEFAULT_SPACE_QUOTA_BYTES`](crate::config::MIN_DEFAULT_SPACE_QUOTA_BYTES) and
/// [`MAX_DEFAULT_SPACE_QUOTA_BYTES`](crate::config::MAX_DEFAULT_SPACE_QUOTA_BYTES) (a quota of
/// that many bytes for every space without a quota of its own), `0` (no limit per space; only the
/// total of the relay applies) or `null` (forget what the panel set: back to `max_space_bytes` of
/// the configuration, else no limit). Anything else (missing, text, a fraction, a
/// negative number, a number below the minimum or above the maximum) is refused with a generic
/// message that does not repeat the value.
pub(crate) fn parse_default_quota(body: &Value) -> Result<Option<Option<u64>>, ApiError> {
    use crate::config::{MAX_DEFAULT_SPACE_QUOTA_BYTES, MIN_DEFAULT_SPACE_QUOTA_BYTES};
    let refused = || {
        ApiError::bad_request(
            "default_bytes is a whole number of bytes (at least 1 Mio), 0 (no limit per space) or null (the configured default).",
        )
    };
    match body.get("default_bytes") {
        Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(0) => Ok(Some(None)),
            Some(b)
                if (MIN_DEFAULT_SPACE_QUOTA_BYTES..=MAX_DEFAULT_SPACE_QUOTA_BYTES).contains(&b) =>
            {
                Ok(Some(Some(b)))
            }
            _ => Err(refused()),
        },
        _ => Err(refused()),
    }
}

/// Sets the default quota (the web panel). Journal line without any value that belongs to a space.
pub(crate) fn set_default_quota(st: &AppState, value: Option<Option<u64>>) -> Result<(), ApiError> {
    st.spaces.set_default_quota(value, st.now())?;
    match value {
        None => {
            tracing::info!("admin web: the default space quota was reset to the configured one")
        }
        Some(None) => tracing::info!("admin web: the default space quota is now no limit"),
        Some(Some(bytes)) => {
            tracing::info!(bytes, "admin web: the default space quota was changed");
        }
    }
    Ok(())
}

/// `status` without `--space`: the relay as a whole.
async fn status_relay(st: &AppState, as_json: bool) -> Output {
    let v = match relay_status(st).await {
        Ok(v) => v,
        Err(e) => return Output::from_error(&e),
    };
    if as_json {
        return Output::ok(format!("{v}\n"));
    }
    let default_quota = v["default_space_quota"]["bytes"]
        .as_u64()
        .map_or_else(|| "no limit".to_owned(), |b| format!("{b} bytes"));
    Output::ok(format!(
        "tomi-relay {}\nspaces: {}\npending spaces: {}\nspace initialized: {}\nquota (all spaces): {} / {} bytes\ndefault quota of a space: {} (from {})\ndisk free: {} bytes\nfcm: {}\ninvitations pending: {}\n",
        env!("CARGO_PKG_VERSION"),
        v["spaces"],
        v["pending_spaces"],
        v["space_initialized"],
        v["quota"]["used_bytes"],
        v["quota"]["max_bytes"],
        default_quota,
        v["default_space_quota"]["source"].as_str().unwrap_or("?"),
        v["disk_free_bytes"],
        v["fcm"]["state"].as_str().unwrap_or("unknown"),
        v["invitations_pending"]
    ))
}
/// `status --space`: one space.
async fn status_space(st: &AppState, space: &Arc<Space>, as_json: bool) -> Output {
    let now = st.now();
    let cfg = st.cfg.clone();
    let gathered = space
        .db
        .read(move |tx| {
            let sp = store::require_space(tx)?;
            let all = members(tx)?;
            let used = store::used_bytes(tx)?;
            Ok((sp, all, used))
        })
        .await;
    let (sp, all, used) = match gathered {
        Ok(v) => v,
        Err(e) => return Output::from_error(&e),
    };
    let budget = match st.budget(space).await {
        Ok(b) => b,
        Err(e) => return Output::from_error(&e),
    };
    let fcm = match fcm_state(st).await {
        Ok(v) => v,
        Err(e) => return Output::from_error(&e),
    };
    let active = all.iter().filter(|d| d.is_active(now, &cfg)).count();
    let present = all.iter().filter(|d| d.is_present(now)).count();
    let free = st.disk_free();
    let id = space.text_id();
    let v = json!({
        "relay_version": env!("CARGO_PKG_VERSION"),
        "space_id": id,
        "created_at": ts_format(sp.created_at),
        "members": all.len(),
        "active": active,
        "present": present,
        "head_seq": sp.last_seq,
        "retention_floor_seq": sp.retention_floor_seq,
        "key_epoch": sp.key_epoch,
        "rotation_required": sp.rotation_required,
        "quota": {
            "used_bytes": used,
            "max_bytes": budget.effective_max(used),
            "space_max_bytes": budget.space_max,
        },
        "disk_free_bytes": free,
        "fcm": fcm,
    });
    if as_json {
        return Output::ok(format!("{v}\n"));
    }
    let quota = budget
        .space_max
        .map_or_else(|| "none".to_owned(), |m| m.to_string());
    Output::ok(format!(
        "tomi-relay {}\nspace: {} (created {})\nmembers: {} ({} active, {} present)\nlog: head_seq {}, retention_floor_seq {}\nkeys: key_epoch {}, rotation_required {}\nquota: {} / {} bytes (quota of the space: {}; effective ceiling {})\ndisk free: {} bytes\nfcm: {}\n",
        env!("CARGO_PKG_VERSION"),
        id,
        ts_format(sp.created_at),
        all.len(),
        active,
        present,
        sp.last_seq,
        sp.retention_floor_seq,
        sp.key_epoch,
        sp.rotation_required,
        used,
        budget.effective_max(used),
        quota,
        budget.effective_max(used),
        free,
        v["fcm"]["state"].as_str().unwrap_or("unknown"),
    ))
}

/// What [`space_rows`] reads in the database of one space: counts, sizes and dates, nothing else.
struct SpaceFigures {
    members: usize,
    active: usize,
    present: usize,
    used: u64,
    last_activity: Option<i64>,
    items: i64,
    blobs: i64,
    events: i64,
    space: Option<store::SpaceRow>,
}

fn count(tx: &rusqlite::Connection, sql: &str) -> Result<i64, ApiError> {
    Ok(tx.query_row(sql, [], |r| r.get(0))?)
}

/// One row per space (`spaces`, and the web panel): the id, the date, the devices, the usage and
/// the quota of each, and (added later, additive) the devices present, the last activity of a
/// member, the number of live items, of blobs and of retained events, the log bounds and whether a
/// key rotation is pending. Counts, sizes and dates: metadata only, nothing the relay would have to
/// decrypt.
pub(crate) async fn space_rows(st: &AppState) -> Result<Vec<Value>, ApiError> {
    let (now, cfg) = (st.now(), st.cfg.clone());
    let mut rows: Vec<Value> = Vec::new();
    for s in st.spaces.list() {
        let c = cfg.clone();
        let f =
            s.db.read(move |tx| {
                let all = members(tx)?;
                Ok(SpaceFigures {
                    members: all.len(),
                    active: all.iter().filter(|d| d.is_active(now, &c)).count(),
                    present: all.iter().filter(|d| d.is_present(now)).count(),
                    used: store::used_bytes(tx)?,
                    last_activity: all.iter().filter_map(|d| d.last_seen_at).max(),
                    items: count(tx, "SELECT count(*) FROM items WHERE purged_at IS NULL")?,
                    blobs: count(tx, "SELECT count(*) FROM blobs")?,
                    events: count(tx, "SELECT count(*) FROM events")?,
                    space: store::space_row(tx)?,
                })
            })
            .await?;
        rows.push(json!({
            "space_id": s.text_id(),
            "created_at": ts_format(s.created_at),
            "members": f.members,
            "active": f.active,
            "present": f.present,
            "last_activity_at": f.last_activity.map(ts_format),
            "used_bytes": f.used,
            "max_bytes": st.spaces.space_max(&s.id, &cfg),
            // `space`: an override of `admin quota`; `default`: the default quota of the relay
            // (see `default_space_quota` of `status --json`; `max_bytes: null` = no limit).
            "quota_source": st.spaces.quota_source(&s.id),
            "items": f.items,
            "blobs": f.blobs,
            "events": f.events,
            "head_seq": f.space.as_ref().map(|r| r.last_seq),
            "retention_floor_seq": f.space.as_ref().map(|r| r.retention_floor_seq),
            "rotation_required": f.space.as_ref().map(|r| r.rotation_required),
        }));
    }
    Ok(rows)
}

/// `spaces`: one line per space.
async fn spaces(st: &AppState, as_json: bool) -> Output {
    let rows = match space_rows(st).await {
        Ok(v) => v,
        Err(e) => return Output::from_error(&e),
    };
    if as_json {
        return Output::ok(format!("{}\n", json!({ "spaces": rows })));
    }
    if rows.is_empty() {
        return Output::ok("no spaces\n");
    }
    let mut out = String::new();
    for r in &rows {
        out.push_str(&format!(
            "{}  created {}  members {} ({} active)  used {}  quota {}\n",
            r["space_id"].as_str().unwrap_or(""),
            r["created_at"].as_str().unwrap_or(""),
            r["members"],
            r["active"],
            r["used_bytes"],
            r["max_bytes"]
                .as_u64()
                .map_or_else(|| "none".to_owned(), |m| m.to_string()),
        ));
    }
    Output::ok(out)
}

/// The member devices of a space (`devices`, and the web panel): metadata only, never a key
/// other than the short fingerprint of the signing key.
pub(crate) async fn device_rows(st: &AppState, space: &Space) -> Result<Vec<Value>, ApiError> {
    let (now, cfg) = (st.now(), st.cfg.clone());
    let all = space.db.read(|tx| members(tx)).await?;
    Ok(all
        .iter()
        .map(|d| {
            let fingerprint = d
                .sign_pk
                .as_deref()
                .map(|pk| hex::encode(sha256(pk))[..16].to_owned());
            json!({
                "device_id": d.device_id,
                "name": d.name,
                "platform": d.platform,
                "app_version": d.app_version,
                "format_min": d.format_min,
                "format_max": d.format_max,
                "last_seen_at": d.last_seen_at.map(ts_format),
                "registered_at": ts_format(d.registered_at),
                "active": d.is_active(now, &cfg),
                "present": d.is_present(now),
                "push": d.push,
                "cursor": d.cursor_hint,
                "fingerprint": fingerprint,
            })
        })
        .collect())
}
async fn devices(st: &AppState, requested: Option<&str>, as_json: bool) -> Output {
    // A relay without a space has no devices: that is an answer, not an error.
    if requested.is_none() && st.spaces.is_empty() {
        return Output::ok(if as_json {
            format!("{}\n", json!({ "devices": [] }))
        } else {
            "no devices\n".to_owned()
        });
    }
    let space = match pick_space(st, requested) {
        Ok(s) => s,
        Err(out) => return out,
    };
    let rows = match device_rows(st, &space).await {
        Ok(v) => v,
        Err(e) => return Output::from_error(&e),
    };
    if as_json {
        return Output::ok(format!("{}\n", json!({ "devices": rows })));
    }
    let mut out = String::new();
    for r in &rows {
        out.push_str(&format!(
            "{}  {}  {}  v{}  format {}..{}  seen {}  active {}  present {}  push {}  cursor {}  key {}\n",
            r["device_id"].as_str().unwrap_or(""),
            r["name"].as_str().unwrap_or(""),
            r["platform"].as_str().unwrap_or(""),
            r["app_version"].as_str().unwrap_or("-"),
            r["format_min"].as_i64().map_or("-".into(), |v| v.to_string()),
            r["format_max"].as_i64().map_or("-".into(), |v| v.to_string()),
            r["last_seen_at"].as_str().unwrap_or("never"),
            r["active"],
            r["present"],
            r["push"],
            r["cursor"].as_i64().map_or("-".into(), |v| v.to_string()),
            r["fingerprint"].as_str().unwrap_or("-"),
        ));
    }
    if out.is_empty() {
        out.push_str("no devices\n");
    }
    Output::ok(out)
}

/// `quota`: `<bytes>`, `none` (no limit for this space) or `default` (the default quota of the relay).
fn quota(st: &AppState, requested: Option<&str>, value: &str) -> Output {
    let parsed: Option<Option<u64>> =
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(None),
            "default" => None,
            n => match n.parse::<u64>() {
                Ok(b) if b > 0 && i64::try_from(b).is_ok() => Some(Some(b)),
                _ => return Output::fail(
                    2,
                    "the quota is a number of bytes (at least 1), `none` (no limit) or `default`",
                ),
            },
        };
    let space = match pick_space(st, requested) {
        Ok(s) => s,
        Err(out) => return out,
    };
    if let Err(e) = st.spaces.set_quota_override(&space.id, parsed) {
        return Output::from_error(&e);
    }
    let shown = match st.spaces.space_max(&space.id, &st.cfg) {
        Some(b) => format!("{b} bytes"),
        None => "no limit".to_owned(),
    };
    Output::ok(format!("quota of space {}: {shown}\n", space.short_id()))
}

/// Creates an invitation and returns its token, the only time it exists in clear. With `rotate`,
/// the pending invitations are replaced (the consumed ones stay). Shared with the web panel.
pub(crate) fn issue_invitation(st: &AppState, rotate: bool) -> Result<String, ApiError> {
    let routing = st.spaces.routing();
    if rotate {
        install::clear(routing)?;
    }
    install::create(routing, st.now())
}

/// `invite`: lists the pending invitations; `--new` adds one; `--rotate` replaces them.
///
/// The relay keeps only the hash of a token, so the token of an earlier invitation cannot be
/// shown again: it is printed once, when the invitation is created.
fn invite(st: &AppState, rotate: bool, new: bool) -> Output {
    let routing = st.spaces.routing();
    if rotate || new {
        return match issue_invitation(st, rotate) {
            Ok(t) => Output::ok(format!("{t}\n")),
            Err(e) => Output::fail(
                1,
                format!("cannot create the invitation: {}", e.code.as_str()),
            ),
        };
    }
    match install::list(routing) {
        Err(e) => Output::from_error(&e),
        Ok(list) if list.is_empty() => Output::ok(
            "no pending invitation; `tomi-relay admin invite --new` creates one and shows its token\n",
        ),
        Ok(list) => {
            let mut out = String::new();
            for i in &list {
                out.push_str(&format!(
                    "invitation {}  created {}\n",
                    i.short_id(),
                    ts_format(i.created_at)
                ));
            }
            out.push_str(
                "a token is shown once, when it is created; `--new` adds an invitation, `--rotate` replaces them all\n",
            );
            Output::ok(out)
        }
    }
}

async fn revoke(st: &AppState, requested: Option<&str>, target: &str) -> Output {
    let target = target.trim().to_ascii_lowercase();
    if target.len() < 8 || !target.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Output::fail(
            2,
            "give a device id or a prefix of at least 8 hexadecimal characters",
        );
    }
    let space = match pick_space(st, requested) {
        Ok(s) => s,
        Err(out) => return out,
    };
    let now = st.now();
    let t = target.clone();
    let done = space
        .db
        .write(move |tx| {
            let mut fx = Effects::default();
            let all = members(tx)?;
            let matching: Vec<_> = all.iter().filter(|d| d.device_id.starts_with(&t)).collect();
            let device = match matching.as_slice() {
                [] => return Ok((Err(Output::fail(1, "no such device")), fx)),
                [one] => (*one).clone(),
                _ => return Ok((Err(Output::fail(2, "the prefix is ambiguous")), fx)),
            };
            if all.len() <= 1 {
                return Ok((
                    Err(Output::fail(
                        2,
                        "this is the last member: use `tomi-relay admin reset`",
                    )),
                    fx,
                ));
            }
            let seq = revoke_device(tx, now, &device.device_id, Via::Admin, &mut fx)?;
            tx.execute("UPDATE space SET rotation_required = 1 WHERE id = 1", [])?;
            Ok((Ok((device.device_id, device.name, seq)), fx))
        })
        .await;
    match done {
        Err(e) => Output::from_error(&e),
        Ok((Err(out), _)) => out,
        Ok((Ok((id, name, seq)), fx)) => {
            // The sockets of the device are cut with 4003 (§3.4).
            st.apply(&space, fx).await;
            Output::ok(format!(
                "revoked {id} ({name}), event seq {}; a member must now rotate the space key\n",
                seq.unwrap_or(0)
            ))
        }
    }
}

/// `reset`: deletes one space (its database, its blobs, its routing entries, its quota) and
/// nothing else. The other spaces are not touched, not even their connections. A new
/// invitation is created only when no space is left, so that the operator of a relay that has
/// just been emptied gets a token to start again (the behaviour of the one-space relay). The invitations that are
/// already pending are kept: they may have been made for other people.
async fn reset(st: &AppState, requested: Option<&str>, confirm: Option<&str>) -> Output {
    // A relay without a space: nothing to delete, and a fresh token to start with.
    if requested.is_none() && st.spaces.is_empty() {
        return match install::create(st.spaces.routing(), st.now()) {
            Ok(token) => Output::ok(format!("{token}\n")),
            Err(e) => Output::from_error(&e),
        };
    }
    let space = match pick_space(st, requested) {
        Ok(s) => s,
        Err(out) => return out,
    };
    let id = space.text_id();
    let given = confirm.map(|c| c.trim().to_ascii_lowercase());
    let ok = given
        .as_deref()
        .is_some_and(|g| g.len() >= 8 && (g == id || g == &id[..8]));
    if !ok {
        return Output::fail(
            2,
            format!(
                "confirmation needed: retype the first 8 characters of the space id ({})",
                &id[..8]
            ),
        );
    }

    // No creation, registration or other reset meanwhile. The deletion itself (unregistering,
    // `4004` to the WebSockets of the space, directory, quota, invitations) is the one of
    // `DELETE /v1/space` (`AppState::delete_space`).
    // A reset by hand writes no receipt (§4.4.2): the devices of the space get no proof and do
    // not erase themselves.
    let registry = st.spaces.lock_registry().await;
    let mut note = "";
    match st.delete_space(&space, registry, None).await {
        DeleteOutcome::Deleted => {}
        DeleteOutcome::DeletedWithoutReceipt => {
            note = " (routing.db could not be cleaned: its quota and invitation rows stay)";
        }
        DeleteOutcome::NotDeleted => {
            return Output::fail(
                1,
                "nothing could be deleted (see the logs); the space is registered again",
            )
        }
        DeleteOutcome::NotDurable => {
            return Output::fail(
                1,
                "the deletion could not be made durable (see the logs); the space is unavailable until the next start",
            )
        }
        DeleteOutcome::Unknown => {
            return Output::fail(
                1,
                "the deletion ended in an unknown state (see the logs); the space is unavailable until the next start",
            )
        }
        DeleteOutcome::NotRegistered => return Output::fail(1, "no such space"),
    }
    tracing::warn!(space = space.short_id(), "space reset by the administrator");
    let mut out = if st.spaces.is_empty() {
        match install::create(st.spaces.routing(), st.now()) {
            Ok(token) => Output::ok(format!("{token}\n")),
            Err(e) => Output::fail(
                1,
                format!(
                    "the space was deleted but no invitation could be created: {}",
                    e.code.as_str()
                ),
            ),
        }
    } else {
        Output::ok(format!("space {} deleted\n", space.short_id()))
    };
    if !note.is_empty() {
        // On stderr: stdout may be the token alone, which a script reads.
        out.stderr = format!("space {} deleted{note}", space.short_id());
    }
    out
}
/// The administration socket of a running relay (Unix only; on Windows the commands that
/// change something need the relay stopped).
#[cfg(unix)]
pub mod socket {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    use super::{run, Cmd, Output};
    use crate::state::AppState;

    /// `<data_dir>/admin.sock`.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("admin.sock")
    }

    /// Listens on the socket (mode 0600) and serves commands until the process ends.
    pub fn spawn(state: AppState) -> std::io::Result<()> {
        let p = path(&state.cfg.data_dir);
        let _ = std::fs::remove_file(&p);
        let listener = UnixListener::bind(&p)?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let st = state.clone();
                tokio::spawn(async move {
                    let _ = serve_one(st, stream).await;
                });
            }
        });
        Ok(())
    }

    async fn serve_one(st: AppState, stream: UnixStream) -> std::io::Result<()> {
        let (rd, mut wr) = stream.into_split();
        let mut line = String::new();
        BufReader::new(rd.take(8192)).read_line(&mut line).await?;
        let out = match serde_json::from_str::<Cmd>(&line) {
            Ok(cmd) => run(&st, cmd).await,
            Err(_) => Output {
                exit: 2,
                stdout: String::new(),
                stderr: "unreadable command".into(),
            },
        };
        wr.write_all(format!("{}\n", serde_json::to_string(&out).unwrap_or_default()).as_bytes())
            .await?;
        wr.shutdown().await
    }

    /// Sends a command to a running relay. `None` when no relay answers on the socket.
    pub async fn send(data_dir: &Path, cmd: &Cmd) -> Option<Output> {
        let stream = UnixStream::connect(path(data_dir)).await.ok()?;
        let (rd, mut wr) = stream.into_split();
        wr.write_all(format!("{}\n", serde_json::to_string(cmd).ok()?).as_bytes())
            .await
            .ok()?;
        let mut line = String::new();
        BufReader::new(rd).read_line(&mut line).await.ok()?;
        serde_json::from_str(&line).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_as_json() {
        let c = Cmd::Reset {
            confirm: Some("6b5167fa".into()),
            space: None,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, r#"{"cmd":"reset","confirm":"6b5167fa"}"#);
        assert_eq!(serde_json::from_str::<Cmd>(&s).unwrap(), c);
        assert!(needs_exclusive_access(&c));
        let q = Cmd::Quota {
            space: Some("6b5167fa".into()),
            value: "none".into(),
        };
        let s = serde_json::to_string(&q).unwrap();
        assert_eq!(s, r#"{"cmd":"quota","space":"6b5167fa","value":"none"}"#);
        assert_eq!(serde_json::from_str::<Cmd>(&s).unwrap(), q);
        // An older client that knows no `space` and no `new` is still understood.
        assert_eq!(
            serde_json::from_str::<Cmd>(r#"{"cmd":"invite","rotate":true}"#).unwrap(),
            Cmd::Invite {
                rotate: true,
                new: false
            }
        );
        let status = Cmd::Status {
            json: true,
            space: None,
        };
        assert!(!needs_exclusive_access(&status));
        assert!(!needs_exclusive_access(&Cmd::Spaces { json: false }));
        assert!(needs_exclusive_access(&q));
        assert!(needs_exclusive_access(&Cmd::Invite {
            rotate: true,
            new: false
        }));
        assert!(needs_exclusive_access(&Cmd::Invite {
            rotate: false,
            new: true
        }));
        assert!(!needs_exclusive_access(&Cmd::Invite {
            rotate: false,
            new: false
        }));
    }
}
