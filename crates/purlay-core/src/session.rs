//! One signed-in server: the API client, the engine and its store, and every action the UI
//! can take. Methods never block: they run on the tokio runtime and either update the
//! store (the UI redraws on the next revision) or return a handle the UI can await.

use crate::api::{ApiClient, ApiError, ApiResult};
use crate::auth::{credentials, StoredSession};
use crate::clock::{SharedClock, SystemClock};
use crate::engine::{self, Command, EngineConfig, EngineHandle, Event};
use crate::outbox::{sanitize, Outbox};
use crate::prefs::Prefs;
use crate::store::{Key, Store};
use parking_lot::RwLock;
use purlay_contract::models::*;
use purlay_contract::{Timestamp, Uuid};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct Session {
    pub rt: Handle,
    pub api: ApiClient,
    pub origin: url::Url,
    pub info: ClientInfo,
    pub user_id: Uuid,
    pub engine: EngineHandle,
    pub prefs: Prefs,
}

pub struct Started {
    pub session: Session,
    pub events: mpsc::UnboundedReceiver<Event>,
}

impl Session {
    /// Opens a session with a stored cookie: reads `/api/me`, then starts the engine.
    pub async fn open(rt: Handle, info: ClientInfo, origin: url::Url, stored: StoredSession, prefs: Prefs, data_dir: Option<PathBuf>) -> ApiResult<Started> {
        let api = ApiClient::new(origin.clone(), APP_VERSION);
        api.set_session(Some(stored.cookie.clone()));
        let me = api.get_me().await?;
        let user_id = me.user.id.clone();
        let outbox_dir = data_dir.map(|d| d.join("outbox").join(sanitize(&crate::handshake::origin_text(&origin))));
        let outbox = Outbox::new(outbox_dir.as_deref(), &user_id);
        let store = Arc::new(RwLock::new(Store::default()));
        {
            let mut s = store.write();
            s.client_info = Some(info.clone());
            s.remember_user(&User {
                id: me.user.id.clone(),
                username: me.user.username.clone(),
                display_name: me.user.display_name.clone(),
                avatar_url: me.user.avatar_url.clone(),
            });
        }
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let clock: SharedClock = Arc::new(SystemClock);
        let socket_origin = crate::handshake::origin_text(&url::Url::parse(&info.app_url).unwrap_or(origin.clone()));
        let engine = {
            let _guard = rt.enter();
            engine::spawn(EngineConfig { api: api.clone(), socket_origin, user_id: user_id.clone(), outbox, clock }, store, ev_tx.clone())
        };
        let ev = ev_tx.clone();
        api.on_session_rejected(Arc::new(move || {
            let _ = ev.send(Event::SessionEnded("expired".into()));
        }));
        engine.send(Command::Mutate(Box::new(move |s| s.me = Some(me))));
        engine.send(Command::LoadMe);
        engine.send(Command::Start);
        Ok(Started { session: Session { rt, api, origin, info, user_id, engine, prefs }, events: ev_rx })
    }

    pub fn store(&self) -> &Arc<RwLock<Store>> {
        &self.engine.store
    }

    pub fn send(&self, c: Command) {
        self.engine.send(c);
    }

    pub fn server_key(&self) -> String {
        crate::handshake::origin_text(&self.origin)
    }

    pub fn capability(&self, f: impl Fn(&ClientInfoCapabilities) -> Option<bool>) -> bool {
        self.info.capabilities.as_ref().and_then(f) == Some(true)
    }

