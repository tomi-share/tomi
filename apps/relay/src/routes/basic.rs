//! `GET /`, `GET /healthz`, `GET /v1/hello`, `POST /v1/space`, `GET /v1/space`,
//! `DELETE /v1/space`, `POST /v1/space/receipt`.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY};
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use tomi_core::{device_id_from_ed25519, DeviceCert};

use super::json;
use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, Auth, Gate, Rl};
use crate::input::json_body;
use crate::spaces::{DeleteOutcome, Route};
use crate::state::AppState;
use crate::store::{self, add_event, device_by_id, device_record, members};
use crate::wire::{ts_format, uuid_format};
use crate::{install, wire};

/// The landing page served at `GET /`: a small, self-contained, script-free HTML page. It is a
/// courtesy for a human who opens the relay's URL in a browser; it reveals nothing the public
/// endpoints do not already (`GET /v1/hello`, `/healthz`), carries no space or device data, and
/// runs no script (strict CSP, same posture as the installation page of §4.12).
const LANDING_PAGE: &str = include_str!("landing.html");

/// The page behind `GET /create` and `GET /create/<anything>`: the link that creates a space
/// (`https://<relay>/create#<installation token>`) opened in a browser by someone who does not
/// have the app. The token is in the fragment, which a browser never sends; the route does not
/// even read the path (so a `/create/<token>` form reaches nothing either), and the page is the
/// same for everybody: static, no script, nothing third-party. It validates nothing.
const CREATE_PAGE: &str = include_str!("create.html");

/// The landing page with the host of `public_url` in its footer (escaped: the configuration is
/// not trusted to be free of markup).
fn landing_page(public_url: &str) -> String {
    let mut host = String::new();
    for c in wire::host_of(public_url).chars() {
        match c {
            '<' => host.push_str("&lt;"),
            '>' => host.push_str("&gt;"),
            '&' => host.push_str("&amp;"),
            '"' => host.push_str("&quot;"),
            '\'' => host.push_str("&#39;"),
            c => host.push(c),
        }
    }
    LANDING_PAGE.replace("{{host}}", &host)
}

/// `GET /`: the landing page. No authentication, no space required (same gate as `/healthz`),
/// counted against the static rate-limit bucket.
pub async fn root(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let page = landing_page(&st.cfg.public_url);
    html_page(&st, req, page).await
}

/// `GET /create`, `GET /create/` and `GET /create/{*path}`: the same static page for all of them
/// (see [`CREATE_PAGE`]). The request is gated like any static page and otherwise ignored.
pub async fn create(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    html_page(&st, req, CREATE_PAGE.to_owned()).await
}

async fn html_page(st: &AppState, req: Request, page: String) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpStatic],
        space: false,
        auth: Auth::None,
        dev: &[],
    };
    let _ = gate(st, req, g).await?;
    let mut resp = Response::new(Body::from(page));
    let h = resp.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    // No script, no third-party anything: everything is inline, images none.
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    Ok(resp)
}

/// `GET /healthz`: 200 if every database (`routing.db` and each space) answers within two
/// seconds (§4.12).
pub async fn healthz(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpStatic],
        space: false,
        auth: Auth::None,
        dev: &[],
    };
    let _ = gate(&st, req, g).await?;
    let mut dbs = vec![st.spaces.routing().clone()];
    dbs.extend(st.spaces.list().into_iter().map(|s| s.db.clone()));
    let probe = async move {
        for db in dbs {
            db.read(|tx| Ok(tx.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?))
                .await?;
        }
        Ok::<(), ApiError>(())
    };
    match tokio::time::timeout(Duration::from_secs(2), probe).await {
        Ok(Ok(())) => Ok(json(StatusCode::OK, json!({ "status": "ok" }))),
        _ => Ok(json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "status": "unavailable" }),
        )),
    }
}

/// `GET /v1/hello`: no authentication, no check of the space. `space_initialized` says only that
/// the relay hosts at least one space: it tells nothing about the space of the caller.
pub async fn hello(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[],
        space: false,
        auth: Auth::None,
        dev: &[],
    };
    let _ = gate(&st, req, g).await?;
    let initialized = !st.spaces.is_empty();
    Ok(json(
        StatusCode::OK,
        json!({
            "service": "tomi-relay",
            "relay_version": env!("CARGO_PKG_VERSION"),
            "api_versions": [1],
            "space_initialized": initialized,
            "server_time": ts_format(st.now()),
            "limits": store::limits_json(&st.cfg),
            "features": store::FEATURES,
        }),
    ))
}

