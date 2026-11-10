//! Reconnect and replay (`ARCHITECTURE.md` §5.4), against a socket and timers a test drives by
//! hand: no network, no waiting. A port of the web client's `realtime/connection.test.ts`; each
//! test keeps its original title in its doc comment. A `test.each` row is its own test here.

use purlay_contract::constants::{
    backoff, close_codes, CHECK_SESSION_AFTER_FAILURES, PING_AFTER_IDLE_MS, PONG_DEADLINE_MS, STABLE_AFTER_MS,
    SUBSCRIBE_MAX_CHANNELS, TOO_BUSY_BACKOFF_STEP,
};
use purlay_contract::models::{ClientFrame, ErrorCode, PingFrame};
use purlay_contract::{EmptyObject, Timestamp, Uuid};
use purlay_core::clock::{Clock, SharedClock};
use purlay_core::realtime::backoff::reconnect_delay;
use purlay_core::realtime::connection::{Connection, Incoming, Out, SessionCheck, Status, Timer};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

// --- fixtures --------------------------------------------------------------------------------

fn t0() -> i64 {
    Timestamp::new("2026-10-09T12:00:00.000Z").unix_ms().unwrap()
}

/// A UUIDv7 made at `T0 + offset_ms`: ids sort by time as text.
fn id_at(offset_ms: i64) -> String {
    let ms = (t0() + offset_ms) as u64;
    format!("{:08x}-{:04x}-7000-8000-000000000000", ms >> 16, ms & 0xffff)
}

fn channel(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

const BASE: u64 = backoff::BASE_MS as u64;
const CAP: u64 = backoff::CAP_MS as u64;
const MAX: usize = SUBSCRIBE_MAX_CHANNELS as usize;
const CHECK_AFTER: usize = CHECK_SESSION_AFTER_FAILURES as usize;
const PING_IDLE: u64 = PING_AFTER_IDLE_MS as u64;
const PONG_DEADLINE: u64 = PONG_DEADLINE_MS as u64;
const STABLE: u64 = STABLE_AFTER_MS as u64;

fn created(channel_id: &str, id: &str) -> String {
    json!({
        "event": "MESSAGE_CREATED",
        "ts": t0(),
        "data": {
            "id": id,
            "channelId": channel_id,
            "sender": null,
            "githubEvent": null,
            "content": "hi",
            "editedAt": null,
            "deletedAt": null,
            "createdAt": Timestamp::from_unix_ms(t0()).as_str(),
            "parentId": null,
            "thread": null,
            "reactions": [],
            "mentions": [],
            "attachments": [],
        }
    })
    .to_string()
}

fn pong(ref_id: Option<&str>, ts: i64) -> String {
    match ref_id {
        None => json!({ "event": "PONG", "data": {}, "ts": ts }),
        Some(r) => json!({ "event": "PONG", "data": {}, "ts": ts, "refId": r }),
    }
    .to_string()
}

/// An `ACK` of a `SUBSCRIBE` that answers nothing in particular (no `refId`).
fn subscribed(channel_id: &str, next_cursor: Option<&str>) -> String {
    json!({ "event": "ACK", "ts": t0(), "data": { "channels": [{ "channelId": channel_id, "nextCursor": next_cursor }] } })
        .to_string()
}

/// The `ACK` answering a `SUBSCRIBE` chunk, every channel caught up.
fn ack_for(chunk: &Value) -> String {
    ack_for_id(chunk, chunk["id"].as_str().unwrap())
}

fn ack_for_id(chunk: &Value, ref_id: &str) -> String {
    let channels: Vec<Value> = chunk["payload"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({ "channelId": c["channelId"], "nextCursor": null }))
        .collect();
    json!({ "event": "ACK", "ts": t0(), "refId": ref_id, "data": { "channels": channels } }).to_string()
}

fn limited(ref_id: &str) -> String {
    json!({ "event": "ERROR", "ts": t0(), "refId": ref_id, "data": { "code": "RATE_LIMITED", "message": "slow down" } })
        .to_string()
}

fn error_for(ref_id: &str, code: &str, message: &str) -> String {
    json!({ "event": "ERROR", "ts": t0(), "refId": ref_id, "data": { "code": code, "message": message } }).to_string()
}

fn id_of(frame: &Value) -> String {
    frame["id"].as_str().unwrap().to_string()
}

/// A SUBSCRIBE frame's channels as `(channelId, afterMessageId)`.
fn channels_of(frame: &Value) -> Vec<(String, Option<String>)> {
    frame["payload"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["channelId"].as_str().unwrap().to_string(), c["afterMessageId"].as_str().map(str::to_string)))
        .collect()
}

// --- harness ---------------------------------------------------------------------------------

struct FakeClock(AtomicI64);

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct FakeSocket {
    generation: u64,
    sent: Vec<String>,
    closed_by_client: bool,
}

#[derive(Debug, Clone, Copy)]
struct LiveTimer {
    timer: Timer,
    ms: u64,
    due: i64,
}

/// The TS harness: the sockets opened, the timers live (in the order they were scheduled), the
/// frames and statuses handed on, and one entry per session check.
struct Harness {
    conn: Connection,
    clock: Arc<FakeClock>,
    sockets: Vec<FakeSocket>,
    timers: Vec<LiveTimer>,
    /// Every wait ever scheduled, live or not.
    scheduled: Vec<u64>,
    frames: Vec<Incoming>,
    statuses: Vec<Status>,
    /// The epoch of each `CheckSession` asked for, in order.
    checks: Vec<u64>,
    refused: Vec<(Vec<String>, ErrorCode, String)>,
    /// Every socket fails to be created, as if `openSocket` threw.
    open_fails: bool,
}

fn harness() -> Harness {
    harness_with(|| 0.0)
}

fn harness_with(random: impl FnMut() -> f64 + Send + 'static) -> Harness {
    let clock = Arc::new(FakeClock(AtomicI64::new(t0())));
    let shared: SharedClock = clock.clone();
    Harness {
        conn: Connection::new(shared, Box::new(random)),
        clock,
        sockets: Vec::new(),
        timers: Vec::new(),
        scheduled: Vec::new(),
        frames: Vec::new(),
        statuses: Vec::new(),
        checks: Vec::new(),
        refused: Vec::new(),
        open_fails: false,
    }
}

impl Harness {
    fn drain(&mut self) {
        loop {
            let outs = self.conn.take();
            if outs.is_empty() {
                return;
            }
            for out in outs {
                match out {
                    Out::Open { generation } => {
                        self.sockets.push(FakeSocket { generation, sent: Vec::new(), closed_by_client: false });
                        if self.open_fails {
                            self.conn.on_open_failed(generation);
                        }
                    }
                    Out::Send(text) => self.sockets.last_mut().expect("a send with no socket").sent.push(text),
                    Out::Close => self.sockets.last_mut().expect("a close with no socket").closed_by_client = true,
                    Out::Schedule(timer, ms) => {
                        self.timers.retain(|t| t.timer != timer);
                        let due = self.clock.now_ms() + ms as i64;
                        self.timers.push(LiveTimer { timer, ms, due });
                        self.scheduled.push(ms);
                    }
                    Out::Cancel(timer) => self.timers.retain(|t| t.timer != timer),
                    Out::CheckSession { epoch } => self.checks.push(epoch),
                    Out::Frame(inc) => self.frames.push(inc),
                    Out::Status(s) => self.statuses.push(s),
                    Out::SubscribeRefused { channel_ids, code, ref_id } => self.refused.push((channel_ids, code, ref_id)),
                }
            }
        }
    }

