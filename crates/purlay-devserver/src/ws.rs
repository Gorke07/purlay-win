//! `GET /ws`: the socket. The upgrade needs the session cookie (401) and `Origin` equal to
//! `appUrl` (403). Frames are handled one at a time, under the world's lock, so what a
//! frame does and what it publishes are ordered with every HTTP write.

use crate::error::AppError;
use crate::http::{Shared, session_cookie};
use crate::ids::{is_uuid, now_ts};
use crate::world::{Out, SessionInfo, World, error_frame, frame_text};
use axum::extract::State;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use purlay_contract::Uuid;
use purlay_contract::constants::{MAX_FRAME_BYTES, SUBSCRIBE_MAX_CHANNELS};
use purlay_contract::models::*;
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::sync::mpsc;

/// How many roots one channel's replay carries before it says "continue over HTTP".
pub const REPLAY_CAP: usize = 500;

pub async fn upgrade(
    State(app): State<Shared>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(token) = session_cookie(&headers) else {
        return AppError::unauthenticated().into_response();
    };
    let Some(info) = app.world.lock().session_for_token(&token) else {
        return AppError::unauthenticated().into_response();
    };
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).unwrap_or("");
    if origin.trim_end_matches('/') != app.origin {
        return AppError::new("FORBIDDEN", "The socket's Origin is not this server's address.").into_response();
    }
    let upgrade = match upgrade {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    upgrade
        .max_message_size(MAX_FRAME_BYTES as usize)
        .max_frame_size(MAX_FRAME_BYTES as usize)
        .on_upgrade(move |socket| run(app, info, socket))
}

async fn run(app: Shared, info: SessionInfo, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let id = app.world.lock().register_socket(&info, tx);
    tracing::info!("socket {id} open for {}", info.user);

    // The writer ends when the world lets go of the socket (its sender is dropped) or is
    // told to close it.
    let mut writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Out::Text(text) => {
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Out::Close(code, reason) => {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                    return;
                }
            }
        }
        let _ = sink.close().await;
    });

    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => handle_frame(&app, id, &info, text.as_str()),
                Some(Ok(Message::Binary(_))) => {
                    let w = app.world.lock();
                    send_to(&w, id, &error_frame(&AppError::validation("Frames are text."), None));
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            _ = &mut writer => break,
        }
    }
    app.world.lock().unregister_socket(id);
    tracing::info!("socket {id} closed");
}

fn send_to(w: &World, socket_id: u64, frame: &ServerFrame) {
    if let Some(s) = w.sockets.get(&socket_id) {
        let _ = s.tx.send(Out::Text(frame_text(frame)));
    }
}

