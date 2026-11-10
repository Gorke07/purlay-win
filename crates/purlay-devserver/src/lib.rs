//! An in-memory fake of the Purlay server (contract 0.6.1), for driving the Windows client
//! end-to-end and for integration tests of its engine.
//!
//! Every JSON body and socket frame is one of `purlay_contract`'s generated types, so the
//! shapes are the contract's. Nothing is stored on disk and nothing outside the process is
//! touched.
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! let (addr, server) = purlay_devserver::spawn(purlay_devserver::Config::default()).await?;
//! let cookie = server.issue_session("ada"); // the value of `__Host-purlay_session`
//! println!("{addr} {}", server.origin());
//! # let _ = cookie; Ok(()) }
//! ```

pub mod error;
pub mod http;
pub mod ids;
pub mod live;
pub mod media;
pub mod routes;
pub mod seed;
pub mod signin;
pub mod world;
pub mod ws;

use http::{App, Shared};
use parking_lot::Mutex;
use purlay_contract::Uuid;
use purlay_contract::models::{DeviceSessionKind, User};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use world::World;

pub use world::{SESSION_COOKIE, SERVER_VERSION};

#[derive(Debug, Clone)]
pub struct Config {
    /// Where to listen; `127.0.0.1` by default.
    pub host: IpAddr,
    /// `0` picks a free port.
    pub port: u16,
    /// Seed the demo workspace (people, channels, messages). Without it the server has the
    /// people and channels but no messages.
    pub seed_messages: bool,
    /// The bot posts into #random now and then.
    pub live: bool,
    /// About how often the bot acts in live mode.
    pub live_every: Duration,
    /// The origin to advertise as `appUrl`; by default `http://<host>:<port>` (with
    /// `127.0.0.1` for an unspecified host).
    pub public_origin: Option<String>,
    /// The native sign-in page signs straight in as this username (for scripted runs).
    pub auto_sign_in: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            seed_messages: true,
            live: false,
            live_every: Duration::from_secs(20),
            public_origin: None,
            auto_sign_in: None,
        }
    }
}

/// A running server. Dropping it stops the server.
pub struct ServerHandle {
    addr: SocketAddr,
    app: Shared,
    shutdown: Option<oneshot::Sender<()>>,
    tasks: Vec<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port>`: what `GET /api/client` says as `appUrl`, and the `Origin`
    /// the socket upgrade must carry.
    pub fn origin(&self) -> &str {
        &self.app.origin
    }

    /// `ws://127.0.0.1:<port>/ws`
    pub fn socket_url(&self) -> String {
        format!("{}/ws", self.app.origin.replacen("http", "ws", 1))
    }

    /// Signs a person in (by username, e.g. `"ada"`, or by user id) and answers the session
    /// cookie's value — what the app keeps after `POST /api/native/token`.
    ///
    /// Panics on somebody who is not seeded.
    pub fn issue_session(&self, user: &str) -> String {
        let mut w = self.app.world.lock();
        let id = w
            .person_by_username(user)
            .or_else(|| w.person(user))
            .map(|p| p.id.clone())
            .unwrap_or_else(|| panic!("no seeded person {user:?}"));
        w.create_session(&id, DeviceSessionKind::Native, Some("Purlay for Windows/test (integration test)".into()))
    }

    /// `__Host-purlay_session=<value>`, ready for a `Cookie` header.
    pub fn cookie_header(&self, user: &str) -> String {
        format!("{SESSION_COOKIE}={}", self.issue_session(user))
    }

    pub fn user_id(&self, username: &str) -> Option<Uuid> {
        self.app.world.lock().person_by_username(username).map(|p| p.id.clone())
    }

    pub fn workspace_id(&self) -> Uuid {
        self.app.world.lock().workspace.id.clone()
    }

    /// A channel's id by its name (`"general"`).
    pub fn channel_id(&self, name: &str) -> Option<Uuid> {
        self.app.world.lock().channel_by_name(name).map(|c| c.id.clone())
    }

    /// The seeded people.
    pub fn people(&self) -> Vec<User> {
        let w = self.app.world.lock();
        w.people.keys().map(|id| w.user_view(id)).collect()
    }

    /// Direct access to the state, for tests that need to arrange or inspect something.
    /// Frames published from inside reach the open sockets.
    pub fn with_world<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.app.world.lock())
    }

    /// Closes every socket (`RESTARTING`) and stops serving.
    pub async fn shutdown(mut self) {
        self.close_sockets();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let tasks = std::mem::take(&mut self.tasks);
        if let Some((server, rest)) = tasks.split_first() {
            for t in rest {
                t.abort();
            }
            let _ = tokio::time::timeout(Duration::from_secs(3), async {
                while !server.is_finished() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            server.abort();
        }
    }

    /// Drops every open socket with `RESTARTING` and keeps serving, as a restarted node
    /// would look to its clients: they reconnect and resubscribe.
    pub fn drop_sockets(&self) {
        self.close_sockets();
    }

    fn close_sockets(&self) {
        let mut w = self.app.world.lock();
        let ids: Vec<u64> = w.sockets.keys().copied().collect();
        for id in ids {
            if let Some(s) = w.sockets.get(&id) {
                let _ = s.tx.send(world::Out::Close(purlay_contract::constants::close_codes::RESTARTING, "restarting".into()));
            }
            w.unregister_socket(id);
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Starts a server on its own tasks and answers where it listens.
pub async fn spawn(config: Config) -> std::io::Result<(SocketAddr, ServerHandle)> {
    let listener = TcpListener::bind((config.host, config.port)).await?;
    let addr = listener.local_addr()?;
    let origin = config.public_origin.clone().map(|o| o.trim_end_matches('/').to_string()).unwrap_or_else(|| {
        let host = if addr.ip().is_unspecified() { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { addr.ip() };
        format!("http://{}", SocketAddr::new(host, addr.port()))
    });

    let mut world = World::new(origin.clone());
    seed::seed(&mut world);
    if !config.seed_messages {
        strip_messages(&mut world);
    }
    if config.live
        && let Some(bot) = world.bot.clone()
            && let Some(p) = world.people.get_mut(&bot) {
                p.always_online = true;
            }
    let app: Shared = Arc::new(App { world: Mutex::new(world), origin, auto_sign_in: config.auto_sign_in.clone() });

    let (tx, rx) = oneshot::channel::<()>();
    let router = http::router(app.clone());
    let server = tokio::spawn(async move {
        let serve = axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = rx.await;
        });
        if let Err(e) = serve.await {
            tracing::error!("server stopped: {e}");
        }
    });
    let mut tasks = vec![server];

    // Scheduled messages and reminders.
    let due_app = app.clone();
    tasks.push(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            due_app.world.lock().run_due();
        }
    }));
    if config.live {
        tasks.push(tokio::spawn(live::run(app.clone(), config.live_every)));
    }

    Ok((addr, ServerHandle { addr, app, shutdown: Some(tx), tasks }))
}

/// Leaves the people and channels, drops the talk.
fn strip_messages(w: &mut World) {
    w.messages.clear();
    w.notes.clear();
    w.saved.clear();
    w.uploads.clear();
    w.inbox.clear();
    for c in w.channels.values_mut() {
        c.roots.clear();
        c.pins.clear();
        c.last_message_ms = None;
        for m in c.members.values_mut() {
            m.last_read = None;
        }
    }
}
