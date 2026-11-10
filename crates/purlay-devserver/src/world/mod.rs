//! Everything the server knows, in memory, behind one lock.
//!
//! `World` holds the rows (people, channels, messages, …) and the open sockets, so a
//! mutation and the frames it publishes happen under the same lock: a subscriber can never
//! see a frame for a write that is not visible to its next read, and the socket replay
//! cannot miss a message written between its read and its subscription.
//!
//! The operations are split by area: `chat` (messages, threads, reactions), `channels`,
//! `personal` (saved, reminders, scheduled, notifications, inbox, search, issues) and
//! `account` (sessions, presence, deep work).

mod account;
mod channels;
pub mod chat;
mod personal;

pub use account::SessionInfo;

use crate::error::{AppError, AppResult};
use crate::ids::{now_ms, now_ts};
use purlay_contract::models::*;
use purlay_contract::{Timestamp, Uuid};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

pub const SESSION_COOKIE: &str = "__Host-purlay_session";
pub const SESSION_MAX_AGE_S: i64 = 2_592_000;
pub const SERVER_VERSION: &str = "0.6.1";

pub fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms)
}

// --- rows ------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Person {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub role: UserRole,
    pub title: Option<String>,
    pub time_zone: Option<String>,
    pub status: Option<String>,
    pub joined_ms: i64,
    /// Shown online whether or not a socket is open (the live bot).
    pub always_online: bool,
}

#[derive(Debug, Clone)]
pub struct WorkspaceRow {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone)]
pub struct Member {
    pub joined_ms: i64,
    /// The newest root read; `None` is "nothing read".
    pub last_read: Option<Uuid>,
    pub state_version: i64,
    pub starred: bool,
    pub level: NotificationLevel,
    pub group: Option<Uuid>,
}

impl Member {
    pub fn new(joined_ms: i64, last_read: Option<Uuid>) -> Self {
        Member { joined_ms, last_read, state_version: 1, starred: false, level: NotificationLevel::All, group: None }
    }
}