/// `GET /v1/space`.
pub async fn get_space(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, _body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let space = ctx.space()?.clone();
    let budget = st.budget(&space).await?;
    let cfg = st.cfg.clone();
    let now = ctx.now;
    let v = space
        .db
        .read(move |tx| store::space_state(tx, &cfg, now, &budget))
        .await?;
    Ok(json(StatusCode::OK, v))
}

/// Largest body of `DELETE /v1/space` and of `POST /v1/space/receipt` (a `deletion_id` is 44
/// characters): the generic limit of a JSON body is far above what these two ever need.
const MAX_DELETION_BODY: u64 = 1024;

/// `DELETE /v1/space` (§4.4.1): the last device of a space deletes the whole space.
///
/// Allowed only for an active member **while it is the only active member**; with another member
/// left it is `409 not_last_device` and nothing changes. Nothing is read: the relay deletes
/// opaque rows and files, exactly as `admin reset` does (`AppState::delete_space`).
///
/// The body is optional: empty, or `{"deletion_id": b64}` (32 bytes). It is validated before
/// anything else is done, so a bad body deletes nothing. With a `deletion_id`, a successful
/// deletion records `SHA-256(deletion_id)` as its receipt (§4.4.2).
///
/// The check and the deletion are one critical section. The registry lock is taken first: it is
/// what every writer of a device (`POST /v1/devices`), a channel or a space takes, so nobody can
/// join the space between the check and the unregistering. The check itself runs in a write
/// transaction of the space database (serialised with every other write, `DELETE /v1/me`
/// included), after the lock, so it sees the members as they are at that instant. Two devices that
/// leave at the same time are therefore ordered: either the space still has two members when the
/// check runs (`409`), or one of them left first and the other is alone (`200`). A call that
/// finds the space already deleted (a concurrent call, or `admin reset`) is answered like any
/// request for a space that is gone (§1.4): `401`, or `409 space_not_initialized` if the relay
/// has no space left; neither is a proof (the receipt is).
///
/// **Fail-closed**: `200 {"space_deleted": true}` comes out of [`DeleteOutcome::Deleted`] only.
///
/// No invitation is created for the owner to start again: the relay keeps no token in clear and
/// may not log one (§9), so the owner makes one with `tomi-relay admin invite --new` (§3.1).
pub async fn delete_space(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let (ctx, body) = gate(&st, req, Gate::bearer(&[Rl::Device])).await?;
    let mut o = json_body(
        &ctx.parts,
        body,
        st.cfg.max_json_body_bytes.min(MAX_DELETION_BODY),
        false,
    )
    .await?;
    let deletion_id = o.b64_exact("deletion_id", false, 32);
    o.finish()?;
    let receipt: Option<[u8; 32]> = match deletion_id {
        None => None,
        Some(v) => Some(
            <[u8; 32]>::try_from(v.as_slice())
                .map_err(|_| ApiError::internal("validated field of the wrong length"))?,
        ),
    };
    let me = ctx.dev()?.device_id.clone();
    // The relay is the one place that knows whether a space is gone: see the doc above.
    let gone = |st: &AppState| {
        if st.spaces.is_empty() {
            ApiError::space_not_initialized()
        } else {
            ApiError::unauthorized()
        }
    };
    let registry = st.spaces.lock_registry().await;
    // The space may have been deleted (or deleted and created again) while this call waited, or
    // a deletion that failed may have registered it again (another `Space` object for the same
    // database). The session is the proof of who is asking: a space that still knows it is the
    // one to delete, whichever object holds it now.
    let token_hash = ctx.dev()?.token_hash;
    let space = match st.spaces.route_token(&token_hash) {
        Route::Live(space) => space,
        Route::Pending => return Err(ApiError::pending()),
        Route::Unknown => return Err(gone(&st)),
    };
    space
        .db
        .write(move |tx| {
            store::require_space(tx)?;
            // Revoked or unknown since the gate looked: the same answers as the gate.
            match device_by_id(tx, &me)? {
                None => return Err(ApiError::unauthorized()),
                Some(d) if d.revoked_at.is_some() => {
                    return Err(ApiError::revoked(d.revoked_at));
                }
                Some(_) => {}
            }
            if members(tx)?.len() != 1 {
                return Err(ApiError::new(Code::NotLastDevice).detail(
                    "Other devices are still in the space: they must leave or be revoked first.",
                ));
            }
            Ok(())
        })
        .await?;
    // The guard goes into the deletion, which holds it to the end, even if this request is
    // dropped meanwhile (the client hangs up).
    let short = space.short_id();
    match st.delete_space(&space, registry, receipt).await {
        DeleteOutcome::Deleted => {
            tracing::warn!(space = short, "space deleted by its last device");
            Ok(json(StatusCode::OK, json!({ "space_deleted": true })))
        }
        // The details went to the logs (by `Spaces::delete`); the body names nothing.
        DeleteOutcome::NotDeleted => Err(ApiError::internal(format!(
            "space {short}: nothing could be deleted, the space is registered again"
        ))),
        DeleteOutcome::NotDurable => Err(ApiError::internal(format!(
            "space {short}: the deletion could not be made durable"
        ))),
        DeleteOutcome::DeletedWithoutReceipt => Err(ApiError::internal(format!(
            "space {short}: deleted, but routing.db could not record it"
        ))),
        DeleteOutcome::Unknown | DeleteOutcome::NotRegistered => Err(ApiError::pending()),
    }
}