    // operations

    fn start(&mut self) {
        self.conn.start();
        self.drain();
    }
    fn stop(&mut self) {
        self.conn.stop();
        self.drain();
    }
    fn status(&self) -> Status {
        self.conn.status()
    }
    fn watch(&mut self, c: &str) {
        self.conn.watch(c);
        self.drain();
    }
    fn unwatch(&mut self, cs: &[String]) {
        self.conn.unwatch(cs);
        self.drain();
    }
    fn forget(&mut self, cs: &[String]) {
        self.conn.forget(cs);
        self.drain();
    }
    fn observe(&mut self, c: &str, id: &str) {
        self.conn.observe(c, id);
        self.drain();
    }
    fn continuation_done(&mut self, c: &str, from: &str) {
        self.conn.continuation_done(c, from);
        self.drain();
    }
    fn send(&mut self, frame: &ClientFrame) -> bool {
        let sent = self.conn.send(frame);
        self.drain();
        sent
    }
    fn network_lost(&mut self) {
        self.conn.network_lost();
        self.drain();
    }
    fn network_back(&mut self) {
        self.conn.network_back();
        self.drain();
    }
    /// Settles the `i`-th session check.
    fn answer(&mut self, i: usize, check: SessionCheck) {
        let epoch = self.checks[i];
        self.conn.on_session_checked(epoch, check);
        self.drain();
    }

    // what a socket says (`socket.handlers.*`)

    fn last(&self) -> usize {
        assert!(!self.sockets.is_empty(), "no socket was opened");
        self.sockets.len() - 1
    }
    fn opened(&mut self, s: usize) {
        let g = self.sockets[s].generation;
        self.conn.on_opened(g);
        self.drain();
    }
    fn message(&mut self, s: usize, text: &str) {
        let g = self.sockets[s].generation;
        self.conn.on_message(g, text);
        self.drain();
    }
    fn binary(&mut self, s: usize) {
        let g = self.sockets[s].generation;
        self.conn.on_binary(g);
        self.drain();
    }
    fn closed(&mut self, s: usize, code: u16) {
        let g = self.sockets[s].generation;
        self.conn.on_closed(g, code);
        self.drain();
    }

    // timers

    fn waits(&self) -> Vec<u64> {
        self.timers.iter().map(|t| t.ms).collect()
    }
    fn fire(&mut self, timer: Timer) {
        self.timers.retain(|t| t.timer != timer);
        self.conn.on_timer(timer);
        self.drain();
    }
    /// Fire the first timer that is waiting, and say how long it waited.
    fn fire_next(&mut self) -> u64 {
        let t = *self.timers.first().expect("nothing is scheduled");
        self.fire(t.timer);
        t.ms
    }
    /// Fire the live timer that waits `ms`.
    fn fire_waiting(&mut self, ms: u64) {
        let t = *self.timers.iter().find(|t| t.ms == ms).unwrap_or_else(|| panic!("no timer is waiting {ms} ms"));
        self.fire(t.timer);
    }
    /// Moves the local clock by `ms` and fires, in time order, every timer that falls due.
    #[allow(dead_code)]
    fn advance(&mut self, ms: i64) {
        let until = self.clock.now_ms() + ms;
        loop {
            let next = self.timers.iter().filter(|t| t.due <= until).min_by_key(|t| t.due).copied();
            let Some(t) = next else { break };
            self.clock.0.store(t.due.max(self.clock.now_ms()), Ordering::SeqCst);
            self.fire(t.timer);
        }
        self.clock.0.store(until, Ordering::SeqCst);
    }
    /// Moves the local clock without firing anything (`local.advance`).
    fn local_advance(&mut self, ms: i64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    // what a socket was sent

    fn sent_ops(&self, s: usize, op: &str) -> Vec<Value> {
        self.sockets[s]
            .sent
            .iter()
            .map(|t| serde_json::from_str::<Value>(t).unwrap())
            .filter(|v| v["op"] == op)
            .collect()
    }
    fn subscribes(&self, s: usize) -> Vec<Value> {
        self.sent_ops(s, "SUBSCRIBE")
    }
    fn pings(&self, s: usize) -> Vec<Value> {
        self.sent_ops(s, "PING")
    }
    fn first_subscribe(&self, s: usize) -> Value {
        self.subscribes(s).into_iter().next().expect("no SUBSCRIBE was sent")
    }
    fn first_ping_id(&self, s: usize) -> String {
        id_of(&self.pings(s)[0])
    }

    /// `start`, then the socket opens.
    fn open_one(&mut self) -> usize {
        self.start();
        let s = self.last();
        self.opened(s);
        s
    }

    /// The wait before a chunk is sent again: the first live timer that is not the heartbeat's
    /// or the backoff's forgiveness.
    fn retry_timer(&self) -> Option<LiveTimer> {
        self.timers.iter().find(|t| ![STABLE, PONG_DEADLINE, PING_IDLE].contains(&t.ms)).copied()
    }

    fn watch_many(&mut self, total: u32) {
        for n in 1..=total {
            self.watch(&channel(n));
        }
    }

    fn frame_events(&self) -> Vec<&'static str> {
        use purlay_contract::models::ServerFrame as F;
        self.frames
            .iter()
            .map(|i| match i.frame {
                F::Pong(_) => "PONG",
                F::MessageCreated(_) => "MESSAGE_CREATED",
                F::Subscribed(_) | F::Ack(_) | F::Unsubscribed(_) => "ACK",
                F::Error(_) => "ERROR",
                _ => "OTHER",
            })
            .collect()
    }
}

// --- connecting ------------------------------------------------------------------------------

/// start opens one socket, and a second start opens no more
#[test]
fn start_opens_one_socket_and_a_second_start_opens_no_more() {
    let mut h = harness();
    h.start();
    h.start();
    assert_eq!(h.sockets.len(), 1);
    assert_eq!(h.status(), Status::Connecting);
}

/// is open once the socket says so
#[test]
fn is_open_once_the_socket_says_so() {
    let mut h = harness();
    h.start();
    let s = h.last();
    h.opened(s);
    assert_eq!(h.status(), Status::Open);
}

// --- reconnecting ----------------------------------------------------------------------------

/// a dropped socket is retried after the backoff, and backs off further each time
#[test]
fn a_dropped_socket_is_retried_after_the_backoff_and_backs_off_further_each_time() {
    let mut h = harness();
    h.start();
    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.status(), Status::Waiting);
    assert_eq!(h.fire_next(), BASE / 2);

    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.fire_next(), BASE);
    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.fire_next(), BASE * 2);
    assert_eq!(h.sockets.len(), 4);
}

/// the wait never exceeds the cap, however long the outage
#[test]
fn the_wait_never_exceeds_the_cap_however_long_the_outage() {
    let mut h = harness_with(|| 1.0);
    h.start();
    let mut waits = Vec::new();
    for _ in 0..12 {
        let s = h.last();
        h.closed(s, 1006);
        waits.push(h.fire_next());
    }
    assert_eq!(*waits.iter().max().unwrap(), CAP);
    assert_eq!(*waits.last().unwrap(), CAP);
}

