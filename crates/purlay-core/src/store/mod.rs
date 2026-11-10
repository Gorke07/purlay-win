//! What the client knows, shared between the engine (which writes) and the UI (which reads).
//!
//! Messages and channels are first-class: frames patch them in place. Everything else is a
//! keyed resource in [`Cache`]: read over HTTP when a screen asks for it, and read again
//! when a frame says it moved (`readAgain` in the web client) — only while it is in use;
//! otherwise it is marked stale and read when next shown.

pub mod window;

use crate::realtime::connection::Status;
use purlay_contract::models::*;
use purlay_contract::Uuid;
use std::any::Any;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
pub use window::{ChannelWindow, ThreadWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Sending,
    Sent,
    Failed,
}

/// A message the person sent that the server has not yet been heard to have.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingMessage {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub content: String,
    /// By the corrected clock: the row's time until the echo says.
    pub created_at: purlay_contract::Timestamp,
    pub delivery: Delivery,
    /// Why it failed (an error code, `TIMEOUT`, or `NETWORK`).
    pub reason: Option<String>,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    pub online: bool,
    pub away: bool,
    pub version: i64,
}

/// What a channel in `degraded` was refused for: the server's code, or `REPLAY`.
pub type DegradedReason = String;
/// The `degraded` key for an error that answers nothing the client can name.
pub const UNNAMED: &str = "*";

/// Every resource the cache can hold. A key names what to read and where to read it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Me,
    Members(Uuid),
    Presence(Uuid),
    ChannelGroups(Uuid),
    Pins(Uuid),
    Bookmarks(Uuid),
    Files(Uuid),
    ChannelMembers(Uuid),
    Unreads(Uuid),
    /// Workspace, the `types` CSV (or empty for all), unread only.
    Notifications(Uuid, String, bool),
    Inbox(Uuid, bool),
    Saved(Uuid, SavedState),
    SavedIds(Uuid),
    Unresolved(Uuid),
    Reminders(Uuid),
    Scheduled(Uuid),
    Issues(Uuid, IssueQueryStatus),
    SavedSearches(Uuid),
    ThreadFollow(Uuid),
    MessageVersions(Uuid),
    Sessions,
}

impl Key {
    pub fn workspace(&self) -> Option<&Uuid> {
        match self {
            Key::Members(w)
            | Key::Presence(w)
            | Key::ChannelGroups(w)
            | Key::Notifications(w, _, _)
            | Key::Inbox(w, _)
            | Key::Saved(w, _)
            | Key::SavedIds(w)
            | Key::Unresolved(w)
            | Key::Reminders(w)
            | Key::Scheduled(w)
            | Key::Issues(w, _)
            | Key::SavedSearches(w) => Some(w),
            _ => None,
        }
    }

