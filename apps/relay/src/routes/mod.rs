//! The HTTP surface: the 42 endpoints of `api-v1.md` §4.1.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use serde_json::Value;

use crate::error::ApiError;
use crate::middleware::outer;
use crate::state::AppState;

pub mod auth;
pub mod basic;
pub mod blobs;
pub mod devices;
pub mod events;
pub mod items;
pub mod pairing;
pub mod rotations;
pub mod statics;
pub mod transfers;
pub mod ws;

/// How an endpoint authenticates (column "Auth" of the table of §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthClass {
    /// No credential.
    None,
    /// Session of a member device.
    Bearer,
    /// Pairing token of the new device, or the session of the inviter.
    BearerOrPairing,
    /// Installation token in the body.
    Install,
    /// The first WebSocket message.
    FirstMessage,
}

/// One row of the table of §4.1.
#[derive(Debug, Clone, Copy)]
pub struct Endpoint {
    pub n: u8,
    pub method: &'static str,
    pub path: &'static str,
    pub auth: AuthClass,
}

/// The 43 endpoints. Nothing else is routed: no administration route exists on the network
/// (`revoke`, `reset` and the installation token are reachable only through the local
/// `admin` commands, §8.7).
/// The web administration panel (off by default, read-only plus invitations) is not routed here:
/// it is served by the fallback below, only under its secret path (admin_web.rs).
pub const ENDPOINTS: [Endpoint; 43] = [
    Endpoint {
        n: 1,
        method: "GET",
        path: "/healthz",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 2,
        method: "GET",
        path: "/v1/hello",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 3,
        method: "POST",
        path: "/v1/space",
        auth: AuthClass::Install,
    },
    Endpoint {
        n: 4,
        method: "POST",
        path: "/v1/auth/challenge",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 5,
        method: "POST",
        path: "/v1/auth/session",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 6,
        method: "DELETE",
        path: "/v1/auth/session",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 7,
        method: "GET",
        path: "/v1/space",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 8,
        method: "GET",
        path: "/v1/devices",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 9,
        method: "POST",
        path: "/v1/devices",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 10,
        method: "PATCH",
        path: "/v1/devices/{device_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 11,
        method: "DELETE",
        path: "/v1/me",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 12,
        method: "PUT",
        path: "/v1/me/push",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 13,
        method: "DELETE",
        path: "/v1/me/push",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 14,
        method: "PUT",
        path: "/v1/me/presence",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 15,
        method: "POST",
        path: "/v1/pairing/channels",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 16,
        method: "POST",
        path: "/v1/pairing/channels/{channel_id}/claim",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 17,
        method: "POST",
        path: "/v1/pairing/channels/{channel_id}/messages",
        auth: AuthClass::BearerOrPairing,
    },
    Endpoint {
        n: 18,
        method: "GET",
        path: "/v1/pairing/channels/{channel_id}/messages",
        auth: AuthClass::BearerOrPairing,
    },
    Endpoint {
        n: 19,
        method: "DELETE",
        path: "/v1/pairing/channels/{channel_id}",
        auth: AuthClass::BearerOrPairing,
    },
    Endpoint {
        n: 20,
        method: "POST",
        path: "/v1/rotations",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 21,
        method: "GET",
        path: "/v1/keys",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 22,
        method: "POST",
        path: "/v1/items",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 23,
        method: "GET",
        path: "/v1/items/{item_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 24,
        method: "POST",
        path: "/v1/items/{item_id}/ack",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 25,
        method: "POST",
        path: "/v1/items/{item_id}/read",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 26,
        method: "POST",
        path: "/v1/tombstones",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 27,
        method: "POST",
        path: "/v1/blobs",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 28,
        method: "GET",
        path: "/v1/blobs/{blob_id}/status",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 29,
        method: "PATCH",
        path: "/v1/blobs/{blob_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 30,
        method: "GET",
        path: "/v1/blobs/{blob_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 31,
        method: "DELETE",
        path: "/v1/blobs/{blob_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 32,
        method: "GET",
        path: "/v1/events",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 33,
        method: "GET",
        path: "/v1/ws",
        auth: AuthClass::FirstMessage,
    },
    Endpoint {
        n: 34,
        method: "GET",
        path: "/updates/{*path}",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 35,
        method: "GET",
        path: "/.well-known/assetlinks.json",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 36,
        method: "GET",
        path: "/l/{*path}",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 37,
        method: "POST",
        path: "/v1/transfers",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 38,
        method: "GET",
        path: "/v1/transfers/{transfer_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 39,
        method: "DELETE",
        path: "/v1/transfers/{transfer_id}",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 40,
        method: "POST",
        path: "/v1/item-states",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 41,
        method: "DELETE",
        path: "/v1/space",
        auth: AuthClass::Bearer,
    },
    Endpoint {
        n: 42,
        method: "POST",
        path: "/v1/space/receipt",
        auth: AuthClass::None,
    },
    Endpoint {
        n: 43,
        method: "POST",
        path: "/v1/attachment-tombstones",
        auth: AuthClass::Bearer,
    },
];

