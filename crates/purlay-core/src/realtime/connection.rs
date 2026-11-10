//! The socket client's state machine: connect, and when it drops, come back and catch up.
//!
//! A line-for-line port of the web client's `connection.ts`:
//! - It owns the socket's life, the watched channels and the highest id heard in each.
//!   On every (re)connect it sends `SUBSCRIBE` with those ids, one chunk of at most
//!   `SUBSCRIBE_MAX_CHANNELS` at a time, the next after the last is answered.
//! - Every frame is handed on untouched ([`Out::Frame`]); this reads them only to move a
//!   cursor, and withholds that while an HTTP continuation of a replay is owed (D-82).
//! - It keeps the socket honest: a `PING` after `PING_AFTER_IDLE_MS` of silence, and
//!   silence past `PONG_DEADLINE_MS` is a dead socket. The first `PONG` sets the server clock.
//!
//! Nothing here does IO. Events go in through the `on_*` methods and the public
//! operations; what to do comes out of [`Connection::take`].

use super::backoff::reconnect_delay;
use super::cursors::Cursors;
use crate::clock::{ServerClock, SharedClock};
use crate::ids::{random_u128, IdGen};
use purlay_contract::constants::{
    close_codes, CHECK_SESSION_AFTER_FAILURES, PING_AFTER_IDLE_MS, PONG_DEADLINE_MS, STABLE_AFTER_MS,
    SUBSCRIBE_MAX_CHANNELS, TOO_BUSY_BACKOFF_STEP,
};
use purlay_contract::models::*;
use purlay_contract::{EmptyObject, Uuid};
use std::collections::{HashMap, HashSet, VecDeque};

/// `Idle` before `start` and after `stop` · `Connecting` · `Open` · `Waiting` for the next
/// attempt · `Ended` when the session is over and reconnecting would only be refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Idle,
    Connecting,
    Open,
    Waiting,
    Ended,
}

/// What asking the API about the session found. `Ended` is a 401 and nothing else; an
/// outage is `Unknown`, never a sign-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCheck {
    Active,
    Ended,
    Unknown,
}

/// The connection's timers; at most one of each runs, and scheduling one replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    /// The next connection attempt.
    Retry,
    /// The connection has held long enough to forgive the failures.
    Stable,
    /// Either the idle wait before a `PING`, or the deadline for its answer.
    Beat,
    /// Sending a refused `SUBSCRIBE` chunk again.
    Resend,
}

/// A frame the server sent, with the parts every frame has.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub frame: ServerFrame,
    /// The client frame it answers.
    pub ref_id: Option<String>,
    /// The server's clock in milliseconds; for drift correction only, never ordering.
    pub ts: f64,
    /// Present on frames that replace a whole state.
    pub version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Open a socket; its events come back tagged with this generation.
    Open { generation: u64 },
    /// Send a text frame on the current socket.
    Send(String),
    /// Close the current socket. Its close still comes back through `on_closed`.
    Close,
    Schedule(Timer, u64),
    Cancel(Timer),
    /// Ask `GET /api/me` whether the session exists; answer with `on_session_checked`.
    CheckSession { epoch: u64 },
    Frame(Incoming),
    Status(Status),
    /// A chunk of channels the server refused for a reason waiting will not change. Comes
    /// before the `Frame` carrying the same `ERROR`.
    SubscribeRefused { channel_ids: Vec<String>, code: ErrorCode, ref_id: String },
}

pub struct Connection {
    clock: SharedClock,
    random: Box<dyn FnMut() -> f64 + Send>,
    server_clock: ServerClock,
    ids: IdGen,
    cursors: Cursors,
    watched: HashSet<String>,
    /// Channel → the `nextCursor` of a replay whose HTTP continuation is still owed.
    pending: HashMap<String, String>,
    status: Status,
    has_socket: bool,
    generation: u64,
    failures: u32,
    never_opened: u32,
    running: HashSet<Timer>,
    ping_sent: Option<(String, i64)>,
    /// The `Beat` timer is the deadline for a PONG (true) or the idle wait before a PING.
    beat_is_deadline: bool,
    checking: bool,
    check_epoch: u64,
    queue: VecDeque<String>,
    in_flight: Option<(String, Vec<String>)>,
    limited_times: u32,
    out: Vec<Out>,
}

