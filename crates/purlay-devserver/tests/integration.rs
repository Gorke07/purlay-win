//! End-to-end checks of the dev server over real HTTP and a real socket, with the
//! contract's generated types on the client side too.

use base64::Engine;
use futures::{SinkExt, StreamExt};
use purlay_contract::Uuid;
use purlay_contract::constants::close_codes;
use purlay_contract::models::*;
use purlay_devserver::{Config, ServerHandle, spawn};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start() -> (ServerHandle, reqwest::Client) {
    let (_addr, server) = spawn(Config::default()).await.expect("server starts");
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    (server, http)
}

fn url(server: &ServerHandle, path: &str) -> String {
    format!("{}{}", server.origin(), path)
}

async fn get<T: DeserializeOwned>(http: &reqwest::Client, server: &ServerHandle, path: &str, cookie: &str) -> T {
    let res = http.get(url(server, path)).header("Cookie", cookie).send().await.unwrap();
    assert!(res.status().is_success(), "GET {path}: {}", res.status());
    res.json::<T>().await.unwrap_or_else(|e| panic!("GET {path}: {e}"))
}

async fn error_of(res: reqwest::Response) -> (u16, String) {
    let status = res.status().as_u16();
    let body: ErrorResponse = res.json().await.expect("the error envelope");
    (status, body.error.code.as_str().to_string())
}

async fn open_socket(server: &ServerHandle, cookie: Option<&str>, origin: Option<&str>) -> Result<Ws, u16> {
    let mut req = server.socket_url().into_client_request().unwrap();
    if let Some(c) = cookie {
        req.headers_mut().insert("Cookie", c.parse().unwrap());
    }
    if let Some(o) = origin {
        req.headers_mut().insert("Origin", o.parse().unwrap());
    }
    req.headers_mut().insert("X-Purlay-Client", "windows/0.1.0 contract/1".parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("socket: {e}"),
    }
}

async fn send_frame(ws: &mut Ws, frame: &ClientFrame) {
    ws.send(WsMessage::text(serde_json::to_string(frame).unwrap())).await.unwrap();
}

/// Reads frames until one matches, failing after five seconds.
async fn wait_for(ws: &mut Ws, what: &str, mut pred: impl FnMut(&ServerFrame) -> bool) -> ServerFrame {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .unwrap_or_else(|_| panic!("no {what} within 5 s"))
            .unwrap_or_else(|| panic!("socket ended waiting for {what}"))
            .unwrap();
        if let WsMessage::Text(t) = msg {
            let frame: ServerFrame = serde_json::from_str(t.as_str()).unwrap_or_else(|e| panic!("bad frame {t}: {e}"));
            if pred(&frame) {
                return frame;
            }
        }
    }
}

fn ack_ref(frame: &ServerFrame) -> Option<&Uuid> {
    match frame {
        ServerFrame::Ack(f) => f.ref_id.as_ref(),
        ServerFrame::Subscribed(f) => f.ref_id.as_ref(),
        ServerFrame::Unsubscribed(f) => f.ref_id.as_ref(),
        ServerFrame::Error(f) => f.ref_id.as_ref(),
        ServerFrame::Pong(f) => f.ref_id.as_ref(),
        _ => None,
    }
}

