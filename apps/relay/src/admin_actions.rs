//! The actions of the web panel (`admin_web.rs` is the access, this is what the administrator can
//! then do).
//!
//! * **Step-up** (`POST api/stepup/options`, `POST api/actions`): what destroys or creates a right
//!   (remove devices, delete a space, create or replace invitations) needs a passkey assertion
//!   **tied to that action**. The server resolves the action to full identifiers, hashes it
//!   (`SHA-256("tomi-admin-action-v1" 0x00 canonical JSON)`), and issues the challenge
//!   `SHA-256(nonce || hash)`, single use, 2 minutes, tied to the session. At execution it
//!   resolves and hashes again from the request, so an assertion made for another action, another
//!   session or an earlier challenge is refused; the target is checked again in the write
//!   transaction. A session alone (a stolen cookie) can do none of this.
//! * **Session only** (what can be undone at once): the quota of a space, invalidating the pending
//!   invitations, closing every session.
//! * **Ceilings**, per relay and in memory, on the successful actions only; 5 refused step-ups
//!   close the session (never the passkey).
//! * Everything else (passkeys, policy, path, the trash restore and purge) is the CLI only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::admin;
use crate::admin_audit::{self, kind as audit};
use crate::admin_web::{
    authorize, b64_field, blocking, check_origin, clear_cookie_value, credentials, json_response,
    no_content, read_json, session_cookie, short, throttle, AdminWeb, JournalKind, Taken, MAX_KEYS,
    MAX_REFUSALS, STEPUP_TTL_MS,
};
use crate::db::lock;
use crate::error::{ApiError, Code};
use crate::install;
use crate::revoke::{revoke_device, Via};
use crate::spaces::{DeleteOutcome, Space};
use crate::state::{AppState, Effects};
use crate::store::{self, members};
use crate::webauthn::{self, Expected};
use crate::wire::{b64_encode, sha256_parts, ts_format};

// ===== ceilings ===============================================================================

/// What a ceiling counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    SpaceDeletion,
    DeviceRemoval,
    Invitation,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::SpaceDeletion, Kind::DeviceRemoval, Kind::Invitation];

    /// How many in the window: 2 deletions of a space a day, 10 removals of a device an hour, 5
    /// invitations an hour. Fixed in the code (a setting could only lower the guard).
    pub fn max(self) -> u32 {
        match self {
            Kind::SpaceDeletion => 2,
            Kind::DeviceRemoval => 10,
            Kind::Invitation => 5,
        }
    }

    pub fn window_ms(self) -> i64 {
        match self {
            Kind::SpaceDeletion => 24 * 3_600_000,
            Kind::DeviceRemoval | Kind::Invitation => 3_600_000,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::SpaceDeletion => "space_deletions",
            Kind::DeviceRemoval => "device_removals",
            Kind::Invitation => "invitations",
        }
    }

    /// The command of the CLI that does the same without the ceiling.
    pub fn cli(self) -> &'static str {
        match self {
            Kind::SpaceDeletion => "tomi-relay admin reset --space <id>",
            Kind::DeviceRemoval => "tomi-relay admin revoke <device> --space <id>",
            Kind::Invitation => "tomi-relay admin invite --new",
        }
    }
}

struct Event {
    id: u64,
    kind: Kind,
    at: i64,
    n: u32,
}

/// The ceilings of the actions, kept in memory for the whole relay (not per session or address:
/// opening another session gives no more). A restart resets them, which a stolen session cannot
/// provoke.
#[derive(Default)]
pub struct Ceilings {
    next: AtomicU64,
    events: Mutex<Vec<Event>>,
}

impl Ceilings {
    fn used_in(list: &[Event], kind: Kind, now: i64) -> u32 {
        list.iter()
            .filter(|e| e.kind == kind && now - e.at < kind.window_ms())
            .map(|e| e.n)
            .sum()
    }

    fn retry_after(list: &[Event], kind: Kind, now: i64) -> u64 {
        let oldest = list
            .iter()
            .filter(|e| e.kind == kind && now - e.at < kind.window_ms())
            .map(|e| e.at)
            .min()
            .unwrap_or(now);
        u64::try_from((oldest + kind.window_ms() - now).max(1000) / 1000).unwrap_or(1)
    }