    fn spawn<T: Send + 'static>(&self, f: impl std::future::Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        self.rt.spawn(f)
    }

    fn mutate(&self, f: impl FnOnce(&mut Store) + Send + 'static) {
        self.engine.send(Command::Mutate(Box::new(f)));
    }

    /// Runs an action; its answer is applied to the store with `apply`.
    fn act<T: Send + 'static>(
        &self,
        f: impl std::future::Future<Output = ApiResult<T>> + Send + 'static,
        apply: impl FnOnce(&mut Store, T) + Send + 'static,
    ) -> JoinHandle<ApiResult<()>> {
        let engine = self.engine.clone();
        self.spawn(async move {
            let v = f.await?;
            engine.send(Command::Mutate(Box::new(move |s| apply(s, v))));
            Ok(())
        })
    }

    // --- reading -------------------------------------------------------------------

    pub fn open_channel(&self, c: &Uuid) {
        self.send(Command::OpenChannel(c.clone()));
    }

    pub fn open_around(&self, c: &Uuid, m: &Uuid) {
        self.send(Command::OpenAround(c.clone(), m.clone()));
    }

    pub fn open_thread(&self, root: &Uuid, channel: &Uuid) {
        self.send(Command::OpenThread { root: root.clone(), channel: channel.clone() });
    }

    pub fn set_active(&self, keys: Vec<Key>) {
        self.send(Command::SetActive(keys));
    }

    pub fn refresh(&self, key: Key) {
        self.send(Command::Refresh(key));
    }

    // --- sending ---------------------------------------------------------------------

    /// Sends a message (painted at once as `sending`); resolves to its id.
    pub fn send_message(&self, channel: &Uuid, parent: Option<&Uuid>, content: String, attachments: Vec<Attachment>) -> oneshot::Receiver<Uuid> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Send { channel_id: channel.clone(), parent_id: parent.cloned(), content, attachments, reply: Some(tx) });
        rx
    }

    pub fn retry(&self, id: &Uuid) {
        self.send(Command::Retry(id.clone()));
    }

    pub fn discard(&self, id: &Uuid) {
        self.send(Command::Discard(id.clone()));
    }

    pub fn typing(&self, channel: &Uuid) {
        self.send(Command::Typing(channel.clone()));
    }

    pub fn active(&self, workspace: &Uuid) {
        self.send(Command::Active(workspace.clone()));
    }

    /// Uploads a file as soon as it is picked; the message later names its id.
    pub fn upload(&self, workspace: &Uuid, name: String, bytes: Vec<u8>) -> JoinHandle<ApiResult<Attachment>> {
        let api = self.api.clone();
        let w = workspace.clone();
        let id = {
            let mut g = crate::ids::IdGen::default();
            g.next(SystemClock.now_ms_compat(), crate::ids::random_u128())
        };
        self.spawn(async move { api.upload_file(&w, &UploadQuery { id, name }, bytes).await })
    }

    pub fn discard_upload(&self, attachment: &Uuid) {
        let api = self.api.clone();
        let a = attachment.clone();
        self.spawn(async move {
            let _ = api.discard_attachment(&a).await;
        });
    }

    pub fn attachment_bytes(&self, attachment: &Uuid, thumbnail: bool) -> JoinHandle<ApiResult<bytes::Bytes>> {
        let api = self.api.clone();
        let a = attachment.clone();
        self.spawn(async move {
            if thumbnail { api.download_attachment_thumbnail(&a).await } else { api.download_attachment(&a).await }
        })
    }

    pub fn external_image(&self, url: &str) -> JoinHandle<ApiResult<bytes::Bytes>> {
        let api = self.api.clone();
        let q = ExternalImageQuery { url: url.to_string() };
        self.spawn(async move { api.fetch_external_image(&q).await })
    }

    // --- message actions -------------------------------------------------------------

    pub fn edit(&self, m: &Message, content: String) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        self.act(async move { api.edit_message(&id, &EditMessageRequest { content }).await }, |s, msg: Message| {
            let (c, i) = (msg.channel_id.clone(), msg.id.clone());
            s.for_message(&c, &i, |x| *x = msg.clone());
        })
    }

    pub fn delete(&self, m: &Message) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        self.spawn(async move { api.delete_message(&id).await })
    }

    /// "Delete for me" (`personalMessageDeletion`).
    pub fn hide(&self, m: &Message) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        self.act(async move { api.hide_message(&id).await }, |s, msg: Message| {
            if let Some(w) = s.windows.get_mut(&msg.channel_id) {
                w.hide(&msg.id);
            }
            for t in s.threads.values_mut() {
                t.replies.hide(&msg.id);
            }
        })
    }

    pub fn react(&self, m: &Message, emoji: &str, add: bool) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let (id, ch, e) = (m.id.clone(), m.channel_id.clone(), emoji.to_string());
        if add {
            self.prefs.push_recent_emoji(emoji);
        }
        self.act(
            async move { if add { api.put_reaction(&id, &e).await } else { api.delete_reaction(&id, &e).await } },
            {
                let (id, ch) = (m.id.clone(), ch);
                move |s, r: ReactionsResponse| {
                    s.for_message(&ch, &id, |x| x.reactions = r.reactions.clone());
                }
            },
        )
    }

    pub fn pin(&self, m: &Message, on: bool) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        self.spawn(async move { if on { api.pin_message(&id).await } else { api.unpin_message(&id).await } })
    }

    pub fn save(&self, m: &Message, state: Option<SavedState>) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        let st = state.map(|s| SaveMessageRequestState::parse(s.as_str()));
        self.spawn(async move { api.save_message(&id, &SaveMessageRequest { state: st }).await })
    }

    pub fn unsave(&self, m: &Message) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        self.spawn(async move { api.unsave_message(&id).await })
    }

    pub fn resolve(&self, root: &Message, resolved: bool) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = root.id.clone();
        self.act(async move { if resolved { api.resolve_thread(&id).await } else { api.reopen_thread(&id).await } }, |s, msg: Message| {
            let (c, i) = (msg.channel_id.clone(), msg.id.clone());
            s.for_message(&c, &i, |x| x.thread = msg.thread.clone());
        })
    }

    pub fn follow(&self, root: &Uuid, following: bool) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = root.clone();
        self.act(async move { api.set_thread_follow(&id, &SetThreadFollowRequest { following }).await }, {
            let id = root.clone();
            move |s, f: ThreadFollow| {
                if let Some(t) = s.threads.get_mut(&id) {
                    t.following = Some(f.following);
                }
                s.cache.set(Key::ThreadFollow(id), f);
            }
        })
    }

    pub fn remind(&self, m: &Message, at: Timestamp) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.id.clone();
        self.spawn(async move { api.set_message_reminder(&id, &SetMessageReminderRequest { remind_at: at }).await.map(|_| ()) })
    }

    pub fn cancel_reminder(&self, m: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = m.clone();
        self.spawn(async move { api.cancel_message_reminder(&id).await })
    }

    pub fn schedule(&self, channel: &Uuid, parent: Option<&Uuid>, content: String, at: Timestamp, attachments: Vec<Uuid>) -> JoinHandle<ApiResult<ScheduledMessage>> {
        let api = self.api.clone();
        let c = channel.clone();
        let id = crate::ids::message_id(SystemClock.now_ms_compat() as u64, crate::ids::random_u128());
        let body = ScheduleMessageRequest {
            content,
            send_at: at,
            parent_id: parent.cloned(),
            attachment_ids: if attachments.is_empty() { None } else { Some(attachments) },
        };
        self.spawn(async move { api.schedule_message(&c, &id, &body).await })
    }

    pub fn cancel_scheduled(&self, channel: &Uuid, id: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let (c, i) = (channel.clone(), id.clone());
        self.spawn(async move { api.cancel_scheduled_message(&c, &i).await })
    }

    pub fn link_preview(&self, m: &Message, url: &str) {
        let key = (m.id.clone(), url.to_string());
        if self.store().read_recursive().link_previews.contains_key(&key) {
            return;
        }
        self.mutate({
            let key = key.clone();
            move |s| {
                s.link_previews.insert(key, None);
            }
        });
        let api = self.api.clone();
        let id = m.id.clone();
        let u = url.to_string();
        let engine = self.engine.clone();
        self.spawn(async move {
            if let Ok(r) = api.preview_message_link(&id, &WebLinkPreviewRequest { url: u }).await {
                engine.send(Command::Mutate(Box::new(move |s| {
                    s.link_previews.insert(key, r);
                })));
            }
        });
    }

    pub fn versions(&self, m: &Uuid) -> JoinHandle<ApiResult<MessageVersionPage>> {
        let api = self.api.clone();
        let id = m.clone();
        self.spawn(async move { api.list_message_versions(&id, &OlderPageQuery::default()).await })
    }

    pub fn set_knowledge(&self, root: &Uuid, req: SetKnowledgeRequest) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let id = root.clone();
        self.act(async move { api.set_knowledge(&id, &req).await }, |s, msg: Message| {
            let (c, i) = (msg.channel_id.clone(), msg.id.clone());
            s.for_message(&c, &i, |x| *x = msg.clone());
        })
    }

    // --- read state --------------------------------------------------------------------

    pub fn mark_read(&self, channel: &Uuid, last: &Uuid) {
        let api = self.api.clone();
        let (c, l) = (channel.clone(), last.clone());
        let already = self.store().read_recursive().channel(channel).and_then(|ch| ch.state.as_ref()).is_some_and(|st| st.unread_count == 0 && st.last_read_message_id.as_ref().is_some_and(|x| *x >= l));
        if already {
            return;
        }
        self.act(async move { api.mark_channel_read(&c, &ReadChannelRequest { last_read_message_id: l }).await }, {
            let c = channel.clone();
            move |s, st: ChannelState| {
                if let Some(ch) = s.channel_mut(&c) {
                    ch.state = Some(st);
                }
            }
        });
    }

    pub fn mark_unread(&self, channel: &Uuid, from: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let (c, f) = (channel.clone(), from.clone());
        self.act(async move { api.mark_channel_unread(&c, &MarkUnreadRequest { from_message_id: f }).await }, {
            let c = channel.clone();
            move |s, st: ChannelState| {
                if let Some(ch) = s.channel_mut(&c) {
                    ch.state = Some(st);
                }
            }
        })
    }

    pub fn mark_all_read(&self, workspace: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let w = workspace.clone();
        self.act(async move { api.mark_all_channels_read(&w).await }, |s, r: ReadAllChannelsResponse| {
            for item in r.data {
                if let Some(ch) = s.channel_mut(&item.channel_id) {
                    ch.state = Some(item.state);
                }
            }
        })
    }

    pub fn mark_thread_read(&self, root: &Uuid, last: &Uuid) {
        let api = self.api.clone();
        let (r, l) = (root.clone(), last.clone());
        self.spawn(async move {
            let _ = api.mark_thread_read(&r, &ReadThreadRequest { last_read_message_id: l }).await;
        });
    }

    pub fn mark_notifications_read(&self, req: ReadNotificationsRequest) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        self.spawn(async move { api.mark_notifications_read(&req).await })
    }

    pub fn mark_inbox_read(&self, workspace: &Uuid, req: MarkInboxReadRequest) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let w = workspace.clone();
        self.spawn(async move { api.mark_inbox_read(&w, &req).await })
    }

    // --- channels ----------------------------------------------------------------------

    fn channel_patch(&self, f: impl std::future::Future<Output = ApiResult<()>> + Send + 'static, channel: &Uuid, patch: impl FnOnce(&mut Channel) + Send + 'static) -> JoinHandle<ApiResult<()>> {
        let c = channel.clone();
        self.act(f, move |s, ()| {
            if let Some(ch) = s.channel_mut(&c) {
                patch(ch);
            }
        })
    }

    pub fn star(&self, channel: &Uuid, on: bool) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let c = channel.clone();
        self.channel_patch(async move { if on { api.star_channel(&c).await } else { api.unstar_channel(&c).await } }, channel, move |ch| {
            ch.starred = on;
            if on {
                ch.sidebar_group_id = None;
            }
        })
    }

    pub fn set_notification_level(&self, channel: &Uuid, level: NotificationLevel) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let c = channel.clone();
        let l = level.clone();
        self.channel_patch(async move { api.set_notification_level(&c, &SetNotificationLevelRequest { level: l }).await }, channel, move |ch| {
            ch.notification_level = Some(level)
        })
    }

    pub fn join(&self, channel: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let c = channel.clone();
        self.act(async move { api.join_channel(&c).await }, |s, ch: Channel| {
            s.channels.entry(ch.workspace_id.clone()).or_default().insert(ch.id.clone(), ch);
        })
    }

    pub fn leave(&self, channel: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let c = channel.clone();
        self.spawn(async move { api.leave_channel(&c).await })
    }

    pub fn create_channel(&self, workspace: &Uuid, req: CreateChannelRequest) -> JoinHandle<ApiResult<Channel>> {
        let api = self.api.clone();
        let w = workspace.clone();
        let engine = self.engine.clone();
        self.spawn(async move {
            let ch = api.create_channel(&w, &req).await?;
            let c2 = ch.clone();
            engine.send(Command::Mutate(Box::new(move |s| {
                s.channels.entry(c2.workspace_id.clone()).or_default().insert(c2.id.clone(), c2);
            })));
            Ok(ch)
        })
    }

    /// A direct message with one person, or a group conversation with several.
    pub fn open_conversation(&self, workspace: &Uuid, people: Vec<Uuid>) -> JoinHandle<ApiResult<Channel>> {
        let api = self.api.clone();
        let w = workspace.clone();
        let engine = self.engine.clone();
        self.spawn(async move {
            let ch = if people.len() == 1 {
                api.open_direct_message(&w, &CreateDirectMessageRequest { user_id: people[0].clone() }).await?
            } else {
                api.open_conversation(&w, &OpenConversationRequest { user_ids: people }).await?
            };
            let c2 = ch.clone();
            engine.send(Command::Mutate(Box::new(move |s| {
                s.channels.entry(c2.workspace_id.clone()).or_default().insert(c2.id.clone(), c2);
            })));
            engine.send(Command::Watch(vec![]));
            engine.send(Command::LoadChannels(w));
            Ok(ch)
        })
    }

    pub fn move_to_group(&self, channel: &Uuid, group: Option<Uuid>) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let c = channel.clone();
        self.act(async move { api.move_channel_to_group(&c, &MoveChannelGroupRequest { group_id: group }).await }, |s, ch: Channel| {
            if let Some(slot) = s.channel_mut(&ch.id) {
                *slot = ch;
            }
        })
    }

    pub fn add_bookmark(&self, channel: &Uuid, url: String, title: String) -> JoinHandle<ApiResult<ChannelBookmark>> {
        let api = self.api.clone();
        let c = channel.clone();
        self.spawn(async move { api.add_bookmark(&c, &AddBookmarkRequest { url, title }).await })
    }

    pub fn remove_bookmark(&self, channel: &Uuid, id: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let (c, i) = (channel.clone(), id.clone());
        self.spawn(async move { api.remove_bookmark(&c, &i).await })
    }

    // --- workspace & person ----------------------------------------------------------

    pub fn set_deep_work(&self, workspace: &Uuid, until: Option<Timestamp>) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let w = workspace.clone();
        let req = match until {
            Some(u) => SetDeepWorkRequest::ActiveOn(SetDeepWorkRequestActiveOn { until: u }),
            None => SetDeepWorkRequest::ActiveOff(SetDeepWorkRequestActiveOff {}),
        };
        self.act(async move { api.set_deep_work(&w, &req).await }, {
            let w = workspace.clone();
            move |s, r: DeepWorkResponse| {
                if let Some(me) = s.me.as_mut() {
                    for m in me.memberships.iter_mut().filter(|m| m.workspace.id == w) {
                        m.deep_work_until = r.until.clone();
                    }
                }
            }
        })
    }

    pub fn set_away(&self, away: bool) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        self.act(async move { api.set_away(&SetAwayRequest { away }).await }, move |s, _r: SetAwayResponse| {
            if let Some(me) = s.me.as_mut() {
                me.away_chosen = away;
            }
        })
    }

    pub fn search(&self, workspace: &Uuid, q: SearchQuery) -> JoinHandle<ApiResult<SearchPage>> {
        let api = self.api.clone();
        let w = workspace.clone();
        self.spawn(async move { api.search_messages(&w, &q).await })
    }

    pub fn find(&self, workspace: &Uuid, q: String) -> JoinHandle<ApiResult<FindResponse>> {
        let api = self.api.clone();
        let w = workspace.clone();
        self.spawn(async move { api.find_across_sources(&w, &FindQuery { q, limit: None }).await })
    }

    pub fn create_issue(&self, workspace: &Uuid, req: CreateIssueRequest) -> JoinHandle<ApiResult<Issue>> {
        let api = self.api.clone();
        let w = workspace.clone();
        let engine = self.engine.clone();
        self.spawn(async move {
            let i = api.create_issue(&w, &req).await?;
            engine.send(Command::Refresh(Key::Issues(w, IssueQueryStatus::Open)));
            Ok(i)
        })
    }

    pub fn update_issue(&self, workspace: &Uuid, id: &Uuid, req: UpdateIssueRequest) -> JoinHandle<ApiResult<Issue>> {
        let api = self.api.clone();
        let (w, i) = (workspace.clone(), id.clone());
        let engine = self.engine.clone();
        self.spawn(async move {
            let r = api.update_issue(&w, &i, &req).await?;
            engine.send(Command::Refresh(Key::Issues(w.clone(), IssueQueryStatus::Open)));
            engine.send(Command::Refresh(Key::Issues(w, IssueQueryStatus::Done)));
            Ok(r)
        })
    }

    pub fn delete_issue(&self, workspace: &Uuid, id: &Uuid) -> JoinHandle<ApiResult<()>> {
        let api = self.api.clone();
        let (w, i) = (workspace.clone(), id.clone());
        let engine = self.engine.clone();
        self.spawn(async move {
            api.delete_issue(&w, &i).await?;
            engine.send(Command::Refresh(Key::Issues(w.clone(), IssueQueryStatus::Open)));
            engine.send(Command::Refresh(Key::Issues(w, IssueQueryStatus::Done)));
            Ok(())
        })
    }

    /// The web page for a settings section: writes happen in the browser.
    pub fn web_url(&self, path: &str) -> String {
        let base = url::Url::parse(&self.info.app_url).unwrap_or(self.origin.clone());
        base.join(path).map(|u| u.to_string()).unwrap_or_default()
    }

    /// Signs this device (or every device) out, then forgets the credential and local data.
    pub fn sign_out(&self, everywhere: bool) -> JoinHandle<()> {
        let api = self.api.clone();
        let engine = self.engine.clone();
        let origin = self.server_key();
        let prefs = self.prefs.clone();
        self.spawn(async move {
            credentials::delete(&origin);
            let _ = if everywhere { api.logout_everywhere().await } else { api.logout().await };
            engine.send(Command::Mutate(Box::new(|s| s.pending.clear())));
            engine.send(Command::Shutdown);
            prefs.forget_account(&origin);
            api.set_session(None);
        })
    }

    pub fn shutdown(&self) {
        self.send(Command::Shutdown);
    }
}

trait NowCompat {
    fn now_ms_compat(&self) -> i64;
}

impl NowCompat for SystemClock {
    fn now_ms_compat(&self) -> i64 {
        crate::clock::Clock::now_ms(self)
    }
}

pub fn is_transient(e: &ApiError) -> bool {
    e.is_transient()
}