/// a connection that stayed up long enough starts the backoff over
#[test]
fn a_connection_that_stayed_up_long_enough_starts_the_backoff_over() {
    let mut h = harness();
    h.start();
    for _ in 0..3 {
        let s = h.last();
        h.closed(s, 1006);
        h.fire_next();
    }
    let s = h.last();
    h.opened(s);
    assert_eq!(h.fire_next(), STABLE); // the stability timer: the connection held
    h.closed(s, 1006);
    assert_eq!(h.fire_next(), BASE / 2);
}

/// a connection that opens and drops at once does not start the backoff over
#[test]
fn a_connection_that_opens_and_drops_at_once_does_not_start_the_backoff_over() {
    let mut h = harness();
    h.start();
    let s = h.last();
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    h.closed(s, 1006); // before the stability timer
    assert_eq!(h.waits().iter().filter(|&&ms| ms == BASE).count(), 1);
}

fn is_reconnected_from(code: u16) {
    let mut h = harness();
    let s = h.open_one();
    h.closed(s, code);
    assert_eq!(h.status(), Status::Waiting);
    assert_eq!(h.timers.len(), 1);
}

/// %s is reconnected from — a restart
#[test]
fn a_restart_is_reconnected_from() {
    is_reconnected_from(close_codes::RESTARTING);
}

/// %s is reconnected from — falling behind
#[test]
fn falling_behind_is_reconnected_from() {
    is_reconnected_from(close_codes::RESYNC);
}

/// %s is reconnected from — a network drop
#[test]
fn a_network_drop_is_reconnected_from() {
    is_reconnected_from(1006);
}

/// an ended session is not reconnected into a refusal
#[test]
fn an_ended_session_is_not_reconnected_into_a_refusal() {
    let mut h = harness();
    let s = h.open_one();
    h.closed(s, close_codes::SESSION_ENDED);
    assert_eq!(h.status(), Status::Ended);
    assert!(h.timers.is_empty());
    assert_eq!(h.sockets.len(), 1);
}

/// a socket that cannot even be created is a failed attempt, not a crash
#[test]
fn a_socket_that_cannot_even_be_created_is_a_failed_attempt_not_a_crash() {
    let mut h = harness();
    h.open_fails = true;
    h.start();
    assert_eq!(h.status(), Status::Waiting);
    assert_eq!(h.scheduled, vec![BASE / 2]);
}

/// stop cancels a pending retry and ignores a late close from the old socket
#[test]
fn stop_cancels_a_pending_retry_and_ignores_a_late_close_from_the_old_socket() {
    let mut h = harness();
    h.start();
    let old = h.last();
    h.closed(old, 1006);
    h.stop();
    assert!(h.timers.is_empty());
    assert_eq!(h.status(), Status::Idle);

    h.closed(old, 1006);
    assert!(h.timers.is_empty());
}

// --- a socket that has been replaced ---------------------------------------------------------

/// its late frames are neither shown nor remembered once a newer socket exists
#[test]
fn its_late_frames_are_neither_shown_nor_remembered_once_a_newer_socket_exists() {
    let mut h = harness();
    h.watch(&channel(1));
    let old = h.open_one();
    h.closed(old, 1006);
    h.fire_next();
    let current = h.last();
    assert_ne!(current, old);

    h.message(old, &created(&channel(1), &id_at(9_000)));
    assert!(h.frames.is_empty());

    // The control: the same frame on the current socket is shown.
    h.opened(current);
    h.message(current, &created(&channel(1), &id_at(9_000)));
    assert_eq!(h.frames.len(), 1);
}

/// its late frames do not advance the cursor the next connection resumes from
#[test]
fn its_late_frames_do_not_advance_the_cursor_the_next_connection_resumes_from() {
    let mut h = harness();
    h.watch(&channel(1));
    let old = h.open_one();
    h.closed(old, 1006);
    h.fire_next();
    h.message(old, &created(&channel(1), &id_at(9_000)));

    let s = h.last();
    h.opened(s);
    assert_eq!(channels_of(&h.first_subscribe(s)), vec![(channel(1), None)]);
}

/// after stop, a frame from the old socket is dropped
#[test]
fn after_stop_a_frame_from_the_old_socket_is_dropped() {
    let mut h = harness();
    let old = h.open_one();
    h.stop();
    h.message(old, &created(&channel(1), &id_at(1_000)));
    assert!(h.frames.is_empty());
}

// --- replay by cursor ------------------------------------------------------------------------

/// a channel nothing was heard in resubscribes from null
#[test]
fn a_channel_nothing_was_heard_in_resubscribes_from_null() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    assert_eq!(channels_of(&h.first_subscribe(s)), vec![(channel(1), None)]);
}

/// a reconnect names the highest id heard per channel
#[test]
fn a_reconnect_names_the_highest_id_heard_per_channel() {
    let mut h = harness();
    h.watch(&channel(1));
    h.watch(&channel(2));
    let s = h.open_one();

    let low = id_at(1_000);
    let high = id_at(5_000);
    h.message(s, &created(&channel(1), &high));
    h.message(s, &created(&channel(1), &low)); // out of order
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);

    let channels = channels_of(&h.first_subscribe(s));
    assert!(channels.contains(&(channel(1), Some(high))));
    assert!(channels.contains(&(channel(2), None)));
}

/// an id loaded some other way, such as a history page, moves the cursor
#[test]
fn an_id_loaded_some_other_way_such_as_a_history_page_moves_the_cursor() {
    let mut h = harness();
    h.watch(&channel(1));
    let from_history = id_at(9_000);
    h.observe(&channel(1), &from_history);
    let s = h.open_one();
    assert_eq!(channels_of(&h.first_subscribe(s))[0].1, Some(from_history));
}

/// more channels than one frame may name are split across frames, each once
#[test]
fn more_channels_than_one_frame_may_name_are_split_across_frames_each_once() {
    let mut h = harness();
    let total = MAX as u32 + 30;
    h.watch_many(total);
    let s = h.open_one();

    // One at a time: the second leaves when the first is acknowledged (pacing, below).
    let first = h.first_subscribe(s);
    h.message(s, &ack_for(&first));
    let frames = h.subscribes(s);
    assert_eq!(frames.iter().map(|f| channels_of(f).len()).collect::<Vec<_>>(), vec![MAX, 30]);
    let named: HashSet<String> = frames.iter().flat_map(|f| channels_of(f).into_iter().map(|c| c.0)).collect();
    assert_eq!(named.len(), total as usize);
    let ids: HashSet<String> = frames.iter().map(id_of).collect();
    assert_eq!(ids.len(), 2);
}

