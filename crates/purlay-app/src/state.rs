//! The application model: which server, whether signed in, and the live session.
//!
//! Views observe [`AppModel`]; it re-notifies them whenever the engine's store moves.

use crate::i18n::{t, tf};
use gpui_kit::*;
use purlay_core::api::ApiError;
use purlay_core::auth::{self, credentials, CallbackError, ExchangeError, Handoff, StoredSession};
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::engine::Event as EngineEvent;
use purlay_core::handshake::{self, Handshake};
use purlay_core::prefs::{Prefs, ServerEntry};
use purlay_core::session::{Session, Started, APP_VERSION};
use std::path::PathBuf;

pub enum Phase {
    /// Choose a server.
    Connect { error: Option<String>, busy: bool },
    /// The server answered; sign in through the browser.
    SignIn { address: url::Url, info: ClientInfo, origin: url::Url, handoff: Option<Handoff>, error: Option<String>, busy: bool },
    /// Opening a stored session.
    Opening { address: String },
    Ready(Session),
}

/// What views are told once.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Notice(String),
    /// A `purlay://open?url=…` (or web permalink) to show.
    OpenLink(String),
    SessionEnded,
}

pub struct AppModel {
    pub rt: tokio::runtime::Handle,
    pub prefs: Prefs,
    pub data_dir: Option<PathBuf>,
    pub phase: Phase,
    pub window_active: bool,
    pending_links: Vec<String>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<AppEvent> for AppModel {}

pub struct AppGlobal(pub Entity<AppModel>);
impl Global for AppGlobal {}

pub fn app(cx: &App) -> Entity<AppModel> {
    cx.global::<AppGlobal>().0.clone()
}

impl AppModel {
    pub fn new(rt: tokio::runtime::Handle, prefs: Prefs, data_dir: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let mut m = AppModel { rt, prefs, data_dir, phase: Phase::Connect { error: None, busy: false }, window_active: true, pending_links: vec![], _tasks: vec![] };
        m.resume(cx);
        m
    }