#[derive(Debug, Clone)]
pub struct Pin {
    pub message_id: Uuid,
    pub by: Uuid,
    pub at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Chan {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub kind: ChannelType,
    pub members: BTreeMap<Uuid, Member>,
    /// Everybody who was ever a member: a refused `SUBSCRIBE` tells them why (D-264).
    pub ever_members: HashSet<Uuid>,
    pub roots: BTreeSet<Uuid>,
    pub last_message_ms: Option<i64>,
    pub pins: Vec<Pin>,
    pub bookmarks: Vec<ChannelBookmarkRow>,
    pub only_admins_post: bool,
}

#[derive(Debug, Clone)]
pub struct ChannelBookmarkRow {
    pub id: Uuid,
    pub url: String,
    pub title: String,
    pub added_by: Uuid,
    pub added_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub sender: Option<Uuid>,
    pub content: String,
    pub created_ms: i64,
    pub edited_ms: Option<i64>,
    pub deleted_ms: Option<i64>,
    pub parent_id: Option<Uuid>,
    pub replies: BTreeSet<Uuid>,
    pub resolved: Option<(i64, Uuid)>,
    /// In the order each emoji was first given.
    pub reactions: Vec<(String, Vec<Uuid>)>,
    pub mentions: Vec<Uuid>,
    pub attachments: Vec<Uuid>,
    /// The message's own change counter: `THREAD_UPDATED`, `REACTIONS_UPDATED` and
    /// `MESSAGE_REVISED` carry it.
    pub version: i64,
    /// Earlier words, oldest first.
    pub history: Vec<MessageVersion>,
    pub hidden_for: HashMap<Uuid, i64>,
    /// When the server last wrote the row (the changes feed is ordered by it).
    pub changed_ms: i64,
    /// Explicit follow choices; without one, the author and repliers follow.
    pub follows: HashMap<Uuid, bool>,
    /// Per person, the newest reply of this thread they have read.
    pub thread_reads: HashMap<Uuid, Uuid>,
    pub knowledge: Option<ThreadSummaryKnowledge>,
}

#[derive(Debug, Clone)]
pub struct Upload {
    pub attachment: Attachment,
    pub bytes: Arc<Vec<u8>>,
    pub uploader: Uuid,
    pub workspace_id: Uuid,
    pub message_id: Option<Uuid>,
    /// Held by a scheduled message.
    pub reserved: bool,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id: Uuid,
    pub user: Uuid,
    pub workspace_id: Uuid,
    pub kind: NotificationType,
    pub channel_id: Uuid,
    pub message_id: Uuid,
    pub created_ms: i64,
    pub read_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct InboxRow {
    pub user: Uuid,
    pub workspace_id: Uuid,
    pub entry: InboxEntry,
}

#[derive(Debug, Clone)]
pub struct SavedRow {
    pub id: Uuid,
    pub user: Uuid,
    pub message_id: Uuid,
    pub state: SavedState,
    pub saved_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ReminderRow {
    pub id: Uuid,
    pub user: Uuid,
    pub message_id: Uuid,
    pub remind_ms: i64,
    pub created_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ScheduledRow {
    pub id: Uuid,
    pub user: Uuid,
    pub workspace_id: Uuid,
    pub channel_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub content: String,
    pub attachment_ids: Vec<Uuid>,
    pub send_ms: i64,
    pub state: ScheduledMessageState,
    pub sent_message_id: Option<Uuid>,
    pub failure_code: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone)]
pub struct IssueRow {
    pub workspace_id: Uuid,
    pub issue: Issue,
}

#[derive(Debug, Clone)]
pub struct GroupRow {
    pub user: Uuid,
    pub workspace_id: Uuid,
    pub group: ChannelGroup,
}

#[derive(Debug, Clone)]
pub struct SavedSearchRow {
    pub user: Uuid,
    pub workspace_id: Uuid,
    pub search: SavedSearch,
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: Uuid,
    pub token: String,
    pub user: Uuid,
    pub kind: DeviceSessionKind,
    pub name: Option<String>,
    pub created_ms: i64,
    pub last_used_ms: i64,
    pub expires_ms: i64,
}

#[derive(Debug, Clone)]
pub struct NativeCode {
    pub challenge: String,
    pub state: String,
    pub user: Uuid,
    pub created_ms: i64,
    pub wrong: u32,
}

/// What the writer task of a socket is told to do.
#[derive(Debug, Clone)]
pub enum Out {
    Text(String),
    Close(u16, String),
}

pub struct Socket {
    pub user: Uuid,
    pub session_id: Uuid,
    pub subs: HashSet<Uuid>,
    pub tx: UnboundedSender<Out>,
}

#[derive(Debug, Clone, Default)]
pub struct PresenceRow {
    pub version: i64,
    pub online: bool,
    pub away: bool,
}

// --- the world -------------------------------------------------------------------------

pub struct World {
    pub app_url: String,
    pub workspace: WorkspaceRow,
    pub people: BTreeMap<Uuid, Person>,
    pub channels: BTreeMap<Uuid, Chan>,
    pub messages: HashMap<Uuid, Msg>,
    pub uploads: HashMap<Uuid, Upload>,
    pub notes: Vec<Note>,
    pub inbox: Vec<InboxRow>,
    pub saved: Vec<SavedRow>,
    pub reminders: Vec<ReminderRow>,
    pub scheduled: Vec<ScheduledRow>,
    pub issues: Vec<IssueRow>,
    pub groups: Vec<GroupRow>,
    pub saved_searches: Vec<SavedSearchRow>,
    pub sessions: HashMap<Uuid, SessionRow>,
    pub session_by_token: HashMap<String, Uuid>,
    pub codes: HashMap<String, NativeCode>,
    pub sockets: HashMap<u64, Socket>,
    pub next_socket: u64,
    pub presence: HashMap<Uuid, PresenceRow>,
    pub presence_version: i64,
    pub away_chosen: HashSet<Uuid>,
    pub deep_work: HashMap<Uuid, i64>,
    /// The person who last signed in or opened a socket (the live bot talks to them).
    pub last_active_user: Option<Uuid>,
    /// The person the sign-in page lists first and the bot talks to by default.
    pub default_user: Uuid,
    pub bot: Option<Uuid>,
    pub release_published_ms: i64,
}

impl World {
    pub fn new(app_url: String) -> Self {
        World {
            app_url,
            workspace: WorkspaceRow { id: crate::ids::new_id(), name: String::new(), slug: String::new() },
            people: BTreeMap::new(),
            channels: BTreeMap::new(),
            messages: HashMap::new(),
            uploads: HashMap::new(),
            notes: Vec::new(),
            inbox: Vec::new(),
            saved: Vec::new(),
            reminders: Vec::new(),
            scheduled: Vec::new(),
            issues: Vec::new(),
            groups: Vec::new(),
            saved_searches: Vec::new(),
            sessions: HashMap::new(),
            session_by_token: HashMap::new(),
            codes: HashMap::new(),
            sockets: HashMap::new(),
            next_socket: 1,
            presence: HashMap::new(),
            presence_version: 1,
            away_chosen: HashSet::new(),
            deep_work: HashMap::new(),
            last_active_user: None,
            default_user: Uuid::default(),
            bot: None,
            release_published_ms: now_ms() - 3 * 86_400_000,
        }
    }

