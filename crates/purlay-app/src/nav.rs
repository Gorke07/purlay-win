//! Where the person is: the workspace, the page, and the one inspector beside it.
//! Views ask `Nav` to move; it tells the engine what to read and keep live.

use crate::state::AppModel;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::session::Session;
use purlay_core::store::Key;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Channel(Uuid),
    Inbox,
    Search,
    Saved,
    OpenTopics,
    Issues,
    Drafts,
    Scheduled,
    Reminders,
    Settings,
    Home,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inspector {
    Thread { root: Uuid, channel: Uuid },
    Files(Uuid),
    Pins(Uuid),
    Bookmarks(Uuid),
    Members(Uuid),
}

#[derive(Debug, Clone)]
pub enum NavEvent {
    /// Scroll the open channel to this message and highlight it.
    Reveal(Uuid),
    FocusComposer,
    /// Put this message in the composer of the feed (or the thread) to edit it.
    Edit { message: Box<Message>, in_thread: bool },
}

pub struct Nav {
    pub app: Entity<AppModel>,
    pub workspace: Option<Uuid>,
    pub route: Route,
    pub inspector: Option<Inspector>,
    pub palette: bool,
    pub shortcuts: bool,
    /// The message a permalink asked for, highlighted until the next move.
    pub highlight: Option<Uuid>,
    view_keys: HashMap<&'static str, Vec<Key>>,
    sent_keys: BTreeSet<Key>,
    back: Vec<Route>,
    /// A conversation window's own navigation: it leaves the workspace choice and the
    /// resources kept live to the main window's.
    passive: bool,
}

impl EventEmitter<NavEvent> for Nav {}

impl Nav {
    pub fn new(app: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        let mut n = Nav {
            app,
            workspace: None,
            route: Route::Home,
            inspector: None,
            palette: false,
            shortcuts: false,
            highlight: None,
            view_keys: HashMap::new(),
            sent_keys: BTreeSet::new(),
            back: vec![],
            passive: false,
        };
        n.pick_workspace(cx);
        n
    }

    /// Navigation for a conversation window, showing `channel`.
    pub fn passive(app: Entity<AppModel>, channel: &Uuid, cx: &mut Context<Self>) -> Self {
        let s = app.read(cx).session().cloned();
        let workspace = s.and_then(|s| s.store().read_recursive().workspace_of_channel(channel));
        Nav {
            app,
            workspace,
            route: Route::Channel(channel.clone()),
            inspector: None,
            palette: false,
            shortcuts: false,
            highlight: None,
            view_keys: HashMap::new(),
            sent_keys: BTreeSet::new(),
            back: vec![],
            passive: true,
        }
    }

    pub fn session(&self, cx: &App) -> Option<Session> {
        self.app.read(cx).session().cloned()
    }

