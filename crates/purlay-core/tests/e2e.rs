//! The engine against the in-memory Purlay server, end to end: sign-in, the session, live
//! messages, optimistic sends, reactions, threads, read state and a dropped connection.

use purlay_contract::models::*;
use purlay_contract::Uuid;
use purlay_core::api::ApiClient;
use purlay_core::auth::{self, Handoff, StoredSession};
use purlay_core::engine::{Command, Event};
use purlay_core::handshake::{self, Handshake};
use purlay_core::prefs::Prefs;
use purlay_core::realtime::connection::Status;
use purlay_core::session::{Session, Started};
use purlay_core::store::{Delivery, Store};
use purlay_devserver::{Config, ServerHandle};
use std::time::Duration;
use tokio::sync::mpsc;

async fn server() -> ServerHandle {
    let (_addr, h) = purlay_devserver::spawn(Config::default()).await.expect("dev server");
    h
}

async fn open(h: &ServerHandle, who: &str, dir: &std::path::Path) -> (Session, mpsc::UnboundedReceiver<Event>) {
    let address = handshake::parse_server_address(h.origin(), true).unwrap();
    let Handshake::Ready { info, origin } = handshake::handshake(&address, "0.1.0").await.unwrap() else { panic!("handshake") };
    let stored = StoredSession { cookie: h.issue_session(who), expires_at_ms: None, origin: handshake::origin_text(&origin) };
    let Started { session, events } = Session::open(tokio::runtime::Handle::current(), info, origin, stored, Prefs::in_memory(), Some(dir.to_path_buf())).await.expect("session");
    (session, events)
}

/// Waits until the store satisfies `f`, or fails after a few seconds.
async fn until(s: &Session, what: &str, f: impl Fn(&Store) -> bool) {
    let mut rev = s.engine.revisions.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if f(&s.store().read()) {
            return;
        }
        if tokio::time::timeout_at(deadline, rev.changed()).await.is_err() {
            let st = s.store().read();
            let windows: Vec<String> = st.windows.iter().map(|(k, w)| format!("{k}: {} msgs, newest={}", w.messages.len(), w.reaches_newest)).collect();
            panic!("timed out waiting for: {what}\n status={:?}\n pending={:#?}\n windows={windows:?}\n loading={:?}", st.status, st.pending, st.window_loading);
        }
    }
}

fn general(h: &ServerHandle) -> Uuid {
    h.channel_id("general").expect("#general")
}

#[tokio::test(flavor = "multi_thread")]
async fn native_sign_in_exchanges_the_code_for_a_session() {
    let h = server().await;
    let origin = url::Url::parse(h.origin()).unwrap();
    let api = ApiClient::new(origin.clone(), "0.1.0");
    let handoff = Handoff::new();
    // What the server's sign-in page does in the browser, with the browser's session.
    let browser = ApiClient::new(origin.clone(), "0.1.0");
    browser.set_session(Some(h.issue_session("ada")));
    let r = browser.native_authorize(&NativeAuthorizeRequest { challenge: handoff.challenge.clone(), state: handoff.state.clone() }).await.unwrap();
    let code = handoff.read_callback(&r.redirect).expect("callback");
    let stored = auth::exchange(&api, &handoff, &code, 0).await.expect("exchange");
    api.set_session(Some(stored.cookie.clone()));
    let me = api.get_me().await.unwrap();
    assert_eq!(me.user.username, "ada");
    // The code is single use.
    assert!(matches!(auth::exchange(&api, &handoff, &code, 0).await, Err(auth::ExchangeError::CodeNotFound)));
}