#[tokio::test]
async fn handshake_answers_client_info() {
    let (server, http) = start().await;
    let res = http.get(url(&server, "/api/client")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
    let info: ClientInfo = res.json().await.unwrap();
    assert_eq!(info.server.version, "0.6.1");
    assert_eq!(info.contract_version, 1);
    assert_eq!(info.app_url, server.origin());
    assert!(info.auth.email && info.auth.password && !info.auth.github && !info.auth.google);
    assert_eq!(info.auth.sign_up, AuthMethodsSignUp::Open);
    assert_eq!(info.min_client.macos.contract_version, None);
    assert_eq!(info.capabilities.and_then(|c| c.notification_type_filters), Some(true));
}

#[tokio::test]
async fn me_needs_a_session() {
    let (server, http) = start().await;
    let res = http.get(url(&server, "/api/me")).send().await.unwrap();
    assert_eq!(error_of(res).await, (401, "UNAUTHENTICATED".into()));
    let res = http.get(url(&server, "/api/me")).header("Cookie", "__Host-purlay_session=nope").send().await.unwrap();
    assert_eq!(error_of(res).await, (401, "UNAUTHENTICATED".into()));

    let cookie = server.cookie_header("ada");
    let me: MeResponse = get(&http, &server, "/api/me", &cookie).await;
    assert_eq!(me.user.username, "ada");
    assert_eq!(me.memberships.len(), 1);
    assert_eq!(me.memberships[0].workspace.slug, "acme");
    assert_eq!(me.memberships[0].role, UserRole::Owner);
    assert_eq!(me.modules, vec![ModuleId::Chat, ModuleId::Issues]);
}

#[tokio::test]
async fn unknown_routes_and_edge_checks() {
    let (server, http) = start().await;
    let cookie = server.cookie_header("ada");
    let res = http.get(url(&server, "/api/nothing-here")).header("Cookie", &cookie).send().await.unwrap();
    assert_eq!(error_of(res).await, (404, "ROUTE_NOT_FOUND".into()));
    let general = server.channel_id("general").unwrap();
    let res = http
        .post(url(&server, &format!("/api/channels/{general}/messages")))
        .header("Cookie", &cookie)
        .header("Content-Type", "text/plain")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(error_of(res).await, (415, "UNSUPPORTED_MEDIA_TYPE".into()));
}

#[tokio::test]
async fn channels_list_as_the_person_sees_them() {
    let (server, http) = start().await;
    let cookie = server.cookie_header("ada");
    let ws = server.workspace_id();
    let channels: Vec<Channel> = get(&http, &server, &format!("/api/workspaces/{ws}/channels"), &cookie).await;
    let named = |n: &str| channels.iter().find(|c| c.name.as_deref() == Some(n)).unwrap_or_else(|| panic!("#{n}"));
    let general = named("general");
    assert_eq!(general.r#type, ChannelType::Public);
    let state = general.state.as_ref().expect("a member's state");
    assert!(state.unread_count >= 3, "unread in #general: {}", state.unread_count);
    assert!(state.unread_mention_count >= 1);
    assert!(named("design").state.is_none(), "not joined");
    assert_eq!(named("leadership").r#type, ChannelType::Private);
    let dms: Vec<&Channel> = channels.iter().filter(|c| c.r#type == ChannelType::DirectMessage).collect();
    assert_eq!(dms.len(), 2);
    assert!(dms.iter().all(|d| d.peer.is_some()));

    // History: newest first, paged by `before`.
    let page: MessagePage = get(&http, &server, &format!("/api/channels/{}/messages?limit=10", general.id), &cookie).await;
    assert_eq!(page.data.len(), 10);
    assert!(page.data.windows(2).all(|w| w[0].id > w[1].id));
    let cursor = page.next_cursor.clone().expect("more");
    let older: MessagePage =
        get(&http, &server, &format!("/api/channels/{}/messages?before={cursor}&limit=100", general.id), &cookie).await;
    assert!(older.data.iter().all(|m| m.id < cursor));
    assert!(page.data.len() + older.data.len() >= 45);
    assert!(older.data.iter().any(|m| !m.attachments.is_empty()));
    assert!(older.data.iter().any(|m| m.thread.as_ref().is_some_and(|t| t.resolved_at.is_some())));
}

#[tokio::test]
async fn http_send_is_idempotent() {
    let (server, http) = start().await;
    let cookie = server.cookie_header("ada");
    let general = server.channel_id("general").unwrap();
    let id = purlay_devserver::ids::new_id();
    let body = SendMessageRequest { id: id.clone(), content: "Hello from the test".into(), attachment_ids: None, parent_id: None };
    let path = format!("/api/channels/{general}/messages");
    let mut sent = Vec::new();
    for _ in 0..2 {
        let res = http.post(url(&server, &path)).header("Cookie", &cookie).json(&body).send().await.unwrap();
        assert_eq!(res.status(), 200);
        sent.push(res.json::<Message>().await.unwrap());
    }
    assert_eq!(sent[0], sent[1]);
    assert_eq!(sent[0].id, id);
    let page: MessagePage = get(&http, &server, &format!("{path}?limit=5"), &cookie).await;
    assert_eq!(page.data.iter().filter(|m| m.id == id).count(), 1);

    // The same id from somebody else is a conflict; an id from long ago is refused.
    let grace = server.cookie_header("grace");
    let res = http.post(url(&server, &path)).header("Cookie", &grace).json(&body).send().await.unwrap();
    assert_eq!(error_of(res).await, (409, "MESSAGE_ID_CONFLICT".into()));
    let old = purlay_devserver::ids::id_at(purlay_devserver::ids::now_ms() - 10 * 60_000);
    let stale = SendMessageRequest { id: old, ..body.clone() };
    let res = http.post(url(&server, &path)).header("Cookie", &cookie).json(&stale).send().await.unwrap();
    assert_eq!(error_of(res).await, (400, "INVALID_MESSAGE_ID".into()));
}

#[tokio::test]
async fn socket_subscribe_send_and_ping() {
    let (server, http) = start().await;
    let cookie = server.cookie_header("ada");
    let general = server.channel_id("general").unwrap();
    let mut ws = open_socket(&server, Some(&cookie), Some(server.origin())).await.expect("upgrade");

    // Subscribe with the newest id held: the replay reaches back REPLAY_OVERLAP_MS.
    let page: MessagePage = get(&http, &server, &format!("/api/channels/{general}/messages?limit=1"), &cookie).await;
    let newest = page.data[0].id.clone();
    let sub_id = purlay_devserver::ids::new_id();
    let sub = ClientFrame::Subscribe(SubscribeFrame {
        id: sub_id.clone(),
        payload: SubscribeFramePayload {
            channels: vec![SubscribeFramePayloadChannelsItem { channel_id: general.clone(), after_message_id: Some(newest.clone()) }],
        },
    });
    send_frame(&mut ws, &sub).await;
    let mut replayed = Vec::new();
    let ack = wait_for(&mut ws, "the SUBSCRIBE ack", |f| {
        if let ServerFrame::MessageCreated(m) = f {
            replayed.push(m.data.id.clone());
        }
        ack_ref(f) == Some(&sub_id)
    })
    .await;
    match ack {
        ServerFrame::Subscribed(s) => {
            assert_eq!(s.data.channels.len(), 1);
            assert_eq!(s.data.channels[0].channel_id, general);
            assert_eq!(s.data.channels[0].next_cursor, None);
        }
        other => panic!("not a SUBSCRIBE ack: {other:?}"),
    }
    assert!(replayed.contains(&newest), "the replay overlaps the cursor");

    // MESSAGE_SEND → MESSAGE_CREATED to the room (us included), then ACK { id }.
    let msg_id = purlay_devserver::ids::new_id();
    let send = ClientFrame::MessageSend(MessageSendFrame {
        id: msg_id.clone(),
        payload: MessageSendFramePayload {
            channel_id: general.clone(),
            content: "From the socket, **bold**".into(),
            parent_id: None,
            attachment_ids: None,
        },
    });
    send_frame(&mut ws, &send).await;
    let created = wait_for(&mut ws, "MESSAGE_CREATED", |f| matches!(f, ServerFrame::MessageCreated(m) if m.data.id == msg_id)).await;
    if let ServerFrame::MessageCreated(m) = created {
        assert_eq!(m.data.sender.unwrap().username, "ada");
        assert!(m.ts > 0.0);
    }
    let ack = wait_for(&mut ws, "the send ack", |f| ack_ref(f) == Some(&msg_id)).await;
    match ack {
        ServerFrame::Ack(a) => assert_eq!(a.data.id, msg_id),
        other => panic!("not an ACK: {other:?}"),
    }

    // PING → PONG with the frame's id.
    let ping_id = purlay_devserver::ids::new_id();
    send_frame(&mut ws, &ClientFrame::Ping(PingFrame { id: ping_id.clone(), payload: Default::default() })).await;
    let pong = wait_for(&mut ws, "PONG", |f| ack_ref(f) == Some(&ping_id)).await;
    assert!(matches!(pong, ServerFrame::Pong(p) if p.ts > 0.0));

    // Somebody else's HTTP send reaches the room, and our unread state moves.
    let grace = server.cookie_header("grace");
    let other = purlay_devserver::ids::new_id();
    let body = SendMessageRequest { id: other.clone(), content: "@ada ping from Grace".into(), attachment_ids: None, parent_id: None };
    let res = http.post(url(&server, &format!("/api/channels/{general}/messages"))).header("Cookie", &grace).json(&body).send().await.unwrap();
    assert_eq!(res.status(), 200);
    wait_for(&mut ws, "Grace's message", |f| matches!(f, ServerFrame::MessageCreated(m) if m.data.id == other)).await;
    wait_for(&mut ws, "CHANNEL_STATE", |f| matches!(f, ServerFrame::ChannelState(s) if s.data.channel_id == general)).await;

    // A channel the person cannot see refuses the whole frame.
    let leadership = server.channel_id("leadership").unwrap();
    let linus = server.cookie_header("linus");
    let mut ws2 = open_socket(&server, Some(&linus), Some(server.origin())).await.expect("upgrade");
    let bad_id = purlay_devserver::ids::new_id();
    let bad = ClientFrame::Subscribe(SubscribeFrame {
        id: bad_id.clone(),
        payload: SubscribeFramePayload {
            channels: vec![SubscribeFramePayloadChannelsItem { channel_id: leadership, after_message_id: None }],
        },
    });
    send_frame(&mut ws2, &bad).await;
    let refused = wait_for(&mut ws2, "the refusal", |f| ack_ref(f) == Some(&bad_id)).await;
    assert!(matches!(refused, ServerFrame::Error(e) if e.data.code == ErrorCode::ChannelNotFound));
}

#[tokio::test]
async fn socket_upgrade_needs_cookie_and_origin() {
    let (server, _http) = start().await;
    let cookie = server.cookie_header("ada");
    assert_eq!(open_socket(&server, None, Some(server.origin())).await.err(), Some(401));
    assert_eq!(open_socket(&server, Some(&cookie), Some("https://evil.example")).await.err(), Some(403));
    assert_eq!(open_socket(&server, Some(&cookie), None).await.err(), Some(403));
    assert!(open_socket(&server, Some(&cookie), Some(server.origin())).await.is_ok());
}

#[tokio::test]
async fn logout_ends_the_socket() {
    let (server, http) = start().await;
    let cookie = server.cookie_header("ada");
    let mut ws = open_socket(&server, Some(&cookie), Some(server.origin())).await.expect("upgrade");
    let res = http.post(url(&server, "/auth/logout")).header("Cookie", &cookie).header("Content-Length", "0").send().await.unwrap();
    assert_eq!(res.status(), 204);
    wait_for(&mut ws, "SESSION_ENDED", |f| {
        matches!(f, ServerFrame::SessionEnded(s) if s.data.reason == SessionEndedFrameDataReason::SignedOut)
    })
    .await;
    let close = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
    match close {
        Some(Ok(WsMessage::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), close_codes::SESSION_ENDED),
        other => panic!("expected a 4401 close, got {other:?}"),
    }
    let res = http.get(url(&server, "/api/me")).header("Cookie", &cookie).send().await.unwrap();
    assert_eq!(res.status(), 401);
}

fn pkce() -> (String, String, String) {
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let verifier = b64(&rand_bytes::<32>());
    let challenge = b64(&Sha256::digest(verifier.as_bytes()));
    let state = b64(&rand_bytes::<16>());
    (verifier, challenge, state)
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    let hex = purlay_devserver::ids::random_hex(N);
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

/// The browser half: the page, the person picked, the `purlay://auth` address.
async fn browser_sign_in(server: &ServerHandle, http: &reqwest::Client, who: &str, challenge: &str, state: &str) -> String {
    let page = http
        .get(url(server, &format!("/sign-in/native?challenge={challenge}&state={state}")))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(html.contains("Sign in as Ada Lovelace"), "{html}");
    let res = http
        .post(url(server, "/sign-in/native/choose"))
        .form(&[("challenge", challenge), ("state", state), ("user", who)])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let html = res.text().await.unwrap();
    let start = html.find("purlay://auth?code=").expect("the redirect");
    let redirect: String = html[start..].chars().take_while(|c| *c != '"' && *c != '<').collect();
    let redirect = redirect.replace("&amp;", "&");
    assert!(redirect.ends_with(&format!("&state={state}")), "{redirect}");
    let code = redirect.trim_start_matches("purlay://auth?code=").split('&').next().unwrap().to_string();
    assert_eq!(code.len(), 64);
    assert!(code.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    code
}

async fn exchange(server: &ServerHandle, http: &reqwest::Client, code: &str, verifier: &str) -> reqwest::Response {
    let body = NativeTokenRequest {
        code: code.into(),
        verifier: verifier.into(),
        device: NativeTokenRequestDevice { name: "TEST-PC".into() },
    };
    http.post(url(server, "/api/native/token")).header("X-Purlay-Client", "windows/0.1.0 contract/1").json(&body).send().await.unwrap()
}

#[tokio::test]
async fn native_sign_in_exchanges_the_code_once() {
    let (server, http) = start().await;
    let (verifier, challenge, state) = pkce();
    let code = browser_sign_in(&server, &http, "ada", &challenge, &state).await;

    let res = exchange(&server, &http, &code, &verifier).await;
    assert_eq!(res.status(), 204);
    let set_cookie = res.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(set_cookie.starts_with("__Host-purlay_session="), "{set_cookie}");
    for attr in ["Max-Age=2592000", "Path=/", "HttpOnly", "Secure", "SameSite=Lax"] {
        assert!(set_cookie.contains(attr), "{set_cookie} lacks {attr}");
    }
    let value = set_cookie.split(';').next().unwrap().to_string();
    let me: MeResponse = get(&http, &server, "/api/me", &value).await;
    assert_eq!(me.user.username, "ada");
    let sessions: DeviceSessionList = get(&http, &server, "/api/me/sessions", &value).await;
    let current = sessions.data.iter().find(|s| s.current).expect("the current session");
    assert_eq!(current.kind, DeviceSessionKind::Native);
    assert!(current.name.as_deref().unwrap_or("").contains("TEST-PC"));

    // Single use.
    let again = exchange(&server, &http, &code, &verifier).await;
    assert_eq!(error_of(again).await, (404, "NATIVE_CODE_NOT_FOUND".into()));
}

#[tokio::test]
async fn native_sign_in_refuses_a_wrong_verifier() {
    let (server, http) = start().await;
    let (verifier, challenge, state) = pkce();
    let code = browser_sign_in(&server, &http, "grace", &challenge, &state).await;
    let (other_verifier, _, _) = pkce();
    let res = exchange(&server, &http, &code, &other_verifier).await;
    assert_eq!(error_of(res).await, (404, "NATIVE_CODE_NOT_FOUND".into()));
    // A wrong verifier does not burn the code before the fifth.
    let res = exchange(&server, &http, &code, &verifier).await;
    assert_eq!(res.status(), 204);
    let res = exchange(&server, &http, &"0".repeat(64), &verifier).await;
    assert_eq!(error_of(res).await, (404, "NATIVE_CODE_NOT_FOUND".into()));
}

#[tokio::test]
async fn threads_reactions_and_reads() {
    let (server, http) = start().await;
    let cookie = server.cookie_header("ada");
    let general = server.channel_id("general").unwrap();
    let mut ws = open_socket(&server, Some(&cookie), Some(server.origin())).await.expect("upgrade");
    let sub_id = purlay_devserver::ids::new_id();
    send_frame(
        &mut ws,
        &ClientFrame::Subscribe(SubscribeFrame {
            id: sub_id.clone(),
            payload: SubscribeFramePayload {
                channels: vec![SubscribeFramePayloadChannelsItem { channel_id: general.clone(), after_message_id: None }],
            },
        }),
    )
    .await;
    wait_for(&mut ws, "the ack", |f| ack_ref(f) == Some(&sub_id)).await;

    let page: MessagePage = get(&http, &server, &format!("/api/channels/{general}/messages?limit=1"), &cookie).await;
    let root = page.data[0].id.clone();

    // A reaction: the whole set back, REACTIONS_UPDATED to the room.
    let res = http
        .put(url(&server, &format!("/api/messages/{root}/reactions/%F0%9F%91%8D")))
        .header("Cookie", &cookie)
        .header("Content-Length", "0")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let set: ReactionsResponse = res.json().await.unwrap();
    assert!(set.reactions.iter().any(|r| r.emoji == "👍"));
    wait_for(&mut ws, "REACTIONS_UPDATED", |f| matches!(f, ServerFrame::ReactionsUpdated(r) if r.data.message_id == root)).await;

    // A reply: THREAD_UPDATED with the count.
    let reply = SendMessageRequest {
        id: purlay_devserver::ids::new_id(),
        content: "A reply".into(),
        attachment_ids: None,
        parent_id: Some(root.clone()),
    };
    let res = http.post(url(&server, &format!("/api/channels/{general}/messages"))).header("Cookie", &cookie).json(&reply).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let t = wait_for(&mut ws, "THREAD_UPDATED", |f| matches!(f, ServerFrame::ThreadUpdated(t) if t.data.message_id == root)).await;
    if let ServerFrame::ThreadUpdated(t) = t {
        assert!(t.data.thread.reply_count >= 1);
    }
    let thread: MessagePage = get(&http, &server, &format!("/api/messages/{root}/thread"), &cookie).await;
    assert_eq!(thread.data.last().unwrap().id, reply.id);

    // Reading the channel answers the state and tells our sockets.
    let res = http
        .put(url(&server, &format!("/api/channels/{general}/read")))
        .header("Cookie", &cookie)
        .json(&ReadChannelRequest { last_read_message_id: root.clone() })
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let state: ChannelState = res.json().await.unwrap();
    assert_eq!(state.unread_count, 0);
    assert_eq!(state.last_read_message_id, Some(root));
}