    /// The last workspace used on this server, or the first.
    pub fn pick_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.session(cx) else { return };
        let me = s.store().read_recursive().me.clone();
        let Some(me) = me else { return };
        let prefs = s.prefs.get();
        let last = prefs.servers.iter().find(|e| Some(&e.address) == prefs.current_server.as_ref()).and_then(|e| e.last_workspace.clone());
        let ws = me
            .memberships
            .iter()
            .find(|m| Some(m.workspace.id.as_str()) == last.as_deref())
            .or_else(|| me.memberships.first())
            .map(|m| m.workspace.id.clone());
        if ws != self.workspace {
            self.set_workspace(ws, cx);
        }
    }

    pub fn set_workspace(&mut self, ws: Option<Uuid>, cx: &mut Context<Self>) {
        self.workspace = ws.clone();
        self.route = Route::Home;
        self.inspector = None;
        if let (Some(s), Some(w)) = (self.session(cx), ws.as_ref()) {
            let current = s.prefs.get().current_server.clone();
            let wid = w.to_string();
            s.prefs.update(|d| {
                if let Some(e) = d.servers.iter_mut().find(|e| Some(&e.address) == current.as_ref()) {
                    e.last_workspace = Some(wid);
                }
            });
            s.active(w);
        }
        self.sync_keys(cx);
        cx.notify();
    }

    pub fn has_module(&self, module: &str, cx: &App) -> bool {
        let Some(s) = self.session(cx) else { return false };
        let store = s.store().read_recursive();
        let Some(me) = &store.me else { return false };
        let server = me.modules.iter().any(|m| m.as_str() == module);
        let ws = self
            .workspace
            .as_ref()
            .and_then(|w| me.memberships.iter().find(|m| m.workspace.id == *w))
            .and_then(|m| m.modules.as_ref())
            .map(|ms| ms.iter().any(|m| m.as_str() == module));
        server && ws.unwrap_or(true)
    }

    pub fn go(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route != route {
            self.back.push(self.route.clone());
            if self.back.len() > 50 {
                self.back.remove(0);
            }
        }
        self.highlight = None;
        self.route = route;
        // The thread inspector belongs to its channel.
        if let (Some(Inspector::Thread { channel, .. }), Route::Channel(c)) = (&self.inspector, &self.route) {
            if channel != c {
                self.inspector = None;
            }
        }
        if !matches!(self.route, Route::Channel(_)) {
            self.inspector = None;
        }
        self.sync_keys(cx);
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.back.pop() {
            self.route = r;
            self.inspector = None;
            self.sync_keys(cx);
            cx.notify();
        }
    }

    pub fn open_channel(&mut self, channel: &Uuid, cx: &mut Context<Self>) {
        let Some(s) = self.session(cx) else { return };
        // A channel in another workspace switches to it.
        if let Some(w) = s.store().read_recursive().workspace_of_channel(channel) {
            if Some(&w) != self.workspace.as_ref() {
                self.workspace = Some(w);
            }
        }
        s.open_channel(channel);
        self.go(Route::Channel(channel.clone()), cx);
        cx.emit(NavEvent::FocusComposer);
    }

    /// A permalink: the channel opened around the message.
    pub fn open_message(&mut self, channel: &Uuid, message: &Uuid, cx: &mut Context<Self>) {
        let Some(s) = self.session(cx) else { return };
        let loaded = s.store().read_recursive().windows.get(channel).is_some_and(|w| w.get(message).is_some());
        if !loaded {
            s.open_around(channel, message);
        }
        self.go(Route::Channel(channel.clone()), cx);
        self.highlight = Some(message.clone());
        cx.emit(NavEvent::Reveal(message.clone()));
        cx.notify();
    }

    pub fn open_thread(&mut self, root: &Uuid, channel: &Uuid, cx: &mut Context<Self>) {
        let Some(s) = self.session(cx) else { return };
        if self.route != Route::Channel(channel.clone()) {
            s.open_channel(channel);
            self.route = Route::Channel(channel.clone());
        }
        s.open_thread(root, channel);
        self.inspector = Some(Inspector::Thread { root: root.clone(), channel: channel.clone() });
        self.sync_keys(cx);
        cx.notify();
    }

    pub fn toggle_inspector(&mut self, i: Inspector, cx: &mut Context<Self>) {
        if self.inspector.as_ref() == Some(&i) {
            self.inspector = None;
        } else {
            self.inspector = Some(i);
        }
        self.sync_keys(cx);
        cx.notify();
    }

    pub fn close_inspector(&mut self, cx: &mut Context<Self>) {
        if self.inspector.take().is_some() {
            self.sync_keys(cx);
            cx.notify();
        }
    }

    /// A view says which resources it shows; the engine keeps the union fresh.
    pub fn set_keys(&mut self, view: &'static str, keys: Vec<Key>, cx: &mut Context<Self>) {
        if self.view_keys.get(view) == Some(&keys) {
            return;
        }
        self.view_keys.insert(view, keys);
        self.sync_keys(cx);
    }

    fn sync_keys(&mut self, cx: &mut Context<Self>) {
        if self.passive {
            return;
        }
        let Some(s) = self.session(cx) else { return };
        let mut keys: BTreeSet<Key> = BTreeSet::new();
        if let Some(w) = &self.workspace {
            keys.insert(Key::Members(w.clone()));
            keys.insert(Key::Presence(w.clone()));
            keys.insert(Key::ChannelGroups(w.clone()));
            keys.insert(Key::SavedIds(w.clone()));
            // Unread inbox notices: the count beside Inbox and on the toolbar.
            keys.insert(Key::Notifications(w.clone(), String::new(), true));
        }
        match &self.inspector {
            Some(Inspector::Files(c)) => {
                keys.insert(Key::Files(c.clone()));
            }
            Some(Inspector::Pins(c)) => {
                keys.insert(Key::Pins(c.clone()));
            }
            Some(Inspector::Bookmarks(c)) => {
                keys.insert(Key::Bookmarks(c.clone()));
            }
            Some(Inspector::Members(c)) => {
                keys.insert(Key::ChannelMembers(c.clone()));
            }
            Some(Inspector::Thread { root, .. }) => {
                keys.insert(Key::ThreadFollow(root.clone()));
            }
            None => {}
        }
        for v in self.view_keys.values() {
            keys.extend(v.iter().cloned());
        }
        if keys != self.sent_keys {
            self.sent_keys = keys.clone();
            s.set_active(keys.into_iter().collect());
        }
    }

    pub fn current_channel(&self) -> Option<&Uuid> {
        match &self.route {
            Route::Channel(c) => Some(c),
            _ => None,
        }
    }

    /// Channels of the current workspace in sidebar order, for Alt+↑/↓ and the palette.
    pub fn ordered_channels(&self, cx: &App) -> Vec<Channel> {
        let Some(s) = self.session(cx) else { return vec![] };
        let store = s.store().read_recursive();
        let Some(w) = &self.workspace else { return vec![] };
        let Some(map) = store.channels.get(w) else { return vec![] };
        let mut list: Vec<Channel> = map.values().filter(|c| c.state.is_some() || c.r#type == ChannelType::DirectMessage).cloned().collect();
        list.sort_by_key(|c| (!c.starred, c.r#type == ChannelType::DirectMessage, crate::sidebar::channel_title(c, &s.user_id).to_lowercase()));
        list
    }

    pub fn step_unread(&mut self, forward: bool, cx: &mut Context<Self>) {
        let list = self.ordered_channels(cx);
        let unread: Vec<&Channel> = list.iter().filter(|c| c.state.as_ref().is_some_and(|s| s.unread_count > 0)).collect();
        if unread.is_empty() {
            return;
        }
        let cur = self.current_channel().cloned();
        let pos = cur.as_ref().and_then(|c| list.iter().position(|x| x.id == *c)).unwrap_or(0);
        let pick = if forward {
            unread.iter().find(|c| list.iter().position(|x| x.id == c.id).unwrap_or(0) > pos).or(unread.first())
        } else {
            unread.iter().rev().find(|c| list.iter().position(|x| x.id == c.id).unwrap_or(0) < pos).or(unread.last())
        };
        if let Some(c) = pick.map(|c| c.id.clone()) {
            self.open_channel(&c, cx);
        }
    }

    /// Alt+↑/↓: the channel above or below in the sidebar, wrapping around.
    pub fn step_channel(&mut self, forward: bool, cx: &mut Context<Self>) {
        let list = self.ordered_channels(cx);
        if list.is_empty() {
            return;
        }
        let n = list.len();
        let pos = self.current_channel().and_then(|c| list.iter().position(|x| x.id == *c));
        let next = match (pos, forward) {
            (Some(p), true) => (p + 1) % n,
            (Some(p), false) => (p + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        };
        let id = list[next].id.clone();
        self.open_channel(&id, cx);
    }
}