    /// Would `n` more fit? `Err(seconds to wait)` if not.
    pub fn check(&self, kind: Kind, n: u32, now: i64) -> Result<(), u64> {
        let list = lock(&self.events);
        if Self::used_in(&list, kind, now) + n > kind.max() {
            Err(Self::retry_after(&list, kind, now))
        } else {
            Ok(())
        }
    }

    /// Takes `n` places, or `Err(seconds to wait)`; the id gives them back ([`Self::release`]) if
    /// the action then fails: only the successful actions count.
    pub fn reserve(&self, kind: Kind, n: u32, now: i64) -> Result<u64, u64> {
        let mut list = lock(&self.events);
        list.retain(|e| now - e.at < e.kind.window_ms());
        if Self::used_in(&list, kind, now) + n > kind.max() {
            return Err(Self::retry_after(&list, kind, now));
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        list.push(Event {
            id,
            kind,
            at: now,
            n,
        });
        Ok(id)
    }

    pub fn release(&self, id: u64) {
        lock(&self.events).retain(|e| e.id != id);
    }

    /// What `api/about` says: for each ceiling, the maximum, the window and what is used.
    pub fn json(&self, now: i64) -> Value {
        let list = lock(&self.events);
        let mut v = serde_json::Map::new();
        for k in Kind::ALL {
            v.insert(
                k.word().to_owned(),
                json!({
                    "max": k.max(),
                    "window_secs": k.window_ms() / 1000,
                    "used": Self::used_in(&list, k, now),
                }),
            );
        }
        Value::Object(v)
    }
}

fn ceiling_error(kind: Kind, retry_secs: u64) -> ApiError {
    ApiError::rate_limited(retry_secs)
        .with("reason", "ceiling")
        .with("ceiling", kind.word())
        .with("max", kind.max())
        .with("window_secs", kind.window_ms() / 1000)
        .with("cli", kind.cli())
}

// ===== errors =================================================================================

fn bad(reason: &str) -> ApiError {
    ApiError::bad_request("The request is not valid.").with("reason", reason)
}

fn conflict(reason: &str) -> ApiError {
    let mut e = ApiError::new(Code::DeviceSetMismatch).detail("The target is not what it was.");
    e.status = StatusCode::CONFLICT;
    e.with("reason", reason)
}

/// The target was changed or removed since it was shown ("already done elsewhere").
fn target_changed() -> ApiError {
    conflict("target_changed")
}

// ===== the actions ============================================================================

enum Resolved {
    Revoke { space: Arc<Space>, ids: Vec<String> },
    Delete { space: Arc<Space> },
    Invite { rotate: bool },
}

struct Prepared {
    resolved: Resolved,
    /// The hash the challenge is tied to.
    hash: [u8; 32],
    /// What the sheet of the page shows: built here, from the data of the relay.
    summary: Value,
    kind: Kind,
    count: u32,
}

fn action_hash(canonical: &Value) -> [u8; 32] {
    sha256_parts(&[
        b"tomi-admin-action-v1",
        &[0u8],
        canonical.to_string().as_bytes(),
    ])
}

fn pick(st: &AppState, prefix: Option<&str>) -> Result<Arc<Space>, ApiError> {
    let prefix = prefix.ok_or_else(|| bad("space"))?;
    admin::pick_space(st, Some(prefix)).map_err(|out| {
        if out.exit == 2 {
            bad("space")
        } else {
            target_changed()
        }
    })
}

/// Resolves an action of the page (short ids) to full identifiers, checks what can be checked,
/// and builds the summary and the hash. Used by `stepup/options` and again by `actions`.
async fn prepare(st: &AppState, action: &Value) -> Result<Prepared, ApiError> {
    let kind = action
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("type"))?;
    match kind {
        "revoke_devices" => {
            let space = pick(st, action.get("space_id").and_then(Value::as_str))?;
            let wanted: Vec<String> = action
                .get("device_ids")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.trim().to_ascii_lowercase()))
                        .collect()
                })
                .unwrap_or_default();
            let given = action
                .get("device_ids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if wanted.is_empty()
                || wanted.len() != given
                || wanted.len() > 16
                || wanted
                    .iter()
                    .any(|w| w.len() < 8 || !w.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(bad("device_ids"));
            }
            let all = space.db.read(|tx| members(tx)).await?;
            let mut ids: Vec<String> = Vec::new();
            let mut shown: Vec<Value> = Vec::new();
            for w in &wanted {
                let hits: Vec<_> = all.iter().filter(|d| d.device_id.starts_with(w)).collect();
                let [d] = hits.as_slice() else {
                    return Err(target_changed());
                };
                if !ids.contains(&d.device_id) {
                    ids.push(d.device_id.clone());
                    shown.push(json!({
                        "id": short(&d.device_id),
                        "name": d.name,
                        "platform": d.platform,
                        "app_version": d.app_version,
                        "last_seen_at": d.last_seen_at.map(ts_format),
                    }));
                }
            }
            ids.sort();
            if all.len() <= ids.len() {
                return Err(conflict("last_device"));
            }
            let canonical = json!({
                "type": "revoke_devices",
                "space_id": space.text_id(),
                "device_ids": ids,
            });
            let count = u32::try_from(ids.len()).unwrap_or(u32::MAX);
            Ok(Prepared {
                summary: json!({
                    "type": "revoke_devices",
                    "space": space.short_id(),
                    "devices": shown,
                    "members_after": all.len() - ids.len(),
                }),
                hash: action_hash(&canonical),
                resolved: Resolved::Revoke { space, ids },
                kind: Kind::DeviceRemoval,
                count,
            })
        }
        "delete_space" => {
            let space = pick(st, action.get("space_id").and_then(Value::as_str))?;
            let given = action
                .get("confirm")
                .and_then(Value::as_str)
                .map(|c| c.trim().to_ascii_lowercase());
            if given.as_deref() != Some(space.short_id().as_str()) {
                return Err(bad("confirm"));
            }
            let (count, used) = space
                .db
                .read(|tx| Ok((members(tx)?.len(), store::used_bytes(tx)?)))
                .await?;
            let canonical = json!({ "type": "delete_space", "space_id": space.text_id() });
            Ok(Prepared {
                summary: json!({
                    "type": "delete_space",
                    "space": space.short_id(),
                    "members": count,
                    "used_bytes": used,
                    "trash_days": crate::spaces::TRASH_TTL_MS / 86_400_000,
                }),
                hash: action_hash(&canonical),
                resolved: Resolved::Delete { space },
                kind: Kind::SpaceDeletion,
                count: 1,
            })
        }
        "create_invitation" => {
            let rotate = match action.get("mode").and_then(Value::as_str) {
                Some("new") => false,
                Some("rotate") => true,
                _ => return Err(bad("mode")),
            };
            let pending = if rotate {
                blocking(st, |st| Ok(install::list(st.spaces.routing())?.len())).await?
            } else {
                0
            };
            let canonical = json!({
                "type": "create_invitation",
                "mode": if rotate { "rotate" } else { "new" },
            });
            Ok(Prepared {
                summary: json!({
                    "type": "create_invitation",
                    "mode": if rotate { "rotate" } else { "new" },
                    "replaced": pending,
                }),
                hash: action_hash(&canonical),
                resolved: Resolved::Invite { rotate },
                kind: Kind::Invitation,
                count: 1,
            })
        }
        _ => Err(bad("type")),
    }
}