    // --- lookups -------------------------------------------------------------------------

    pub fn person(&self, id: &str) -> Option<&Person> {
        self.people.get(id)
    }

    pub fn person_by_username(&self, username: &str) -> Option<&Person> {
        self.people.values().find(|p| p.username.eq_ignore_ascii_case(username))
    }

    pub fn is_admin(&self, user: &str) -> bool {
        self.people.get(user).is_some_and(|p| matches!(p.role, UserRole::Owner | UserRole::Admin))
    }

    /// `:workspaceId` → the workspace, or 404 for anything else (everybody is a member).
    pub fn check_workspace(&self, workspace_id: &str, user: &str) -> AppResult<()> {
        if workspace_id == self.workspace.id.as_str() && self.people.contains_key(user) {
            Ok(())
        } else {
            Err(AppError::workspace_not_found())
        }
    }

    pub fn channel_by_name(&self, name: &str) -> Option<&Chan> {
        self.channels.values().find(|c| c.name.as_deref() == Some(name))
    }

    pub fn is_member(&self, chan: &Chan, user: &str) -> bool {
        chan.members.contains_key(user)
    }

    /// Public channels are seen by every member of the workspace; the rest by their members.
    pub fn can_see(chan: &Chan, user: &str) -> bool {
        chan.kind == ChannelType::Public || chan.members.contains_key(user)
    }

    /// The channel if the person may see it, else `404 CHANNEL_NOT_FOUND`.
    pub fn visible_channel(&self, channel_id: &str, user: &str) -> AppResult<&Chan> {
        match self.channels.get(channel_id) {
            Some(c) if Self::can_see(c, user) => Ok(c),
            _ => Err(AppError::channel_not_found()),
        }
    }

    /// The channel if the person is in it: `404` when they cannot see it, `403` when they can
    /// and are not in it.
    pub fn member_channel(&self, channel_id: &str, user: &str) -> AppResult<&Chan> {
        let c = self.visible_channel(channel_id, user)?;
        if c.members.contains_key(user) { Ok(c) } else { Err(AppError::not_a_member()) }
    }

    /// The message if the person may read it (its channel visible, not deleted unless
    /// `allow_deleted`), else `404 MESSAGE_NOT_FOUND`.
    pub fn readable_message(&self, message_id: &str, user: &str, allow_deleted: bool) -> AppResult<&Msg> {
        let m = self.messages.get(message_id).ok_or_else(AppError::message_not_found)?;
        let c = self.channels.get(&m.channel_id).ok_or_else(AppError::message_not_found)?;
        if !Self::can_see(c, user) || (!allow_deleted && m.deleted_ms.is_some()) {
            return Err(AppError::message_not_found());
        }
        Ok(m)
    }

    // --- views ---------------------------------------------------------------------------

