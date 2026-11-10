//! Channels: the list, joining and leaving, conversations, read positions, stars, groups,
//! bookmarks and files.

use super::chat::{Page, page_ids};
use super::*;
use crate::ids::new_id;
use purlay_contract::constants::CONVERSATION_PEOPLE_MAX;

const BOOKMARKS_MAX: usize = 50;
const CHANNEL_GROUPS_MAX: usize = 50;

impl World {
    pub fn list_channels(&self, user: &str) -> Vec<Channel> {
        let mut out: Vec<&Chan> = self.channels.values().filter(|c| Self::can_see(c, user)).collect();
        out.sort_by(|a, b| {
            let rank = |c: &Chan| if c.kind == ChannelType::DirectMessage { 1 } else { 0 };
            rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)).then_with(|| a.id.cmp(&b.id))
        });
        out.into_iter().map(|c| self.channel_view(c, user)).collect()
    }

    pub fn get_channel(&self, user: &str, channel_id: &str) -> AppResult<Channel> {
        let c = self.visible_channel(channel_id, user)?;
        Ok(self.channel_view(c, user))
    }

    /// The newest root of a channel, the position of somebody who has read everything.
    fn newest_root(&self, chan: &Chan) -> Option<Uuid> {
        chan.roots.iter().next_back().cloned()
    }

    pub fn add_member(&mut self, channel_id: &str, user: &str, read_all: bool) {
        let newest = self.channels.get(channel_id).and_then(|c| self.newest_root(c));
        let Some(chan) = self.channels.get_mut(channel_id) else { return };
        chan.ever_members.insert(Uuid::new(user));
        chan.members.entry(Uuid::new(user)).or_insert_with(|| Member::new(now_ms(), if read_all { newest } else { None }));
    }

    pub fn create_channel(&mut self, user: &str, req: &CreateChannelRequest) -> AppResult<Channel> {
        let name = req.name.trim().trim_start_matches('#').to_lowercase();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(AppError::field("name", "is not a channel name"));
        }
        let taken = self
            .channels
            .values()
            .any(|c| c.kind != ChannelType::DirectMessage && c.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&name)));
        if taken {
            return Err(AppError::new("CHANNEL_NAME_TAKEN", "A channel has that name already."));
        }
        let kind = match &req.r#type {
            Some(CreateChannelRequestType::Private) => ChannelType::Private,
            Some(CreateChannelRequestType::Unknown(_)) => return Err(AppError::field("type", "is not public or private")),
            _ => ChannelType::Public,
        };
        let id = new_id();
        self.channels.insert(
            id.clone(),
            Chan {
                id: id.clone(),
                workspace_id: self.workspace.id.clone(),
                name: Some(name),
                topic: req.topic.clone().filter(|t| !t.trim().is_empty()),
                kind,
                members: BTreeMap::new(),
                ever_members: HashSet::new(),
                roots: BTreeSet::new(),
                last_message_ms: None,
                pins: Vec::new(),
                bookmarks: Vec::new(),
                only_admins_post: false,
            },
        );
        self.add_member(&id, user, true);
        self.publish_channel_added(&id, user);
        Ok(self.channel_view(&self.channels[&id], user))
    }

    /// The direct conversation of exactly these people, made if it does not exist; the bool
    /// says whether it was made now.
    pub fn open_conversation(&mut self, user: &str, others: &[Uuid]) -> AppResult<(Channel, bool)> {
        let mut set: BTreeSet<Uuid> = BTreeSet::new();
        for o in others {
            if !self.people.contains_key(o) {
                return Err(AppError::new("USER_NOT_FOUND", "No such person."));
            }
            if !set.insert(o.clone()) {
                return Err(AppError::field("userIds", "names a person twice"));
            }
        }
        if set.len() > 1 && set.contains(user) {
            return Err(AppError::field("userIds", "includes you among others"));
        }
        if set.len() > CONVERSATION_PEOPLE_MAX as usize - 1 {
            return Err(AppError::field("userIds", "names too many people"));
        }
        set.insert(Uuid::new(user));
        let existing = self.channels.values().find(|c| {
            c.kind == ChannelType::DirectMessage && c.members.keys().cloned().collect::<BTreeSet<Uuid>>() == set
        });
        if let Some(c) = existing {
            return Ok((self.channel_view(c, user), false));
        }
        let id = new_id();
        self.channels.insert(
            id.clone(),
            Chan {
                id: id.clone(),
                workspace_id: self.workspace.id.clone(),
                name: None,
                topic: None,
                kind: ChannelType::DirectMessage,
                members: BTreeMap::new(),
                ever_members: HashSet::new(),
                roots: BTreeSet::new(),
                last_message_ms: None,
                pins: Vec::new(),
                bookmarks: Vec::new(),
                only_admins_post: false,
            },
        );
        for p in &set {
            self.add_member(&id, p, true);
        }
        for p in &set {
            self.publish_channel_added(&id, p);
        }
        Ok((self.channel_view(&self.channels[&id], user), true))
    }

    pub fn join_channel(&mut self, user: &str, channel_id: &str) -> AppResult<Channel> {
        let chan = self.visible_channel(channel_id, user)?;
        if chan.kind != ChannelType::Public && !chan.members.contains_key(user) {
            return Err(AppError::channel_not_found());
        }
        if !chan.members.contains_key(user) {
            self.add_member(channel_id, user, true);
            self.publish_channel_added(channel_id, user);
            self.publish_channel_state(channel_id, user);
        }
        Ok(self.channel_view(&self.channels[channel_id], user))
    }

    /// The person's sockets stop hearing a channel, and are told why.
    pub fn revoke(&mut self, user: &str, channel_id: &str, reason: AccessRevokedFrameDataReason) {
        for s in self.sockets.values_mut().filter(|s| s.user.as_str() == user) {
            s.subs.remove(channel_id);
        }
        let frame = ServerFrame::AccessRevoked(AccessRevokedFrame {
            data: AccessRevokedFrameData { channel_ids: vec![Uuid::new(channel_id)], reason: Some(reason) },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    pub fn leave_channel(&mut self, user: &str, channel_id: &str) -> AppResult<()> {
        let chan = self.visible_channel(channel_id, user)?;
        if chan.kind == ChannelType::DirectMessage {
            return Err(AppError::channel_not_found());
        }
        if !chan.members.contains_key(user) {
            return Ok(());
        }
        if chan.kind == ChannelType::Private && chan.members.len() == 1 {
            return Err(AppError::new("LAST_CHANNEL_MEMBER", "You are the last member of this channel."));
        }
        self.channels.get_mut(channel_id).expect("checked").members.remove(user);
        self.revoke(user, channel_id, AccessRevokedFrameDataReason::Left);
        Ok(())
    }

    pub fn add_channel_member(&mut self, user: &str, channel_id: &str, other: &str) -> AppResult<()> {
        let chan = self.visible_channel(channel_id, user)?;
        if chan.kind == ChannelType::DirectMessage {
            return Err(AppError::forbidden());
        }
        if !chan.members.contains_key(user) {
            return Err(AppError::not_a_member());
        }
        if !self.people.contains_key(other) {
            return Err(AppError::new("USER_NOT_FOUND", "No such person."));
        }
        if chan.members.contains_key(other) {
            return Ok(());
        }
        self.add_member(channel_id, other, true);
        self.publish_channel_added(channel_id, other);
        Ok(())
    }

    pub fn channel_members(&self, user: &str, channel_id: &str) -> AppResult<ChannelMemberList> {
        let chan = self.visible_channel(channel_id, user)?;
        let mut members: Vec<(&Uuid, &Member)> = chan.members.iter().collect();
        members.sort_by_key(|(id, m)| (m.joined_ms, (*id).clone()));
        Ok(members.into_iter().map(|(id, m)| ChannelMember { user: self.user_view(id), joined_at: ts(m.joined_ms) }).collect())
    }

    fn joined_channel_mut(&mut self, user: &str, channel_id: &str) -> AppResult<&mut Member> {
        match self.channels.get_mut(channel_id) {
            Some(c) if c.members.contains_key(user) => Ok(c.members.get_mut(user).expect("checked")),
            _ => Err(AppError::channel_not_found()),
        }
    }

    pub fn mark_read(&mut self, user: &str, channel_id: &str, last_read: &str) -> AppResult<ChannelState> {
        let chan = self.visible_channel(channel_id, user)?;
        if !chan.members.contains_key(user) {
            return Err(AppError::channel_not_found());
        }
        let m = self.messages.get(last_read).filter(|m| m.channel_id.as_str() == channel_id);
        let Some(m) = m else { return Err(AppError::message_not_found()) };
        let bound = m.parent_id.clone().unwrap_or_else(|| m.id.clone());
        let member = self.joined_channel_mut(user, channel_id)?;
        if member.last_read.as_ref().is_none_or(|lr| *lr < bound) {
            member.last_read = Some(bound.clone());
        }
        // The notifications of the roots at or behind the position.
        let now = now_ms();
        let mut read = false;
        for n in self.notes.iter_mut().filter(|n| n.user.as_str() == user && n.channel_id.as_str() == channel_id) {
            let is_root = self.messages.get(&n.message_id).is_some_and(|m| m.parent_id.is_none());
            if n.read_ms.is_none() && is_root && n.message_id <= bound {
                n.read_ms = Some(now);
                read = true;
            }
        }
        if read {
            self.publish_notifications_updated(user);
        }
        self.publish_channel_state(channel_id, user);
        Ok(self.channel_state(&self.channels[channel_id], user).expect("member"))
    }

    pub fn mark_unread(&mut self, user: &str, channel_id: &str, from: &str) -> AppResult<ChannelState> {
        let chan = self.visible_channel(channel_id, user)?;
        if !chan.members.contains_key(user) {
            return Err(AppError::channel_not_found());
        }
        let m = self.messages.get(from).filter(|m| m.channel_id.as_str() == channel_id);
        let Some(m) = m else { return Err(AppError::message_not_found()) };
        let root = m.parent_id.clone().unwrap_or_else(|| m.id.clone());
        let before = chan.roots.range::<Uuid, _>(..&root).next_back().cloned();
        let member = self.joined_channel_mut(user, channel_id)?;
        if member.last_read.as_ref().is_none_or(|lr| *lr >= root) {
            member.last_read = before;
        }
        self.publish_channel_state(channel_id, user);
        Ok(self.channel_state(&self.channels[channel_id], user).expect("member"))
    }

    pub fn mark_all_read(&mut self, user: &str) -> ReadAllChannelsResponse {
        let ids: Vec<Uuid> = self
            .channels
            .values()
            .filter(|c| c.members.contains_key(user))
            .filter(|c| self.channel_state(c, user).is_some_and(|s| s.unread_count > 0 || s.unread_mention_count > 0))
            .map(|c| c.id.clone())
            .collect();
        let mut data = Vec::new();
        let now = now_ms();
        for id in ids {
            let newest = self.newest_root(&self.channels[&id]);
            if let Some(m) = self.channels.get_mut(&id).and_then(|c| c.members.get_mut(user)) {
                m.last_read = newest;
            }
            for n in self.notes.iter_mut().filter(|n| n.user.as_str() == user && n.channel_id == id) {
                n.read_ms.get_or_insert(now);
            }
            self.publish_channel_state(&id, user);
            let state = self.channel_state(&self.channels[&id], user).expect("member");
            data.push(ReadAllChannelsResponseDataItem { channel_id: id, state });
        }
        ReadAllChannelsResponse { data }
    }

    pub fn set_star(&mut self, user: &str, channel_id: &str, starred: bool) -> AppResult<()> {
        let m = self.joined_channel_mut(user, channel_id)?;
        if m.starred != starred {
            m.starred = starred;
            if starred {
                m.group = None;
            }
            self.publish_channels_updated(user);
        }
        Ok(())
    }

    pub fn set_level(&mut self, user: &str, channel_id: &str, level: NotificationLevel) -> AppResult<()> {
        if let NotificationLevel::Unknown(_) = level {
            return Err(AppError::field("level", "is not all, mentions or none"));
        }
        self.joined_channel_mut(user, channel_id)?.level = level;
        Ok(())
    }

    // --- personal channel groups ---------------------------------------------------------

    pub fn list_groups(&self, user: &str, q: &ChannelGroupQuery) -> AppResult<ChannelGroupPage> {
        let page = Page::from_query(&PageQuery { before: q.before.clone(), after: None, limit: q.limit })?;
        let mut ids: Vec<Uuid> = self.groups.iter().filter(|g| g.user.as_str() == user).map(|g| g.group.id.clone()).collect();
        ids.sort();
        let (chosen, next) = page_ids(&ids, &page, false);
        Ok(ChannelGroupPage {
            data: chosen
                .iter()
                .filter_map(|id| self.groups.iter().find(|g| g.user.as_str() == user && g.group.id == *id))
                .map(|g| g.group.clone())
                .collect(),
            next_cursor: next,
        })
    }

    pub fn save_group(&mut self, user: &str, group_id: &str, name: &str) -> AppResult<ChannelGroup> {
        if !crate::ids::is_uuid(group_id) {
            return Err(AppError::field("groupId", "is not a UUID"));
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(AppError::field("name", "is not a group name"));
        }
        if self.groups.iter().any(|g| g.group.id.as_str() == group_id && g.user.as_str() != user) {
            return Err(AppError::new("CHANNEL_GROUP_NOT_FOUND", "No such group."));
        }
        if self
            .groups
            .iter()
            .any(|g| g.user.as_str() == user && g.group.id.as_str() != group_id && g.group.name.eq_ignore_ascii_case(name))
        {
            return Err(AppError::new("CHANNEL_GROUP_NAME_TAKEN", "You have a group of that name."));
        }
        let changed;
        let group = match self.groups.iter_mut().find(|g| g.group.id.as_str() == group_id) {
            Some(g) => {
                changed = g.group.name != name;
                g.group.name = name.to_string();
                g.group.clone()
            }
            None => {
                if self.groups.iter().filter(|g| g.user.as_str() == user).count() >= CHANNEL_GROUPS_MAX {
                    return Err(AppError::new("CHANNEL_GROUP_LIMIT", "You have the most groups you may."));
                }
                let group = ChannelGroup { id: Uuid::new(group_id), name: name.to_string(), collapsed: false };
                self.groups.push(GroupRow { user: Uuid::new(user), workspace_id: self.workspace.id.clone(), group: group.clone() });
                changed = true;
                group
            }
        };
        if changed {
            self.publish_channels_updated(user);
        }
        Ok(group)
    }

    pub fn fold_group(&mut self, user: &str, group_id: &str, collapsed: bool) -> AppResult<ChannelGroup> {
        let g = self
            .groups
            .iter_mut()
            .find(|g| g.user.as_str() == user && g.group.id.as_str() == group_id)
            .ok_or_else(|| AppError::new("CHANNEL_GROUP_NOT_FOUND", "No such group."))?;
        let changed = g.group.collapsed != collapsed;
        g.group.collapsed = collapsed;
        let group = g.group.clone();
        if changed {
            self.publish_channels_updated(user);
        }
        Ok(group)
    }

    pub fn remove_group(&mut self, user: &str, group_id: &str) {
        let before = self.groups.len();
        self.groups.retain(|g| !(g.user.as_str() == user && g.group.id.as_str() == group_id));
        if self.groups.len() != before {
            for c in self.channels.values_mut() {
                if let Some(m) = c.members.get_mut(user)
                    && m.group.as_deref() == Some(group_id) {
                        m.group = None;
                    }
            }
            self.publish_channels_updated(user);
        }
    }

    pub fn move_to_group(&mut self, user: &str, channel_id: &str, group_id: Option<&Uuid>) -> AppResult<Channel> {
        let chan = self.visible_channel(channel_id, user)?;
        if chan.kind == ChannelType::DirectMessage || !chan.members.contains_key(user) {
            return Err(AppError::channel_not_found());
        }
        if let Some(g) = group_id
            && !self.groups.iter().any(|r| r.user.as_str() == user && r.group.id == *g) {
                return Err(AppError::new("CHANNEL_GROUP_NOT_FOUND", "No such group."));
            }
        let m = self.joined_channel_mut(user, channel_id)?;
        let changed = m.group.as_ref() != group_id || (group_id.is_some() && m.starred);
        m.group = group_id.cloned();
        if group_id.is_some() {
            m.starred = false;
        }
        if changed {
            self.publish_channels_updated(user);
        }
        Ok(self.channel_view(&self.channels[channel_id], user))
    }

    // --- bookmarks and files -------------------------------------------------------------

    fn bookmark_view(&self, chan: &Chan, b: &ChannelBookmarkRow) -> ChannelBookmark {
        ChannelBookmark {
            id: b.id.clone(),
            channel_id: chan.id.clone(),
            url: b.url.clone(),
            title: b.title.clone(),
            added_by: Some(self.user_view(&b.added_by)),
            added_at: ts(b.added_ms),
        }
    }

    pub fn list_bookmarks(&self, user: &str, channel_id: &str) -> AppResult<BookmarkList> {
        let chan = self.visible_channel(channel_id, user)?;
        Ok(chan.bookmarks.iter().map(|b| self.bookmark_view(chan, b)).collect())
    }

    fn publish_bookmarks(&self, channel_id: &str) {
        let frame = ServerFrame::BookmarksUpdated(BookmarksUpdatedFrame {
            data: BookmarksUpdatedFrameData { channel_id: Uuid::new(channel_id) },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel(channel_id, &frame);
    }

    pub fn add_bookmark(&mut self, user: &str, channel_id: &str, req: &AddBookmarkRequest) -> AppResult<ChannelBookmark> {
        let chan = self.member_channel(channel_id, user)?;
        let url = req.url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2048 {
            return Err(AppError::field("url", "is not an http or https address"));
        }
        let title = req.title.trim();
        if title.is_empty() || title.chars().count() > 120 {
            return Err(AppError::field("title", "is not a title"));
        }
        if let Some(b) = chan.bookmarks.iter().find(|b| b.url == url) {
            return Ok(self.bookmark_view(chan, b));
        }
        if chan.bookmarks.len() >= BOOKMARKS_MAX {
            return Err(AppError::new("BOOKMARKS_LIMIT", "This channel has the most bookmarks it may."));
        }
        let row = ChannelBookmarkRow {
            id: new_id(),
            url: url.to_string(),
            title: title.to_string(),
            added_by: Uuid::new(user),
            added_ms: now_ms(),
        };
        self.channels.get_mut(channel_id).expect("checked").bookmarks.push(row.clone());
        self.publish_bookmarks(channel_id);
        Ok(self.bookmark_view(&self.channels[channel_id], &row))
    }

    pub fn remove_bookmark(&mut self, user: &str, channel_id: &str, bookmark_id: &str) -> AppResult<()> {
        self.member_channel(channel_id, user)?;
        let chan = self.channels.get_mut(channel_id).expect("checked");
        let before = chan.bookmarks.len();
        chan.bookmarks.retain(|b| b.id.as_str() != bookmark_id);
        if chan.bookmarks.len() == before {
            return Err(AppError::new("BOOKMARK_NOT_FOUND", "No such bookmark."));
        }
        self.publish_bookmarks(channel_id);
        Ok(())
    }

    pub fn list_files(&self, user: &str, channel_id: &str, q: &FileQuery) -> AppResult<FilePage> {
        let chan = self.visible_channel(channel_id, user)?;
        let limit = page_limit(q.limit)?;
        // Newest message first, then newest file.
        let mut files: Vec<(&Msg, &Upload)> = self
            .messages
            .values()
            .filter(|m| m.channel_id == chan.id && m.deleted_ms.is_none() && !m.hidden_for.contains_key(user))
            .flat_map(|m| m.attachments.iter().filter_map(|a| self.uploads.get(a)).map(move |u| (m, u)))
            .collect();
        files.sort_by(|a, b| b.0.id.cmp(&a.0.id).then_with(|| b.1.attachment.id.cmp(&a.1.attachment.id)));
        let start = match &q.before {
            Some(b) => match files.iter().position(|(_, u)| u.attachment.id == *b) {
                Some(p) => p + 1,
                None => return Err(AppError::field("before", "is not a file of this channel")),
            },
            None => 0,
        };
        let rest = &files[start.min(files.len())..];
        let chosen: Vec<&(&Msg, &Upload)> = rest.iter().take(limit).collect();
        let next = if rest.len() > limit { chosen.last().map(|(_, u)| u.attachment.id.clone()) } else { None };
        Ok(FilePage {
            data: chosen
                .iter()
                .map(|(m, u)| ChannelFile {
                    attachment: u.attachment.clone(),
                    message_id: m.id.clone(),
                    parent_id: m.parent_id.clone(),
                    sender: self.user_view(m.sender.as_deref().unwrap_or_default()),
                    sent_at: ts(m.created_ms),
                })
                .collect(),
            next_cursor: next,
        })
    }

    /// `GET …/unreads`: the oldest unread roots and the threads waiting for the person.
    pub fn channel_unreads(&self, user: &str, channel_id: &str, q: &CatchUpQuery) -> AppResult<ChannelCatchUp> {
        let chan = self.visible_channel(channel_id, user)?;
        let Some(member) = chan.members.get(user) else { return Err(AppError::channel_not_found()) };
        let limit = match q.limit {
            None => 20,
            Some(l) if (1..=100).contains(&l) => l as usize,
            Some(_) => return Err(AppError::field("limit", "must be 1 to 100")),
        };
        let unread: Vec<Uuid> = chan
            .roots
            .iter()
            .filter(|id| member.last_read.as_ref().is_none_or(|lr| *id > lr))
            .filter(|id| self.messages.get(*id).is_some_and(|m| self.counts_for(m, user)))
            .cloned()
            .collect();
        let more = unread.len() > limit;
        let roots: Vec<Message> =
            unread.iter().take(limit).filter_map(|id| self.message_view_by_id(id, Some(user))).collect();
        let roots_next = if more { roots.last().map(|m| m.id.clone()) } else { None };

        let mut threads: Vec<UnreadThread> = Vec::new();
        for root_id in chan.roots.iter() {
            if q.after_thread.as_ref().is_some_and(|a| root_id <= a) {
                continue;
            }
            let Some(root) = self.messages.get(root_id) else { continue };
            if root.replies.is_empty() || !self.is_following(root, user) {
                continue;
            }
            let read = root.thread_reads.get(user);
            let waiting: Vec<&Msg> = root
                .replies
                .iter()
                .filter(|r| read.is_none_or(|b| *r > b))
                .filter_map(|r| self.messages.get(r))
                .filter(|r| self.counts_for(r, user))
                .collect();
            if let Some(first) = waiting.first() {
                threads.push(UnreadThread {
                    root: self.message_view(root, Some(user)),
                    first_unread_message_id: first.id.clone(),
                    unread_count: waiting.len() as i64,
                });
            }
            if threads.len() > limit {
                break;
            }
        }
        let next_thread = if threads.len() > limit {
            threads.truncate(limit);
            threads.last().map(|t| t.root.id.clone())
        } else {
            None
        };
        Ok(ChannelCatchUp {
            roots: MessagePage { data: roots, next_cursor: roots_next },
            threads,
            next_thread_cursor: next_thread,
        })
    }
}