/// `POST /v1/space/receipt` (§4.4.2): the proof that a `DELETE /v1/space` which carried this
/// `deletion_id` deleted its space, for a client whose `200` was lost. No credential (the
/// session went with the space), no check of the space (never `space_not_initialized`).
///
/// The relay keeps only `SHA-256(deletion_id)` and the time, written in the transaction that
/// recorded the deletion, after the deletion was made durable. Any identifier it does not hold
/// (never seen, deletion that did not succeed, receipt expired) gets the same `404`, so the
/// route tells nothing to whoever does not know the 32 random bytes. Never logged.
pub async fn space_receipt(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpPublic, Rl::IpAuth],
        space: false,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, body) = gate(&st, req, g).await?;
    let mut o = json_body(
        &ctx.parts,
        body,
        st.cfg.max_json_body_bytes.min(MAX_DELETION_BODY),
        true,
    )
    .await?;
    let deletion_id = o.b64_exact("deletion_id", true, 32);
    o.finish()?;
    let deletion_id = deletion_id.ok_or_else(|| ApiError::internal("validated field missing"))?;
    let hash = wire::sha256(&deletion_id);
    let ttl_ms =
        i64::try_from(st.cfg.deletion_receipt_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let after = ctx.now.saturating_sub(ttl_ms);
    let found = st
        .spaces
        .routing()
        .read(move |tx| {
            Ok(tx
                .query_row(
                    "SELECT 1 FROM space_deletions WHERE receipt_hash = ?1 AND deleted_at > ?2",
                    params![hash.to_vec(), after],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?)
        })
        .await?;
    match found {
        Some(_) => Ok(json(StatusCode::OK, json!({ "space_deleted": true }))),
        None => Err(ApiError::not_found()),
    }
}

/// `POST /v1/space` (§3.2): creates a space, with the invitation its token designates.
///
/// The order is the one that leaks least. A token no invitation knows is `install_token_invalid`
/// before anything else is looked at, so that nobody without a token learns which `space_id`
/// exist. A token that already created a space is `space_exists`, whatever `space_id` comes with
/// it: the founder of that space, whose `201` was lost, retries and must be able to prove with
/// its keys that the space is its own (`pairing-v1.md` §10.2); any other device fails that
/// proof (`401`) and is offered "Rejoindre". The invitation is consumed before the space is
/// created and given back if the creation fails. `routing.db` and the space database are two
/// databases and so two transactions: a crash between them loses an invitation (it stays
/// consumed, designating a space that does not exist), never a space.
pub async fn post_space(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpPublic, Rl::IpSpaceCreate],
        space: false,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, body) = gate(&st, req, g).await?;
    let mut o = json_body(&ctx.parts, body, st.cfg.max_json_body_bytes, true).await?;
    let token = o.string("install_token", true);
    let space_id = o.uuid("space_id", true);
    let name = o.name("name", true);
    let platform = o.platform("platform", true);
    let cert = o.b64("certificate", true);
    o.finish()?;
    let (Some(token), Some(space_id), Some(name), Some(platform), Some(cert)) =
        (token, space_id, name, platform, cert)
    else {
        return Err(ApiError::internal("validated fields missing"));
    };

    // Creation, device registration and channel creation are one at a time: they check and
    // then insert identifiers that must stay unique across the whole relay.
    let _registry = st.spaces.lock_registry().await;
    let spaces = st.spaces.clone();
    let cfg = st.cfg.clone();
    let now = ctx.now;
    let created = tokio::task::spawn_blocking(move || {
        // 1. Some invitation designates the token; if it already created a space, that is
        // `space_exists`, for any `space_id` (a founder whose `201` was lost, or a second device).
        match install::state(spaces.routing(), &token)? {
            install::TokenState::Unknown => {
                return Err(ApiError::new(Code::InstallTokenInvalid)
                    .detail("The installation token is wrong."));
            }
            install::TokenState::Consumed => {
                return Err(ApiError::new(Code::SpaceExists)
                    .detail("This installation token has already created a space."));
            }
            install::TokenState::Pending => {}
        }
        // 2. The space does not exist yet (the invitation is not consumed). A space of this id
        // that is pending (its deletion failed in an unknown state) may come back whole at the
        // next start: `503`, and its folder is not touched.
        match spaces.route_space(&space_id) {
            Route::Live(_) => {
                return Err(ApiError::new(Code::SpaceExists)
                    .detail("This space already exists on this relay."));
            }
            Route::Pending => return Err(ApiError::pending()),
            Route::Unknown => {}
        }
        // 3. The founder: a self-signed DeviceCert, never seen anywhere on this relay.
        let founder = Founder::check(&cert, space_id)?;
        if spaces.key_known(&founder.sign_pk) || spaces.device_known(&founder.device_id) {
            return Err(
                ApiError::new(Code::DeviceExists).detail("This device is already registered.")
            );
        }
        // 4. The invitation goes, then the space is created; if that fails the invitation is
        // given back.
        let Some(_invited_at) =
            install::consume(spaces.routing(), &token, now, &uuid_format(&space_id))?
        else {
            return Err(ApiError::new(Code::SpaceExists)
                .detail("This installation token has already created a space."));
        };
        let made = spaces.create(space_id, |tx| {
            create_space(tx, &cfg, now, space_id, &name, &platform, &cert, &founder)
        });
        match made {
            // The registry has indexed the founder (device and key) with the space.
            Ok((_space, value)) => Ok(value),
            Err(e) => {
                let _ = install::restore(spaces.routing(), &token);
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("blocking task: {e}")))??;
    tracing::info!(space = &uuid_format(&space_id)[..8], "space created");
    Ok(json(StatusCode::CREATED, created))
}

/// The founder of a space: the subject of the self-signed certificate of `POST /v1/space`.
struct Founder {
    cert: DeviceCert,
    device_id: String,
    sign_pk: Vec<u8>,
}

impl Founder {
    /// Decodes and verifies the certificate (by `tomi-core`): it names `space_id`, is
    /// self-signed and correctly signed.
    fn check(cert_bytes: &[u8], space_id: [u8; 16]) -> ApiResult<Self> {
        let bad = |why: &str| ApiError::field("/certificate", why);
        let cert =
            DeviceCert::from_bytes(cert_bytes).map_err(|_| bad("not a device certificate"))?;
        if wire::uuid_parse(&cert.space_id) != Some(space_id) {
            return Err(bad("space_id differs from the request"));
        }
        if !cert.is_self_signed() {
            return Err(bad("the certificate must be self-signed"));
        }
        cert.verify_signature()
            .map_err(|_| bad("invalid signature"))?;
        let device_id =
            device_id_from_ed25519(&cert.subject.ed25519).map_err(|_| bad("invalid key"))?;
        Ok(Self {
            sign_pk: cert.subject.ed25519.clone(),
            cert,
            device_id,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn create_space(
    tx: &Connection,
    cfg: &crate::config::Config,
    now: i64,
    space_id: [u8; 16],
    name: &str,
    platform: &str,
    cert_bytes: &[u8],
    founder: &Founder,
) -> ApiResult<serde_json::Value> {
    // The space, the founder and the first event.
    tx.execute(
        "INSERT INTO space (id, space_id, created_at, key_epoch, rotation_required, retention_floor_seq, last_seq)
         VALUES (1, ?1, ?2, 1, 0, 0, 0)",
        params![space_id.to_vec(), now],
    )?;
    tx.execute(
        "INSERT INTO devices (device_id, name, platform, sign_pk, kx_pk, certificate, issuer_device_id, joined_seq, registered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?1, 0, ?7)",
        params![
            founder.device_id,
            name,
            platform,
            founder.cert.subject.ed25519,
            founder.cert.subject.x25519,
            cert_bytes,
            now
        ],
    )?;
    let row = device_by_id(tx, &founder.device_id)?
        .ok_or_else(|| ApiError::internal("founder vanished"))?;
    let seq = add_event(
        tx,
        now,
        "device_added",
        Some(&founder.device_id),
        None,
        &json!({ "device": device_record(&row, now, cfg, false) }),
    )?;
    Ok(json!({
        "space_id": uuid_format(&space_id),
        "created_at": ts_format(now),
        "device_id": founder.device_id,
        "key_epoch": 1,
        "head_seq": seq,
    }))
}