/// channels unwatched together go in one frame, chunked at the frame bound (#523)
#[test]
fn channels_unwatched_together_go_in_one_frame_chunked_at_the_frame_bound() {
    let mut h = harness();
    let s = h.open_one();
    let many: Vec<String> = (1..=MAX as u32 + 2).map(channel).collect();
    for id in &many {
        h.watch(id);
    }
    let before = h.sockets[s].sent.len();
    h.unwatch(&many);
    let frames: Vec<Value> = h.sockets[s].sent[before..].iter().map(|t| serde_json::from_str(t).unwrap()).collect();
    assert_eq!(frames.iter().map(|f| f["op"].as_str().unwrap()).collect::<Vec<_>>(), vec!["UNSUBSCRIBE", "UNSUBSCRIBE"]);
    let named: Vec<String> = frames
        .iter()
        .flat_map(|f| f["payload"]["channelIds"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()))
        .collect();
    assert_eq!(named, many);
}

/// a channel the server already let go is forgotten without a frame, and not subscribed again (#523)
#[test]
fn a_channel_the_server_already_let_go_is_forgotten_without_a_frame_and_not_subscribed_again() {
    let mut h = harness();
    let s = h.open_one();
    h.watch(&channel(1));
    h.watch(&channel(2));
    let before = h.sockets[s].sent.len();
    h.forget(&[channel(1), channel(2)]);
    assert_eq!(h.sockets[s].sent.len(), before);
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert!(h.subscribes(s).is_empty());
}

/// nothing watched, nothing subscribed
#[test]
fn nothing_watched_nothing_subscribed() {
    let mut h = harness();
    let s = h.open_one();
    assert!(h.subscribes(s).is_empty());
}

/// watching while open subscribes at once, and unwatching says so
#[test]
fn watching_while_open_subscribes_at_once_and_unwatching_says_so() {
    let mut h = harness();
    let s = h.open_one();
    h.watch(&channel(1));
    assert_eq!(h.subscribes(s).len(), 1);

    h.unwatch(&[channel(1)]);
    let last: Value = serde_json::from_str(h.sockets[s].sent.last().unwrap()).unwrap();
    assert_eq!(last["op"], "UNSUBSCRIBE");
    assert_eq!(last["payload"]["channelIds"], json!([channel(1)]));

    // And a channel no longer watched is not resubscribed after a drop.
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert!(h.subscribes(s).is_empty());
}

/// watching twice subscribes once
#[test]
fn watching_twice_subscribes_once() {
    let mut h = harness();
    let s = h.open_one();
    h.watch(&channel(1));
    h.watch(&channel(1));
    assert_eq!(h.subscribes(s).len(), 1);
}

// --- frames ----------------------------------------------------------------------------------

/// a frame the contract defines is passed on
#[test]
fn a_frame_the_contract_defines_is_passed_on() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &json!({ "event": "PONG", "data": {}, "ts": t0() }).to_string());
    assert_eq!(h.frame_events(), vec!["PONG"]);
}

/// %s ends the connection rather than being guessed at — text that is not a frame
#[test]
fn text_that_is_not_a_frame_ends_the_connection_rather_than_being_guessed_at() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, "garbage");
    assert!(h.sockets[s].closed_by_client);
    assert!(h.frames.is_empty());
}

/// %s ends the connection rather than being guessed at — binary data
#[test]
fn binary_data_ends_the_connection_rather_than_being_guessed_at() {
    let mut h = harness();
    let s = h.open_one();
    h.binary(s);
    assert!(h.sockets[s].closed_by_client);
    assert!(h.frames.is_empty());
}

/// send reports whether the frame left
#[test]
fn send_reports_whether_the_frame_left() {
    let mut h = harness();
    let frame = ClientFrame::Ping(PingFrame { id: Uuid::new(id_at(0)), payload: EmptyObject {} });
    assert!(!h.send(&frame));
    h.start();
    assert!(!h.send(&frame));
    let s = h.last();
    h.opened(s);
    assert!(h.send(&frame));
    let last: Value = serde_json::from_str(h.sockets[s].sent.last().unwrap()).unwrap();
    assert_eq!(last, serde_json::to_value(&frame).unwrap());
    assert_eq!(last, json!({ "id": id_at(0), "op": "PING", "payload": {} }));
}

/// status changes are reported once each, in order
#[test]
fn status_changes_are_reported_once_each_in_order() {
    let mut h = harness();
    let s = h.open_one();
    h.closed(s, 1006);
    h.fire_next();
    assert_eq!(h.statuses, vec![Status::Connecting, Status::Open, Status::Waiting, Status::Connecting]);
}

// --- the heartbeat ---------------------------------------------------------------------------

/// a PING leaves as soon as the socket opens, so the clock is measured early
#[test]
fn a_ping_leaves_as_soon_as_the_socket_opens_so_the_clock_is_measured_early() {
    let mut h = harness();
    let s = h.open_one();
    assert_eq!(h.pings(s).len(), 1);
}

/// silence past the deadline is a dead socket: closed, and reconnected without waiting for it
#[test]
fn silence_past_the_deadline_is_a_dead_socket_closed_and_reconnected_without_waiting_for_it() {
    let mut h = harness();
    let s = h.open_one();
    h.fire_waiting(PONG_DEADLINE);
    assert!(h.sockets[s].closed_by_client);
    assert_eq!(h.status(), Status::Waiting);
    assert_eq!(h.waits(), vec![BASE / 2]);

    // A half-open socket may announce its close minutes later; that must change nothing.
    h.closed(s, 1006);
    assert_eq!(h.timers.len(), 1);
    h.fire_next();
    assert_eq!(h.sockets.len(), 2);
}

/// an answer inside the deadline keeps the socket, and the next PING waits for idleness
#[test]
fn an_answer_inside_the_deadline_keeps_the_socket_and_the_next_ping_waits_for_idleness() {
    let mut h = harness();
    let s = h.open_one();
    let ping = h.first_ping_id(s);
    h.message(s, &pong(Some(&ping), t0()));
    assert_eq!(h.waits(), vec![STABLE, PING_IDLE]);

    h.fire_waiting(PING_IDLE);
    assert_eq!(h.pings(s).len(), 2);
    assert!(h.waits().contains(&PONG_DEADLINE));
}

/// any frame at all is proof of life, not only a PONG
#[test]
fn any_frame_at_all_is_proof_of_life_not_only_a_pong() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &created(&channel(1), &id_at(1_000)));
    assert!(!h.waits().contains(&PONG_DEADLINE));
    assert!(h.waits().contains(&PING_IDLE));
    assert_eq!(h.status(), Status::Open);
}

/// an event this client does not know is also proof of life
#[test]
fn an_event_this_client_does_not_know_is_also_proof_of_life() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &json!({ "event": "FROM_THE_FUTURE", "data": {}, "ts": t0() }).to_string());
    assert!(!h.waits().contains(&PONG_DEADLINE));
}

/// a frame that is malformed is not proof of life
#[test]
fn a_frame_that_is_malformed_is_not_proof_of_life() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, "garbage");
    assert!(h.waits().contains(&PONG_DEADLINE));
}

/// a dropped socket stops the heartbeat, and the next one starts its own
#[test]
fn a_dropped_socket_stops_the_heartbeat_and_the_next_one_starts_its_own() {
    let mut h = harness();
    let first = h.open_one();
    h.closed(first, 1006);
    assert_eq!(h.waits(), vec![BASE / 2]);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert_eq!(h.pings(s).len(), 1);
    assert_eq!(h.pings(first).len(), 1);
}