impl Connection {
    pub fn new(clock: SharedClock, random: Box<dyn FnMut() -> f64 + Send>) -> Self {
        Connection {
            clock,
            random,
            server_clock: ServerClock::default(),
            ids: IdGen::default(),
            cursors: Cursors::default(),
            watched: HashSet::new(),
            pending: HashMap::new(),
            status: Status::Idle,
            has_socket: false,
            generation: 0,
            failures: 0,
            never_opened: 0,
            running: HashSet::new(),
            ping_sent: None,
            beat_is_deadline: false,
            checking: false,
            check_epoch: 0,
            queue: VecDeque::new(),
            in_flight: None,
            limited_times: 0,
            out: Vec::new(),
        }
    }

    /// What to do, in order, since the last call.
    pub fn take(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn server_clock(&self) -> &ServerClock {
        &self.server_clock
    }

    /// Now, by the server-corrected clock.
    pub fn server_now_ms(&self) -> i64 {
        self.server_clock.now_ms(self.clock.now_ms())
    }

    /// A fresh UUIDv7 against the corrected clock.
    pub fn new_id(&mut self) -> Uuid {
        let now = self.server_now_ms();
        self.ids.next(now, random_u128())
    }

    pub fn cursor(&self, channel_id: &str) -> Option<&str> {
        self.cursors.get(channel_id)
    }

    pub fn is_watched(&self, channel_id: &str) -> bool {
        self.watched.contains(channel_id)
    }

    // --- internals -------------------------------------------------------------------

    fn set_status(&mut self, next: Status) {
        if next == self.status {
            return;
        }
        self.status = next;
        self.out.push(Out::Status(next));
    }

    fn schedule(&mut self, t: Timer, ms: u64) {
        self.running.insert(t);
        self.out.push(Out::Schedule(t, ms));
    }

    fn cancel(&mut self, t: Timer) {
        if self.running.remove(&t) {
            self.out.push(Out::Cancel(t));
        }
    }

    fn clear_timers(&mut self) {
        for t in [Timer::Retry, Timer::Stable, Timer::Beat, Timer::Resend] {
            self.cancel(t);
        }
        self.in_flight = None;
    }

    fn write(&mut self, frame: &ClientFrame) -> bool {
        if self.status != Status::Open || !self.has_socket {
            return false;
        }
        match serde_json::to_string(frame) {
            Ok(text) => {
                self.out.push(Out::Send(text));
                true
            }
            Err(_) => false,
        }
    }

    fn let_go(&mut self, channel_ids: &[String]) -> Vec<String> {
        let mut gone = Vec::new();
        for id in channel_ids {
            if !self.watched.remove(id) {
                continue;
            }
            self.pending.remove(id);
            gone.push(id.clone());
        }
        gone
    }

    /// Sends the next chunk if none is on the wire: one at a time (D-82).
    fn pump(&mut self) {
        if self.in_flight.is_some() || self.status != Status::Open {
            return;
        }
        let mut chunk = Vec::new();
        while chunk.len() < SUBSCRIBE_MAX_CHANNELS as usize {
            match self.queue.pop_front() {
                Some(id) => chunk.push(id),
                None => break,
            }
        }
        if !chunk.is_empty() {
            self.send_chunk(chunk);
        }
    }

    fn send_chunk(&mut self, channel_ids: Vec<String>) {
        let live: Vec<String> = channel_ids.into_iter().filter(|id| self.watched.contains(id)).collect();
        if live.is_empty() {
            self.in_flight = None;
            self.pump();
            return;
        }
        let id = self.new_id();
        let channels = live
            .iter()
            .map(|c| SubscribeFramePayloadChannelsItem {
                channel_id: Uuid::new(c),
                after_message_id: self.cursors.get(c).map(Uuid::new),
            })
            .collect();
        self.in_flight = Some((id.to_string(), live));
        self.write(&ClientFrame::Subscribe(SubscribeFrame { id, payload: SubscribeFramePayload { channels } }));
    }

    /// The answer to the chunk on the wire: go on, or send it again after a wait.
    fn answered(&mut self, inc: &Incoming) {
        let Some((id, ids)) = self.in_flight.clone() else { return };
        if inc.ref_id.as_deref() != Some(id.as_str()) {
            return;
        }
        match &inc.frame {
            ServerFrame::Subscribed(_) => {
                self.in_flight = None;
                self.limited_times = 0;
                self.pump();
            }
            ServerFrame::Error(e) => {
                let code = e.data.code.clone();
                if matches!(code, ErrorCode::RateLimited | ErrorCode::DatabaseUnavailable) {
                    let r = (self.random)();
                    let delay = reconnect_delay(self.limited_times, r);
                    self.schedule(Timer::Resend, delay);
                    self.limited_times += 1;
                } else {
                    self.in_flight = None;
                    self.out.push(Out::SubscribeRefused { channel_ids: ids, code, ref_id: id });
                    self.pump();
                }
            }
            _ => {}
        }
    }

    fn end(&mut self) {
        self.has_socket = false;
        self.ping_sent = None;
        self.clear_timers();
        self.failures = 0;
        self.never_opened = 0;
        self.check_epoch += 1;
        self.set_status(Status::Ended);
    }

    fn ask_about_session(&mut self) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.out.push(Out::CheckSession { epoch: self.check_epoch });
    }