fn handle_frame(app: &Shared, socket_id: u64, info: &SessionInfo, text: &str) {
    let mut w = app.world.lock();
    if !w.sockets.contains_key(&socket_id) {
        return; // its session ended; the close is on its way
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        send_to(&w, socket_id, &error_frame(&AppError::validation("The frame is not JSON."), None));
        return;
    };
    let ref_id = value.get("id").and_then(|v| v.as_str()).filter(|s| is_uuid(s)).map(Uuid::new);
    let frame = match serde_json::from_value::<ClientFrame>(value) {
        Ok(f) => f,
        Err(e) => {
            let err = AppError { details: Some(json!({ "fields": { "payload": e.to_string() } })), ..AppError::validation("The frame is not valid.") };
            send_to(&w, socket_id, &error_frame(&err, ref_id));
            return;
        }
    };
    let user = info.user.clone();
    match frame {
        ClientFrame::Subscribe(f) => subscribe(&mut w, socket_id, &user, f),
        ClientFrame::Unsubscribe(f) => {
            if let Some(s) = w.sockets.get_mut(&socket_id) {
                for c in &f.payload.channel_ids {
                    s.subs.remove(c);
                }
            }
            let ack = ServerFrame::Unsubscribed(UnsubscribedFrame {
                data: UnsubscribedFrameData { channel_ids: f.payload.channel_ids.clone() },
                ts: now_ts(),
                ref_id: Some(f.id),
            });
            send_to(&w, socket_id, &ack);
        }
        ClientFrame::MessageSend(f) => {
            let p = &f.payload;
            let attachments = p.attachment_ids.clone().unwrap_or_default();
            match w.send_message(&user, &p.channel_id, &f.id, &p.content, p.parent_id.as_ref(), &attachments) {
                Ok(_) => {
                    let ack = ServerFrame::Ack(AckFrame { data: AckFrameData { id: f.id.clone() }, ts: now_ts(), ref_id: Some(f.id) });
                    send_to(&w, socket_id, &ack);
                }
                Err(e) => send_to(&w, socket_id, &error_frame(&e, Some(f.id))),
            }
        }
        ClientFrame::Ping(f) => {
            let pong = ServerFrame::Pong(PongFrame { data: Default::default(), ts: now_ts(), ref_id: Some(f.id) });
            send_to(&w, socket_id, &pong);
        }
        ClientFrame::Typing(f) => {
            let ok = w.channels.get(&f.payload.channel_id).is_some_and(|c| c.members.contains_key(&user));
            if ok {
                w.publish_typing(&f.payload.channel_id, &user);
            }
        }
        ClientFrame::Active(_) => {}
        ClientFrame::Unknown(_) => {
            let err = AppError {
                details: Some(json!({ "fields": { "op": "is not an operation this server accepts" } })),
                ..AppError::validation("Unknown operation.")
            };
            send_to(&w, socket_id, &error_frame(&err, ref_id));
        }
    }
}

/// `SUBSCRIBE`, judged whole: one channel the person cannot see refuses the frame.
fn subscribe(w: &mut World, socket_id: u64, user: &str, f: SubscribeFrame) {
    let items = &f.payload.channels;
    let mut seen: HashSet<&str> = HashSet::new();
    if items.len() > SUBSCRIBE_MAX_CHANNELS as usize || !items.iter().all(|c| seen.insert(c.channel_id.as_str())) {
        let err = AppError::field("channels", "names more than 100 channels, or one twice");
        send_to(w, socket_id, &error_frame(&err, Some(f.id)));
        return;
    }
    let refused = items.iter().any(|c| w.visible_channel(&c.channel_id, user).is_err());
    if refused {
        send_to(w, socket_id, &error_frame(&AppError::channel_not_found(), Some(f.id.clone())));
        // Those the person was in and is not any more are told why (D-264).
        let lost: Vec<Uuid> = items
            .iter()
            .filter(|c| {
                w.channels.get(&c.channel_id).is_some_and(|ch| ch.ever_members.contains(user) && !ch.members.contains_key(user))
            })
            .map(|c| c.channel_id.clone())
            .collect();
        if !lost.is_empty() {
            let frame = ServerFrame::AccessRevoked(AccessRevokedFrame {
                data: AccessRevokedFrameData { channel_ids: lost, reason: Some(AccessRevokedFrameDataReason::Left) },
                ts: now_ts(),
                ref_id: None,
            });
            send_to(w, socket_id, &frame);
        }
        return;
    }
    if let Some(s) = w.sockets.get_mut(&socket_id) {
        for c in items {
            s.subs.insert(c.channel_id.clone());
        }
    }
    let mut states = Vec::new();
    for c in items {
        let mut next_cursor = None;
        if let Some(after) = &c.after_message_id {
            let (replay, next) = w.replay(&c.channel_id, user, after, REPLAY_CAP);
            for m in replay {
                send_to(w, socket_id, &ServerFrame::MessageCreated(MessageCreatedFrame { data: m, ts: now_ts(), ref_id: None }));
            }
            next_cursor = next;
        }
        states.push(ReplayState { channel_id: c.channel_id.clone(), next_cursor });
    }
    let ack = ServerFrame::Subscribed(SubscribedFrame {
        data: SubscribedFrameData { channels: states },
        ts: now_ts(),
        ref_id: Some(f.id),
    });
    send_to(w, socket_id, &ack);
}