    pub fn user_view(&self, id: &str) -> User {
        match self.people.get(id) {
            Some(p) => User {
                id: p.id.clone(),
                username: p.username.clone(),
                display_name: p.display_name.clone(),
                avatar_url: None,
            },
            None => User {
                id: Uuid::new(id),
                username: "deleted".into(),
                display_name: "Deleted user".into(),
                avatar_url: None,
            },
        }
    }

    pub fn attachment_views(&self, ids: &[Uuid]) -> Vec<Attachment> {
        ids.iter().filter_map(|id| self.uploads.get(id)).map(|u| u.attachment.clone()).collect()
    }

    pub fn thread_view(&self, m: &Msg) -> Option<ThreadSummary> {
        if m.parent_id.is_some() || (m.replies.is_empty() && m.knowledge.is_none()) {
            return None;
        }
        let live: Vec<&Msg> =
            m.replies.iter().filter_map(|r| self.messages.get(r)).filter(|r| r.deleted_ms.is_none()).collect();
        Some(ThreadSummary {
            knowledge: m.knowledge.clone(),
            github_work: None,
            reply_count: live.len() as i64,
            last_reply_at: live.iter().map(|r| r.created_ms).max().map(ts),
            resolved_at: m.resolved.as_ref().map(|(at, _)| ts(*at)),
            resolved_by: m.resolved.as_ref().map(|(_, by)| self.user_view(by)),
        })
    }

    /// A message as `viewer` sees it (`None`: as the room sees it, nobody's personal hide).
    pub fn message_view(&self, m: &Msg, viewer: Option<&str>) -> Message {
        let hidden = viewer.is_some_and(|v| m.hidden_for.contains_key(v));
        let gone = hidden || m.deleted_ms.is_some();
        Message {
            id: m.id.clone(),
            channel_id: m.channel_id.clone(),
            sender: m.sender.as_ref().map(|s| self.user_view(s)),
            github_event: None,
            incoming_webhook: None,
            content: if gone { String::new() } else { m.content.clone() },
            edited_at: m.edited_ms.map(ts),
            hidden_for_me: if hidden { Some(true) } else { None },
            deleted_at: m.deleted_ms.map(ts),
            created_at: ts(m.created_ms),
            parent_id: m.parent_id.clone(),
            thread: self.thread_view(m),
            reactions: if gone {
                Vec::new()
            } else {
                m.reactions.iter().map(|(e, u)| Reaction { emoji: e.clone(), user_ids: u.clone() }).collect()
            },
            mentions: if gone { Vec::new() } else { m.mentions.clone() },
            attachments: if gone { Vec::new() } else { self.attachment_views(&m.attachments) },
        }
    }

    pub fn message_view_by_id(&self, id: &str, viewer: Option<&str>) -> Option<Message> {
        self.messages.get(id).map(|m| self.message_view(m, viewer))
    }

    /// Whether a root counts toward someone's unread state.
    fn counts_for(&self, m: &Msg, user: &str) -> bool {
        m.deleted_ms.is_none() && m.sender.as_deref() != Some(user) && !m.hidden_for.contains_key(user)
    }

    pub fn channel_state(&self, chan: &Chan, user: &str) -> Option<ChannelState> {
        let member = chan.members.get(user)?;
        let unread: Vec<&Msg> = match &member.last_read {
            Some(lr) => chan.roots.range::<Uuid, _>((std::ops::Bound::Excluded(lr), std::ops::Bound::Unbounded)),
            None => chan.roots.range::<Uuid, _>(..),
        }
        .filter_map(|id| self.messages.get(id))
        .filter(|m| self.counts_for(m, user))
        .collect();
        let dm = chan.kind == ChannelType::DirectMessage;
        let mentions = unread.iter().filter(|m| dm || m.mentions.iter().any(|x| x.as_str() == user)).count();
        Some(ChannelState {
            last_read_message_id: member.last_read.clone(),
            unread_count: unread.len() as i64,
            unread_mention_count: mentions as i64,
        })
    }