    fn lost(&mut self) {
        self.has_socket = false;
        self.ping_sent = None;
        self.cancel(Timer::Stable);
        self.cancel(Timer::Beat);
        self.cancel(Timer::Resend);
        self.in_flight = None;
        self.set_status(Status::Waiting);
        let r = (self.random)();
        let delay = reconnect_delay(self.failures, r);
        self.schedule(Timer::Retry, delay);
        self.failures += 1;
        self.never_opened += 1;
        if self.never_opened >= CHECK_SESSION_AFTER_FAILURES as u32 {
            self.ask_about_session();
        }
    }

    /// Declares the socket dead without waiting for it to say so.
    fn dead(&mut self) {
        self.generation += 1;
        if self.has_socket {
            self.out.push(Out::Close);
        }
        self.lost();
    }

    fn arm_idle(&mut self) {
        self.cancel(Timer::Beat);
        self.beat_is_deadline = false;
        self.schedule(Timer::Beat, PING_AFTER_IDLE_MS as u64);
    }

    fn ping(&mut self) {
        let id = self.new_id();
        if !self.write(&ClientFrame::Ping(PingFrame { id: id.clone(), payload: EmptyObject {} })) {
            return;
        }
        self.ping_sent = Some((id.to_string(), self.clock.now_ms()));
        self.cancel(Timer::Beat);
        self.beat_is_deadline = true;
        self.schedule(Timer::Beat, PONG_DEADLINE_MS as u64);
    }

    fn measure_clock(&mut self, inc: &Incoming) {
        if !matches!(inc.frame, ServerFrame::Pong(_)) {
            return;
        }
        let Some((id, at)) = self.ping_sent.clone() else { return };
        if inc.ref_id.as_deref() != Some(id.as_str()) {
            return;
        }
        let received = self.clock.now_ms();
        self.ping_sent = None;
        if received < at {
            return;
        }
        if !self.server_clock.corrected() {
            let _ = self.server_clock.observe(at, received, inc.ts);
        }
    }