/// stop leaves no timer behind
#[test]
fn stop_leaves_no_timer_behind() {
    let mut h = harness();
    h.open_one();
    h.stop();
    assert!(h.timers.is_empty());
}

/// nothing is timed before the socket opens
#[test]
fn nothing_is_timed_before_the_socket_opens() {
    let mut h = harness();
    h.start();
    assert!(h.timers.is_empty());
}

// --- the browser says the network went or came back (#626) ----------------------------------

/// gone while open: the socket is dead at once, not after a PING goes unanswered
#[test]
fn gone_while_open_the_socket_is_dead_at_once_not_after_a_ping_goes_unanswered() {
    let mut h = harness();
    let s = h.open_one();
    h.network_lost();
    assert!(h.sockets[s].closed_by_client);
    assert_eq!(h.status(), Status::Waiting);
    // Its own close, when it comes, changes nothing.
    h.closed(s, 1006);
    assert_eq!(h.timers.len(), 1);
}

/// gone while connecting: the attempt is given up and waits like any failed one
#[test]
fn gone_while_connecting_the_attempt_is_given_up_and_waits_like_any_failed_one() {
    let mut h = harness();
    h.start();
    let attempt = h.last();
    h.network_lost();
    assert!(h.sockets[attempt].closed_by_client);
    assert_eq!(h.status(), Status::Waiting);
    h.opened(attempt);
    assert_eq!(h.status(), Status::Waiting);
}

/// back while waiting: the wait it was in is cut to the first rung, jittered
#[test]
fn back_while_waiting_the_wait_it_was_in_is_cut_to_the_first_rung_jittered() {
    let mut h = harness();
    let socket = h.open_one();
    // A long outage: the backoff has climbed to its cap.
    for _ in 0..12 {
        let s = h.last();
        h.closed(s, 1006);
        h.fire_next();
    }
    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.waits(), vec![reconnect_delay(13, 0.0)]);
    assert!(h.waits()[0] > reconnect_delay(0, 1.0));
    h.network_back();
    // Not at once: a network back for every page on it at the same moment is a herd, so each
    // waits the first rung's jittered half-second to a second (D-76).
    assert_eq!(h.waits(), vec![reconnect_delay(0, 0.0)]);
    assert_eq!(h.status(), Status::Waiting);
    h.fire_next();
    assert_eq!(h.status(), Status::Connecting);
    assert!(!h.sockets[socket].closed_by_client);
}

/// back, and the first attempt fails: the next wait is the second rung, not the cap
#[test]
fn back_and_the_first_attempt_fails_the_next_wait_is_the_second_rung_not_the_cap() {
    let mut h = harness();
    h.open_one();
    for _ in 0..12 {
        let s = h.last();
        h.closed(s, 1006);
        h.fire_next();
    }
    let s = h.last();
    h.closed(s, 1006);
    h.network_back();
    h.fire_next();
    // The browser often says online before the route works: that attempt fails.
    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.waits(), vec![reconnect_delay(1, 0.0)]);
}

/// back while open: a PING asks at once whether the socket lived through it
#[test]
fn back_while_open_a_ping_asks_at_once_whether_the_socket_lived_through_it() {
    let mut h = harness();
    let s = h.open_one();
    let ping = h.first_ping_id(s);
    h.message(s, &pong(Some(&ping), t0()));
    assert!(!h.waits().contains(&PONG_DEADLINE));
    h.network_back();
    assert_eq!(h.pings(s).len(), 2);
    assert!(h.waits().contains(&PONG_DEADLINE));
    // A socket that did not live through it is found dead by the usual deadline.
    h.fire_waiting(PONG_DEADLINE);
    assert!(h.sockets[s].closed_by_client);
}

/// back while open with a PING already out sends no second one
#[test]
fn back_while_open_with_a_ping_already_out_sends_no_second_one() {
    let mut h = harness();
    let s = h.open_one();
    h.network_back();
    assert_eq!(h.pings(s).len(), 1);
}

/// back while an attempt hangs: it is given up, and tried again from the first rung
#[test]
fn back_while_an_attempt_hangs_it_is_given_up_and_tried_again_from_the_first_rung() {
    let mut h = harness();
    h.start();
    let hung = h.last();
    h.network_back();
    assert!(h.sockets[hung].closed_by_client);
    assert_eq!(h.waits(), vec![reconnect_delay(0, 0.0)]);
    // Its answer, if it ever comes, changes nothing.
    h.opened(hung);
    assert_eq!(h.status(), Status::Waiting);
    h.fire_next();
    assert_eq!(h.sockets.len(), 2);
}

/// neither starts a connection that was never started, was stopped, or ended
#[test]
fn neither_starts_a_connection_that_was_never_started_was_stopped_or_ended() {
    let mut idle = harness();
    idle.network_lost();
    idle.network_back();
    assert!(idle.sockets.is_empty());
    assert_eq!(idle.status(), Status::Idle);

    let mut stopped = harness();
    stopped.open_one();
    stopped.stop();
    stopped.network_back();
    assert_eq!(stopped.sockets.len(), 1);
    assert_eq!(stopped.status(), Status::Idle);

    let mut ended = harness();
    let s = ended.open_one();
    ended.closed(s, close_codes::SESSION_ENDED);
    ended.network_lost();
    ended.network_back();
    assert_eq!(ended.sockets.len(), 1);
    assert_eq!(ended.status(), Status::Ended);
}

/// gone while already waiting does not count another failure
#[test]
fn gone_while_already_waiting_does_not_count_another_failure() {
    let mut h = harness();
    let s = h.open_one();
    h.closed(s, 1006);
    let before = h.waits();
    h.network_lost();
    assert_eq!(h.waits(), before);
}

// --- the first PONG sets the server clock ----------------------------------------------------

/// its midpoint against the local clock becomes the offset
#[test]
fn its_midpoint_against_the_local_clock_becomes_the_offset() {
    let mut h = harness();
    let s = h.open_one();
    let ping = h.first_ping_id(s);
    h.local_advance(200); // the round trip
    h.message(s, &pong(Some(&ping), t0() + 100 + 5_000));
    assert!(h.conn.server_clock().corrected());
    assert_eq!(h.conn.server_now_ms(), t0() + 200 + 5_000);
}

/// a PONG that answers no PING of ours is not a sample
#[test]
fn a_pong_that_answers_no_ping_of_ours_is_not_a_sample() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &pong(None, t0() + 9_000));
    h.message(s, &pong(Some(&id_at(77_000)), t0() + 9_000));
    assert!(!h.conn.server_clock().corrected());
}

/// a local clock that ran backwards during the round trip is skipped, then measured again
#[test]
fn a_local_clock_that_ran_backwards_during_the_round_trip_is_skipped_then_measured_again() {
    let mut h = harness();
    let s = h.open_one();
    h.local_advance(-1_000);
    let ping = h.first_ping_id(s);
    h.message(s, &pong(Some(&ping), t0()));
    assert!(!h.conn.server_clock().corrected());
    assert_eq!(h.status(), Status::Open);

    h.fire_waiting(PING_IDLE);
    h.local_advance(100);
    let second = id_of(&h.pings(s)[1]);
    h.message(s, &pong(Some(&second), t0() + 3_000));
    assert!(h.conn.server_clock().corrected());
}