/// A JSON response.
pub fn json(status: StatusCode, v: Value) -> Response {
    let body = serde_json::to_vec(&v).unwrap_or_default();
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// A response without body.
pub fn empty(status: StatusCode) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = status;
    r
}

/// Unknown path, unless it is the web administration panel (off by default, then this is always
/// the plain unknown-resource answer; see `admin_web.rs`).
async fn not_found(State(st): State<AppState>, req: Request) -> Response {
    crate::admin_web::serve(&st, req).await
}

/// The router of the relay.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(basic::root))
        .route("/healthz", get(basic::healthz))
        .route("/v1/hello", get(basic::hello))
        .route(
            "/v1/space",
            get(basic::get_space)
                .post(basic::post_space)
                .delete(basic::delete_space),
        )
        .route("/v1/space/receipt", post(basic::space_receipt))
        .route("/v1/auth/challenge", post(auth::challenge))
        .route(
            "/v1/auth/session",
            post(auth::session).delete(auth::delete_session),
        )
        .route("/v1/devices", get(devices::list).post(devices::register))
        .route("/v1/devices/{device_id}", patch(devices::rename))
        .route("/v1/me", delete(devices::leave))
        .route(
            "/v1/me/push",
            put(devices::put_push).delete(devices::delete_push),
        )
        .route("/v1/me/presence", put(devices::put_presence))
        .route("/v1/pairing/channels", post(pairing::create))
        .route("/v1/pairing/channels/{channel_id}", delete(pairing::close))
        .route(
            "/v1/pairing/channels/{channel_id}/claim",
            post(pairing::claim),
        )
        .route(
            "/v1/pairing/channels/{channel_id}/messages",
            post(pairing::post_message).get(pairing::get_messages),
        )
        .route("/v1/rotations", post(rotations::post))
        .route("/v1/keys", get(rotations::keys))
        .route("/v1/items", post(items::post))
        .route("/v1/items/{item_id}", get(items::get))
        .route("/v1/items/{item_id}/ack", post(items::ack))
        .route("/v1/items/{item_id}/read", post(items::read))
        .route("/v1/tombstones", post(items::tombstone))
        .route(
            "/v1/attachment-tombstones",
            post(items::attachment_tombstone),
        )
        .route("/v1/item-states", post(items::item_state))
        .route("/v1/blobs", post(blobs::create))
        .route("/v1/blobs/{blob_id}/status", get(blobs::status))
        .route(
            "/v1/blobs/{blob_id}",
            get(blobs::download)
                .head(blobs::download)
                .patch(blobs::patch)
                .delete(blobs::delete),
        )
        .route("/v1/events", get(events::list))
        .route("/v1/ws", get(ws::upgrade))
        .route("/v1/transfers", post(transfers::post))
        .route(
            "/v1/transfers/{transfer_id}",
            get(transfers::get).delete(transfers::delete),
        )
        .route("/updates/{*path}", get(statics::updates))
        .route("/.well-known/assetlinks.json", get(statics::assetlinks))
        .route("/l", get(statics::page))
        .route("/l/", get(statics::page))
        .route("/l/{*path}", get(statics::page))
        // The link that creates a space (`/create#<token>`; `/create/<token>` is accepted too): one
        // static page, the path is never read (not part of the numbered table of the spec).
        .route("/create", get(basic::create))
        .route("/create/", get(basic::create))
        .route("/create/{*path}", get(basic::create))
        .fallback(not_found)
        .layer(from_fn_with_state(state.clone(), outer))
        .with_state(state)
}

impl IntoResponse for crate::error::Code {
    fn into_response(self) -> Response {
        ApiError::new(self).into_response()
    }
}