    pub fn session(&self) -> Option<&Session> {
        match &self.phase {
            Phase::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// At start: reopen the last server's session if its credential is still kept.
    fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.prefs.get().current_server else { return };
        let Some(entry) = self.prefs.get().servers.into_iter().find(|s| s.address == current) else { return };
        let Some(stored) = credentials::load(&entry.origin).or_else(|| credentials::load(&entry.address)) else { return };
        let Ok(address) = url::Url::parse(&entry.address) else { return };
        self.phase = Phase::Opening { address: entry.address.clone() };
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            let a2 = address.clone();
            let hs = rt.spawn(async move { handshake::handshake(&a2, APP_VERSION).await }).await;
            match hs {
                Ok(Ok(Handshake::Ready { info, origin })) => {
                    let _ = this.update(cx, |m, cx| m.open_session(address, info, origin, stored, cx));
                }
                Ok(Ok(other)) => {
                    let msg = handshake_message(&other);
                    let _ = this.update(cx, |m, cx| {
                        m.phase = Phase::Connect { error: Some(msg), busy: false };
                        cx.notify();
                    });
                }
                _ => {
                    // Offline at start: still open the session; the engine reconnects when it can.
                    let _ = this.update(cx, |m, cx| {
                        m.phase = Phase::Connect { error: Some(t("connect.error.unreachable").into()), busy: false };
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn connect(&mut self, text: String, cx: &mut Context<Self>) {
        let address = match handshake::parse_server_address(&text, std::env::var("PURLAY_ALLOW_HTTP").is_ok()) {
            Ok(a) => a,
            Err(handshake::AddressError::Empty) => {
                self.phase = Phase::Connect { error: Some(t("connect.error.empty").into()), busy: false };
                cx.notify();
                return;
            }
            Err(_) => {
                self.phase = Phase::Connect { error: Some(t("connect.error.address").into()), busy: false };
                cx.notify();
                return;
            }
        };
        self.phase = Phase::Connect { error: None, busy: true };
        cx.notify();
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            let a2 = address.clone();
            let r = rt.spawn(async move { handshake::handshake(&a2, APP_VERSION).await }).await;
            let _ = this.update(cx, |m, cx| {
                m.phase = match r {
                    Ok(Ok(Handshake::Ready { info, origin })) => {
                        Phase::SignIn { address, info, origin, handoff: None, error: None, busy: false }
                    }
                    Ok(Ok(other)) => Phase::Connect { error: Some(handshake_message(&other)), busy: false },
                    Ok(Err(ApiError::Decode(_))) | Ok(Err(ApiError::Http { .. })) => {
                        Phase::Connect { error: Some(t("connect.error.notPurlay").into()), busy: false }
                    }
                    _ => Phase::Connect { error: Some(t("connect.error.unreachable").into()), busy: false },
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn back_to_connect(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Connect { error: None, busy: false };
        cx.notify();
    }

    /// Opens the browser at the server's native sign-in page.
    pub fn open_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Phase::SignIn { origin, handoff, error, .. } = &mut self.phase {
            let h = Handoff::new();
            let url = h.sign_in_url(origin);
            *handoff = Some(h);
            *error = None;
            cx.open_url(url.as_str());
            cx.notify();
        }
    }

    /// A `purlay:` URL from the OS (or pasted).
    pub fn handle_url(&mut self, raw: String, cx: &mut Context<Self>) {
        if auth::is_sign_in_callback(&raw) {
            self.finish_sign_in(raw, cx);
            return;
        }
        let target = parse_open_link(&raw).unwrap_or(raw);
        if self.session().is_some() {
            cx.emit(AppEvent::OpenLink(target));
        } else {
            self.pending_links.push(target);
        }
    }

    pub fn take_pending_links(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_links)
    }

    fn finish_sign_in(&mut self, raw: String, cx: &mut Context<Self>) {
        let Phase::SignIn { address, info, origin, handoff, error, busy } = &mut self.phase else { return };
        let Some(h) = handoff.clone() else {
            *error = Some(t("signin.error.state").into());
            cx.notify();
            return;
        };
        let code = match h.read_callback(&raw) {
            Ok(c) => c,
            Err(CallbackError::StateMismatch) => {
                *error = Some(t("signin.error.state").into());
                cx.notify();
                return;
            }
            Err(e) => {
                *error = Some(tf("signin.error.generic", &[&e]));
                cx.notify();
                return;
            }
        };
        *busy = true;
        cx.notify();
        let (address, info, origin) = (address.clone(), info.clone(), origin.clone());
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            let o2 = origin.clone();
            let r = rt
                .spawn(async move {
                    let api = purlay_core::api::ApiClient::new(o2, APP_VERSION);
                    let now = purlay_core::clock::Clock::now_ms(&purlay_core::clock::SystemClock);
                    auth::exchange(&api, &h, &code, now).await
                })
                .await;
            let _ = this.update(cx, |m, cx| match r {
                Ok(Ok(stored)) => {
                    if let Err(e) = credentials::save(&stored) {
                        tracing::warn!("credential store: {e}");
                    }
                    m.open_session(address, info, origin, stored, cx);
                }
                Ok(Err(e)) => {
                    let msg = match e {
                        ExchangeError::CodeNotFound => t("signin.error.code").to_string(),
                        ExchangeError::RateLimited { .. } => t("signin.error.rate").to_string(),
                        other => tf("signin.error.generic", &[&other]),
                    };
                    if let Phase::SignIn { error, busy, handoff, .. } = &mut m.phase {
                        *error = Some(msg);
                        *busy = false;
                        *handoff = None;
                    }
                    cx.notify();
                }
                Err(e) => {
                    if let Phase::SignIn { error, busy, .. } = &mut m.phase {
                        *error = Some(tf("signin.error.generic", &[&e]));
                        *busy = false;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_session(&mut self, address: url::Url, info: ClientInfo, origin: url::Url, stored: StoredSession, cx: &mut Context<Self>) {
        let addr_text = handshake::origin_text(&address);
        self.phase = Phase::Opening { address: addr_text.clone() };
        cx.notify();
        let rt = self.rt.clone();
        let prefs = self.prefs.clone();
        let data_dir = self.data_dir.clone();
        cx.spawn(async move |this, cx| {
            let rt2 = rt.clone();
            let o2 = origin.clone();
            let i2 = info.clone();
            let r = rt.spawn(async move { Session::open(rt2, i2, o2, stored, prefs, data_dir).await }).await;
            let _ = this.update(cx, |m, cx| match r {
                Ok(Ok(started)) => m.ready(addr_text, started, cx),
                Ok(Err(e)) if e.is_unauthenticated() => {
                    credentials::delete(&handshake::origin_text(&origin));
                    m.phase = Phase::SignIn { address: url::Url::parse(&addr_text).unwrap_or(origin.clone()), info, origin, handoff: None, error: Some(t("session.ended").into()), busy: false };
                    cx.notify();
                }
                Ok(Err(e)) => {
                    m.phase = Phase::Connect { error: Some(tf("error.action", &[&e])), busy: false };
                    cx.notify();
                }
                Err(e) => {
                    m.phase = Phase::Connect { error: Some(tf("error.action", &[&e])), busy: false };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn ready(&mut self, address: String, started: Started, cx: &mut Context<Self>) {
        let Started { session, mut events } = started;
        let origin = session.server_key();
        let user_id = session.user_id.to_string();
        let username = session.store().read_recursive().me.as_ref().map(|m| m.user.username.clone());
        self.prefs.update(|d| {
            d.current_server = Some(address.clone());
            if let Some(e) = d.servers.iter_mut().find(|s| s.address == address) {
                e.origin = origin.clone();
                e.user_id = Some(user_id.clone());
                e.username = username.clone();
            } else {
                d.servers.push(ServerEntry { address: address.clone(), origin: origin.clone(), user_id: Some(user_id.clone()), username, last_workspace: None });
            }
        });
        // Redraw whenever the store moves.
        let mut revisions = session.engine.revisions.clone();
        let t1 = cx.spawn(async move |this, cx| {
            while revisions.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let t2 = cx.spawn(async move |this, cx| {
            while let Some(e) = events.recv().await {
                if this.update(cx, |m, cx| m.on_engine_event(e, cx)).is_err() {
                    break;
                }
            }
        });
        self._tasks = vec![t1, t2];
        self.phase = Phase::Ready(session);
        for l in self.take_pending_links() {
            cx.emit(AppEvent::OpenLink(l));
        }
        cx.notify();
    }

    fn on_engine_event(&mut self, e: EngineEvent, cx: &mut Context<Self>) {
        match e {
            EngineEvent::SessionEnded(_) => {
                if let Phase::Ready(s) = &self.phase {
                    credentials::delete(&s.server_key());
                    s.shutdown();
                    let info = s.info.clone();
                    let origin = s.origin.clone();
                    self.phase = Phase::SignIn { address: origin.clone(), info, origin, handoff: None, error: Some(t("session.ended").into()), busy: false };
                    self._tasks.clear();
                    cx.emit(AppEvent::SessionEnded);
                    cx.notify();
                }
            }
            EngineEvent::LostUnsent(n) => cx.emit(AppEvent::Notice(tf("conn.lost", &[&n]))),
            EngineEvent::Arrived(m) => self.maybe_notify(&m),
            EngineEvent::ChannelGone(..) => {}
            EngineEvent::LoadFailed(what, why) => tracing::debug!("load failed {what}: {why}"),
        }
    }

    /// A desktop notification for what the badge counts, only while the window is not in front.
    fn maybe_notify(&self, m: &Message) {
        let Some(s) = self.session() else { return };
        if self.window_active || !self.prefs.get().notifications_enabled {
            return;
        }
        let store = s.store().read_recursive();
        let Some(ch) = store.channel(&m.channel_id) else { return };
        let ws = ch.workspace_id.clone();
        if let Some(mem) = store.membership(&ws) {
            if let Some(until) = mem.deep_work_until.as_ref().and_then(|u| u.unix_ms()) {
                if until > purlay_core::clock::Clock::now_ms(&purlay_core::clock::SystemClock) {
                    return;
                }
            }
        }
        let me = &s.user_id;
        let mentioned = m.mentions.iter().any(|u| u == me);
        let dm = ch.r#type == ChannelType::DirectMessage;
        let level = ch.notification_level.clone().unwrap_or(NotificationLevel::Mentions);
        let wanted = match level {
            NotificationLevel::None => false,
            NotificationLevel::All => true,
            _ => dm || mentioned,
        };
        if !wanted {
            return;
        }
        let sender = m.sender.as_ref().map(|u| u.display_name.clone()).unwrap_or_default();
        let title = if dm {
            sender.clone()
        } else {
            tf("notify.mention", &[&sender, &format!("#{}", ch.name.clone().unwrap_or_default())])
        };
        let body = purlay_core::markdown::excerpt_of(&m.content);
        let link = format!("purlay://open?channel={}&message={}", m.channel_id, m.id);
        let prefs = self.prefs.get();
        drop(store);
        // Purlay's own tone plays beside a silent toast; "Windows default" lets the toast sound.
        let system = prefs.notification_sound && prefs.notification_tone == purlay_core::prefs::NotificationTone::System;
        purlay_platform::toast(&title, &body, Some(&link), !system);
        if prefs.notification_sound && !system {
            play_tone(prefs.notification_tone);
        }
    }

    pub fn sign_out(&mut self, everywhere: bool, cx: &mut Context<Self>) {
        if let Phase::Ready(s) = &self.phase {
            let h = s.sign_out(everywhere);
            let rt = self.rt.clone();
            self._tasks.clear();
            self.prefs.update(|d| d.current_server = None);
            self.phase = Phase::Connect { error: None, busy: false };
            cx.notify();
            rt.spawn(async move {
                let _ = h.await;
            });
        }
    }

    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.window_active != active {
            self.window_active = active;
            cx.notify();
        }
    }
}

fn handshake_message(h: &Handshake) -> String {
    match h {
        Handshake::ServerTooOld { version } => tf("connect.error.serverOld", &[version]),
        Handshake::AppTooOld { .. } => t("connect.error.appOld").into(),
        Handshake::UnusableOrigin => t("connect.error.origin").into(),
        Handshake::Ready { .. } => String::new(),
    }
}

/// `purlay://open?url=<permalink>` → the permalink; `purlay://open?channel=…&message=…` kept.
fn parse_open_link(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw).ok()?;
    if u.scheme() != "purlay" || u.host_str() != Some("open") {
        return None;
    }
    u.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned()).or(Some(raw.to_string()))
}

/// What a link points at: a channel and maybe a message.
pub fn link_target(raw: &str) -> Option<(Option<String>, Uuid, Option<Uuid>)> {
    let u = url::Url::parse(raw).ok()?;
    let q = |k: &str| u.query_pairs().find(|(kk, _)| kk == k).map(|(_, v)| v.into_owned());
    if u.scheme() == "purlay" {
        let ch = q("channel")?;
        return Some((None, Uuid::new(ch), q("message").map(Uuid::new)));
    }
    // <origin>/w/<slug>/c/<channelId>?message=<id>
    let segs: Vec<&str> = u.path_segments()?.collect();
    let w = segs.iter().position(|s| *s == "w").and_then(|i| segs.get(i + 1)).map(|s| s.to_string());
    let c = segs.iter().position(|s| *s == "c").and_then(|i| segs.get(i + 1))?;
    Some((w, Uuid::new(*c), q("message").map(Uuid::new)))
}

/// Plays a notification tone now: one of Purlay's, or Windows' own.
pub fn play_tone(tone: purlay_core::prefs::NotificationTone) {
    use purlay_core::prefs::NotificationTone as N;
    use purlay_platform::sound::{self, Tone};
    match tone {
        N::Drop => sound::play(Tone::Drop),
        N::Chime => sound::play(Tone::Chime),
        N::Marimba => sound::play(Tone::Marimba),
        N::Glass => sound::play(Tone::Glass),
        N::System => sound::play_system(),
    }
}