    pub fn channel_view(&self, chan: &Chan, viewer: &str) -> Channel {
        let member = chan.members.get(viewer);
        let (peer, peers) = if chan.kind == ChannelType::DirectMessage {
            let others: Vec<User> =
                chan.members.keys().filter(|k| k.as_str() != viewer).map(|k| self.user_view(k)).collect();
            let peer = match others.len() {
                0 => Some(self.user_view(viewer)), // a note to self
                1 => Some(others[0].clone()),
                _ => None,
            };
            (peer, others)
        } else {
            (None, Vec::new())
        };
        Channel {
            id: chan.id.clone(),
            workspace_id: chan.workspace_id.clone(),
            name: chan.name.clone(),
            topic: chan.topic.clone(),
            r#type: chan.kind.clone(),
            last_message_at: chan.last_message_ms.map(ts),
            state: self.channel_state(chan, viewer),
            notification_level: member.map(|m| m.level.clone()),
            starred: member.is_some_and(|m| m.starred),
            sidebar_group_id: member
                .filter(|m| !m.starred && chan.kind != ChannelType::DirectMessage)
                .and_then(|m| m.group.clone()),
            peer,
            peers,
            member_count: chan.members.len() as i64,
            incident: None,
            code_repository: None,
            only_admins_post: chan.only_admins_post,
        }
    }

    pub fn workspace_view(&self) -> Workspace {
        Workspace {
            id: self.workspace.id.clone(),
            name: self.workspace.name.clone(),
            slug: self.workspace.slug.clone(),
            avatar_url: None,
        }
    }

    pub fn profile_view(p: &Person) -> Profile {
        Profile { title: p.title.clone(), time_zone: p.time_zone.clone(), status: p.status.clone() }
    }

    pub fn membership_view(&self, user: &str) -> Option<Membership> {
        let p = self.people.get(user)?;
        Some(Membership {
            workspace: self.workspace_view(),
            role: p.role.clone(),
            joined_at: ts(p.joined_ms),
            deep_work_until: self.deep_work.get(user).copied().filter(|u| *u > now_ms()).map(ts),
            modules: Some(vec![ModuleId::Chat, ModuleId::Issues]),
        })
    }

    // --- publishing ----------------------------------------------------------------------

    fn send_text(&self, socket: &Socket, text: &str) {
        let _ = socket.tx.send(Out::Text(text.to_string()));
    }

    /// To every socket subscribed to the channel.
    pub fn publish_channel(&self, channel_id: &str, frame: &ServerFrame) {
        let text = frame_text(frame);
        for s in self.sockets.values().filter(|s| s.subs.contains(channel_id)) {
            self.send_text(s, &text);
        }
    }

    /// To every socket subscribed to the channel, except those of one person.
    pub fn publish_channel_except(&self, channel_id: &str, except_user: &str, frame: &ServerFrame) {
        let text = frame_text(frame);
        for s in self.sockets.values().filter(|s| s.subs.contains(channel_id) && s.user.as_str() != except_user) {
            self.send_text(s, &text);
        }
    }

    /// To every socket of one person (their `user:` topic).
    pub fn publish_user(&self, user: &str, frame: &ServerFrame) {
        let text = frame_text(frame);
        for s in self.sockets.values().filter(|s| s.user.as_str() == user) {
            self.send_text(s, &text);
        }
    }

    /// To every socket of the workspace.
    pub fn publish_all(&self, frame: &ServerFrame) {
        let text = frame_text(frame);
        for s in self.sockets.values() {
            self.send_text(s, &text);
        }
    }

