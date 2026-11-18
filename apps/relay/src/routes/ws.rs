//! `GET /v1/ws`: the doorbell (`api-v1.md` §5). The socket carries no data, only `wake`
//! messages telling a device to read the log. The first message authenticates (no token in
//! the URL, ever).

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request, State};
use axum::response::Response;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::error::{ApiError, ApiResult, Code};
use crate::gate::{gate, lookup_session, Auth, Ctx, Gate, Rl};
use crate::hub::{close, Down};
use crate::spaces::Route;
use crate::state::AppState;
use crate::store;
use crate::wire::ts_format;

/// Largest text message accepted from a client (§5.1).
const MAX_MESSAGE: usize = 4096;

/// `GET /v1/ws`: the upgrade. The per-IP limit and the existence of a space on the relay are
/// checked before the upgrade; the credential, which also designates the space, is checked
/// afterwards, in the first message.
pub async fn upgrade(State(st): State<AppState>, req: Request) -> ApiResult<Response> {
    let g = Gate {
        ip: &[Rl::IpWs],
        space: true,
        auth: Auth::None,
        dev: &[],
    };
    let (ctx, _body) = gate(&st, req, g).await?;
    let Ctx { mut parts, .. } = ctx;
    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| ApiError::bad_request("A WebSocket upgrade is expected."))?;
    Ok(ws
        .max_message_size(4 * MAX_MESSAGE)
        .max_frame_size(4 * MAX_MESSAGE)
        .on_upgrade(move |socket| connection(st, socket)))
}

fn close_message(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: Utf8Bytes::from_static(reason),
    }))
}

/// Sends a close frame, then waits a moment for the peer's answer so the frame is not lost
/// to a reset.
async fn finish(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket.send(close_message(code, reason)).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(Ok(_)) = socket.recv().await {}
    })
    .await;
}

/// The close code and text for an authentication failure (§5.4). `unavailable` (the space of
/// the session is pending, or its database is closed) is `1013 try_again_later`: neither
/// "unknown" nor "gone".
fn auth_failure(e: &ApiError) -> (u16, &'static str) {
    match e.code {
        Code::SessionExpired => (close::SESSION_EXPIRED, "session_expired"),
        Code::Revoked => (close::REVOKED, "revoked"),
        Code::Unavailable => (close::TRY_AGAIN_LATER, "try_again_later"),
        _ => (close::UNAUTHORIZED, "unauthorized"),
    }
}

async fn connection(st: AppState, mut socket: WebSocket) {
    // 1. The first message, within the time allowed.
    let first = tokio::time::timeout(st.timing.ws_auth_timeout, socket.recv()).await;
    let token = match first {
        Err(_) => return finish(&mut socket, close::UNAUTHORIZED, "unauthorized").await,
        Ok(None | Some(Err(_))) => return,
        Ok(Some(Ok(Message::Text(t)))) if t.as_str().len() <= MAX_MESSAGE => {
            match serde_json::from_str::<Value>(t.as_str()) {
                Ok(v) if v["type"] == "auth" => v["token"].as_str().map(str::to_owned),
                _ => None,
            }
        }
        Ok(Some(Ok(Message::Close(_)))) => return,
        Ok(Some(Ok(_))) => {
            return finish(&mut socket, close::PROTOCOL_ERROR, "protocol_error").await
        }
    };
    let Some(token) = token else {
        return finish(&mut socket, close::PROTOCOL_ERROR, "protocol_error").await;
    };

    // 2. The token is validated like a REST request.
    let now = st.now();
    let (space, dev, expires_at) = match lookup_session(&st, &token, now).await {
        Ok((space, dev, expires_at, _)) => (space, dev, expires_at),
        Err(e) => {
            let (code, reason) = auth_failure(&e);
            return finish(&mut socket, code, reason).await;
        }
    };
    let head = space
        .db
        .read(|tx| Ok(store::require_space(tx)?.last_seq))
        .await
        .unwrap_or(0);
    let (conn_id, mut rx) = st.hub.register(&dev.device_id);
    // A deletion unregisters the space (pending), and closes the sockets of its devices with
    // `4004` only once the space is gone for good. A connection registered after the session
    // was looked up looks again: pending is `1013` (the deletion may still fail), gone for good
    // is `4004` (it may have missed the closing), registered goes on.
    match st.spaces.route_space(&space.id) {
        Route::Live(_) => {}
        Route::Pending => {
            st.hub.unregister(&dev.device_id, conn_id);
            return finish(&mut socket, close::TRY_AGAIN_LATER, "try_again_later").await;
        }
        Route::Unknown => {
            st.hub.unregister(&dev.device_id, conn_id);
            return finish(&mut socket, close::SPACE_RESET, "space_reset").await;
        }
    }
    let ready =
        json!({ "type": "ready", "head_seq": head, "session_expires_at": ts_format(expires_at) });
    if socket
        .send(Message::Text(ready.to_string().into()))
        .await
        .is_err()
    {
        st.hub.unregister(&dev.device_id, conn_id);
        return;
    }

    // 3. The doorbell loop.
    let remaining = Duration::from_millis(u64::try_from(expires_at - st.now()).unwrap_or(0));
    let expiry = tokio::time::sleep(remaining);
    tokio::pin!(expiry);
    let mut ping = tokio::time::interval_at(
        Instant::now() + st.timing.ws_ping_interval,
        st.timing.ws_ping_interval,
    );
    let mut pong_deadline: Option<Instant> = None;
    loop {
        let pong_wait = async {
            match pong_deadline {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            msg = socket.recv() => match msg {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(Message::Pong(_))) => pong_deadline = None,
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Text(t))) => {
                    // After `ready`, client messages are ignored (reserved), but not unbounded.
                    if t.as_str().len() > MAX_MESSAGE {
                        finish(&mut socket, close::PROTOCOL_ERROR, "protocol_error").await;
                        break;
                    }
                }
                Some(Ok(Message::Binary(_))) => {
                    finish(&mut socket, close::PROTOCOL_ERROR, "protocol_error").await;
                    break;
                }
            },
            down = rx.recv() => match down {
                None => break,
                Some(Down::Wake { mut seq, mut ring }) => {
                    // Several wake-ups already waiting are merged into one.
                    while let Ok(Down::Wake { seq: s, ring: r }) = rx.try_recv() {
                        seq = seq.max(s);
                        ring |= r;
                    }
                    let m = json!({ "type": "wake", "seq": seq, "ring": ring });
                    if socket.send(Message::Text(m.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Some(Down::Close { code, reason, ack }) => {
                    let _ = socket.send(close_message(code, reason)).await;
                    if let Some(a) = ack {
                        let _ = a.send(());
                    }
                    let _ = tokio::time::timeout(Duration::from_millis(500), async {
                        while let Some(Ok(_)) = socket.recv().await {}
                    })
                    .await;
                    break;
                }
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
                pong_deadline.get_or_insert(Instant::now() + st.timing.ws_pong_timeout);
            }
            () = pong_wait => {
                // No answer to the ping: the connection is dead; drop it without a handshake.
                break;
            }
            () = &mut expiry => {
                finish(&mut socket, close::SESSION_EXPIRED, "session_expired").await;
                break;
            }
        }
    }
    st.hub.unregister(&dev.device_id, conn_id);
}