/// once corrected, a later PONG does not move it
#[test]
fn once_corrected_a_later_pong_does_not_move_it() {
    let mut h = harness();
    let s = h.open_one();
    let ping = h.first_ping_id(s);
    h.message(s, &pong(Some(&ping), t0() + 5_000));
    let corrected = h.conn.server_now_ms();
    h.fire_waiting(PING_IDLE);
    let second = id_of(&h.pings(s)[1]);
    h.message(s, &pong(Some(&second), t0() + 99_000));
    assert_eq!(h.conn.server_now_ms(), corrected);
}

// --- a refusal is not an outage --------------------------------------------------------------

/// `times` handshakes in a row that never reached `open`.
fn fail_times(h: &mut Harness, times: usize) {
    h.start();
    for i in 0..times {
        let s = h.last();
        h.closed(s, 1006);
        if i < times - 1 {
            h.fire_next();
        }
    }
}

/// before enough failures, nobody is asked
#[test]
fn before_enough_failures_nobody_is_asked() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER - 1);
    assert!(h.checks.is_empty());
}

/// after repeated failures that never reached open, the session is asked about
#[test]
fn after_repeated_failures_that_never_reached_open_the_session_is_asked_about() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    assert_eq!(h.checks.len(), 1);
}

/// a session that has ended stops the retrying
#[test]
fn a_session_that_has_ended_stops_the_retrying() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.answer(0, SessionCheck::Ended);
    assert_eq!(h.status(), Status::Ended);
    assert!(h.timers.is_empty());
    assert_eq!(h.sockets.len(), CHECK_AFTER);
}

fn retrying_goes_on(answer: SessionCheck) {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.answer(0, answer);
    assert_eq!(h.status(), Status::Waiting);
    assert_eq!(h.timers.len(), 1);
}

/// %s: retrying goes on (%s) — active: the session is fine, so this is an outage
#[test]
fn active_retrying_goes_on_the_session_is_fine_so_this_is_an_outage() {
    retrying_goes_on(SessionCheck::Active);
}

/// %s: retrying goes on (%s) — unknown: the answer is a 5xx or a network error
#[test]
fn unknown_retrying_goes_on_the_answer_is_a_5xx_or_a_network_error() {
    retrying_goes_on(SessionCheck::Unknown);
}

/// a check that itself fails is an outage too
///
/// A rejected `checkSession` reads as `unknown` in the TS; the Rust driver answers it so.
#[test]
fn a_check_that_itself_fails_is_an_outage_too() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.answer(0, SessionCheck::Unknown);
    assert_eq!(h.status(), Status::Waiting);
    assert_eq!(h.timers.len(), 1);
}

/// only one check is in flight at a time
#[test]
fn only_one_check_is_in_flight_at_a_time() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER + 2);
    assert_eq!(h.checks.len(), 1);
}

/// having reached open once, the count starts over
#[test]
fn having_reached_open_once_the_count_starts_over() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER - 1);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.closed(s, 1006);
    assert!(h.checks.is_empty());
}

/// an answer that arrives after the connection recovered is ignored
#[test]
fn an_answer_that_arrives_after_the_connection_recovered_is_ignored() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.fire_next(); // the retry goes out while the check is still in flight
    let s = h.last();
    h.opened(s);
    h.answer(0, SessionCheck::Ended);
    assert_eq!(h.status(), Status::Open);
}

/// an answer that arrives after stop is ignored
#[test]
fn an_answer_that_arrives_after_stop_is_ignored() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.stop();
    h.answer(0, SessionCheck::Ended);
    assert_eq!(h.status(), Status::Idle);
}

/// once answered, a later failure asks again
#[test]
fn once_answered_a_later_failure_asks_again() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.answer(0, SessionCheck::Active);
    h.fire_next();
    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.checks.len(), 2);
}

// --- an ended session ------------------------------------------------------------------------

/// forgets its failures, and start works again from it
#[test]
fn forgets_its_failures_and_start_works_again_from_it() {
    let mut h = harness();
    h.start();
    for _ in 0..3 {
        let s = h.last();
        h.closed(s, 1006);
        h.fire_next();
    }
    let s = h.last();
    h.closed(s, close_codes::SESSION_ENDED);
    assert_eq!(h.status(), Status::Ended);

    h.watch(&channel(1));
    h.start();
    assert_eq!(h.status(), Status::Connecting);
    let s = h.last();
    h.opened(s);
    assert_eq!(h.subscribes(s).len(), 1);

    h.closed(s, 1006);
    assert_eq!(h.fire_next(), BASE / 2); // not 4 s: the old failures are gone
}

/// ended by a check, it too can be started again, with the count started over
#[test]
fn ended_by_a_check_it_too_can_be_started_again_with_the_count_started_over() {
    let mut h = harness();
    fail_times(&mut h, CHECK_AFTER);
    h.answer(0, SessionCheck::Ended);
    h.start();
    assert_eq!(h.status(), Status::Connecting);
    let s = h.last();
    h.closed(s, 1006);
    assert_eq!(h.checks.len(), 1);
}

// --- an event from a newer server ------------------------------------------------------------

/// is ignored: not passed on, and the connection stays
#[test]
fn is_ignored_not_passed_on_and_the_connection_stays() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &json!({ "event": "REACTION_ADDED", "data": { "x": 1 }, "ts": t0() }).to_string());
    assert!(!h.sockets[s].closed_by_client);
    assert!(h.frames.is_empty());
    assert_eq!(h.status(), Status::Open);
}

/// a known event with the wrong data still ends the connection
#[test]
fn a_known_event_with_the_wrong_data_still_ends_the_connection() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &json!({ "event": "ACK", "data": { "id": 5 }, "ts": t0() }).to_string());
    assert!(h.sockets[s].closed_by_client);
}

/// %s still ends the connection — an object with no event
#[test]
fn an_object_with_no_event_still_ends_the_connection() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, &json!({ "data": {}, "ts": t0() }).to_string());
    assert!(h.sockets[s].closed_by_client);
}

/// %s still ends the connection — a JSON array
#[test]
fn a_json_array_still_ends_the_connection() {
    let mut h = harness();
    let s = h.open_one();
    h.message(s, "[]");
    assert!(h.sockets[s].closed_by_client);
}

// --- the cursor and a replay that was one page -----------------------------------------------

/// a replay with more to come is reported as pending, with where to continue
#[test]
fn a_replay_with_more_to_come_is_reported_as_pending_with_where_to_continue() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    let page = id_at(2_000);
    h.message(s, &subscribed(&channel(1), Some(&page)));
    let pending: Vec<(String, String)> = h.conn.pending_continuations().into_iter().collect();
    assert_eq!(pending, vec![(channel(1), page)]);
}

/// a replay that reached the present is not pending
#[test]
fn a_replay_that_reached_the_present_is_not_pending() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    h.message(s, &subscribed(&channel(1), Some(&id_at(2_000))));
    h.message(s, &subscribed(&channel(1), None));
    assert!(h.conn.pending_continuations().is_empty());
}