/// The live session of a request: its hash and the number of the passkey that opened it.
pub(crate) fn identity(
    st: &AppState,
    web: &AdminWeb,
    headers: &HeaderMap,
) -> Result<([u8; 32], i64), ApiError> {
    let token = session_cookie(headers).ok_or_else(ApiError::unauthorized)?;
    web.session_identity(&token, st.now())
        .ok_or_else(ApiError::unauthorized)
}

async fn device_key_required(st: &AppState) -> Result<bool, ApiError> {
    st.spaces
        .routing()
        .read(crate::admin_web::device_key_required)
        .await
}

/// `POST api/stepup/options` `{action}`: the challenge of one action, and the summary the sheet
/// shows while the passkey is awaited (the authenticator shows only the name of the site).
pub async fn stepup_options(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    let (session, _key) = identity(st, web, req.headers())?;
    throttle(st, &req)?;
    let body = read_json(req).await?;
    let action = body.get("action").ok_or_else(|| bad("action"))?;
    let p = prepare(st, action).await?;
    let now = st.now();
    // Before any challenge: a ceiling that is reached asks for no passkey at all.
    web.ceilings
        .check(p.kind, p.count, now)
        .map_err(|retry| ceiling_error(p.kind, retry))?;
    let bound_only =
        matches!(p.resolved, Resolved::Delete { .. }) && device_key_required(st).await?;
    let allowed: Vec<Value> = credentials(st)
        .await?
        .iter()
        .filter(|c| !bound_only || !c.backup_eligible)
        .map(|c| json!({ "type": "public-key", "id": b64_encode(&c.credential_id) }))
        .collect();
    if allowed.is_empty() {
        return Err(conflict("device_key_required"));
    }
    let challenge = web.issue_stepup(session, p.hash, now)?;
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "publicKey": {
                "challenge": b64_encode(&challenge),
                "rpId": web.rp_id(),
                "allowCredentials": allowed,
                "userVerification": "required",
                "timeout": STEPUP_TTL_MS,
            },
            "summary": p.summary,
            "expires_in_secs": STEPUP_TTL_MS / 1000,
        }),
    ))
}