    /// `CHANNEL_STATE` to the person, at a new version of their row.
    pub fn publish_channel_state(&mut self, channel_id: &str, user: &str) {
        let Some(chan) = self.channels.get_mut(channel_id) else { return };
        let Some(member) = chan.members.get_mut(user) else { return };
        member.state_version += 1;
        let version = member.state_version;
        let chan = &self.channels[channel_id];
        let Some(state) = self.channel_state(chan, user) else { return };
        let frame = ServerFrame::ChannelState(ChannelStateFrame {
            data: ChannelStateFrameData { channel_id: chan.id.clone(), state },
            version,
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    pub fn publish_notifications_updated(&self, user: &str) {
        let frame = ServerFrame::NotificationsUpdated(NotificationsUpdatedFrame {
            data: NotificationsUpdatedFrameData { workspace_id: self.workspace.id.clone() },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    pub fn publish_channels_updated(&self, user: &str) {
        let frame = ServerFrame::ChannelsUpdated(ChannelsUpdatedFrame {
            data: ChannelsUpdatedFrameData { workspace_id: self.workspace.id.clone() },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    pub fn publish_channel_added(&self, channel_id: &str, user: &str) {
        let Some(chan) = self.channels.get(channel_id) else { return };
        let frame = ServerFrame::ChannelAdded(ChannelAddedFrame {
            data: self.channel_view(chan, user),
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    /// `THREAD_UPDATED` for a root, at its current version.
    pub fn publish_thread(&self, root_id: &str) {
        let Some(root) = self.messages.get(root_id) else { return };
        let Some(thread) = self.thread_view(root) else { return };
        let frame = ServerFrame::ThreadUpdated(ThreadUpdatedFrame {
            data: ThreadUpdatedFrameData { channel_id: root.channel_id.clone(), message_id: root.id.clone(), thread },
            version: root.version,
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel(&root.channel_id, &frame);
    }

    pub fn publish_revised(&self, message_id: &str) {
        let Some(m) = self.messages.get(message_id) else { return };
        let frame = ServerFrame::MessageRevised(MessageRevisedFrame {
            data: MessageRevisedFrameData {
                channel_id: m.channel_id.clone(),
                message_id: m.id.clone(),
                parent_id: m.parent_id.clone(),
                content: if m.deleted_ms.is_some() { String::new() } else { m.content.clone() },
                edited_at: m.edited_ms.map(ts),
                deleted_at: m.deleted_ms.map(ts),
                mentions: m.mentions.clone(),
            },
            version: m.version,
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel(&m.channel_id, &frame);
    }

    pub fn publish_reactions(&self, message_id: &str) {
        let Some(m) = self.messages.get(message_id) else { return };
        let frame = ServerFrame::ReactionsUpdated(ReactionsUpdatedFrame {
            data: ReactionsUpdatedFrameData {
                channel_id: m.channel_id.clone(),
                message_id: m.id.clone(),
                reactions: m.reactions.iter().map(|(e, u)| Reaction { emoji: e.clone(), user_ids: u.clone() }).collect(),
            },
            version: m.version,
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel(&m.channel_id, &frame);
    }

    /// `TYPING` to the room, not to the typist's own sockets.
    pub fn publish_typing(&self, channel_id: &str, user: &str) {
        let frame = ServerFrame::TypingServer(TypingServerFrame {
            data: TypingServerFrameData {
                channel_id: Uuid::new(channel_id),
                user_id: Uuid::new(user),
                ttl_ms: purlay_contract::constants::TYPING_TTL_MS,
            },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel_except(channel_id, user, &frame);
    }
}

pub fn frame_text(frame: &ServerFrame) -> String {
    serde_json::to_string(frame).unwrap_or_else(|e| {
        tracing::error!("frame did not serialize: {e}");
        String::new()
    })
}

/// A frame answering a client frame.
pub fn error_frame(err: &AppError, ref_id: Option<Uuid>) -> ServerFrame {
    ServerFrame::Error(ErrorFrame { data: err.body(), ts: now_ts(), ref_id })
}

/// Keyset helpers: `limit` from a query, 1–100, default 50.
pub fn page_limit(limit: Option<i64>) -> AppResult<usize> {
    use purlay_contract::constants::page_limit;
    match limit {
        None => Ok(page_limit::DEFAULT as usize),
        Some(l) if (1..=page_limit::MAX).contains(&l) => Ok(l as usize),
        Some(_) => Err(AppError::field("limit", "must be 1 to 100")),
    }
}

/// The first line of some text, at most `max` characters, with markdown marks thinned.
pub fn excerpt(text: &str, max: usize) -> Option<String> {
    let line = text
        .lines()
        .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']).trim())
        .find(|l| !l.is_empty() && !l.starts_with("```"))?;
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.pop();
        out.push('…');
    }
    Some(out)
}