/// while pending, a live frame beyond the page does not move the cursor
#[test]
fn while_pending_a_live_frame_beyond_the_page_does_not_move_the_cursor() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    let page = id_at(2_000);
    h.message(s, &subscribed(&channel(1), Some(&page)));
    h.message(s, &created(&channel(1), &id_at(1_000)));
    h.message(s, &created(&channel(1), &page));
    h.message(s, &created(&channel(1), &id_at(9_000))); // live, past the gap

    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert_eq!(channels_of(&h.first_subscribe(s))[0].1, Some(page));
}

/// the live frame is still delivered, only its place in the cursor is withheld
#[test]
fn the_live_frame_is_still_delivered_only_its_place_in_the_cursor_is_withheld() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    h.message(s, &subscribed(&channel(1), Some(&id_at(2_000))));
    h.message(s, &created(&channel(1), &id_at(9_000)));
    assert_eq!(h.frame_events().iter().filter(|e| **e == "MESSAGE_CREATED").count(), 1);
}

/// pages fetched over HTTP move the cursor, and finishing lets live frames count again
#[test]
fn pages_fetched_over_http_move_the_cursor_and_finishing_lets_live_frames_count_again() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    let page = id_at(2_000);
    h.message(s, &subscribed(&channel(1), Some(&page)));

    h.observe(&channel(1), &id_at(5_000)); // the continuation, reaching the present
    h.continuation_done(&channel(1), &page);
    assert!(h.conn.pending_continuations().is_empty());

    let live = id_at(9_000);
    h.message(s, &created(&channel(1), &live));
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert_eq!(channels_of(&h.first_subscribe(s))[0].1, Some(live));
}

/// finishing a continuation that is not the current one changes nothing
#[test]
fn finishing_a_continuation_that_is_not_the_current_one_changes_nothing() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    let page = id_at(2_000);
    h.message(s, &subscribed(&channel(1), Some(&page)));
    h.continuation_done(&channel(1), &id_at(1_000)); // an older replay's
    assert_eq!(h.conn.pending_continuations().get(&channel(1)), Some(&page));
}

/// a new connection starts from what its own replay says, not the last one
#[test]
fn a_new_connection_starts_from_what_its_own_replay_says_not_the_last_one() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    h.message(s, &subscribed(&channel(1), Some(&id_at(2_000))));
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert!(h.conn.pending_continuations().is_empty());

    let live = id_at(9_000);
    h.message(s, &subscribed(&channel(1), None));
    h.message(s, &created(&channel(1), &live));
    h.closed(s, 1006);
    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert_eq!(channels_of(&h.first_subscribe(s))[0].1, Some(live));
}

/// a channel no longer watched has nothing pending
#[test]
fn a_channel_no_longer_watched_has_nothing_pending() {
    let mut h = harness();
    h.watch(&channel(1));
    let s = h.open_one();
    h.message(s, &subscribed(&channel(1), Some(&id_at(2_000))));
    h.unwatch(&[channel(1)]);
    assert!(h.conn.pending_continuations().is_empty());
}

// --- pacing a large subscription -------------------------------------------------------------

fn chunks_of(total: usize) -> usize {
    total.div_ceil(MAX)
}

/// only one chunk is in flight; the next leaves when the last is acknowledged
#[test]
fn only_one_chunk_is_in_flight_the_next_leaves_when_the_last_is_acknowledged() {
    let mut h = harness();
    let total = MAX as u32 * 3 + 5;
    h.watch_many(total);
    let s = h.open_one();
    assert_eq!(h.subscribes(s).len(), 1);

    for sent in 1..=3 {
        let chunk = h.subscribes(s)[sent - 1].clone();
        h.message(s, &ack_for(&chunk));
        assert_eq!(h.subscribes(s).len(), sent + 1);
    }
    let sizes: Vec<usize> = h.subscribes(s).iter().map(|f| channels_of(f).len()).collect();
    assert_eq!(sizes, vec![MAX, MAX, MAX, 5]);
    let named: HashSet<String> = h.subscribes(s).iter().flat_map(|f| channels_of(f).into_iter().map(|c| c.0)).collect();
    assert_eq!(named.len(), total as usize);
}

/// a RATE_LIMITED answer is retried after a wait, with the same channels
#[test]
fn a_rate_limited_answer_is_retried_after_a_wait_with_the_same_channels() {
    let mut h = harness();
    h.watch_many(MAX as u32 + 1);
    let s = h.open_one();
    let first = h.first_subscribe(s);
    h.message(s, &limited(&id_of(&first)));
    assert_eq!(h.subscribes(s).len(), 1); // not at once

    let wait = h.retry_timer().expect("no retry scheduled");
    h.fire(wait.timer);
    let retry = h.subscribes(s)[1].clone();
    assert_eq!(retry["payload"]["channels"], first["payload"]["channels"]);
    assert_ne!(id_of(&retry), id_of(&first));
}

/// a database that is busy is retried the same way
#[test]
fn a_database_that_is_busy_is_retried_the_same_way() {
    let mut h = harness();
    h.watch_many(1);
    let s = h.open_one();
    let first = h.first_subscribe(s);
    h.message(s, &error_for(&id_of(&first), "DATABASE_UNAVAILABLE", "busy"));
    assert!(h.retry_timer().is_some());
}

/// a channel unwatched during the wait is not sent when the wait ends
#[test]
fn a_channel_unwatched_during_the_wait_is_not_sent_when_the_wait_ends() {
    let mut h = harness();
    h.watch_many(1);
    let s = h.open_one();
    let first = h.first_subscribe(s);
    h.message(s, &limited(&id_of(&first)));
    h.unwatch(&[channel(1)]);
    let wait = h.retry_timer().expect("no retry scheduled");
    h.fire(wait.timer);
    assert_eq!(h.subscribes(s).len(), 1);
}

/// an answer to something else does not release the chunk on the wire
#[test]
fn an_answer_to_something_else_does_not_release_the_chunk_on_the_wire() {
    let mut h = harness();
    h.watch_many(MAX as u32 + 1);
    let s = h.open_one();
    let first = h.first_subscribe(s);
    h.message(s, &ack_for_id(&first, &id_at(88_000)));
    assert_eq!(h.subscribes(s).len(), 1);
}

/// an acknowledgement starts the wait over
#[test]
fn an_acknowledgement_starts_the_wait_over() {
    let mut h = harness();
    h.watch_many(MAX as u32 + 1);
    let s = h.open_one();
    let first = h.first_subscribe(s);
    h.message(s, &limited(&id_of(&first)));
    let wait = h.retry_timer().expect("no retry scheduled");
    h.fire(wait.timer);
    let second = h.subscribes(s)[1].clone();
    h.message(s, &ack_for(&second));
    let third = h.subscribes(s)[2].clone();
    h.message(s, &limited(&id_of(&third)));
    assert_eq!(h.retry_timer().map(|t| t.ms), Some(BASE / 2));
}

/// the wait grows while the answer stays RATE_LIMITED
#[test]
fn the_wait_grows_while_the_answer_stays_rate_limited() {
    let mut h = harness();
    h.watch_many(1);
    let s = h.open_one();
    let mut seen = Vec::new();
    for _ in 0..3 {
        let chunk = h.subscribes(s).last().cloned().expect("chunk missing");
        h.message(s, &limited(&id_of(&chunk)));
        let timer = h.retry_timer().expect("no retry scheduled");
        seen.push(timer.ms);
        h.fire(timer.timer);
    }
    assert_eq!(seen, vec![BASE / 2, BASE, BASE * 2]);
}