/// A step-up that was refused: counted in the session, written to the logs; the fifth closes the
/// session. The answer never says why.
async fn refused(st: &AppState, web: &AdminWeb, session: &[u8; 32], why: &str) -> ApiError {
    let now = st.now();
    let (n, closed) = web.note_refusal(session);
    tracing::info!("admin web: step-up refused ({why}) [{n}/{MAX_REFUSALS}]");
    web.record(JournalKind::StepUpRefused, None, now);
    admin_audit::record(st, audit::STEPUP_REFUSED, None, None).await;
    let mut e = ApiError::unauthorized()
        .with("reason", "refused")
        .with("refusals", n)
        .with("max_refusals", MAX_REFUSALS);
    if closed {
        web.record(JournalKind::SessionClosedByRefusals, None, now);
        admin_audit::record(st, audit::SESSION_CLOSED_BY_REFUSALS, None, None).await;
        tracing::warn!("admin web: five step-ups refused, the session is closed");
        e = e.with("session_closed", true);
        if let Ok(v) = HeaderValue::from_str(&clear_cookie_value()) {
            e.headers.push((header::SET_COOKIE, v));
        }
    }
    e
}

/// `POST api/actions` `{action, assertion}`.
pub async fn run_action(st: &AppState, web: &AdminWeb, req: Request) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    let (session, _session_key) = identity(st, web, req.headers())?;
    throttle(st, &req)?;
    let body = read_json(req).await?;
    let action = body.get("action").ok_or_else(|| bad("action"))?;
    let assertion = body.get("assertion").ok_or_else(|| bad("assertion"))?;
    let id = b64_field(assertion, "id")?;
    let client_data = b64_field(assertion, "clientDataJSON")?;
    let auth_data = b64_field(assertion, "authenticatorData")?;
    let signature = b64_field(assertion, "signature")?;
    let now = st.now();

    // The challenge works once, whatever follows.
    let taken = webauthn::claimed_challenge(&client_data)
        .map_or(Taken::Unknown, |c| web.take_stepup(&c, now));
    let pending = match taken {
        Taken::Found(p) => p,
        Taken::Expired => {
            return Err(ApiError::unauthorized().with("reason", "expired"));
        }
        Taken::Unknown => return Err(refused(st, web, &session, "challenge").await),
    };
    let same_session = bool::from(pending.session.as_slice().ct_eq(session.as_slice()));
    if !same_session || pending.epoch != web.epoch() {
        return Err(refused(st, web, &session, "session").await);
    }
    let p = prepare(st, action).await?;
    if !bool::from(p.hash.as_slice().ct_eq(pending.action.as_slice())) {
        return Err(refused(st, web, &session, "action").await);
    }

    let creds = credentials(st).await?;
    let mut found = None;
    for c in &creds {
        if id.len() == c.credential_id.len()
            && bool::from(id.as_slice().ct_eq(c.credential_id.as_slice()))
        {
            found = Some(c);
        }
    }
    let Some(cred) = found else {
        return Err(refused(st, web, &session, "credential").await);
    };
    let needs_bound =
        matches!(p.resolved, Resolved::Delete { .. }) && device_key_required(st).await?;
    if needs_bound && cred.backup_eligible {
        return Err(refused(st, web, &session, "policy").await);
    }
    let expected = Expected {
        origin: web.origin(),
        rp_id: web.rp_id(),
        challenge: &pending.challenge,
    };
    let new_count = match webauthn::verify_assertion(
        &expected,
        &cred.public_key,
        cred.sign_count,
        &client_data,
        &auth_data,
        &signature,
    ) {
        Ok(n) => n,
        Err(why) => return Err(refused(st, web, &session, why.as_str()).await),
    };
    let (num, from) = (cred.num, cred.sign_count);
    let signed_by = cred.num;
    let advanced = st
        .spaces
        .routing()
        .write(move |tx| {
            let n = tx.execute(
                "UPDATE admin_web_credentials SET sign_count = ?1, last_used_at = ?2
                 WHERE num = ?3 AND sign_count = ?4",
                rusqlite::params![i64::from(new_count), now, num, i64::from(from)],
            )?;
            Ok(n == 1)
        })
        .await?;
    if !advanced {
        return Err(refused(st, web, &session, "counter").await);
    }

    // The ceiling is taken now and given back if the action does not happen.
    let ticket = web
        .ceilings
        .reserve(p.kind, p.count, now)
        .map_err(|retry| ceiling_error(p.kind, retry))?;
    // The audit names the key that signed this step-up, not the one that opened the session.
    let done = execute(st, web, p.resolved, signed_by).await;
    if done.is_err() {
        web.ceilings.release(ticket);
    }
    done
}

