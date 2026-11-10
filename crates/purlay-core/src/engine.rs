//! The client's realtime half: frames in, the store taught; a send out, painted at once.
//! A port of the web client's `engine.ts` onto one tokio task that owns the connection.
//!
//! A send (`ARCHITECTURE.md` §5.3):
//! 1. The id is a UUIDv7 made now against the corrected server clock: the message's id for
//!    good, so the row never moves when the ACK lands (D-8).
//! 2. The row is in the store as `sending` before anything leaves, and in the outbox file.
//! 3. The frame goes over the socket; if the socket is not open, over HTTP (same id).
//! 4. No ACK within `ACK_DEADLINE_MS` is `failed`, and Retry sends the *same id* again.
//! 5. `MESSAGE_CREATED` teaches the store the real row and retires the pending one; a
//!    reconnect re-sends what was still `sending`.
//! 6. What the server refuses is said, not swallowed: such channels go `degraded`.

use crate::api::{ApiClient, ApiError};
use crate::clock::SharedClock;
use crate::ids::is_plausible;
use crate::outbox::Outbox;
use crate::realtime::connection::{Connection, Incoming, Out, SessionCheck, Status, Timer};
use crate::realtime::driver::{sleep_until, socket_url, SocketEvent, SocketTarget, Sockets, Timers};
use crate::store::{ChannelWindow, Delivery, Key, PendingMessage, Presence, Store, ThreadWindow, UNNAMED};
use parking_lot::RwLock;
use purlay_contract::constants::{ACK_DEADLINE_MS, ACTIVE_EVERY_MS, PRESENCE_GRACE_MS, REPLAY_RETRY_DELAYS_MS, TYPING_TTL_MS};
use purlay_contract::models::*;
use purlay_contract::{Timestamp, Uuid};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};

/// One-off things the UI should say or do (a toast, a desktop notification, sign-out).
#[derive(Debug, Clone)]
pub enum Event {
    /// The session is over (signed out elsewhere, expired, account deleted).
    SessionEnded(String),
    /// Unsent messages went with a channel the person lost.
    LostUnsent(usize),
    /// A message arrived that may deserve a desktop notification (never the person's own).
    Arrived(Box<Message>),
    /// A channel went away while it may be on screen.
    ChannelGone(Uuid, String),
    /// A read failed in a way the UI should show (the key's name and why).
    LoadFailed(String, String),
}

pub type AnyValue = Arc<dyn Any + Send + Sync>;
pub type StoreFn = Box<dyn FnOnce(&mut Store) + Send>;

pub enum Command {
    Start,
    Stop,
    /// Receive exactly these channels; the others are let go.
    Watch(Vec<Uuid>),
    Send { channel_id: Uuid, parent_id: Option<Uuid>, content: String, attachments: Vec<Attachment>, reply: Option<oneshot::Sender<Uuid>> },
    Retry(Uuid),
    Discard(Uuid),
    RetryDegraded,
    Typing(Uuid),
    Active(Uuid),
    NetworkLost,
    NetworkBack,
    /// Channels lost without being told (gone from a list read again).
    Lose(Vec<Uuid>),
    ApplyPresence(Vec<PresenceChange>),
    // --- reads ---
    LoadMe,
    LoadChannels(Uuid),
    OpenChannel(Uuid),
    ReloadChannel(Uuid),
    LoadOlder(Uuid),
    LoadNewer(Uuid),
    OpenAround(Uuid, Uuid),
    OpenThread { root: Uuid, channel: Uuid },
    LoadMoreThread(Uuid),
    Ensure(Key),
    Refresh(Key),
    SetActive(Vec<Key>),
    /// Apply a change the UI learned from an HTTP answer.
    Mutate(StoreFn),
    Shutdown,
}

enum Internal {
    Socket(u64, SocketEvent),
    SessionChecked(u64, SessionCheck),
    Me(Result<MeResponse, ApiError>),
    Channels(Uuid, Result<Vec<Channel>, ApiError>),
    Window(Uuid, Result<ChannelWindow, ApiError>),
    Refreshed(Uuid, Result<MessagePage, ApiError>),
    Older(Uuid, Result<MessagePage, ApiError>),
    Newer(Uuid, Result<MessagePage, ApiError>),
    Thread(Uuid, Uuid, bool, Result<(Option<Message>, MessagePage), ApiError>),
    Loaded(Key, Result<AnyValue, ApiError>),
    SentOverHttp(Uuid, Result<Message, ApiError>),
    /// An acknowledged send read back because its echo did not come.
    Confirmed(Uuid, Result<Message, ApiError>),
    Continued(Uuid, String, usize, Result<(), ApiError>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TimerKey {
    Conn(Timer),
    Ack(Uuid),
    TypingEnd(Uuid, Uuid),
    PresenceForget,
    PresenceReread,
    ContinueAgain(Uuid, String, usize),
    /// An acknowledged send whose echo has not arrived yet.
    Confirm(Uuid),
}

const CONFIRM_AFTER_MS: u64 = 1500;

/// Refused for a reason waiting will not change.
fn permanent(code: &ErrorCode) -> bool {
    matches!(code, ErrorCode::ChannelNotFound | ErrorCode::Forbidden)
}

/// Reasons that mean the server was not reached; a reconnect re-sends these.
fn unreached(reason: &str) -> bool {
    matches!(reason, "NETWORK" | "UNAVAILABLE")
}

fn reason_of(e: &ApiError) -> String {
    match e {
        ApiError::Transport(_) => "NETWORK".into(),
        ApiError::Server { status: 502..=504, .. } | ApiError::Http { status: 502..=504, .. } => "UNAVAILABLE".into(),
        ApiError::Server { body, .. } => body.code.as_str().to_string(),
        ApiError::Http { status, .. } => format!("HTTP_{status}"),
        ApiError::Decode(_) => "DECODE".into(),
        ApiError::Request(_) => "REQUEST".into(),
    }
}

pub struct EngineConfig {
    pub api: ApiClient,
    /// The handshake's `appUrl` origin: the socket upgrade's `Origin`.
    pub socket_origin: String,
    pub user_id: Uuid,
    pub outbox: Outbox,
    pub clock: SharedClock,
}

/// What the UI holds: commands in, revisions and events out, the store to read.
#[derive(Clone)]
pub struct EngineHandle {
    pub commands: mpsc::UnboundedSender<Command>,
    pub store: Arc<RwLock<Store>>,
    pub revisions: watch::Receiver<u64>,
}

impl EngineHandle {
    pub fn send(&self, c: Command) {
        let _ = self.commands.send(c);
    }
}

pub fn spawn(cfg: EngineConfig, store: Arc<RwLock<Store>>, events: mpsc::UnboundedSender<Event>) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (int_tx, int_rx) = mpsc::unbounded_channel();
    let (sock_tx, sock_rx) = mpsc::unbounded_channel();
    let (rev_tx, rev_rx) = watch::channel(0u64);
    let restored = cfg.outbox.load();
    {
        let mut s = store.write();
        s.pending = restored;
    }
    let random: Box<dyn FnMut() -> f64 + Send> = Box::new(rand::random::<f64>);
    let conn = Connection::new(cfg.clock.clone(), random);
    let engine = Engine {
        api: cfg.api,
        socket_origin: cfg.socket_origin,
        user_id: cfg.user_id,
        outbox: cfg.outbox,
        clock: cfg.clock,
        store: store.clone(),
        events,
        revision: rev_tx,
        internal: int_tx,
        conn,
        sockets: Sockets::new(sock_tx),
        timers: Timers::default(),
        watched: HashSet::new(),
        applied: HashMap::new(),
        opened: false,
        resume_on_open: false,
        epoch: 0,
        continuing: HashSet::new(),
        reported: HashSet::new(),
        reminted: HashSet::new(),
        announced: HashMap::new(),
        said_active: HashMap::new(),
        dirty: false,
        early: HashMap::new(),
    };
    tokio::spawn(engine.run(cmd_rx, int_rx, sock_rx));
    EngineHandle { commands: cmd_tx, store, revisions: rev_rx }
}

struct Engine {
    api: ApiClient,
    socket_origin: String,
    user_id: Uuid,
    outbox: Outbox,
    clock: SharedClock,
    store: Arc<RwLock<Store>>,
    events: mpsc::UnboundedSender<Event>,
    revision: watch::Sender<u64>,
    internal: mpsc::UnboundedSender<Internal>,
    conn: Connection,
    sockets: Sockets,
    timers: Timers<TimerKey>,
    watched: HashSet<String>,
    /// Newest version applied per (kind, thing): an older frame is dropped (D-136).
    applied: HashMap<(String, String), i64>,
    opened: bool,
    resume_on_open: bool,
    epoch: u64,
    continuing: HashSet<Uuid>,
    reported: HashSet<String>,
    reminted: HashSet<Uuid>,
    announced: HashMap<Uuid, (String, i64)>,
    said_active: HashMap<Uuid, (String, i64)>,
    dirty: bool,
    /// Live roots that arrived while a feed's first page was being read.
    early: HashMap<Uuid, Vec<Message>>,
}

impl Engine {
    async fn run(
        mut self,
        mut cmds: mpsc::UnboundedReceiver<Command>,
        mut internal: mpsc::UnboundedReceiver<Internal>,
        mut sockets: mpsc::UnboundedReceiver<(u64, SocketEvent)>,
    ) {
        loop {
            let deadline = self.timers.next_deadline();
            tokio::select! {
                c = cmds.recv() => match c {
                    Some(Command::Shutdown) | None => {
                        self.stop();
                        self.flush();
                        return;
                    }
                    Some(c) => self.command(c),
                },
                Some(i) = internal.recv() => self.internal(i),
                Some((g, e)) = sockets.recv() => self.internal(Internal::Socket(g, e)),
                _ = sleep_until(deadline) => {
                    for k in self.timers.take_due() {
                        self.timer(k);
                    }
                }
            }
            self.pump_connection();
            self.flush();
        }
    }