#[tokio::test(flavor = "multi_thread")]
async fn opens_reads_channels_and_connects() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let (s, _ev) = open(&h, "ada", dir.path()).await;
    until(&s, "socket open", |st| st.status == Some(Status::Open)).await;
    let w = h.workspace_id();
    until(&s, "channels", |st| st.channels.get(&w).is_some_and(|m| m.len() >= 4)).await;
    let g = general(&h);
    s.open_channel(&g);
    until(&s, "#general feed", |st| st.windows.get(&g).is_some_and(|w| w.messages.len() >= 40)).await;
    let st = s.store().read();
    let win = st.windows.get(&g).unwrap();
    assert!(win.reaches_newest);
    assert!(win.messages.windows(2).all(|p| p[0].id < p[1].id), "sorted by id");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_is_painted_then_acknowledged_and_echoed() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let (s, _ev) = open(&h, "ada", dir.path()).await;
    let g = general(&h);
    until(&s, "socket open", |st| st.status == Some(Status::Open)).await;
    s.open_channel(&g);
    until(&s, "feed", |st| st.windows.contains_key(&g)).await;
    let id = s.send_message(&g, None, "hello from **Windows**".into(), vec![]).await.unwrap();
    // Painted at once as pending, then the echo replaces it.
    until(&s, "echo", |st| st.windows.get(&g).is_some_and(|w| w.get(&id).is_some())).await;
    until(&s, "pending retired", |st| st.pending.iter().all(|p| p.id != id)).await;
    let st = s.store().read();
    assert_eq!(st.windows[&g].get(&id).unwrap().content, "hello from **Windows**");
    assert!(st.pending.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn someone_elses_message_arrives_live() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let (s, mut ev) = open(&h, "ada", dir.path()).await;
    let g = general(&h);
    until(&s, "socket open", |st| st.status == Some(Status::Open)).await;
    s.open_channel(&g);
    until(&s, "feed", |st| st.windows.contains_key(&g)).await;
    // Wait until #general is subscribed: give the SUBSCRIBE its answer.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let grace = ApiClient::new(url::Url::parse(h.origin()).unwrap(), "0.1.0");
    grace.set_session(Some(h.issue_session("grace")));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let mid = purlay_core::ids::message_id(now, purlay_core::ids::random_u128());
    let body = SendMessageRequest { id: mid.clone(), content: "@ada ping from Grace".into(), attachment_ids: None, parent_id: None };
    grace.send_message(&g, &body).await.unwrap();
    until(&s, "live message", |st| st.windows.get(&g).is_some_and(|w| w.get(&mid).is_some())).await;
    // It is offered for a desktop notification.
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(e) = ev.recv().await {
            if let Event::Arrived(m) = e {
                if m.id == mid {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(got, "Arrived event");
}

#[tokio::test(flavor = "multi_thread")]
async fn reactions_threads_and_read_state() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let (s, _ev) = open(&h, "ada", dir.path()).await;
    let g = general(&h);
    until(&s, "socket open", |st| st.status == Some(Status::Open)).await;
    s.open_channel(&g);
    until(&s, "feed", |st| st.windows.get(&g).is_some_and(|w| !w.messages.is_empty())).await;
    let root = s.store().read().windows[&g].messages.last().cloned().unwrap();

    // A reaction is applied from the answer.
    s.react(&root, "🎉", true).await.unwrap().unwrap();
    until(&s, "reaction", |st| st.windows[&g].get(&root.id).is_some_and(|m| m.reactions.iter().any(|r| r.emoji == "🎉" && r.user_ids.contains(&s.user_id)))).await;

    // A reply in a thread shows in the thread and moves the root's summary.
    s.open_thread(&root.id, &g);
    until(&s, "thread loaded", |st| st.threads.get(&root.id).is_some_and(|t| t.loaded)).await;
    let reply = s.send_message(&g, Some(&root.id), "a reply".into(), vec![]).await.unwrap();
    until(&s, "reply in thread", |st| st.threads[&root.id].replies.get(&reply).is_some()).await;
    until(&s, "thread summary", |st| st.windows[&g].get(&root.id).and_then(|m| m.thread.as_ref()).is_some_and(|t| t.reply_count >= 1)).await;

    // Reading the feed clears its unread count.
    let newest = s.store().read().windows[&g].newest_id().cloned().unwrap();
    s.mark_read(&g, &newest);
    until(&s, "read", |st| st.channel(&g).and_then(|c| c.state.as_ref()).is_some_and(|x| x.unread_count == 0)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_connection_comes_back_and_sends_what_waited() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let (s, _ev) = open(&h, "ada", dir.path()).await;
    let g = general(&h);
    until(&s, "socket open", |st| st.status == Some(Status::Open)).await;
    s.open_channel(&g);
    until(&s, "feed", |st| st.windows.contains_key(&g)).await;

    h.drop_sockets();
    until(&s, "noticed the drop", |st| st.status != Some(Status::Open)).await;
    // Written while away: kept as pending (and in the outbox), sent over HTTP or after reconnecting.
    let id = s.send_message(&g, None, "written while the socket was down".into(), vec![]).await.unwrap();
    until(&s, "reconnected", |st| st.status == Some(Status::Open)).await;
    until(&s, "the waiting message arrived", |st| st.windows[&g].get(&id).is_some()).await;
    until(&s, "nothing pending", |st| st.pending.iter().all(|p| p.delivery != Delivery::Sending)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsent_message_survives_a_restart_as_failed_and_retries() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let g = general(&h);
    // A message kept in the outbox by an earlier run.
    let (s1, _ev) = open(&h, "ada", dir.path()).await;
    let user = s1.user_id.to_string();
    s1.shutdown();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let id = purlay_core::ids::message_id(now as u64, purlay_core::ids::random_u128());
    let outbox_dir = dir.path().join("outbox").join(purlay_core::outbox::sanitize(h.origin().trim_end_matches('/')));
    let pending = purlay_core::store::PendingMessage {
        id: id.clone(),
        channel_id: g.clone(),
        parent_id: None,
        content: "from before the restart".into(),
        created_at: purlay_contract::Timestamp::from_unix_ms(now),
        delivery: Delivery::Sending,
        reason: None,
        attachments: vec![],
    };
    purlay_core::outbox::Outbox::new(Some(&outbox_dir), &user).save(&[pending]);

    let (s, _ev) = open(&h, "ada", dir.path()).await;
    until(&s, "restored as failed", |st| st.pending.iter().any(|p| p.id == id && p.delivery == Delivery::Failed)).await;
    until(&s, "socket open", |st| st.status == Some(Status::Open)).await;
    s.open_channel(&g);
    s.retry(&id);
    until(&s, "retried with the same id", |st| st.windows.get(&g).is_some_and(|w| w.get(&id).is_some())).await;
    until(&s, "outbox empty", |st| st.pending.is_empty()).await;
    s.send(Command::Shutdown);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_mute_silences_then_gives_the_old_level_back() {
    let h = server().await;
    let dir = tempfile::tempdir().unwrap();
    let (s, _ev) = open(&h, "ada", dir.path()).await;
    let g = general(&h);
    let w = h.workspace_id();
    until(&s, "channels", |st| st.channels.get(&w).is_some_and(|m| m.contains_key(&g))).await;
    s.set_notification_level(&g, NotificationLevel::All).await.unwrap().unwrap();

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    s.mute_until(&g, now + 60_000).await.unwrap().unwrap();
    assert_eq!(s.muted_until(&g), Some(now + 60_000));
    until(&s, "muted", |st| st.channel(&g).is_some_and(|c| c.notification_level == Some(NotificationLevel::None))).await;

    // Muting again keeps the level from before the first mute.
    s.mute_until(&g, now + 120_000).await.unwrap().unwrap();
    // Not yet due: nothing changes.
    s.end_timed_mutes(now + 30_000);
    assert!(s.muted_until(&g).is_some());
    // Due: back to "all", and the mute is forgotten.
    s.end_timed_mutes(now + 200_000);
    assert_eq!(s.muted_until(&g), None);
    until(&s, "restored", |st| st.channel(&g).is_some_and(|c| c.notification_level == Some(NotificationLevel::All))).await;

    // A level chosen by hand ends a timed mute without waiting.
    s.mute_until(&g, now + 60_000).await.unwrap().unwrap();
    s.set_notification_level(&g, NotificationLevel::Mentions).await.unwrap().unwrap();
    assert_eq!(s.muted_until(&g), None);
    s.end_timed_mutes(now + 200_000);
    until(&s, "kept the hand choice", |st| st.channel(&g).is_some_and(|c| c.notification_level == Some(NotificationLevel::Mentions))).await;
}