async fn execute(
    st: &AppState,
    web: &AdminWeb,
    resolved: Resolved,
    key_num: i64,
) -> Result<Response, ApiError> {
    match resolved {
        Resolved::Revoke { space, ids } => {
            let now = st.now();
            let wanted = ids.clone();
            let removed = space
                .db
                .write(move |tx| {
                    let mut fx = Effects::default();
                    // The target is checked again where it is written.
                    let all = members(tx)?;
                    if wanted
                        .iter()
                        .any(|w| !all.iter().any(|d| &d.device_id == w))
                    {
                        return Err(target_changed());
                    }
                    if all.len() <= wanted.len() {
                        return Err(conflict("last_device"));
                    }
                    let mut names = Vec::new();
                    for w in &wanted {
                        revoke_device(tx, now, w, Via::Admin, &mut fx)?;
                        let name = all
                            .iter()
                            .find(|d| &d.device_id == w)
                            .map(|d| d.name.clone())
                            .unwrap_or_default();
                        names.push((w.clone(), name));
                    }
                    tx.execute("UPDATE space SET rotation_required = 1 WHERE id = 1", [])?;
                    Ok(((names, all.len() - wanted.len()), fx))
                })
                .await;
            let ((names, left), fx) = removed?;
            st.apply(&space, fx).await;
            web.record(JournalKind::DevicesRemoved, None, now);
            for (id, _) in &names {
                admin_audit::record(st, audit::DEVICE_REMOVED, Some(id.clone()), Some(key_num))
                    .await;
            }
            tracing::warn!(
                space = space.short_id(),
                count = names.len(),
                "admin web: devices removed"
            );
            Ok(json_response(
                StatusCode::OK,
                &json!({
                    "ok": true,
                    "type": "revoke_devices",
                    "space": space.short_id(),
                    "removed": names.iter().map(|(id, name)| json!({ "id": short(id), "name": name })).collect::<Vec<_>>(),
                    "members": left,
                    "rotation_required": true,
                }),
            ))
        }
        Resolved::Delete { space } => {
            let now = st.now();
            let registry = st.spaces.lock_registry().await;
            if st.spaces.get(&space.id).is_none() {
                return Err(target_changed());
            }
            let outcome = st.delete_space_keep(&space, registry, None, true).await;
            match outcome {
                DeleteOutcome::Deleted | DeleteOutcome::DeletedWithoutReceipt => {}
                DeleteOutcome::NotRegistered => return Err(target_changed()),
                DeleteOutcome::NotDeleted => {
                    return Err(ApiError::internal("the space could not be deleted"))
                }
                DeleteOutcome::NotDurable | DeleteOutcome::Unknown => {
                    return Err(ApiError::unavailable())
                }
            }
            web.record(JournalKind::SpaceDeleted, None, now);
            admin_audit::record(
                st,
                audit::SPACE_DELETED,
                Some(space.short_id()),
                Some(key_num),
            )
            .await;
            tracing::warn!(
                space = space.short_id(),
                "admin web: a space was deleted (trash, 7 days)"
            );
            Ok(json_response(
                StatusCode::OK,
                &json!({
                    "ok": true,
                    "type": "delete_space",
                    "space": space.short_id(),
                    "trash": {
                        "deleted_at": ts_format(now),
                        "purge_after": ts_format(now + crate::spaces::TRASH_TTL_MS),
                        "days": crate::spaces::TRASH_TTL_MS / 86_400_000,
                    },
                    "restore": format!("tomi-relay admin trash restore --space {}", space.short_id()),
                }),
            ))
        }
        Resolved::Invite { rotate } => {
            let token = blocking(st, move |st| admin::issue_invitation(st, rotate)).await?;
            let now = st.now();
            let (jk, ak) = if rotate {
                (
                    JournalKind::InvitationsReplaced,
                    audit::INVITATIONS_REPLACED,
                )
            } else {
                (JournalKind::InvitationCreated, audit::INVITATION_CREATED)
            };
            web.record(jk, None, now);
            admin_audit::record(st, ak, None, Some(key_num)).await;
            let list = invitations(st).await?;
            // The token (and the link that carries it) is in this answer and nowhere else, ever.
            let link = crate::admin_web::creation_link(web.origin(), &token);
            Ok(json_response(
                StatusCode::OK,
                &json!({ "ok": true, "type": "create_invitation", "token": token, "link": link, "invitations": list }),
            ))
        }
    }
}

