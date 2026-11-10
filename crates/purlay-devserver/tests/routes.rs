//! Every read route answers something the contract's types decode, for a seeded person,
//! and the personal writes round-trip. Plus live mode.

use futures::{SinkExt, StreamExt};
use purlay_contract::models::*;
use purlay_devserver::{Config, ServerHandle, spawn};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct T {
    server: ServerHandle,
    http: reqwest::Client,
    cookie: String,
}

impl T {
    async fn new() -> T {
        let (_, server) = spawn(Config::default()).await.unwrap();
        let cookie = server.cookie_header("ada");
        T { server, http: reqwest::Client::new(), cookie }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.server.origin(), path)
    }

    async fn get<R: DeserializeOwned>(&self, path: &str) -> R {
        let res = self.http.get(self.url(path)).header("Cookie", &self.cookie).send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        assert!(status.is_success(), "GET {path}: {status} {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("GET {path}: {e}\n{text}"))
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> reqwest::Response {
        let mut req = self.http.request(method, self.url(path)).header("Cookie", &self.cookie);
        req = match body {
            Some(b) => req.json(&b),
            None => req.header("Content-Length", "0"),
        };
        req.send().await.unwrap()
    }
}

#[tokio::test]
async fn read_routes_decode() {
    let t = T::new().await;
    let ws = t.server.workspace_id();
    let general = t.server.channel_id("general").unwrap();

    let _: MembershipList = t.get("/api/workspaces").await;
    let _: Workspace = t.get(&format!("/api/workspaces/{ws}")).await;
    let members: WorkspaceMemberList = t.get(&format!("/api/workspaces/{ws}/members")).await;
    assert_eq!(members.len(), 5);
    let _: PresenceSnapshot = t.get(&format!("/api/workspaces/{ws}/presence")).await;
    let _: Channel = t.get(&format!("/api/channels/{general}")).await;
    let _: ChannelMemberList = t.get(&format!("/api/channels/{general}/members")).await;
    let pins: PinList = t.get(&format!("/api/channels/{general}/pins")).await;
    assert_eq!(pins.len(), 1);
    let bookmarks: BookmarkList = t.get(&format!("/api/channels/{general}/bookmarks")).await;
    assert_eq!(bookmarks.len(), 1);
    let files: FilePage = t.get(&format!("/api/channels/{general}/files")).await;
    assert_eq!(files.data.len(), 2);
    let image = files.data.iter().find(|f| f.attachment.kind == AttachmentKind::Image).expect("the image");
    assert_eq!((image.attachment.width, image.attachment.height), (Some(480), Some(270)));
    let png = t.http.get(t.url(&format!("/api/attachments/{}", image.attachment.id))).header("Cookie", &t.cookie).send().await.unwrap();
    assert_eq!(png.headers()["content-type"], "image/png");
    assert!(png.bytes().await.unwrap().starts_with(b"\x89PNG"));

    let unreads: ChannelCatchUp = t.get(&format!("/api/channels/{general}/unreads")).await;
    assert!(!unreads.roots.data.is_empty());
    assert_eq!(unreads.threads.len(), 1, "the sidebar thread waits");
    let changes: ChangePage = t.get(&format!("/api/channels/{general}/changes")).await;
    assert!(changes.data.is_empty());
    let since = changes.as_of.clone();
    let _: ChangePage = t.get(&format!("/api/channels/{general}/changes?since={since}")).await;
    let page: MessagePage = t.get(&format!("/api/channels/{general}/messages?limit=100")).await;
    let threaded = page.data.iter().find(|m| m.thread.as_ref().is_some_and(|t| t.reply_count > 0)).unwrap();
    let _: MessagePage = t.get(&format!("/api/messages/{}/thread", threaded.id)).await;
    let _: ThreadFollow = t.get(&format!("/api/messages/{}/follow", threaded.id)).await;
    let window: MessageWindow = t.get(&format!("/api/messages/{}/around", threaded.id)).await;
    assert!(window.data.iter().any(|m| m.id == threaded.id));
    let _: Message = t.get(&format!("/api/messages/{}", threaded.id)).await;
    let _: MessageVersionPage = t.get(&format!("/api/messages/{}/versions", threaded.id)).await;

    let notes: NotificationPage = t.get(&format!("/api/notifications?workspaceId={ws}&unreadOnly=true")).await;
    assert!(notes.data.iter().any(|n| n.r#type == NotificationType::Mention));
    assert!(notes.data.iter().any(|n| n.r#type == NotificationType::DirectMessage));
    let mentions: NotificationPage = t.get(&format!("/api/notifications?workspaceId={ws}&types=mention")).await;
    assert!(mentions.data.iter().all(|n| n.r#type == NotificationType::Mention));
    let inbox: InboxPage = t.get(&format!("/api/workspaces/{ws}/inbox")).await;
    assert_eq!(inbox.unread_count, 1);
    let issues: IssuePage = t.get(&format!("/api/workspaces/{ws}/issues")).await;
    assert_eq!(issues.data.len(), 2);
    let saved: SavedPage = t.get(&format!("/api/workspaces/{ws}/saved?state=in_progress")).await;
    assert_eq!(saved.data.len(), 1);
    let _: SavedIds = t.get(&format!("/api/workspaces/{ws}/saved/ids")).await;
    let _: MessageReminderList = t.get(&format!("/api/workspaces/{ws}/reminders")).await;
    let _: ScheduledMessageList = t.get(&format!("/api/workspaces/{ws}/scheduled")).await;
    let unresolved: UnresolvedPage = t.get(&format!("/api/workspaces/{ws}/unresolved")).await;
    assert!(!unresolved.data.is_empty());
    let search: SearchPage = t.get(&format!("/api/workspaces/{ws}/search?q=SIDEBAR")).await;
    assert!(!search.data.is_empty());
    let find: FindResponse = t.get(&format!("/api/workspaces/{ws}/find?q=reaction")).await;
    assert_eq!(find.groups.len(), 2);
    assert!(find.groups.iter().all(|g| !g.hits.is_empty()));
    let groups: ChannelGroupPage = t.get(&format!("/api/workspaces/{ws}/channel-groups")).await;
    assert!(groups.data.is_empty());
    let _: SavedSearchPage = t.get(&format!("/api/workspaces/{ws}/saved-searches")).await;
    let _: DeviceSessionList = t.get("/api/me/sessions").await;
    let _: LatestReleaseAnswer = t.get("/api/releases/latest").await;
    let _: AuthMethods = t.get("/api/auth/methods").await;
}

#[tokio::test]
async fn personal_writes_round_trip() {
    let t = T::new().await;
    let ws = t.server.workspace_id();
    let general = t.server.channel_id("general").unwrap();
    let design = t.server.channel_id("design").unwrap();
    let page: MessagePage = t.get(&format!("/api/channels/{general}/messages?limit=3")).await;
    let m = &page.data[2].id;

    // Join a channel that was not joined.
    let res = t.send(reqwest::Method::POST, &format!("/api/channels/{design}/join"), None).await;
    assert_eq!(res.status(), 200);
    assert!(res.json::<Channel>().await.unwrap().state.is_some());

    // Save, star, pin, reminder, scheduled.
    assert_eq!(t.send(reqwest::Method::PUT, &format!("/api/messages/{m}/saved"), Some(serde_json::json!({}))).await.status(), 204);
    let ids: SavedIds = t.get(&format!("/api/workspaces/{ws}/saved/ids")).await;
    assert!(ids.iter().any(|s| s.message_id == *m));
    assert_eq!(t.send(reqwest::Method::PUT, &format!("/api/channels/{general}/star"), None).await.status(), 204);
    assert_eq!(t.send(reqwest::Method::PUT, &format!("/api/messages/{m}/pin"), None).await.status(), 204);
    let at = purlay_contract::Timestamp::from_unix_ms(purlay_devserver::ids::now_ms() + 3_600_000);
    let res = t.send(reqwest::Method::PUT, &format!("/api/messages/{m}/reminder"), Some(serde_json::json!({ "remindAt": at }))).await;
    assert_eq!(res.status(), 200);
    let sid = purlay_devserver::ids::new_id();
    let res = t
        .send(
            reqwest::Method::PUT,
            &format!("/api/channels/{general}/scheduled/{sid}"),
            Some(serde_json::json!({ "content": "later", "sendAt": at })),
        )
        .await;
    assert_eq!(res.status(), 200);
    let list: ScheduledMessageList = t.get(&format!("/api/workspaces/{ws}/scheduled")).await;
    assert_eq!(list.len(), 1);

    // Edit and delete one's own message; versions are kept.
    let id = purlay_devserver::ids::new_id();
    let body = serde_json::json!({ "id": id, "content": "first words" });
    assert_eq!(t.send(reqwest::Method::POST, &format!("/api/channels/{general}/messages"), Some(body)).await.status(), 200);
    let res = t.send(reqwest::Method::PATCH, &format!("/api/messages/{id}"), Some(serde_json::json!({ "content": "second words" }))).await;
    assert_eq!(res.json::<Message>().await.unwrap().content, "second words");
    let versions: MessageVersionPage = t.get(&format!("/api/messages/{id}/versions")).await;
    assert_eq!(versions.data[0].content, "first words");
    assert_eq!(t.send(reqwest::Method::DELETE, &format!("/api/messages/{id}"), None).await.status(), 204);

    // Upload, attach, download.
    let up = purlay_devserver::ids::new_id();
    let res = t
        .http
        .post(t.url(&format!("/api/workspaces/{ws}/uploads?id={up}&name=notes.txt")))
        .header("Cookie", &t.cookie)
        .header("Content-Type", "application/octet-stream")
        .body("hello file")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let att: Attachment = res.json().await.unwrap();
    assert_eq!(att.kind, AttachmentKind::File);
    let id = purlay_devserver::ids::new_id();
    let body = serde_json::json!({ "id": id, "content": "", "attachmentIds": [up] });
    let res = t.send(reqwest::Method::POST, &format!("/api/channels/{general}/messages"), Some(body)).await;
    assert_eq!(res.json::<Message>().await.unwrap().attachments.len(), 1);
    let bytes = t.http.get(t.url(&format!("/api/attachments/{up}"))).header("Cookie", &t.cookie).send().await.unwrap();
    assert_eq!(bytes.text().await.unwrap(), "hello file");

    // Issues: assigning someone tells their inbox.
    let linus = t.server.user_id("linus").unwrap();
    let res = t
        .send(reqwest::Method::POST, &format!("/api/workspaces/{ws}/issues"), Some(serde_json::json!({ "title": "Try it", "assigneeId": linus })))
        .await;
    assert_eq!(res.status(), 201);
    let issue: Issue = res.json().await.unwrap();
    let res = t
        .send(reqwest::Method::PATCH, &format!("/api/workspaces/{ws}/issues/{}", issue.id), Some(serde_json::json!({ "status": "done" })))
        .await;
    assert_eq!(res.json::<Issue>().await.unwrap().status, IssueStatus::Done);
    let linus_inbox = t.server.with_world(|w| w.inbox.iter().filter(|r| r.user == linus).count());
    assert_eq!(linus_inbox, 1);

    // Mark everything read.
    let res = t.send(reqwest::Method::PUT, &format!("/api/workspaces/{ws}/read"), None).await;
    let read: ReadAllChannelsResponse = res.json().await.unwrap();
    assert!(read.data.iter().all(|c| c.state.unread_count == 0));
    assert_eq!(t.send(reqwest::Method::PUT, "/api/notifications/read", Some(serde_json::json!({ "workspaceId": ws }))).await.status(), 204);
    assert_eq!(t.send(reqwest::Method::POST, &format!("/api/workspaces/{ws}/inbox/read"), Some(serde_json::json!({ "all": true }))).await.status(), 204);
    let inbox: InboxPage = t.get(&format!("/api/workspaces/{ws}/inbox")).await;
    assert_eq!(inbox.unread_count, 0);
}

#[tokio::test]
async fn live_mode_types_and_posts() {
    let config = Config { live: true, live_every: Duration::from_millis(200), ..Config::default() };
    let (_, server) = spawn(config).await.unwrap();
    let random = server.channel_id("random").unwrap();
    let mut req = server.socket_url().into_client_request().unwrap();
    req.headers_mut().insert("Cookie", server.cookie_header("ada").parse().unwrap());
    req.headers_mut().insert("Origin", server.origin().parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let sub = ClientFrame::Subscribe(SubscribeFrame {
        id: purlay_devserver::ids::new_id(),
        payload: SubscribeFramePayload { channels: vec![SubscribeFramePayloadChannelsItem { channel_id: random, after_message_id: None }] },
    });
    ws.send(WsMessage::text(serde_json::to_string(&sub).unwrap())).await.unwrap();
    let (mut typed, mut changed) = (false, false);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !(typed && changed) {
        let msg = tokio::time::timeout_at(deadline, ws.next()).await.expect("the bot acts").unwrap().unwrap();
        let WsMessage::Text(text) = msg else { continue };
        match serde_json::from_str::<ServerFrame>(text.as_str()).unwrap() {
            ServerFrame::TypingServer(_) => typed = true,
            ServerFrame::MessageCreated(_) | ServerFrame::ReactionsUpdated(_) => changed = true,
            _ => {}
        }
    }
    server.shutdown().await;
}