/// another error for a chunk is not retried, and the rest still go
#[test]
fn another_error_for_a_chunk_is_not_retried_and_the_rest_still_go() {
    let mut h = harness();
    h.watch_many(MAX as u32 + 1);
    let s = h.open_one();
    let first = h.first_subscribe(s);
    h.message(s, &error_for(&id_of(&first), "WORKSPACE_NOT_FOUND", "no such channel"));
    assert_eq!(h.subscribes(s).len(), 2);
    assert_eq!(channels_of(&h.subscribes(s)[1]).len(), 1);
    // The refusal is told, with its chunk, code and id, before the ERROR frame is handed on.
    assert_eq!(h.refused.len(), 1);
    let (ids, code, ref_id) = &h.refused[0];
    assert_eq!(ids.len(), MAX);
    assert_eq!(*code, ErrorCode::WorkspaceNotFound);
    assert_eq!(*ref_id, id_of(&first));
}

/// a channel unwatched while it waits for its turn is not subscribed
#[test]
fn a_channel_unwatched_while_it_waits_for_its_turn_is_not_subscribed() {
    let mut h = harness();
    h.watch_many(MAX as u32 + 1);
    let s = h.open_one();
    h.unwatch(&[channel(MAX as u32 + 1)]);
    let first = h.first_subscribe(s);
    h.message(s, &ack_for(&first));
    assert_eq!(h.subscribes(s).len(), 1);
}

/// watching while a chunk is in flight waits behind it
#[test]
fn watching_while_a_chunk_is_in_flight_waits_behind_it() {
    let mut h = harness();
    h.watch_many(1);
    let s = h.open_one();
    h.watch(&channel(500));
    assert_eq!(h.subscribes(s).len(), 1);
    let first = h.first_subscribe(s);
    h.message(s, &ack_for(&first));
    assert_eq!(channels_of(&h.subscribes(s)[1]), vec![(channel(500), None)]);
}

/// a reconnect starts the whole subscription over, and a pending retry dies with the socket
#[test]
fn a_reconnect_starts_the_whole_subscription_over_and_a_pending_retry_dies_with_the_socket() {
    let mut h = harness();
    h.watch_many(MAX as u32 + 1);
    let socket = h.open_one();
    let first = h.first_subscribe(socket);
    h.message(socket, &limited(&id_of(&first)));
    h.closed(socket, 1006);
    assert_eq!(h.waits(), vec![BASE / 2]);

    h.fire_next();
    let s = h.last();
    h.opened(s);
    assert_eq!(h.subscribes(s).len(), 1);
    assert_eq!(channels_of(&h.subscribes(s)[0]).len(), MAX);
    assert_eq!(h.subscribes(socket).len(), 1);
}

/// TOO_BUSY waits longer than an ordinary drop before reconnecting
#[test]
fn too_busy_waits_longer_than_an_ordinary_drop_before_reconnecting() {
    let mut h = harness();
    let s = h.open_one();
    h.closed(s, close_codes::TOO_BUSY);
    assert_eq!(h.fire_next(), reconnect_delay(TOO_BUSY_BACKOFF_STEP as u32, 0.0));
    assert!(reconnect_delay(TOO_BUSY_BACKOFF_STEP as u32, 0.0) > BASE);
}

/// TOO_BUSY never shortens a wait the failures had already earned
#[test]
fn too_busy_never_shortens_a_wait_the_failures_had_already_earned() {
    let mut h = harness();
    h.start();
    for _ in 0..TOO_BUSY_BACKOFF_STEP + 2 {
        let s = h.last();
        h.closed(s, 1006);
        h.fire_next();
    }
    let s = h.last();
    h.closed(s, close_codes::TOO_BUSY);
    assert_eq!(h.fire_next(), reconnect_delay(TOO_BUSY_BACKOFF_STEP as u32 + 2, 0.0));
}

// --- against a server that limits and closes like the real one -------------------------------
//
// The real limits live in apps/api (`SOCKET_FRAME_BUDGET`, `FRAME_QUEUE_MAX`); the client must not
// import them, so this model states what it relies on: a burst of 20 that refills with time, and a
// close with 1013 when more than 32 frames are waiting to be handled at once.

const BURST: f64 = 20.0;
const QUEUE_MAX: usize = 32;
const REFILL_PER_SECOND: f64 = 10.0;

struct Server {
    acked: HashSet<String>,
    too_busy: u32,
    tokens: f64,
    handled: usize,
}

/// The server takes what has arrived: all of it at once, as the socket delivers it.
fn serve(h: &mut Harness, srv: &mut Server, s: usize) {
    let arrived: Vec<String> = h.sockets[s].sent[srv.handled..].to_vec();
    if arrived.len() > QUEUE_MAX {
        srv.too_busy += 1;
        srv.handled = h.sockets[s].sent.len();
        h.closed(s, close_codes::TOO_BUSY);
        return;
    }
    srv.handled = h.sockets[s].sent.len();
    for text in arrived {
        let frame: Value = serde_json::from_str(&text).unwrap();
        if frame["op"] != "SUBSCRIBE" {
            continue;
        }
        if srv.tokens < 1.0 {
            h.message(s, &limited(&id_of(&frame)));
        } else {
            srv.tokens -= 1.0;
            for (c, _) in channels_of(&frame) {
                srv.acked.insert(c);
            }
            h.message(s, &ack_for(&frame));
        }
    }
    if h.sockets[s].sent.len() > srv.handled {
        serve(h, srv, s);
    }
}

fn run_against_server(total: u32) -> (HashSet<String>, u32) {
    let mut h = harness();
    h.watch_many(total);
    let mut srv = Server { acked: HashSet::new(), too_busy: 0, tokens: BURST, handled: 0 };

    let mut socket = h.open_one();
    srv.tokens -= 1.0; // the PING sent as the socket opened
    for _ in 0..400 {
        if srv.acked.len() >= total as usize {
            break;
        }
        serve(&mut h, &mut srv, socket);
        if srv.acked.len() >= total as usize {
            break;
        }
        let Some(timer) = h.retry_timer() else { break };
        srv.tokens = BURST.min(srv.tokens + (timer.ms as f64 / 1000.0) * REFILL_PER_SECOND);
        let before = h.sockets.len();
        h.fire(timer.timer);
        if h.sockets.len() > before {
            socket = h.last();
            h.opened(socket);
            srv.handled = 0;
        }
    }
    (srv.acked, srv.too_busy)
}

/// 2,500 channels: every chunk is eventually acknowledged
#[test]
fn channels_2500_every_chunk_is_eventually_acknowledged() {
    let (acked, too_busy) = run_against_server(2_500);
    assert_eq!(chunks_of(2_500), 25);
    assert_eq!(acked.len(), 2_500);
    assert_eq!(too_busy, 0);
}

/// 3,500 channels: no close for being too busy, and every channel subscribed
#[test]
fn channels_3500_no_close_for_being_too_busy_and_every_channel_subscribed() {
    let (acked, too_busy) = run_against_server(3_500);
    assert_eq!(too_busy, 0);
    assert_eq!(acked.len(), 3_500);
}