pub(crate) async fn invitations(st: &AppState) -> Result<Vec<Value>, ApiError> {
    blocking(st, |st| {
        Ok(install::list(st.spaces.routing())?
            .iter()
            .map(|i| json!({ "id": i.short_id(), "created_at": ts_format(i.created_at) }))
            .collect())
    })
    .await
}

// ===== session only ===========================================================================

fn confirmed(body: &Value) -> bool {
    body.get("confirm").and_then(Value::as_bool) == Some(true)
}

/// The ids (8 characters) of the spaces that a new default quota would block (they would hold
/// more than the quota; their writes are refused with `relay_full`, nothing is deleted).
pub(crate) async fn blocked_by_default(
    st: &AppState,
    value: Option<Option<u64>>,
) -> Result<Vec<String>, ApiError> {
    let effective = match value {
        None => st.cfg.configured_space_quota(),
        Some(v) => v,
    };
    let Some(limit) = effective else {
        return Ok(Vec::new());
    };
    let mut blocked = Vec::new();
    for s in st.spaces.list() {
        if st.spaces.quota_override(&s.id).is_some() {
            continue;
        }
        if s.db.read(|tx| store::used_bytes(tx)).await? > limit {
            blocked.push(s.short_id());
        }
    }
    Ok(blocked)
}

fn confirmation_required(blocked: &[String]) -> ApiError {
    conflict("confirmation_required").with("blocked", blocked.to_vec())
}

/// `POST api/spaces/quota` `{space_id, value, confirm?}`: the quota of one space (a number of
/// bytes, `0` for no limit, `null` for the default). Refused with `409 confirmation_required` if
/// the space would be blocked, until `confirm: true`.
pub async fn set_space_quota(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    let (_, session_key) = identity(st, web, req.headers())?;
    throttle(st, &req)?;
    let body = read_json(req).await?;
    let space = pick(st, body.get("space_id").and_then(Value::as_str))?;
    let value = body.get("value").cloned().ok_or_else(|| bad("value"))?;
    let parsed = admin::parse_default_quota(&json!({ "default_bytes": value }))?;
    let used = space.db.read(|tx| store::used_bytes(tx)).await?;
    let limit = match parsed {
        None => st.spaces.default_space_quota(&st.cfg).0,
        Some(v) => v,
    };
    if limit.is_some_and(|l| used > l) && !confirmed(&body) {
        return Err(confirmation_required(&[space.short_id()]));
    }
    st.spaces.set_quota_override(&space.id, parsed)?;
    let now = st.now();
    let (jk, bytes) = match parsed {
        None => (JournalKind::SpaceQuotaDefault, None),
        Some(None) => (JournalKind::SpaceQuotaUnlimited, None),
        Some(Some(b)) => (JournalKind::SpaceQuotaSet, Some(b)),
    };
    web.record(jk, bytes, now);
    admin_audit::record(
        st,
        audit::QUOTA_SET,
        Some(space.short_id()),
        Some(session_key),
    )
    .await;
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "space": space.short_id(),
            "max_bytes": st.spaces.space_max(&space.id, &st.cfg),
            "quota_source": st.spaces.quota_source(&space.id),
        }),
    ))
}