    // --- plumbing --------------------------------------------------------------------

    fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    fn write<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> R {
        self.dirty = true;
        let mut s = self.store.write();
        f(&mut s)
    }

    fn read<R>(&self, f: impl FnOnce(&Store) -> R) -> R {
        f(&self.store.read())
    }

    fn flush(&mut self) {
        if self.dirty {
            self.dirty = false;
            let rev = {
                let mut s = self.store.write();
                s.revision += 1;
                s.revision
            };
            let _ = self.revision.send(rev);
        }
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = Internal> + Send + 'static,
    {
        let tx = self.internal.clone();
        tokio::spawn(async move {
            let _ = tx.send(f.await);
        });
    }

    /// Performs what the connection asked for and handles its frames.
    fn pump_connection(&mut self) {
        for _ in 0..64 {
            let outs = self.conn.take();
            if outs.is_empty() {
                return;
            }
            for o in outs {
                match o {
                    Out::Open { generation } => {
                        let target = SocketTarget {
                            url: socket_url(self.api.origin()),
                            cookie: self.api.cookie_header(),
                            origin: self.socket_origin.clone(),
                            client_header: self.api.client_header().to_string(),
                        };
                        self.sockets.open(generation, target);
                    }
                    Out::Send(text) => self.sockets.send(text),
                    Out::Close => self.sockets.close(),
                    Out::Schedule(t, ms) => self.timers.schedule(TimerKey::Conn(t), ms),
                    Out::Cancel(t) => self.timers.cancel(&TimerKey::Conn(t)),
                    Out::CheckSession { epoch } => {
                        let api = self.api.clone();
                        let tx = self.internal.clone();
                        tokio::spawn(async move {
                            let check = match api.get_me().await {
                                Ok(_) => SessionCheck::Active,
                                Err(e) if e.status() == Some(401) => SessionCheck::Ended,
                                Err(_) => SessionCheck::Unknown,
                            };
                            let _ = tx.send(Internal::SessionChecked(epoch, check));
                        });
                    }
                    Out::Status(s) => self.on_status(s),
                    Out::SubscribeRefused { channel_ids, code, ref_id } => {
                        self.reported.insert(ref_id);
                        self.refused(&channel_ids, &code);
                    }
                    Out::Frame(inc) => self.on_frame(inc),
                }
            }
        }
    }

    fn timer(&mut self, k: TimerKey) {
        match k {
            TimerKey::Conn(t) => self.conn.on_timer(t),
            TimerKey::Ack(id) => {
                let reason = self.timer_fail_reason(&id);
                self.fail(&id, &reason)
            }
            TimerKey::TypingEnd(c, u) => self.stop_typing(&c, &u),
            TimerKey::PresenceForget => self.write(|s| s.presence.clear()),
            TimerKey::PresenceReread => {
                self.write(|s| s.presence_epoch += 1);
                let workspaces: Vec<Uuid> = self.read(|s| s.channels.keys().cloned().collect());
                for w in workspaces {
                    self.read_again(Key::Presence(w));
                }
            }
            TimerKey::ContinueAgain(ch, from, attempt) => self.continue_replay(ch, from, attempt),
            TimerKey::Confirm(id) => {
                if self.find(&id).is_some_and(|p| p.delivery == Delivery::Sent) {
                    let api = self.api.clone();
                    self.spawn(async move { Internal::Confirmed(id.clone(), api.get_message(&id).await) });
                }
            }
        }
    }

    // --- pending sends ---------------------------------------------------------------

    fn persist(&mut self) {
        let pending = self.read(|s| s.pending.clone());
        self.outbox.save(&pending);
    }

    fn find(&self, id: &str) -> Option<PendingMessage> {
        self.read(|s| s.pending.iter().find(|p| p.id.as_str() == id).cloned())
    }

    fn update_pending(&mut self, id: &str, f: impl FnOnce(&mut PendingMessage)) {
        self.write(|s| {
            if let Some(p) = s.pending.iter_mut().find(|p| p.id.as_str() == id) {
                f(p);
            }
        });
        self.persist();
    }

    fn forget_pending(&mut self, id: &str) {
        self.timers.cancel(&TimerKey::Ack(Uuid::new(id)));
        let had = self.write(|s| {
            let before = s.pending.len();
            s.pending.retain(|p| p.id.as_str() != id);
            before != s.pending.len()
        });
        if had {
            self.persist();
        }
    }

    fn drop_pending(&mut self, doomed: Vec<PendingMessage>) {
        if doomed.is_empty() {
            return;
        }
        let ids: HashSet<Uuid> = doomed.iter().map(|p| p.id.clone()).collect();
        for id in &ids {
            self.timers.cancel(&TimerKey::Ack(id.clone()));
        }
        let n = ids.len();
        self.write(|s| {
            s.pending.retain(|p| !ids.contains(&p.id));
            let seq = s.lost.map(|(_, q)| q).unwrap_or(0) + 1;
            s.lost = Some((n, seq));
        });
        self.persist();
        self.emit(Event::LostUnsent(n));
    }

    fn drop_unsent(&mut self, channel_ids: &[Uuid]) {
        let doomed = self.read(|s| s.pending.iter().filter(|p| channel_ids.contains(&p.channel_id)).cloned().collect());
        self.drop_pending(doomed);
    }

    fn fail(&mut self, id: &Uuid, reason: &str) {
        let failing = self.find(id);
        if reason == "CHANNEL_NOT_FOUND" {
            if let Some(p) = failing {
                self.drop_pending(vec![p]);
            }
            return;
        }
        self.timers.cancel(&TimerKey::Ack(id.clone()));
        if failing.is_some() {
            let r = reason.to_string();
            self.update_pending(id, |p| {
                p.delivery = Delivery::Failed;
                p.reason = Some(r);
            });
        }
    }

    /// An id that has aged out of the plausibility window is sent once more under a new id.
    fn remint(&mut self, old: &Uuid) -> bool {
        let Some(p) = self.find(old) else { return false };
        if self.reminted.contains(old) {
            return false;
        }
        if is_plausible(old, self.conn.server_now_ms()) {
            return false;
        }
        let id = self.conn.new_id();
        self.reminted.insert(id.clone());
        self.timers.cancel(&TimerKey::Ack(old.clone()));
        let fresh = PendingMessage {
            id: id.clone(),
            created_at: Timestamp::from_unix_ms(self.conn.server_now_ms()),
            delivery: Delivery::Sending,
            reason: None,
            ..p
        };
        let f2 = fresh.clone();
        self.write(|s| {
            if let Some(slot) = s.pending.iter_mut().find(|x| x.id == *old) {
                *slot = f2;
            }
        });
        self.persist();
        self.transmit(fresh);
        true
    }

    fn transmit(&mut self, p: PendingMessage) {
        let attachment_ids = if p.attachments.is_empty() { None } else { Some(p.attachments.iter().map(|a| a.id.clone()).collect()) };
        let frame = ClientFrame::MessageSend(MessageSendFrame {
            id: p.id.clone(),
            payload: MessageSendFramePayload {
                channel_id: p.channel_id.clone(),
                content: p.content.clone(),
                parent_id: p.parent_id.clone(),
                attachment_ids: attachment_ids.clone(),
            },
        });
        if !self.conn.send(&frame) {
            // HTTP is the door when the socket is not: the same service, the same id.
            let api = self.api.clone();
            let body = SendMessageRequest { id: p.id.clone(), content: p.content.clone(), attachment_ids, parent_id: p.parent_id.clone() };
            let id = p.id.clone();
            let channel = p.channel_id.clone();
            self.spawn(async move { Internal::SentOverHttp(id, api.send_message(&channel, &body).await) });
            return;
        }
        self.timers.schedule(TimerKey::Ack(p.id.clone()), ACK_DEADLINE_MS as u64);
    }

    fn sent_over_http(&mut self, id: Uuid, r: Result<Message, ApiError>) {
        match r {
            Ok(m) => {
                self.message_created(m);
                self.forget_pending(&id);
            }
            Err(e) => {
                let reason = reason_of(&e);
                if !unreached(&reason) {
                    self.fail(&id, &reason);
                    return;
                }
                // Not reaching a server is not yet a failure: it stays `sending` until its
                // three seconds are out, and a reconnect inside them sends it (#655).
                let Some(p) = self.find(&id) else { return };
                let age = self.conn.server_now_ms() - p.created_at.unix_ms().unwrap_or(0);
                let wait = (ACK_DEADLINE_MS - age).max(0) as u64;
                self.update_pending(&id, |p| p.reason = Some(reason));
                self.timers.schedule(TimerKey::Ack(id), wait);
            }
        }
    }

    fn timer_fail_reason(&self, id: &Uuid) -> String {
        self.find(id).and_then(|p| p.reason).unwrap_or_else(|| "TIMEOUT".into())
    }

    /// What a reconnect owes: re-send what never got an answer, and look again at what moved.
    fn resync(&mut self) {
        self.said_active.clear();
        let pending = self.read(|s| s.pending.clone());
        for p in pending {
            if p.delivery == Delivery::Sending {
                self.transmit(p);
            } else if p.reason.as_deref().is_some_and(unreached) {
                self.update_pending(&p.id, |x| {
                    x.delivery = Delivery::Sending;
                    x.reason = None;
                });
                self.transmit(PendingMessage { delivery: Delivery::Sending, reason: None, ..p });
            }
        }
        self.read_again_threads();
        self.read_again_where(|k| matches!(k, Key::Unresolved(_) | Key::Unreads(_)));
        self.reload_all_channel_lists();
    }

    /// What the person's own topic says, read again once per open.
    fn read_user_topic_again(&mut self) {
        self.load_me();
        self.read_again_where(|k| {
            matches!(
                k,
                Key::Members(_) | Key::Notifications(..) | Key::Inbox(..) | Key::Saved(..) | Key::SavedIds(_) | Key::Unreads(_) | Key::ChannelGroups(_) | Key::Reminders(_) | Key::Scheduled(_)
            )
        });
    }

    // --- replay continuation -----------------------------------------------------------

    fn continue_replay(&mut self, channel: Uuid, from: String, attempt: usize) {
        if attempt == 0 {
            if self.continuing.contains(&channel) {
                return;
            }
            self.continuing.insert(channel.clone());
        }
        let api = self.api.clone();
        let tx = self.internal.clone();
        let ch = channel.clone();
        let f = from.clone();
        tokio::spawn(async move {
            let mut cursor = Some(Uuid::new(&f));
            let mut result = Ok(());
            while let Some(c) = cursor.take() {
                match api.list_messages(&ch, &PageQuery { after: Some(c), before: None, limit: Some(100) }).await {
                    Ok(page) => {
                        cursor = page.next_cursor.clone();
                        let _ = tx.send(Internal::Refreshed(ch.clone(), Ok(MessagePage { data: page.data, next_cursor: None })));
                    }
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            let _ = tx.send(Internal::Continued(ch, f, attempt, result));
        });
    }

    fn continued(&mut self, channel: Uuid, from: String, attempt: usize, r: Result<(), ApiError>) {
        match r {
            Ok(()) => {
                self.continuing.remove(&channel);
                self.conn.continuation_done(&channel, &from);
                self.recover(&channel);
            }
            Err(_) => {
                if let Some(wait) = REPLAY_RETRY_DELAYS_MS.get(attempt) {
                    self.timers.schedule(TimerKey::ContinueAgain(channel, from, attempt + 1), *wait as u64);
                } else {
                    self.continuing.remove(&channel);
                    self.degrade(&[channel.to_string()], "REPLAY");
                    self.reload_window(&channel);
                }
            }
        }
    }

    fn degrade(&mut self, ids: &[String], reason: &str) {
        let r = reason.to_string();
        self.write(|s| {
            for id in ids {
                s.degraded.insert(id.clone(), r.clone());
            }
        });
    }

    fn recover(&mut self, channel: &str) {
        if self.read(|s| s.degraded.contains_key(channel)) {
            self.write(|s| s.degraded.remove(channel));
        }
    }

    fn refused(&mut self, channel_ids: &[String], code: &ErrorCode) {
        if permanent(code) {
            let ids: Vec<Uuid> = channel_ids.iter().map(Uuid::new).collect();
            self.drop_unsent(&ids);
            self.conn.forget(channel_ids);
            for id in channel_ids {
                self.watched.remove(id);
                self.recover(id);
            }
            self.reload_all_channel_lists();
            return;
        }
        self.degrade(channel_ids, code.as_str());
    }

    // --- frames ----------------------------------------------------------------------

    fn is_newest(&mut self, kind: &str, id: &str, version: Option<i64>) -> bool {
        let Some(v) = version else { return true };
        let key = (kind.to_string(), id.to_string());
        if v < *self.applied.get(&key).unwrap_or(&0) {
            return false;
        }
        self.applied.insert(key, v);
        true
    }

    fn is_announcement(&self, ref_id: &str) -> bool {
        self.announced.values().chain(self.said_active.values()).any(|(id, _)| id == ref_id)
    }

    fn on_frame(&mut self, inc: Incoming) {
        let Incoming { frame, ref_id, version, .. } = inc;
        match frame {
            ServerFrame::Ack(a) => {
                let id = a.data.id;
                self.timers.cancel(&TimerKey::Ack(id.clone()));
                if self.find(&id).is_some() {
                    self.update_pending(&id, |p| {
                        p.delivery = Delivery::Sent;
                        p.reason = None;
                    });
                    // The echo goes only to sockets subscribed to the channel: a send made
                    // before its SUBSCRIBE was answered is read back instead.
                    self.timers.schedule(TimerKey::Confirm(id), CONFIRM_AFTER_MS);
                }
            }
            ServerFrame::Subscribed(s) => {
                for c in &s.data.channels {
                    if let Some(next) = &c.next_cursor {
                        self.continue_replay(c.channel_id.clone(), next.to_string(), 0);
                    }
                    // A state that changed between a read and the subscription is in no frame:
                    // what is held for the channel is read again from this moment on.
                    self.refresh_window(&c.channel_id);
                    for k in [Key::Unreads(c.channel_id.clone()), Key::Files(c.channel_id.clone()), Key::Pins(c.channel_id.clone()), Key::Bookmarks(c.channel_id.clone())] {
                        self.read_again(k);
                    }
                }
                self.read_again_threads();
                self.read_again_where(|k| matches!(k, Key::Unresolved(_) | Key::MessageVersions(_)));
            }
            ServerFrame::Unsubscribed(_) => {}
            ServerFrame::Error(e) => {
                let code = e.data.code.clone();
                match ref_id {
                    Some(r) if self.is_announcement(&r) => {}
                    Some(r) if self.find(&r).is_some() => {
                        let id = Uuid::new(&r);
                        if code == ErrorCode::InvalidMessageId && self.remint(&id) {
                            return;
                        }
                        self.fail(&id, code.as_str());
                    }
                    Some(r) if self.reported.remove(&r) => {}
                    _ if matches!(code, ErrorCode::RateLimited | ErrorCode::DatabaseUnavailable) => {}
                    _ => self.degrade(&[UNNAMED.to_string()], code.as_str()),
                }
            }
            ServerFrame::MessageCreated(m) => {
                let msg = m.data;
                let id = msg.id.clone();
                let mine = msg.sender.as_ref().is_some_and(|s| s.id == self.user_id);
                if let Some(sender) = &msg.sender {
                    let (c, u) = (msg.channel_id.clone(), sender.id.clone());
                    self.stop_typing(&c, &u);
                }
                if !mine {
                    self.emit(Event::Arrived(Box::new(msg.clone())));
                }
                self.message_created(msg);
                self.forget_pending(&id);
            }
            ServerFrame::ThreadUpdated(t) => {
                if self.is_newest("thread", &t.data.message_id, version) {
                    let d = t.data;
                    self.write(|s| {
                        s.for_message(&d.channel_id, &d.message_id, |m| m.thread = Some(d.thread.clone()));
                    });
                }
            }
            ServerFrame::ReactionsUpdated(r) => {
                if self.is_newest("reactions", &r.data.message_id, version) {
                    let d = r.data;
                    self.write(|s| {
                        s.for_message(&d.channel_id, &d.message_id, |m| m.reactions = d.reactions.clone());
                    });
                }
            }
            ServerFrame::MessageHidden(h) => {
                let d = h.data;
                self.write(|s| {
                    if let Some(w) = s.windows.get_mut(&d.channel_id) {
                        w.hide(&d.message_id);
                    }
                    for t in s.threads.values_mut() {
                        t.replies.hide(&d.message_id);
                    }
                });
            }
            ServerFrame::MessageRevised(r) => {
                if self.is_newest("revision", &r.data.message_id, version) {
                    let d = r.data;
                    self.write(|s| {
                        s.for_message(&d.channel_id, &d.message_id, |m| crate::store::window::apply_revision(m, &d));
                    });
                }
            }
            ServerFrame::ChannelState(c) => {
                if self.is_newest("state", &c.data.channel_id, version) {
                    let d = c.data;
                    self.write(|s| {
                        if let Some(ch) = s.channel_mut(&d.channel_id) {
                            ch.state = Some(d.state.clone());
                        }
                    });
                }
            }
            ServerFrame::ChannelUpdated(c) => {
                if self.is_newest("channel", &c.data.channel_id, version) {
                    let d = c.data;
                    self.write(|s| {
                        if let Some(ch) = s.channel_mut(&d.channel_id) {
                            ch.name = Some(d.name.clone());
                            ch.topic = d.topic.clone();
                            ch.only_admins_post = d.only_admins_post;
                        }
                    });
                }
            }
            ServerFrame::ChannelAdded(c) => {
                let ch = c.data;
                let id = ch.id.clone();
                self.write(|s| {
                    s.gone.remove(&id);
                    for p in ch.peers.iter().chain(ch.peer.iter()) {
                        s.people.insert(p.id.clone(), p.clone());
                    }
                    s.channels.entry(ch.workspace_id.clone()).or_default().insert(id.clone(), ch);
                });
                self.watched.insert(id.to_string());
                self.conn.watch(&id);
            }
            ServerFrame::AccessRevoked(a) => {
                let d = a.data;
                let ids = d.channel_ids.clone();
                let reason = d.reason.as_ref().map(|r| r.as_str().to_string());
                self.drop_unsent(&ids);
                self.write(|s| {
                    for id in &ids {
                        for m in s.channels.values_mut() {
                            m.remove(id);
                        }
                        s.windows.remove(id);
                        s.typing.remove(id);
                        match &reason {
                            Some(r) => {
                                s.gone.insert(id.clone(), r.clone());
                            }
                            None => {
                                s.gone.remove(id);
                            }
                        }
                    }
                });
                for id in &ids {
                    self.emit(Event::ChannelGone(id.clone(), reason.clone().unwrap_or_default()));
                }
                if matches!(reason.as_deref(), Some("left") | Some("removed")) {
                    self.reload_all_channel_lists();
                }
                let strs: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
                self.conn.forget(&strs);
                for id in strs {
                    self.watched.remove(&id);
                }
            }
            ServerFrame::DeepWorkUpdated(d) => {
                let d = d.data;
                self.write(|s| {
                    if let Some(me) = s.me.as_mut() {
                        for m in me.memberships.iter_mut() {
                            if m.workspace.id == d.workspace_id {
                                m.deep_work_until = d.until.clone();
                            }
                        }
                    }
                });
            }
            ServerFrame::SessionEnded(e) => {
                self.emit(Event::SessionEnded(e.data.reason.as_str().to_string()));
            }
            ServerFrame::PinsUpdated(p) => self.read_again(Key::Pins(p.data.channel_id)),
            ServerFrame::BookmarksUpdated(b) => self.read_again(Key::Bookmarks(b.data.channel_id)),
            ServerFrame::IncidentUpdated(i) => self.reload_channel_list_of(&i.data.channel_id),
            ServerFrame::MessageVersionsUpdated(v) => self.read_again(Key::MessageVersions(v.data.message_id)),
            ServerFrame::ChannelsUpdated(c) => {
                let w = c.data.workspace_id;
                self.load_channels(w.clone());
                self.read_again(Key::ChannelGroups(w));
            }
            ServerFrame::RemindersUpdated(r) => self.read_again(Key::Reminders(r.data.workspace_id)),
            ServerFrame::SavedUpdated(s) => {
                let w = s.data.workspace_id;
                self.read_again_where(|k| matches!(k, Key::Saved(x, _) | Key::SavedIds(x) if *x == w));
            }
            ServerFrame::NotificationsUpdated(n) => {
                let w = n.data.workspace_id;
                self.read_again_where(|k| matches!(k, Key::Notifications(x, ..) if *x == w));
            }
            ServerFrame::InboxUpdated(n) => {
                let w = n.data.workspace_id;
                self.read_again_where(|k| matches!(k, Key::Inbox(x, _) if *x == w));
            }
            ServerFrame::WorkspaceUpdated(w) => {
                self.load_me();
                self.read_again(Key::Members(w.data.workspace_id));
            }
            ServerFrame::TypingServer(t) => {
                let d = t.data;
                self.start_typing(d.channel_id, d.user_id, d.ttl_ms.max(0) as u64);
            }
            ServerFrame::Presence(p) => self.apply_presence(p.data.changes),
            ServerFrame::Pong(_) | ServerFrame::Unknown(_) => {}
        }
    }

    fn message_created(&mut self, m: Message) {
        let loading = self.read(|s| !s.windows.contains_key(&m.channel_id) && s.window_loading.contains(&m.channel_id));
        if loading && m.parent_id.is_none() {
            // The feed's first page is on its way and may have been read before this message
            // existed: kept, and merged when the page lands.
            self.early.entry(m.channel_id.clone()).or_default().push(m.clone());
        }
        self.write(|s| {
            s.remember_message_people(&m);
            if let Some(ch) = s.channel_mut(&m.channel_id) {
                ch.last_message_at = Some(m.created_at.clone());
            }
            match &m.parent_id {
                None => {
                    if let Some(w) = s.windows.get_mut(&m.channel_id) {
                        w.live(m.clone());
                    }
                }
                Some(parent) => {
                    if let Some(t) = s.threads.get_mut(parent) {
                        if t.loaded && t.replies.reaches_newest {
                            t.replies.live(m.clone());
                        }
                    }
                }
            }
        });
        if m.parent_id.is_none() {
            self.conn.observe(&m.channel_id, &m.id);
        }
    }

    // --- typing & presence -----------------------------------------------------------

    fn stop_typing(&mut self, channel: &Uuid, user: &Uuid) {
        self.timers.cancel(&TimerKey::TypingEnd(channel.clone(), user.clone()));
        let present = self.read(|s| s.typing.get(channel).is_some_and(|v| v.contains(user)));
        if present {
            self.write(|s| {
                if let Some(v) = s.typing.get_mut(channel) {
                    v.retain(|u| u != user);
                    if v.is_empty() {
                        s.typing.remove(channel);
                    }
                }
            });
        }
    }

    fn start_typing(&mut self, channel: Uuid, user: Uuid, ttl: u64) {
        if user == self.user_id {
            return;
        }
        self.timers.schedule(TimerKey::TypingEnd(channel.clone(), user.clone()), ttl);
        self.write(|s| {
            let v = s.typing.entry(channel).or_default();
            if !v.contains(&user) {
                v.push(user);
            }
        });
    }

    fn apply_presence(&mut self, changes: Vec<PresenceChange>) {
        let newer: Vec<PresenceChange> = self.read(|s| {
            changes.into_iter().filter(|c| s.presence.get(&c.user_id).map(|p| p.version).unwrap_or(-1) < c.version).collect()
        });
        if newer.is_empty() {
            return;
        }
        let me = self.user_id.clone();
        self.write(|s| {
            for c in newer {
                s.presence.insert(c.user_id.clone(), Presence { online: c.online, away: c.away, version: c.version });
                if c.user_id == me {
                    if let (Some(chosen), Some(m)) = (c.chosen, s.me.as_mut()) {
                        m.away_chosen = chosen;
                    }
                }
            }
        });
    }

    fn on_status(&mut self, status: Status) {
        self.write(|s| {
            s.status = Some(status);
            if status == Status::Open {
                s.connected_once = true;
            }
        });
        if status != Status::Open {
            if self.opened && !self.timers.is_scheduled(&TimerKey::PresenceForget) {
                self.timers.schedule(TimerKey::PresenceForget, PRESENCE_GRACE_MS as u64);
            }
            return;
        }
        self.timers.cancel(&TimerKey::PresenceForget);
        self.read_user_topic_again();
        if self.opened || self.resume_on_open {
            self.resync();
            self.timers.schedule(TimerKey::PresenceReread, PRESENCE_GRACE_MS as u64);
        }
        self.resume_on_open = false;
        self.opened = true;
    }

    // --- reads -----------------------------------------------------------------------

    fn load_me(&mut self) {
        let api = self.api.clone();
        self.spawn(async move { Internal::Me(api.get_me().await) });
    }

    fn load_channels(&mut self, w: Uuid) {
        self.write(|s| {
            s.channels_loaded.entry(w.clone()).or_insert(crate::store::LoadState::Loading);
        });
        let api = self.api.clone();
        self.spawn(async move {
            let r = api.list_channels(&w).await;
            Internal::Channels(w, r)
        });
    }

    fn reload_all_channel_lists(&mut self) {
        let ws: Vec<Uuid> = self.read(|s| s.channels_loaded.keys().cloned().collect());
        for w in ws {
            self.load_channels(w);
        }
    }

    fn reload_channel_list_of(&mut self, channel: &Uuid) {
        if let Some(w) = self.read(|s| s.workspace_of_channel(channel)) {
            self.load_channels(w);
        }
    }

    fn open_channel(&mut self, ch: Uuid) {
        let have = self.read(|s| s.windows.contains_key(&ch) || s.window_loading.contains(&ch));
        if have {
            self.refresh_window(&ch);
            return;
        }
        self.reload_window(&ch);
    }

    fn reload_window(&mut self, ch: &Uuid) {
        let c = ch.clone();
        self.write(|s| {
            s.window_loading.insert(c.clone());
            s.window_failed.remove(&c);
        });
        let api = self.api.clone();
        let c = ch.clone();
        self.spawn(async move {
            let r = api.list_messages(&c, &PageQuery { before: None, after: None, limit: Some(50) }).await;
            Internal::Window(c, r.map(|p| ChannelWindow::latest(p.data, p.next_cursor)))
        });
    }

    /// Reads the newest page again and merges it (edits and reactions made in a gap).
    fn refresh_window(&mut self, ch: &Uuid) {
        let reaches = self.read(|s| s.windows.get(ch).map(|w| w.reaches_newest));
        match reaches {
            None => {}
            Some(false) => {}
            Some(true) => {
                let api = self.api.clone();
                let c = ch.clone();
                self.spawn(async move {
                    let r = api.list_messages(&c, &PageQuery { before: None, after: None, limit: Some(50) }).await;
                    Internal::Refreshed(c, r)
                });
            }
        }
    }

    fn open_thread(&mut self, root: Uuid, channel: Uuid, more: bool) {
        let after = if more { self.read(|s| s.threads.get(&root).and_then(|t| t.replies.newer_cursor.clone())) } else { None };
        if more && after.is_none() {
            return;
        }
        self.write(|s| {
            let t = s.threads.entry(root.clone()).or_insert_with(|| ThreadWindow { channel_id: channel.clone(), ..Default::default() });
            t.channel_id = channel.clone();
            if t.root.is_none() {
                t.root = s.windows.get(&channel).and_then(|w| w.get(&root)).cloned();
            }
        });
        let api = self.api.clone();
        let (r, c) = (root.clone(), channel.clone());
        self.spawn(async move {
            let root_msg = if more { None } else { api.get_message(&r).await.ok() };
            let page = api.get_thread(&r, &PageQuery { before: None, after, limit: Some(100) }).await;
            Internal::Thread(r, c, more, page.map(|p| (root_msg, p)))
        });
    }

    fn read_again_threads(&mut self) {
        let open: Vec<(Uuid, Uuid)> = self.read(|s| s.threads.iter().filter(|(_, t)| t.loaded).map(|(r, t)| (r.clone(), t.channel_id.clone())).collect());
        for (r, c) in open {
            self.open_thread(r, c, false);
        }
    }

    pub fn read_again(&mut self, key: Key) {
        let active = self.read(|s| s.cache.active.contains(&key));
        let exists = self.read(|s| s.cache.state(&key).is_some());
        if active {
            self.fetch(key);
        } else if exists {
            self.write(|s| s.cache.mark_stale(&key));
        }
    }

    fn read_again_where(&mut self, f: impl Fn(&Key) -> bool) {
        let keys = self.read(|s| s.cache.keys_where(&f));
        for k in keys {
            self.read_again(k);
        }
    }

    fn ensure(&mut self, key: Key) {
        let state = self.read(|s| s.cache.state(&key).cloned());
        match state {
            None | Some(crate::store::LoadState::Stale) | Some(crate::store::LoadState::Failed(_)) => self.fetch(key),
            _ => {}
        }
    }

    fn fetch(&mut self, key: Key) {
        let begun = self.write(|s| s.cache.begin(&key));
        if !begun {
            return;
        }
        let api = self.api.clone();
        let k = key.clone();
        self.spawn(async move { Internal::Loaded(k.clone(), fetch_key(&api, &k).await) });
    }

    // --- dispatch --------------------------------------------------------------------

    fn command(&mut self, c: Command) {
        match c {
            Command::Start => self.conn.start(),
            Command::Stop => self.stop(),
            Command::Watch(ids) => {
                let wanted: HashSet<String> = ids.iter().map(|i| i.to_string()).collect();
                let dropped: Vec<String> = self.watched.iter().filter(|id| !wanted.contains(*id)).cloned().collect();
                for id in &dropped {
                    self.watched.remove(id);
                }
                if !dropped.is_empty() {
                    self.conn.unwatch(&dropped);
                }
                let mut fresh: Vec<&String> = wanted.iter().filter(|id| !self.watched.contains(*id)).collect();
                fresh.sort();
                for id in fresh {
                    self.watched.insert(id.clone());
                    self.conn.watch(id);
                }
            }
            Command::Send { channel_id, parent_id, content, attachments, reply } => {
                self.announced.remove(&channel_id);
                let id = self.conn.new_id();
                let p = PendingMessage {
                    id: id.clone(),
                    channel_id,
                    parent_id,
                    content,
                    created_at: Timestamp::from_unix_ms(self.conn.server_now_ms()),
                    delivery: Delivery::Sending,
                    reason: None,
                    attachments,
                };
                let p2 = p.clone();
                self.write(|s| s.pending.push(p2));
                self.persist();
                self.transmit(p);
                if let Some(r) = reply {
                    let _ = r.send(id);
                }
            }
            Command::Retry(id) => {
                let Some(p) = self.find(&id) else { return };
                if p.delivery != Delivery::Failed {
                    return;
                }
                self.update_pending(&id, |x| {
                    x.delivery = Delivery::Sending;
                    x.reason = None;
                });
                self.transmit(PendingMessage { delivery: Delivery::Sending, reason: None, ..p });
            }
            Command::Discard(id) => {
                if self.find(&id).is_some_and(|p| p.delivery == Delivery::Failed) {
                    self.forget_pending(&id);
                }
            }
            Command::RetryDegraded => {
                let stale: Vec<String> = self.read(|s| s.degraded.keys().filter(|k| *k != UNNAMED).cloned().collect());
                self.write(|s| s.degraded.clear());
                let again: Vec<String> = stale.into_iter().filter(|id| self.watched.contains(id)).collect();
                self.conn.unwatch(&again);
                for id in &again {
                    self.conn.watch(id);
                }
                self.reload_all_channel_lists();
            }
            Command::Typing(ch) => {
                let now = self.now();
                if let Some((_, at)) = self.announced.get(&ch) {
                    if now - at < TYPING_TTL_MS / 2 {
                        return;
                    }
                }
                let id = self.conn.new_id();
                let frame = ClientFrame::Typing(TypingFrame { id: id.clone(), payload: TypingFramePayload { channel_id: ch.clone() } });
                if self.conn.send(&frame) {
                    self.announced.insert(ch, (id.to_string(), now));
                }
            }
            Command::Active(w) => {
                let now = self.now();
                if let Some((_, at)) = self.said_active.get(&w) {
                    if now - at < ACTIVE_EVERY_MS {
                        return;
                    }
                }
                let id = self.conn.new_id();
                let frame = ClientFrame::Active(ActiveFrame { id: id.clone(), payload: ActiveFramePayload { workspace_id: w.clone() } });
                if self.conn.send(&frame) {
                    self.said_active.insert(w, (id.to_string(), now));
                }
            }
            Command::NetworkLost => {
                // No ACK can come: a waiting send stays `sending` and the reconnect re-sends it.
                self.timers.clear_where(|k| matches!(k, TimerKey::Ack(_)));
                self.conn.network_lost();
            }
            Command::NetworkBack => self.conn.network_back(),
            Command::Lose(ids) => self.drop_unsent(&ids),
            Command::ApplyPresence(c) => self.apply_presence(c),
            Command::LoadMe => self.load_me(),
            Command::LoadChannels(w) => self.load_channels(w),
            Command::OpenChannel(c) => self.open_channel(c),
            Command::ReloadChannel(c) => self.reload_window(&c),
            Command::LoadOlder(c) => {
                let before = self.read(|s| s.windows.get(&c).filter(|w| !w.reaches_oldest && !w.loading_older).and_then(|w| w.oldest_id().cloned()));
                if let Some(b) = before {
                    self.write(|s| {
                        if let Some(w) = s.windows.get_mut(&c) {
                            w.loading_older = true;
                        }
                    });
                    let api = self.api.clone();
                    self.spawn(async move {
                        let r = api.list_messages(&c, &PageQuery { before: Some(b), after: None, limit: Some(50) }).await;
                        Internal::Older(c, r)
                    });
                }
            }
            Command::LoadNewer(c) => {
                let after = self.read(|s| s.windows.get(&c).filter(|w| !w.reaches_newest && !w.loading_newer).and_then(|w| w.newest_id().cloned()));
                if let Some(a) = after {
                    self.write(|s| {
                        if let Some(w) = s.windows.get_mut(&c) {
                            w.loading_newer = true;
                        }
                    });
                    let api = self.api.clone();
                    self.spawn(async move {
                        let r = api.list_messages(&c, &PageQuery { before: None, after: Some(a), limit: Some(50) }).await;
                        Internal::Newer(c, r)
                    });
                }
            }
            Command::OpenAround(c, m) => {
                let c2 = c.clone();
                self.write(|s| {
                    s.window_loading.insert(c2);
                });
                let api = self.api.clone();
                self.spawn(async move {
                    let r = api.get_messages_around(&m).await;
                    Internal::Window(
                        c,
                        r.map(|w| {
                            let mut data = w.data;
                            data.push(w.target.clone());
                            ChannelWindow::around(w.target.id.clone(), data, w.older_cursor, w.newer_cursor)
                        }),
                    )
                });
            }
            Command::OpenThread { root, channel } => self.open_thread(root, channel, false),
            Command::LoadMoreThread(root) => {
                if let Some(c) = self.read(|s| s.threads.get(&root).map(|t| t.channel_id.clone())) {
                    self.open_thread(root, c, true);
                }
            }
            Command::Ensure(k) => self.ensure(k),
            Command::Refresh(k) => self.fetch(k),
            Command::SetActive(keys) => {
                let set: HashSet<Key> = keys.into_iter().collect();
                let fresh: Vec<Key> = self.read(|s| set.iter().filter(|k| !s.cache.active.contains(*k)).cloned().collect());
                self.write(|s| s.cache.active = set);
                for k in fresh {
                    self.ensure(k);
                }
            }
            Command::Mutate(f) => self.write(f),
            Command::Shutdown => {}
        }
    }

    fn stop(&mut self) {
        self.conn.stop();
        self.timers.clear_where(|k| !matches!(k, TimerKey::Conn(_)));
        self.announced.clear();
        self.write(|s| s.typing.clear());
        self.epoch += 1;
        self.continuing.clear();
        self.resume_on_open = self.read(|s| s.pending.iter().any(|p| p.delivery == Delivery::Sending));
    }

    fn internal(&mut self, i: Internal) {
        match i {
            Internal::Socket(g, e) => match e {
                SocketEvent::Opened => self.conn.on_opened(g),
                SocketEvent::Text(t) => self.conn.on_message(g, &t),
                SocketEvent::Binary => self.conn.on_binary(g),
                SocketEvent::Closed(code) => self.conn.on_closed(g, code),
                SocketEvent::Refused(status) => {
                    if status == 401 {
                        self.emit(Event::SessionEnded("expired".into()));
                    }
                    self.conn.on_refused(g, status)
                }
                SocketEvent::Failed(why) => {
                    tracing::debug!("socket failed: {why}");
                    self.conn.on_open_failed(g)
                }
            },
            Internal::SessionChecked(epoch, check) => {
                self.conn.on_session_checked(epoch, check);
                if check == SessionCheck::Ended {
                    self.emit(Event::SessionEnded("expired".into()));
                }
            }
            Internal::Me(r) => match r {
                Ok(me) => {
                    // Every workspace whose channels were never read is read now (one joined
                    // since, too); the others are kept current by their frames.
                    let unread: Vec<Uuid> = self.read(|s| {
                        me.memberships.iter().map(|m| m.workspace.id.clone()).filter(|w| !s.channels_loaded.contains_key(w)).collect()
                    });
                    self.write(|s| {
                        s.me = Some(me);
                        s.me_failed = None;
                    });
                    for w in unread {
                        self.load_channels(w);
                    }
                }
                Err(e) => {
                    if e.is_unauthenticated() {
                        self.emit(Event::SessionEnded("expired".into()));
                    }
                    let why = e.to_string();
                    self.write(|s| s.me_failed = Some(why));
                }
            },
            Internal::Channels(w, r) => match r {
                Ok(list) => {
                    let before: HashSet<Uuid> = self.read(|s| s.channels.get(&w).map(|m| m.keys().cloned().collect()).unwrap_or_default());
                    let after: HashSet<Uuid> = list.iter().map(|c| c.id.clone()).collect();
                    let lost: Vec<Uuid> = before.difference(&after).cloned().collect();
                    self.write(|s| {
                        for c in &list {
                            for p in c.peers.iter().chain(c.peer.iter()) {
                                s.people.insert(p.id.clone(), p.clone());
                            }
                        }
                        s.channels.insert(w.clone(), list.into_iter().map(|c| (c.id.clone(), c)).collect());
                        s.channels_loaded.insert(w.clone(), crate::store::LoadState::Loaded);
                    });
                    if !lost.is_empty() {
                        self.drop_unsent(&lost);
                    }
                    self.watch_joined();
                }
                Err(e) => {
                    let why = e.to_string();
                    self.write(|s| {
                        s.channels_loaded.insert(w, crate::store::LoadState::Failed(why));
                    });
                }
            },
            Internal::Window(c, r) => match r {
                Ok(mut w) => {
                    if let Some(early) = self.early.remove(&c) {
                        if w.reaches_newest {
                            for m in early {
                                w.live(m);
                            }
                        }
                    }
                    if let Some(newest) = w.newest_id().cloned() {
                        if w.reaches_newest {
                            self.conn.observe(&c, &newest);
                        }
                    }
                    self.write(|s| {
                        for m in &w.messages {
                            s.remember_message_people(m);
                        }
                        s.window_loading.remove(&c);
                        s.windows.insert(c.clone(), w);
                    });
                }
                Err(e) => {
                    let why = e.to_string();
                    self.write(|s| {
                        s.window_loading.remove(&c);
                        s.window_failed.insert(c, why);
                    });
                }
            },
            Internal::Refreshed(c, r) => {
                if let Ok(page) = r {
                    if let Some(newest) = page.data.iter().map(|m| m.id.clone()).max() {
                        let reaches = self.read(|s| s.windows.get(&c).is_some_and(|w| w.reaches_newest));
                        if reaches {
                            self.conn.observe(&c, &newest);
                        }
                    }
                    self.write(|s| {
                        for m in &page.data {
                            s.remember_message_people(m);
                        }
                        if let Some(w) = s.windows.get_mut(&c) {
                            if w.reaches_newest {
                                for m in page.data {
                                    w.live(m);
                                }
                            }
                        }
                    });
                }
            }
            Internal::Older(c, r) => {
                self.write(|s| match r {
                    Ok(page) => {
                        for m in &page.data {
                            s.remember_message_people(m);
                        }
                        if let Some(w) = s.windows.get_mut(&c) {
                            w.prepend_older(page.data, page.next_cursor);
                        }
                    }
                    Err(_) => {
                        if let Some(w) = s.windows.get_mut(&c) {
                            w.loading_older = false;
                        }
                    }
                });
            }
            Internal::Newer(c, r) => {
                let mut newest = None;
                self.write(|s| match r {
                    Ok(page) => {
                        for m in &page.data {
                            s.remember_message_people(m);
                        }
                        if let Some(w) = s.windows.get_mut(&c) {
                            w.append_newer(page.data, page.next_cursor);
                            if w.reaches_newest {
                                newest = w.newest_id().cloned();
                            }
                        }
                    }
                    Err(_) => {
                        if let Some(w) = s.windows.get_mut(&c) {
                            w.loading_newer = false;
                        }
                    }
                });
                if let Some(n) = newest {
                    self.conn.observe(&c, &n);
                }
            }
            Internal::Thread(root, channel, more, r) => {
                self.write(|s| {
                    let t = s.threads.entry(root.clone()).or_insert_with(|| ThreadWindow { channel_id: channel.clone(), ..Default::default() });
                    match r {
                        Ok((root_msg, page)) => {
                            if let Some(m) = root_msg {
                                t.root = Some(m);
                            }
                            if more {
                                t.replies.append_newer(page.data.clone(), page.next_cursor.clone());
                            } else {
                                let mut w = ChannelWindow::latest(vec![], None);
                                w.merge(page.data.clone());
                                w.reaches_oldest = true;
                                w.reaches_newest = page.next_cursor.is_none();
                                w.newer_cursor = page.next_cursor.clone();
                                // Keep live replies that arrived after the read began.
                                for m in std::mem::take(&mut t.replies.messages) {
                                    if w.get(&m.id).is_none() && w.reaches_newest {
                                        w.merge(vec![m]);
                                    }
                                }
                                t.replies = w;
                            }
                            t.loaded = true;
                            for m in page.data.iter() {
                                if let Some(snd) = &m.sender {
                                    s.people.insert(snd.id.clone(), snd.clone());
                                }
                            }
                        }
                        Err(_) => {
                            t.loaded = true;
                        }
                    }
                });
            }
            Internal::Loaded(k, r) => {
                match r {
                    Ok(v) => {
                        if let Some(ms) = v.downcast_ref::<WorkspaceMemberList>() {
                            let users: Vec<User> = ms.iter().map(|m| m.user.clone()).collect();
                            self.write(|s| {
                                for u in users {
                                    s.people.insert(u.id.clone(), u);
                                }
                            });
                        }
                        if let Some(p) = v.downcast_ref::<PresenceSnapshot>() {
                            self.apply_presence(p.online.clone());
                        }
                        let kk = k.clone();
                        self.write(|s| {
                            let again = s.cache.take_again(&kk);
                            s.cache.set_any(kk.clone(), v);
                            if again {
                                s.cache.mark_stale(&kk);
                            }
                        });
                        if self.read(|s| s.cache.state(&k) == Some(&crate::store::LoadState::Stale) && s.cache.active.contains(&k)) {
                            self.fetch(k);
                        }
                    }
                    Err(e) => {
                        let why = e.to_string();
                        let kk = k.clone();
                        self.write(|s| s.cache.fail(&kk, why.clone()));
                        if !e.is_route_not_found() {
                            self.emit(Event::LoadFailed(format!("{k:?}"), why));
                        }
                    }
                }
            }
            Internal::SentOverHttp(id, r) => self.sent_over_http(id, r),
            Internal::Confirmed(id, r) => {
                // A read that fails leaves the row as sent; the echo or a reread still comes.
                if let Ok(m) = r {
                    if self.find(&id).is_some() {
                        self.message_created(m);
                        self.forget_pending(&id);
                    }
                }
            }
            Internal::Continued(c, f, a, r) => self.continued(c, f, a, r),
        }
    }

    /// Joined channels (those with a read state) and conversations are received live.
    fn watch_joined(&mut self) {
        let ids: Vec<Uuid> = self.read(|s| {
            s.channels
                .values()
                .flat_map(|m| m.values())
                .filter(|c| c.state.is_some() || c.r#type == ChannelType::DirectMessage)
                .map(|c| c.id.clone())
                .collect()
        });
        self.command(Command::Watch(ids));
    }
}

/// One read per key.
async fn fetch_key(api: &ApiClient, key: &Key) -> Result<AnyValue, ApiError> {
    fn arc<T: Any + Send + Sync>(v: T) -> AnyValue {
        Arc::new(v)
    }
    Ok(match key {
        Key::Me => arc(api.get_me().await?),
        Key::Members(w) => arc(api.list_workspace_members(w).await?),
        Key::Presence(w) => arc(api.get_presence(w).await?),
        Key::ChannelGroups(w) => arc(api.list_channel_groups(w, &ChannelGroupQuery { before: None, limit: Some(100) }).await?),
        Key::Pins(c) => arc(api.list_pins(c).await?),
        Key::Bookmarks(c) => arc(api.list_bookmarks(c).await?),
        Key::Files(c) => arc(api.list_files(c, &FileQuery { before: None, limit: Some(50) }).await?),
        Key::ChannelMembers(c) => arc(api.list_channel_members(c).await?),
        Key::Unreads(c) => arc(api.channel_unreads(c, &CatchUpQuery::default()).await?),
        Key::Notifications(w, types, unread) => arc(
            api.list_notifications(&NotificationQuery {
                workspace_id: w.clone(),
                types: if types.is_empty() { None } else { Some(types.clone()) },
                unread_only: if *unread { Some(true) } else { None },
                before: None,
                limit: Some(50),
            })
            .await?,
        ),
        Key::Inbox(w, unread) => arc(api.list_inbox(w, &InboxQuery { unread_only: if *unread { Some(true) } else { None }, before: None, limit: Some(50) }).await?),
        Key::Saved(w, state) => arc(api.list_saved(w, &SavedQuery { state: state.clone(), before: None, limit: Some(50) }).await?),
        Key::SavedIds(w) => arc(api.list_saved_ids(w).await?),
        Key::Unresolved(w) => arc(api.list_unresolved(w, &UnresolvedQuery::default()).await?),
        Key::Reminders(w) => arc(api.list_message_reminders(w).await?),
        Key::Scheduled(w) => arc(api.list_scheduled_messages(w).await?),
        Key::Issues(w, status) => arc(api.list_issues(w, &IssueQuery { status: Some(status.clone()), before: None, limit: Some(100) }).await?),
        Key::SavedSearches(w) => arc(api.list_saved_searches(w, &OlderPageQuery::default()).await?),
        Key::ThreadFollow(m) => arc(api.get_thread_follow(m).await?),
        Key::MessageVersions(m) => arc(api.list_message_versions(m, &OlderPageQuery::default()).await?),
        Key::Sessions => arc(api.list_my_sessions().await?),
    })
}