    pub fn channel(&self) -> Option<&Uuid> {
        match self {
            Key::Pins(c) | Key::Bookmarks(c) | Key::Files(c) | Key::ChannelMembers(c) | Key::Unreads(c) => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadState {
    Loading,
    Loaded,
    /// Loaded once, and a frame or a reconnect said it may have moved.
    Stale,
    Failed(String),
}

#[derive(Clone)]
pub struct Entry {
    pub state: LoadState,
    pub value: Option<Arc<dyn Any + Send + Sync>>,
    /// A read was asked for while one was in flight: read once more when it lands.
    pub again: bool,
}

#[derive(Default, Clone)]
pub struct Cache {
    entries: HashMap<Key, Entry>,
    /// Keys a screen is showing now.
    pub active: HashSet<Key>,
}

impl Cache {
    pub fn get<T: Any + Send + Sync>(&self, key: &Key) -> Option<&T> {
        self.entries.get(key)?.value.as_ref()?.downcast_ref::<T>()
    }

    pub fn state(&self, key: &Key) -> Option<&LoadState> {
        self.entries.get(key).map(|e| &e.state)
    }

    pub fn entry_mut(&mut self, key: &Key) -> Option<&mut Entry> {
        self.entries.get_mut(key)
    }

    pub fn set<T: Any + Send + Sync>(&mut self, key: Key, value: T) {
        let again = self.entries.get(&key).map(|e| e.again).unwrap_or(false);
        self.entries.insert(key, Entry { state: LoadState::Loaded, value: Some(Arc::new(value)), again });
    }

    pub fn set_any(&mut self, key: Key, value: Arc<dyn Any + Send + Sync>) {
        let again = self.entries.get(&key).map(|e| e.again).unwrap_or(false);
        self.entries.insert(key, Entry { state: LoadState::Loaded, value: Some(value), again });
    }

    pub fn update<T: Any + Send + Sync + Clone>(&mut self, key: &Key, f: impl FnOnce(&mut T)) -> bool {
        let Some(e) = self.entries.get_mut(key) else { return false };
        let Some(v) = e.value.as_ref().and_then(|v| v.downcast_ref::<T>()) else { return false };
        let mut owned = v.clone();
        f(&mut owned);
        e.value = Some(Arc::new(owned));
        true
    }

    /// Marks a read as started; false when one is already in flight (it will run again).
    pub fn begin(&mut self, key: &Key) -> bool {
        match self.entries.get_mut(key) {
            Some(e) if e.state == LoadState::Loading => {
                e.again = true;
                false
            }
            Some(e) => {
                e.state = LoadState::Loading;
                e.again = false;
                true
            }
            None => {
                self.entries.insert(key.clone(), Entry { state: LoadState::Loading, value: None, again: false });
                true
            }
        }
    }

    pub fn fail(&mut self, key: &Key, why: String) {
        if let Some(e) = self.entries.get_mut(key) {
            e.state = LoadState::Failed(why);
        }
    }

    /// Takes the "read once more" mark.
    pub fn take_again(&mut self, key: &Key) -> bool {
        self.entries.get_mut(key).map(|e| std::mem::take(&mut e.again)).unwrap_or(false)
    }

    pub fn mark_stale(&mut self, key: &Key) {
        if let Some(e) = self.entries.get_mut(key) {
            if e.state == LoadState::Loaded || matches!(e.state, LoadState::Failed(_)) {
                e.state = LoadState::Stale;
            } else if e.state == LoadState::Loading {
                e.again = true;
            }
        }
    }

    pub fn keys_where(&self, f: impl Fn(&Key) -> bool) -> Vec<Key> {
        self.entries.keys().filter(|k| f(k)).cloned().collect()
    }

    pub fn remove_where(&mut self, f: impl Fn(&Key) -> bool) {
        self.entries.retain(|k, _| !f(k));
        self.active.retain(|k| !f(k));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.active.clear();
    }
}

/// Everything known about one server session. The UI holds a read lock while drawing.
#[derive(Default, Clone)]
pub struct Store {
    pub revision: u64,
    pub status: Option<Status>,
    pub connected_once: bool,
    pub me: Option<MeResponse>,
    pub me_failed: Option<String>,
    pub client_info: Option<ClientInfo>,
    /// Channels by workspace, then by id.
    pub channels: HashMap<Uuid, BTreeMap<Uuid, Channel>>,
    pub channels_loaded: HashMap<Uuid, crate::store::LoadState>,
    pub windows: HashMap<Uuid, ChannelWindow>,
    pub window_loading: HashSet<Uuid>,
    pub window_failed: HashMap<Uuid, String>,
    pub threads: HashMap<Uuid, ThreadWindow>,
    pub pending: Vec<PendingMessage>,
    pub degraded: BTreeMap<String, DegradedReason>,
    /// Who is typing, by channel, in the order they began (never the person themselves).
    pub typing: HashMap<Uuid, Vec<Uuid>>,
    pub presence: HashMap<Uuid, Presence>,
    pub presence_epoch: u64,
    pub gone: HashMap<Uuid, String>,
    /// Unsent messages lost with a channel: how many, and a sequence that rises per loss.
    pub lost: Option<(usize, u64)>,
    /// Ids of messages the person saved, per workspace.
    pub cache: Cache,
    /// Link previews by message id then URL (None = no preview).
    pub link_previews: HashMap<(Uuid, String), Option<WebLinkPreview>>,
    /// Users seen anywhere, by id: senders, members, peers.
    pub people: HashMap<Uuid, User>,
}

impl Store {
    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.values().find_map(|m| m.get(id))
    }

    pub fn channel_mut(&mut self, id: &str) -> Option<&mut Channel> {
        self.channels.values_mut().find_map(|m| m.get_mut(id))
    }

    pub fn workspace_of_channel(&self, id: &str) -> Option<Uuid> {
        self.channels.iter().find(|(_, m)| m.contains_key(id)).map(|(w, _)| w.clone())
    }

    pub fn membership(&self, workspace: &str) -> Option<&Membership> {
        self.me.as_ref()?.memberships.iter().find(|m| m.workspace.id.as_str() == workspace)
    }

    pub fn user(&self, id: &str) -> Option<&User> {
        self.people.get(id)
    }

    pub fn remember_user(&mut self, u: &User) {
        self.people.insert(u.id.clone(), u.clone());
    }

    pub fn remember_message_people(&mut self, m: &Message) {
        if let Some(s) = &m.sender {
            self.people.insert(s.id.clone(), s.clone());
        }
    }

    /// The pending rows for a channel's feed (roots) or a thread (replies to `parent`).
    pub fn pending_for(&self, channel: &str, parent: Option<&str>) -> Vec<&PendingMessage> {
        self.pending
            .iter()
            .filter(|p| p.channel_id.as_str() == channel && p.parent_id.as_deref() == parent)
            .collect()
    }

    /// Finds a message wherever it is loaded: the channel's feed or any thread of it.
    pub fn find_message(&self, channel: &str, id: &str) -> Option<&Message> {
        if let Some(m) = self.windows.get(channel).and_then(|w| w.get(id)) {
            return Some(m);
        }
        for t in self.threads.values() {
            if t.channel_id.as_str() != channel {
                continue;
            }
            if t.root.as_ref().is_some_and(|r| r.id.as_str() == id) {
                return t.root.as_ref();
            }
            if let Some(m) = t.replies.get(id) {
                return Some(m);
            }
        }
        None
    }

    /// Runs `f` on every loaded copy of a message (feed, thread root, thread reply).
    pub fn for_message(&mut self, channel: &str, id: &str, mut f: impl FnMut(&mut Message)) -> bool {
        let mut found = false;
        if let Some(m) = self.windows.get_mut(channel).and_then(|w| w.get_mut(id)) {
            f(m);
            found = true;
        }
        for t in self.threads.values_mut() {
            if t.channel_id.as_str() != channel && !t.root.as_ref().is_some_and(|r| r.id.as_str() == id) {
                continue;
            }
            if let Some(r) = t.root.as_mut().filter(|r| r.id.as_str() == id) {
                f(r);
                found = true;
            }
            if let Some(m) = t.replies.get_mut(id) {
                f(m);
                found = true;
            }
        }
        found
    }

    /// Total unread mentions across workspaces, plus unread direct messages: the badge.
    pub fn attention_count(&self) -> i64 {
        let mut n = 0;
        for chans in self.channels.values() {
            for c in chans.values() {
                let Some(s) = &c.state else { continue };
                if c.notification_level == Some(NotificationLevel::None) {
                    continue;
                }
                if c.r#type == ChannelType::DirectMessage {
                    n += s.unread_count;
                } else {
                    n += s.unread_mention_count;
                }
            }
        }
        n
    }
}