/// The confirmation of a new default quota (`api/quota`).
pub(crate) async fn check_default_quota(
    st: &AppState,
    body: &Value,
    value: Option<Option<u64>>,
) -> Result<(), ApiError> {
    let blocked = blocked_by_default(st, value).await?;
    if blocked.is_empty() || confirmed(body) {
        Ok(())
    } else {
        Err(confirmation_required(&blocked))
    }
}

/// `POST api/invitations/cancel`: invalidates the pending invitations without making another
/// one (it only removes a right, so the session is enough).
pub async fn cancel_invitations(
    st: &AppState,
    web: &AdminWeb,
    req: Request,
) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    let (_, session_key) = identity(st, web, req.headers())?;
    throttle(st, &req)?;
    let n = blocking(st, |st| install::clear(st.spaces.routing())).await?;
    web.record(JournalKind::InvitationsCancelled, None, st.now());
    admin_audit::record(st, audit::INVITATIONS_CANCELLED, None, Some(session_key)).await;
    Ok(json_response(
        StatusCode::OK,
        &json!({ "cancelled": n, "invitations": invitations(st).await? }),
    ))
}

/// `POST api/sessions/close-all`: closes every session, this one included.
pub async fn close_all(st: &AppState, web: &AdminWeb, req: Request) -> Result<Response, ApiError> {
    check_origin(web, req.headers())?;
    let (_, session_key) = identity(st, web, req.headers())?;
    throttle(st, &req)?;
    web.close_all_sessions();
    web.record(JournalKind::SessionsClosed, None, st.now());
    admin_audit::record(st, audit::SESSIONS_CLOSED, None, Some(session_key)).await;
    let mut resp = no_content();
    if let Ok(v) = HeaderValue::from_str(&clear_cookie_value()) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    Ok(resp)
}

/// `GET api/keys`: the passkeys (number, generated label, kind, dates), the policy, and which key
/// opened this session. Read only: keys are enrolled and removed with the CLI.
pub async fn keys(st: &AppState, web: &AdminWeb, req: Request) -> Result<Response, ApiError> {
    let (_, this_key) = identity(st, web, req.headers())?;
    let list: Vec<Value> = credentials(st)
        .await?
        .iter()
        .map(|c| {
            json!({
                "number": c.num,
                "label": c.label(),
                "device_bound": !c.backup_eligible,
                "created_at": ts_format(c.created_at),
                "last_used_at": c.last_used_at.map(ts_format),
                "this_session": c.num == this_key,
            })
        })
        .collect();
    let bound = device_key_required(st).await?;
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "keys": list,
            "max": MAX_KEYS,
            "policy": { "space_deletion": if bound { "device-key" } else { "any-key" } },
        }),
    ))
}

/// `GET api/trash`: what the trash holds (short ids). Read only: restoring and purging are CLI.
pub async fn trash(st: &AppState, web: &AdminWeb, req: Request) -> Result<Response, ApiError> {
    authorize(st, web, req.headers())?;
    let rows = st
        .spaces
        .routing()
        .read(|tx| crate::trash::rows(tx))
        .await?;
    Ok(json_response(
        StatusCode::OK,
        &json!({
            "trash": rows.iter().map(|r| crate::trash::row_json(r, false)).collect::<Vec<_>>(),
            "days": crate::spaces::TRASH_TTL_MS / 86_400_000,
        }),
    ))
}
