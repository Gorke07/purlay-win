//! The IO half of the realtime client: a real socket per generation, and timers.
//!
//! The upgrade carries `Cookie`, `Origin` (the handshake's `appUrl`) and `X-Purlay-Client`;
//! no token ever goes in the URL. Text frames only; incoming messages are capped at 4 MB.

use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::hash::Hash;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, protocol::WebSocketConfig, Message};

#[derive(Debug)]
pub enum SocketEvent {
    Opened,
    Text(String),
    Binary,
    /// The socket ended; the close code (1006 when there was none).
    Closed(u16),
    /// The upgrade was answered with this HTTP status.
    Refused(u16),
    /// The socket could not be created or connected.
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct SocketTarget {
    /// `wss://host/ws`
    pub url: String,
    pub cookie: Option<String>,
    pub origin: String,
    pub client_header: String,
}

enum SocketCmd {
    Send(String),
    Close,
}

/// The one socket in use, tagged with the generation it was opened for.
pub struct Sockets {
    current: Option<(u64, mpsc::UnboundedSender<SocketCmd>)>,
    events: mpsc::UnboundedSender<(u64, SocketEvent)>,
}

impl Sockets {
    pub fn new(events: mpsc::UnboundedSender<(u64, SocketEvent)>) -> Self {
        Sockets { current: None, events }
    }

    pub fn open(&mut self, generation: u64, target: SocketTarget) {
        self.close();
        let (tx, rx) = mpsc::unbounded_channel();
        self.current = Some((generation, tx));
        let events = self.events.clone();
        tokio::spawn(run_socket(generation, target, rx, events));
    }

    pub fn send(&self, text: String) {
        if let Some((_, tx)) = &self.current {
            let _ = tx.send(SocketCmd::Send(text));
        }
    }

    pub fn close(&mut self) {
        if let Some((_, tx)) = self.current.take() {
            let _ = tx.send(SocketCmd::Close);
        }
    }
}

async fn run_socket(
    generation: u64,
    target: SocketTarget,
    mut cmds: mpsc::UnboundedReceiver<SocketCmd>,
    events: mpsc::UnboundedSender<(u64, SocketEvent)>,
) {
    let emit = |e: SocketEvent| {
        let _ = events.send((generation, e));
    };
    let mut req = match target.url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => return emit(SocketEvent::Failed(e.to_string())),
    };
    {
        let h = req.headers_mut();
        if let Some(c) = &target.cookie {
            if let Ok(v) = c.parse() {
                h.insert("Cookie", v);
            }
        }
        if let Ok(v) = target.origin.parse() {
            h.insert("Origin", v);
        }
        if let Ok(v) = target.client_header.parse() {
            h.insert("X-Purlay-Client", v);
        }
    }
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(4 * 1024 * 1024);
    config.max_frame_size = Some(4 * 1024 * 1024);

    let connect = tokio_tungstenite::connect_async_with_config(req, Some(config), false);
    let ws = tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(15), connect) => r,
        _ = wait_close(&mut cmds) => return emit(SocketEvent::Closed(1000)),
    };
    let ws = match ws {
        Err(_) => return emit(SocketEvent::Failed("upgrade timed out".into())),
        Ok(Err(tungstenite::Error::Http(resp))) => return emit(SocketEvent::Refused(resp.status().as_u16())),
        Ok(Err(e)) => return emit(SocketEvent::Failed(e.to_string())),
        Ok(Ok((ws, _resp))) => ws,
    };
    emit(SocketEvent::Opened);
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(SocketCmd::Send(text)) => {
                    if sink.send(Message::text(text)).await.is_err() {
                        return emit(SocketEvent::Closed(1006));
                    }
                }
                Some(SocketCmd::Close) | None => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
                    return emit(SocketEvent::Closed(1000));
                }
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => emit(SocketEvent::Text(t.to_string())),
                Some(Ok(Message::Binary(_))) => emit(SocketEvent::Binary),
                Some(Ok(Message::Close(frame))) => {
                    let code = frame.map(|f| u16::from(f.code)).unwrap_or(1005);
                    return emit(SocketEvent::Closed(code));
                }
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return emit(SocketEvent::Closed(1006)),
            },
        }
    }
}

async fn wait_close(cmds: &mut mpsc::UnboundedReceiver<SocketCmd>) {
    loop {
        match cmds.recv().await {
            Some(SocketCmd::Close) | None => return,
            Some(SocketCmd::Send(_)) => {}
        }
    }
}

/// `https://host` → `wss://host/ws`
pub fn socket_url(origin: &url::Url) -> String {
    let scheme = if origin.scheme() == "http" { "ws" } else { "wss" };
    let host = origin.host_str().unwrap_or_default();
    match origin.port() {
        Some(p) => format!("{scheme}://{host}:{p}/ws"),
        None => format!("{scheme}://{host}/ws"),
    }
}

/// Named one-shot timers; scheduling a key again replaces it.
pub struct Timers<K: Eq + Hash + Clone> {
    due: HashMap<K, Instant>,
}

impl<K: Eq + Hash + Clone> Default for Timers<K> {
    fn default() -> Self {
        Timers { due: HashMap::new() }
    }
}

impl<K: Eq + Hash + Clone> Timers<K> {
    pub fn schedule(&mut self, key: K, ms: u64) {
        self.due.insert(key, Instant::now() + Duration::from_millis(ms));
    }

    pub fn cancel(&mut self, key: &K) {
        self.due.remove(key);
    }

    pub fn is_scheduled(&self, key: &K) -> bool {
        self.due.contains_key(key)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.due.values().min().copied()
    }

    /// Removes and returns every timer that is due.
    pub fn take_due(&mut self) -> Vec<K> {
        let now = Instant::now();
        let due: Vec<K> = self.due.iter().filter(|(_, at)| **at <= now).map(|(k, _)| k.clone()).collect();
        for k in &due {
            self.due.remove(k);
        }
        due
    }

    pub fn clear_where(&mut self, f: impl Fn(&K) -> bool) {
        self.due.retain(|k, _| !f(k));
    }
}

/// Sleeps until the next deadline, or forever when there is none.
pub async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => futures::future::pending::<()>().await,
    }
}