    /// A live frame moves the cursor, unless a continuation is owed for its channel.
    fn advance(&mut self, channel_id: &str, id: &str) {
        if let Some(owed) = self.pending.get(channel_id) {
            if id.to_ascii_lowercase() > *owed {
                return;
            }
        }
        self.cursors.observe(channel_id, id);
    }

    fn note_replay(&mut self, inc: &Incoming) {
        if let ServerFrame::Subscribed(s) = &inc.frame {
            for c in &s.data.channels {
                match &c.next_cursor {
                    None => {
                        self.pending.remove(c.channel_id.as_str());
                    }
                    Some(cur) => {
                        self.pending.insert(c.channel_id.to_string(), cur.to_ascii_lowercase());
                    }
                }
            }
        }
    }

    fn connect(&mut self) {
        self.running.remove(&Timer::Retry);
        self.generation += 1;
        self.set_status(Status::Connecting);
        self.has_socket = true;
        self.out.push(Out::Open { generation: self.generation });
    }

    // --- events ----------------------------------------------------------------------

    /// The socket could not even be created.
    pub fn on_open_failed(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.lost();
    }

    pub fn on_opened(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.set_status(Status::Open);
        self.never_opened = 0;
        self.check_epoch += 1;
        // Whatever the last connection owed is superseded by this one's replay.
        self.pending.clear();
        self.schedule(Timer::Stable, STABLE_AFTER_MS as u64);
        let mut ids: Vec<String> = self.watched.iter().cloned().collect();
        ids.sort();
        self.queue = ids.into();
        self.limited_times = 0;
        self.pump();
        self.ping();
    }

    pub fn on_message(&mut self, generation: u64, text: &str) {
        if generation != self.generation {
            return;
        }
        let Some(inc) = parse_incoming(text) else {
            // Not a frame this contract could have sent: end the connection and replay from
            // the cursor rather than guess (CONTRIBUTING §6.4).
            if self.has_socket {
                self.out.push(Out::Close);
            }
            return;
        };
        // Anything well-formed proves the peer is there, an unknown event included (D-82).
        self.arm_idle();
        if matches!(inc.frame, ServerFrame::Unknown(_)) {
            return;
        }
        self.measure_clock(&inc);
        self.note_replay(&inc);
        self.answered(&inc);
        if let ServerFrame::MessageCreated(m) = &inc.frame {
            let (c, id) = (m.data.channel_id.to_string(), m.data.id.to_string());
            self.advance(&c, &id);
        }
        self.out.push(Out::Frame(inc));
    }

    /// A text frame that is not text (binary) is malformed too.
    pub fn on_binary(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        if self.has_socket {
            self.out.push(Out::Close);
        }
    }

    pub fn on_closed(&mut self, generation: u64, code: u16) {
        if generation != self.generation {
            return;
        }
        if code == close_codes::SESSION_ENDED {
            self.end();
            return;
        }
        if code == close_codes::TOO_BUSY {
            self.failures = self.failures.max(TOO_BUSY_BACKOFF_STEP as u32);
        }
        self.lost();
    }

    /// The upgrade was refused with an HTTP status: 401, 403 and 426 are final.
    pub fn on_refused(&mut self, generation: u64, status: u16) {
        if generation != self.generation {
            return;
        }
        if matches!(status, 401 | 403 | 426) {
            self.end();
        } else {
            self.lost();
        }
    }

    pub fn on_timer(&mut self, t: Timer) {
        if !self.running.remove(&t) {
            return;
        }
        match t {
            Timer::Retry => self.connect(),
            Timer::Stable => self.failures = 0,
            Timer::Beat => {
                if self.beat_is_deadline {
                    self.dead();
                } else {
                    self.ping();
                }
            }
            Timer::Resend => {
                if let Some((_, ids)) = self.in_flight.clone() {
                    self.send_chunk(ids);
                }
            }
        }
    }

    pub fn on_session_checked(&mut self, epoch: u64, check: SessionCheck) {
        self.checking = false;
        if epoch == self.check_epoch && check == SessionCheck::Ended {
            self.end();
        }
    }

    // --- operations ------------------------------------------------------------------

    pub fn start(&mut self) {
        if matches!(self.status, Status::Idle | Status::Ended) {
            self.connect();
        }
    }

    pub fn stop(&mut self) {
        self.generation += 1;
        self.check_epoch += 1;
        self.ping_sent = None;
        self.clear_timers();
        if self.has_socket {
            self.out.push(Out::Close);
        }
        self.has_socket = false;
        self.set_status(Status::Idle);
    }

    pub fn watch(&mut self, channel_id: &str) {
        if !self.watched.insert(channel_id.to_string()) {
            return;
        }
        self.queue.push_back(channel_id.to_string());
        self.pump();
    }

    /// One `UNSUBSCRIBE` for all of them, chunked at the frame's bound (#523).
    pub fn unwatch(&mut self, channel_ids: &[String]) {
        let gone = self.let_go(channel_ids);
        for chunk in gone.chunks(SUBSCRIBE_MAX_CHANNELS as usize) {
            let id = self.new_id();
            let ids = chunk.iter().map(Uuid::new).collect();
            self.write(&ClientFrame::Unsubscribe(UnsubscribeFrame { id, payload: UnsubscribeFramePayload { channel_ids: ids } }));
        }
    }

    /// Lets go of channels the server already dropped (`ACCESS_REVOKED`): nothing is sent.
    pub fn forget(&mut self, channel_ids: &[String]) {
        self.let_go(channel_ids);
    }

    /// Notes an id obtained another way (a history page) so a reconnect resumes above it.
    pub fn observe(&mut self, channel_id: &str, message_id: &str) {
        self.cursors.observe(channel_id, message_id);
    }

    pub fn pending_continuations(&self) -> HashMap<String, String> {
        self.pending.clone()
    }

    /// The continuation that began at `from` reached the present.
    pub fn continuation_done(&mut self, channel_id: &str, from: &str) {
        if self.pending.get(channel_id).map(String::as_str) == Some(from.to_ascii_lowercase().as_str()) {
            self.pending.remove(channel_id);
        }
    }

    /// `false` when the frame did not leave, so the caller keeps it to send again.
    pub fn send(&mut self, frame: &ClientFrame) -> bool {
        self.write(frame)
    }

    /// The network is gone: an open or opening socket is declared dead at once.
    pub fn network_lost(&mut self) {
        if matches!(self.status, Status::Open | Status::Connecting) {
            self.dead();
        }
    }

    /// The network is back: the backoff starts over, and an open socket is asked at once
    /// whether it lived through the gap.
    pub fn network_back(&mut self) {
        if self.status == Status::Open {
            if self.ping_sent.is_none() {
                self.ping();
            }
            return;
        }
        if !matches!(self.status, Status::Waiting | Status::Connecting) {
            return;
        }
        if self.status == Status::Connecting {
            self.generation += 1;
            if self.has_socket {
                self.out.push(Out::Close);
            }
            self.has_socket = false;
        }
        self.cancel(Timer::Retry);
        self.set_status(Status::Waiting);
        let r = (self.random)();
        self.schedule(Timer::Retry, reconnect_delay(0, r));
        self.failures = 1;
    }
}

/// Reads one text frame. `None` is malformed: not JSON, not an object with an `event`, or a
/// known event with the wrong shape. An unknown event reads as `ServerFrame::Unknown`.
pub fn parse_incoming(text: &str) -> Option<Incoming> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object()?;
    obj.get("event")?.as_str()?;
    let ts = obj.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0);
    let ref_id = obj.get("refId").and_then(|r| r.as_str()).map(|s| s.to_ascii_lowercase());
    let version = obj.get("version").and_then(|r| r.as_i64());
    let frame: ServerFrame = serde_json::from_value(v).ok()?;
    Some(Incoming { frame, ref_id, ts, version })
}
