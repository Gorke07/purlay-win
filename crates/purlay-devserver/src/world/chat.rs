//! Messages, threads, reactions, pins, history and the changes feed.

use super::*;
use crate::ids::{floor_id, is_plausible, is_v7_shape, new_id, timestamp_of};
use purlay_contract::constants::{
    ATTACHMENTS_PER_MESSAGE_MAX, CHANGES_OVERLAP_MS, MESSAGE_CONTENT_MAX, MESSAGE_VERSIONS_KEPT, MESSAGE_WINDOW_SIDE,
    PINS_MAX, REACTIONS_PER_PERSON_MAX,
};
use std::ops::Bound;

/// What `post` writes; validation is the caller's.
#[derive(Debug, Clone)]
pub struct PostInput {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub sender: Uuid,
    pub content: String,
    pub parent_id: Option<Uuid>,
    pub attachment_ids: Vec<Uuid>,
    pub at_ms: i64,
}

/// The `@names` in a text, lower-case, outside code spans and fenced blocks.
pub fn scan_mentions(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut in_code = false;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '`' {
                in_code = !in_code;
            } else if c == '@' && !in_code && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '-' || chars[j] == '_') {
                    j += 1;
                }
                if j > i + 1 {
                    let name: String = chars[i + 1..j].iter().collect::<String>().to_ascii_lowercase();
                    let name = name.trim_end_matches('-').to_string();
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
                i = j;
                continue;
            }
            i += 1;
        }
    }
    out
}

/// `SendContent`'s rules: at most `MESSAGE_CONTENT_MAX` characters, no NUL, no broadcast
/// mentions, and empty only beside an attachment.
pub fn check_content(content: &str, has_attachments: bool) -> AppResult<()> {
    if content.chars().count() > MESSAGE_CONTENT_MAX as usize {
        return Err(AppError::field("content", "is too long"));
    }
    if content.contains('\u{0}') {
        return Err(AppError::field("content", "contains a character that is not allowed"));
    }
    if content.trim().is_empty() && !has_attachments {
        return Err(AppError::field("content", "is empty"));
    }
    if scan_mentions(content).iter().any(|n| matches!(n.as_str(), "here" | "channel" | "everyone")) {
        return Err(AppError::field("content", "@here, @channel and @everyone are not supported"));
    }
    Ok(())
}

/// One emoji: a short run of characters with no letter, digit, space or punctuation of ASCII.
fn check_emoji(emoji: &str) -> AppResult<()> {
    let n = emoji.chars().count();
    if n == 0 || n > 16 || emoji.chars().any(|c| c.is_ascii() || c.is_whitespace() || c.is_control()) {
        return Err(AppError::field("emoji", "is not an emoji"));
    }
    Ok(())
}

/// Keyset parameters as the routes read them.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub before: Option<Uuid>,
    pub after: Option<Uuid>,
    pub limit: usize,
}

impl Page {
    pub fn from_query(q: &PageQuery) -> AppResult<Page> {
        if q.before.is_some() && q.after.is_some() {
            return Err(AppError::field("before", "before and after cannot be asked together"));
        }
        for (name, v) in [("before", &q.before), ("after", &q.after)] {
            if let Some(v) = v
                && !crate::ids::is_uuid(v) {
                    return Err(AppError::field(name, "is not a UUID"));
                }
        }
        Ok(Page { before: q.before.clone(), after: q.after.clone(), limit: page_limit(q.limit)? })
    }
}

/// Pages a sorted list of ids (oldest first) the way every list route does: no cursor →
/// the newest `limit`, newest first; `before` → older, newest first; `after` → newer,
/// oldest first. `oldest_first_default` flips the no-cursor case (threads).
pub fn page_ids(ids: &[Uuid], page: &Page, oldest_first_default: bool) -> (Vec<Uuid>, Option<Uuid>) {
    let limit = page.limit.max(1);
    if let Some(after) = &page.after {
        let newer: Vec<Uuid> = ids.iter().filter(|i| *i > after).cloned().collect();
        let more = newer.len() > limit;
        let out: Vec<Uuid> = newer.into_iter().take(limit).collect();
        let next = if more { out.last().cloned() } else { None };
        return (out, next);
    }
    if page.before.is_none() && oldest_first_default {
        let more = ids.len() > limit;
        let out: Vec<Uuid> = ids.iter().take(limit).cloned().collect();
        let next = if more { out.last().cloned() } else { None };
        return (out, next);
    }
    let older: Vec<Uuid> = match &page.before {
        Some(b) => ids.iter().filter(|i| *i < b).cloned().collect(),
        None => ids.to_vec(),
    };
    let more = older.len() > limit;
    let out: Vec<Uuid> = older.into_iter().rev().take(limit).collect();
    let next = if more { out.last().cloned() } else { None };
    (out, next)
}

impl World {
    // --- following -----------------------------------------------------------------------

    pub fn is_following(&self, root: &Msg, user: &str) -> bool {
        if let Some(choice) = root.follows.get(user) {
            return *choice;
        }
        root.sender.as_deref() == Some(user)
            || root.replies.iter().filter_map(|r| self.messages.get(r)).any(|r| r.sender.as_deref() == Some(user))
    }

    fn note(&mut self, user: &Uuid, kind: NotificationType, channel_id: &Uuid, message_id: &Uuid, at_ms: i64) {
        self.notes.push(Note {
            id: new_id(),
            user: user.clone(),
            workspace_id: self.workspace.id.clone(),
            kind,
            channel_id: channel_id.clone(),
            message_id: message_id.clone(),
            created_ms: at_ms,
            read_ms: None,
        });
    }

    fn resolve_mentions(&self, content: &str) -> Vec<Uuid> {
        scan_mentions(content).iter().filter_map(|n| self.person_by_username(n)).map(|p| p.id.clone()).collect()
    }

    // --- writing -------------------------------------------------------------------------

    /// Writes a message with every side effect: thread counters, mentions, notifications,
    /// read positions and the frames. No validation: `send_message` and the seed call it.
    pub fn post(&mut self, input: PostInput) -> Uuid {
        let PostInput { id, channel_id, sender, content, parent_id, attachment_ids, at_ms } = input;
        let mentions = self.resolve_mentions(&content);
        for a in &attachment_ids {
            if let Some(u) = self.uploads.get_mut(a) {
                u.message_id = Some(id.clone());
                u.reserved = false;
            }
        }
        self.messages.insert(
            id.clone(),
            Msg {
                id: id.clone(),
                channel_id: channel_id.clone(),
                sender: Some(sender.clone()),
                content,
                created_ms: at_ms,
                edited_ms: None,
                deleted_ms: None,
                parent_id: parent_id.clone(),
                replies: BTreeSet::new(),
                resolved: None,
                reactions: Vec::new(),
                mentions: mentions.clone(),
                attachments: attachment_ids,
                version: 1,
                history: Vec::new(),
                hidden_for: HashMap::new(),
                changed_ms: at_ms.max(now_ms()),
                follows: HashMap::new(),
                thread_reads: HashMap::new(),
                knowledge: None,
            },
        );
        let chan = self.channels.get_mut(&channel_id).expect("channel of a post");
        chan.last_message_ms = Some(chan.last_message_ms.unwrap_or(0).max(at_ms));
        match &parent_id {
            Some(p) => {
                if let Some(root) = self.messages.get_mut(p) {
                    root.replies.insert(id.clone());
                    root.version += 1;
                    root.changed_ms = now_ms();
                    // A reply read by its author.
                    root.thread_reads.insert(sender.clone(), id.clone());
                }
            }
            None => {
                chan.roots.insert(id.clone());
                if let Some(m) = chan.members.get_mut(&sender)
                    && m.last_read.as_ref().is_none_or(|lr| *lr < id) {
                        m.last_read = Some(id.clone());
                    }
            }
        }

        // The room hears it.
        if let Some(view) = self.message_view_by_id(&id, None) {
            let frame = ServerFrame::MessageCreated(MessageCreatedFrame { data: view, ts: now_ts(), ref_id: None });
            self.publish_channel(&channel_id, &frame);
        }
        if let Some(p) = &parent_id {
            self.publish_thread(p);
        }

        // Who is told.
        let chan = &self.channels[&channel_id];
        let members: Vec<Uuid> = chan.members.keys().cloned().collect();
        let is_dm = chan.kind == ChannelType::DirectMessage;
        let mut told: Vec<Uuid> = Vec::new();
        for m in &mentions {
            if *m != sender && members.contains(m) && !told.contains(m) {
                told.push(m.clone());
                self.note(m, NotificationType::Mention, &channel_id, &id, at_ms);
            }
        }
        if is_dm && parent_id.is_none() {
            for m in &members {
                if *m != sender && !told.contains(m) {
                    told.push(m.clone());
                    self.note(m, NotificationType::DirectMessage, &channel_id, &id, at_ms);
                }
            }
        }
        if let Some(p) = &parent_id {
            let followers: Vec<Uuid> = match self.messages.get(p) {
                Some(root) => members.iter().filter(|u| self.is_following(root, u)).cloned().collect(),
                None => Vec::new(),
            };
            for f in followers {
                if f != sender && !told.contains(&f) {
                    told.push(f.clone());
                    self.note(&f, NotificationType::ThreadReply, &channel_id, &id, at_ms);
                }
            }
        }
        for u in &told {
            self.publish_notifications_updated(u);
        }
        if parent_id.is_none() {
            for m in &members {
                self.publish_channel_state(&channel_id, m);
            }
        }
        id
    }

    /// `sendMessage` and `MESSAGE_SEND`.
    pub fn send_message(
        &mut self,
        user: &str,
        channel_id: &str,
        id: &str,
        content: &str,
        parent_id: Option<&Uuid>,
        attachment_ids: &[Uuid],
    ) -> AppResult<Message> {
        let chan = self.member_channel(channel_id, user)?;
        if !is_v7_shape(id) {
            return Err(AppError::new("INVALID_MESSAGE_ID", "The message id is not a UUIDv7."));
        }
        if let Some(existing) = self.messages.get(id) {
            if existing.sender.as_deref() == Some(user) && existing.channel_id.as_str() == channel_id {
                // A retry: tell everything again from the row as it is.
                let view = self.message_view(existing, Some(user));
                let frame =
                    ServerFrame::MessageCreated(MessageCreatedFrame { data: self.message_view(existing, None), ts: now_ts(), ref_id: None });
                self.publish_channel(channel_id, &frame);
                if let Some(p) = existing.parent_id.clone() {
                    self.publish_thread(&p);
                }
                return Ok(view);
            }
            return Err(AppError::new("MESSAGE_ID_CONFLICT", "That message id is taken."));
        }
        if !is_plausible(id, now_ms()) {
            return Err(AppError::new("INVALID_MESSAGE_ID", "The message id's time is too far from the server's clock."));
        }
        check_content(content, !attachment_ids.is_empty())?;
        if parent_id.is_none() && chan.only_admins_post && !self.is_admin(user) {
            return Err(AppError::new("ONLY_ADMINS_POST", "Only owners and admins post here."));
        }
        if let Some(p) = parent_id {
            match self.messages.get(p) {
                Some(root) if root.channel_id.as_str() == channel_id => {
                    if root.parent_id.is_some() {
                        return Err(AppError::field("parentId", "is a reply"));
                    }
                }
                _ => return Err(AppError::message_not_found()),
            }
        }
        if attachment_ids.len() > ATTACHMENTS_PER_MESSAGE_MAX as usize {
            return Err(AppError::field("attachmentIds", "has too many attachments"));
        }
        for a in attachment_ids {
            match self.uploads.get(a) {
                Some(u) if u.uploader.as_str() == user && u.message_id.is_none() && !u.reserved => {}
                _ => return Err(AppError::new("ATTACHMENT_NOT_FOUND", "No such upload.")),
            }
        }
        let id = self.post(PostInput {
            id: Uuid::new(id),
            channel_id: Uuid::new(channel_id),
            sender: Uuid::new(user),
            content: content.to_string(),
            parent_id: parent_id.cloned(),
            attachment_ids: attachment_ids.to_vec(),
            at_ms: timestamp_of(id).unwrap_or_else(now_ms),
        });
        Ok(self.message_view_by_id(&id, Some(user)).expect("just posted"))
    }

    pub fn edit_message(&mut self, user: &str, message_id: &str, content: &str) -> AppResult<Message> {
        let m = self.readable_message(message_id, user, false)?;
        if m.sender.as_deref() != Some(user) {
            return Err(AppError::forbidden());
        }
        if !self.channels[&m.channel_id].members.contains_key(user) {
            return Err(AppError::not_a_member());
        }
        check_content(content, !m.attachments.is_empty())?;
        if m.content == content {
            return Ok(self.message_view(m, Some(user)));
        }
        let before: Vec<Uuid> = m.mentions.clone();
        let mentions = self.resolve_mentions(content);
        let now = now_ms();
        let m = self.messages.get_mut(message_id).expect("checked");
        let old = std::mem::replace(&mut m.content, content.to_string());
        m.history.push(MessageVersion { id: new_id(), content: old, replaced_at: ts(now) });
        if m.history.len() > MESSAGE_VERSIONS_KEPT as usize {
            m.history.remove(0);
        }
        m.edited_ms = Some(now);
        m.mentions = mentions.clone();
        m.version += 1;
        m.changed_ms = now;
        let (channel_id, id) = (m.channel_id.clone(), m.id.clone());
        self.publish_revised(message_id);
        let frame = ServerFrame::MessageVersionsUpdated(MessageVersionsUpdatedFrame {
            data: MessageVersionsUpdatedFrameData { channel_id: channel_id.clone(), message_id: id.clone() },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel(&channel_id, &frame);
        let members: Vec<Uuid> = self.channels[&channel_id].members.keys().cloned().collect();
        for u in mentions.iter().filter(|u| !before.contains(u) && u.as_str() != user && members.contains(u)) {
            self.note(u, NotificationType::Mention, &channel_id, &id, now);
            self.publish_notifications_updated(u);
            self.publish_channel_state(&channel_id, u);
        }
        Ok(self.message_view_by_id(message_id, Some(user)).expect("exists"))
    }

    pub fn delete_message(&mut self, user: &str, message_id: &str) -> AppResult<()> {
        let m = self.readable_message(message_id, user, true)?;
        if m.deleted_ms.is_some() {
            return Ok(());
        }
        if m.sender.as_deref() != Some(user) && !self.is_admin(user) {
            return Err(AppError::forbidden());
        }
        let now = now_ms();
        let m = self.messages.get_mut(message_id).expect("checked");
        m.deleted_ms = Some(now);
        m.content.clear();
        m.history.clear();
        m.mentions.clear();
        m.version += 1;
        m.changed_ms = now;
        let (channel_id, parent) = (m.channel_id.clone(), m.parent_id.clone());
        self.publish_revised(message_id);
        let chan = self.channels.get_mut(&channel_id).expect("channel");
        let pinned = chan.pins.iter().any(|p| p.message_id.as_str() == message_id);
        chan.pins.retain(|p| p.message_id.as_str() != message_id);
        if pinned {
            self.publish_pins(&channel_id);
        }
        match parent {
            Some(p) => {
                if let Some(root) = self.messages.get_mut(&p) {
                    root.version += 1;
                    root.changed_ms = now;
                }
                self.publish_thread(&p);
            }
            None => {
                let members: Vec<Uuid> = self.channels[&channel_id].members.keys().cloned().collect();
                for u in members {
                    self.publish_channel_state(&channel_id, &u);
                }
            }
        }
        Ok(())
    }

    /// Delete for me (`PUT /api/messages/:id/hidden`).
    pub fn hide_message(&mut self, user: &str, message_id: &str) -> AppResult<Message> {
        let m = self.readable_message(message_id, user, true)?;
        let (channel_id, id) = (m.channel_id.clone(), m.id.clone());
        let now = now_ms();
        let first = !m.hidden_for.contains_key(user);
        if first {
            let m = self.messages.get_mut(message_id).expect("checked");
            m.hidden_for.insert(Uuid::new(user), now);
            let saves = self.saved.len();
            self.saved.retain(|s| !(s.user.as_str() == user && s.message_id.as_str() == message_id));
            if self.saved.len() != saves {
                self.publish_saved_updated(user);
            }
            let reminders = self.reminders.len();
            self.reminders.retain(|r| !(r.user.as_str() == user && r.message_id.as_str() == message_id));
            if self.reminders.len() != reminders {
                self.publish_reminders_updated(user);
            }
            for n in self.notes.iter_mut().filter(|n| n.user.as_str() == user && n.message_id.as_str() == message_id) {
                n.read_ms.get_or_insert(now);
            }
            let frame = ServerFrame::MessageHidden(MessageHiddenFrame {
                data: MessageHiddenFrameData {
                    workspace_id: self.workspace.id.clone(),
                    channel_id: channel_id.clone(),
                    message_id: id,
                    hidden_at: ts(now),
                },
                ts: now_ts(),
                ref_id: None,
            });
            self.publish_user(user, &frame);
            self.publish_channel_state(&channel_id, user);
        }
        Ok(self.message_view_by_id(message_id, Some(user)).expect("exists"))
    }

    pub fn put_reaction(&mut self, user: &str, message_id: &str, emoji: &str, add: bool) -> AppResult<ReactionsResponse> {
        let m = self.readable_message(message_id, user, false)?;
        if !self.channels[&m.channel_id].members.contains_key(user) {
            return Err(AppError::not_a_member());
        }
        check_emoji(emoji)?;
        let mine = m.reactions.iter().filter(|(_, us)| us.iter().any(|u| u.as_str() == user)).count();
        let has = m.reactions.iter().any(|(e, us)| e == emoji && us.iter().any(|u| u.as_str() == user));
        let kind_exists = m.reactions.iter().any(|(e, _)| e == emoji);
        let kinds = m.reactions.len();
        let changed = if add {
            if has {
                false
            } else if mine >= REACTIONS_PER_PERSON_MAX as usize {
                return Err(AppError::field("emoji", "you have given the most reactions you may on this message"));
            } else if !kind_exists && kinds >= 20 {
                return Err(AppError::field("emoji", "this message has the most kinds of reaction it may"));
            } else {
                let m = self.messages.get_mut(message_id).expect("checked");
                match m.reactions.iter_mut().find(|(e, _)| e == emoji) {
                    Some((_, us)) => us.push(Uuid::new(user)),
                    None => m.reactions.push((emoji.to_string(), vec![Uuid::new(user)])),
                }
                true
            }
        } else if has {
            let m = self.messages.get_mut(message_id).expect("checked");
            for (_, us) in m.reactions.iter_mut() {
                us.retain(|u| !(u.as_str() == user));
            }
            m.reactions.retain(|(e, us)| !(e == emoji && us.is_empty()));
            true
        } else {
            false
        };
        if changed {
            let m = self.messages.get_mut(message_id).expect("checked");
            m.version += 1;
            m.changed_ms = now_ms();
            self.publish_reactions(message_id);
        }
        let m = &self.messages[message_id];
        Ok(ReactionsResponse {
            reactions: m.reactions.iter().map(|(e, u)| Reaction { emoji: e.clone(), user_ids: u.clone() }).collect(),
        })
    }

    fn thread_root(&self, user: &str, message_id: &str) -> AppResult<&Msg> {
        let m = self.readable_message(message_id, user, true)?;
        if m.parent_id.is_some() {
            return Err(AppError::message_not_found());
        }
        Ok(m)
    }

    pub fn resolve_thread(&mut self, user: &str, message_id: &str, resolve: bool) -> AppResult<Message> {
        let m = self.readable_message(message_id, user, false)?;
        if m.parent_id.is_some() || m.replies.is_empty() {
            return Err(AppError::new("THREAD_NOT_RESOLVABLE", "Only a thread with replies can be resolved."));
        }
        if !self.channels[&m.channel_id].members.contains_key(user) {
            return Err(AppError::not_a_member());
        }
        if m.resolved.is_some() == resolve {
            return Ok(self.message_view(m, Some(user)));
        }
        let now = now_ms();
        let followers: Vec<Uuid> = {
            let members: Vec<Uuid> = self.channels[&m.channel_id].members.keys().cloned().collect();
            members.into_iter().filter(|u| u.as_str() != user && self.is_following(m, u)).collect()
        };
        let channel_id = m.channel_id.clone();
        let root = self.messages.get_mut(message_id).expect("checked");
        root.resolved = if resolve { Some((now, Uuid::new(user))) } else { None };
        root.version += 1;
        root.changed_ms = now;
        self.publish_thread(message_id);
        if resolve {
            let id = Uuid::new(message_id);
            for f in followers {
                self.note(&f, NotificationType::ThreadResolved, &channel_id, &id, now);
                self.publish_notifications_updated(&f);
            }
        }
        Ok(self.message_view_by_id(message_id, Some(user)).expect("exists"))
    }

    pub fn set_knowledge(&mut self, user: &str, message_id: &str, req: &SetKnowledgeRequest) -> AppResult<Message> {
        let m = self.readable_message(message_id, user, false)?;
        if m.parent_id.is_some() {
            return Err(AppError::message_not_found());
        }
        let chan = &self.channels[&m.channel_id];
        if !chan.members.contains_key(user) {
            return Err(AppError::not_a_member());
        }
        let current = m.knowledge.as_ref().map(|k| k.version).unwrap_or(0);
        let accepted = match &req.accepted_answer_id {
            Some(a) => match self.messages.get(a) {
                Some(r) if r.parent_id.as_deref() == Some(message_id) && r.deleted_ms.is_none() => {
                    Some(ThreadSummaryKnowledgeAcceptedAnswer { id: r.id.clone(), content: r.content.chars().take(800).collect() })
                }
                _ => return Err(AppError::field("acceptedAnswerId", "is not a reply of this thread")),
            },
            None => None,
        };
        if let Some(a) = &req.assignee_id
            && !chan.members.contains_key(a) {
                return Err(AppError::field("assigneeId", "is not a member of the channel"));
            }
        let next = ThreadSummaryKnowledge {
            version: current + 1,
            is_question: req.is_question,
            assignee_id: req.assignee_id.clone(),
            accepted_answer: accepted,
            decision_note: req.decision_note.clone(),
            decision_url: req.decision_url.clone(),
        };
        if req.version != current {
            // An identical retry of the write that made `current` succeeds.
            let same = m.knowledge.as_ref().is_some_and(|k| {
                req.version + 1 == current
                    && k.is_question == next.is_question
                    && k.assignee_id == next.assignee_id
                    && k.decision_note == next.decision_note
                    && k.decision_url == next.decision_url
                    && k.accepted_answer.as_ref().map(|a| &a.id) == next.accepted_answer.as_ref().map(|a| &a.id)
            });
            if same {
                return Ok(self.message_view(m, Some(user)));
            }
            return Err(AppError::new("KNOWLEDGE_CONFLICT", "Someone changed this thread meanwhile."));
        }
        let root = self.messages.get_mut(message_id).expect("checked");
        root.knowledge = Some(next);
        root.version += 1;
        root.changed_ms = now_ms();
        self.publish_thread(message_id);
        Ok(self.message_view_by_id(message_id, Some(user)).expect("exists"))
    }

    // --- reading -------------------------------------------------------------------------

    /// The roots a person's feed shows, oldest first.
    fn feed_roots(&self, chan: &Chan, user: &str) -> Vec<Uuid> {
        chan.roots
            .iter()
            .filter(|id| {
                let Some(m) = self.messages.get(*id) else { return false };
                let gone = m.deleted_ms.is_some() || m.hidden_for.contains_key(user);
                !gone || !m.replies.is_empty()
            })
            .cloned()
            .collect()
    }

    pub fn list_messages(&self, user: &str, channel_id: &str, q: &PageQuery) -> AppResult<MessagePage> {
        let chan = self.visible_channel(channel_id, user)?;
        let page = Page::from_query(q)?;
        let roots = self.feed_roots(chan, user);
        let (ids, next) = page_ids(&roots, &page, false);
        Ok(MessagePage {
            data: ids.iter().filter_map(|id| self.message_view_by_id(id, Some(user))).collect(),
            next_cursor: next,
        })
    }

    pub fn get_message(&self, user: &str, message_id: &str) -> AppResult<Message> {
        let m = self.readable_message(message_id, user, false)?;
        Ok(self.message_view(m, Some(user)))
    }

    pub fn messages_around(&self, user: &str, message_id: &str) -> AppResult<MessageWindow> {
        let target = self.readable_message(message_id, user, false)?;
        let root_id = target.parent_id.clone().unwrap_or_else(|| target.id.clone());
        let chan = &self.channels[&target.channel_id];
        let mut roots = self.feed_roots(chan, user);
        if !roots.contains(&root_id) {
            roots.push(root_id.clone());
            roots.sort();
        }
        let pos = roots.iter().position(|r| *r == root_id).expect("present");
        let side = MESSAGE_WINDOW_SIDE as usize;
        let lo = pos.saturating_sub(side);
        let hi = (pos + side + 1).min(roots.len());
        let window: Vec<&Uuid> = roots[lo..hi].iter().rev().collect();
        Ok(MessageWindow {
            target: self.message_view(target, Some(user)),
            data: window.iter().filter_map(|id| self.message_view_by_id(id, Some(user))).collect(),
            older_cursor: if lo > 0 { Some(roots[lo].clone()) } else { None },
            newer_cursor: if hi < roots.len() { Some(roots[hi - 1].clone()) } else { None },
        })
    }

    pub fn get_thread(&self, user: &str, message_id: &str, q: &PageQuery) -> AppResult<MessagePage> {
        let root = self.thread_root(user, message_id)?;
        let page = Page::from_query(q)?;
        let replies: Vec<Uuid> = root
            .replies
            .iter()
            .filter(|r| self.messages.get(*r).is_some_and(|m| !m.hidden_for.contains_key(user)))
            .cloned()
            .collect();
        let (ids, next) = page_ids(&replies, &page, true);
        Ok(MessagePage {
            data: ids.iter().filter_map(|id| self.message_view_by_id(id, Some(user))).collect(),
            next_cursor: next,
        })
    }

    pub fn mark_thread_read(&mut self, user: &str, message_id: &str, bound: &str) -> AppResult<()> {
        let root = self.thread_root(user, message_id)?;
        if !root.replies.contains(bound) {
            return Err(AppError::message_not_found());
        }
        let channel_id = root.channel_id.clone();
        let newer = root.thread_reads.get(user).is_none_or(|r| r.as_str() < bound);
        let replies: Vec<Uuid> =
            root.replies.range::<Uuid, _>((Bound::Unbounded, Bound::Included(&Uuid::new(bound)))).cloned().collect();
        let now = now_ms();
        let mut changed = false;
        for n in self.notes.iter_mut().filter(|n| n.user.as_str() == user && n.read_ms.is_none()) {
            if replies.contains(&n.message_id) {
                n.read_ms = Some(now);
                changed = true;
            }
        }
        if newer {
            self.messages.get_mut(message_id).expect("checked").thread_reads.insert(Uuid::new(user), Uuid::new(bound));
            changed = true;
        }
        if changed {
            self.publish_channel_state(&channel_id, user);
        }
        Ok(())
    }

    pub fn thread_follow(&self, user: &str, message_id: &str) -> AppResult<ThreadFollow> {
        let root = self.thread_root(user, message_id)?;
        Ok(ThreadFollow { following: self.is_following(root, user) })
    }

    pub fn set_thread_follow(&mut self, user: &str, message_id: &str, following: bool) -> AppResult<ThreadFollow> {
        self.thread_root(user, message_id)?;
        self.messages.get_mut(message_id).expect("checked").follows.insert(Uuid::new(user), following);
        Ok(ThreadFollow { following })
    }

    pub fn message_versions(&self, user: &str, message_id: &str, q: &OlderPageQuery) -> AppResult<MessageVersionPage> {
        let m = self.readable_message(message_id, user, false)?;
        let page = Page::from_query(&PageQuery { before: q.before.clone(), after: None, limit: q.limit })?;
        let ids: Vec<Uuid> = m.history.iter().map(|v| v.id.clone()).collect();
        let (chosen, next) = page_ids(&ids, &page, false);
        Ok(MessageVersionPage {
            data: chosen.iter().filter_map(|id| m.history.iter().find(|v| v.id == *id).cloned()).collect(),
            next_cursor: next,
        })
    }

    /// `GET …/changes`: messages of the channel by when the server last wrote them.
    /// The cursor is `<changedMs>.<messageId>.<asOfMs>`.
    pub fn list_changes(&self, user: &str, channel_id: &str, q: &ChangesQuery) -> AppResult<ChangePage> {
        let chan = self.visible_channel(channel_id, user)?;
        let limit = page_limit(q.limit)?;
        let now = now_ms();
        let (from, as_of): ((i64, Option<Uuid>), i64) = match (&q.since, &q.after) {
            (Some(_), Some(_)) => return Err(AppError::field("after", "since and after cannot be asked together")),
            (None, None) => return Ok(ChangePage { data: Vec::new(), next_cursor: None, as_of: ts(now) }),
            (Some(since), None) => {
                let s = since.unix_ms().ok_or_else(|| AppError::field("since", "is not a time"))?;
                ((s - CHANGES_OVERLAP_MS, None), now)
            }
            (None, Some(cursor)) => {
                let parts: Vec<&str> = cursor.split('.').collect();
                let ok = parts.len() == 3
                    && parts[0].len() == 13
                    && parts[2].len() == 13
                    && crate::ids::is_uuid(parts[1]);
                let (Some(changed), Some(as_of)) =
                    (parts.first().and_then(|p| p.parse::<i64>().ok()), parts.get(2).and_then(|p| p.parse::<i64>().ok()))
                else {
                    return Err(AppError::field("after", "is not a cursor of this server"));
                };
                if !ok {
                    return Err(AppError::field("after", "is not a cursor of this server"));
                }
                ((changed, Some(Uuid::new(parts[1]))), as_of)
            }
        };
        let mut rows: Vec<(i64, Uuid)> = self
            .messages
            .values()
            .filter(|m| m.channel_id.as_str() == chan.id.as_str() && m.changed_ms <= as_of)
            .filter(|m| match &from.1 {
                None => m.changed_ms >= from.0,
                Some(id) => (m.changed_ms, &m.id) > (from.0, id),
            })
            .map(|m| (m.changed_ms, m.id.clone()))
            .collect();
        rows.sort();
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next = if more { rows.last().map(|(c, id)| format!("{c:013}.{id}.{as_of:013}")) } else { None };
        Ok(ChangePage {
            data: rows.iter().filter_map(|(_, id)| self.message_view_by_id(id, Some(user))).collect(),
            next_cursor: next,
            as_of: ts(as_of),
        })
    }

    /// Roots of a channel newer than `REPLAY_OVERLAP_MS` before `after`'s time, oldest first,
    /// at most `cap`; the cursor is the last one when it was capped.
    pub fn replay(&self, channel_id: &str, user: &str, after: &Uuid, cap: usize) -> (Vec<Message>, Option<Uuid>) {
        let Some(chan) = self.channels.get(channel_id) else { return (Vec::new(), None) };
        let from = timestamp_of(after).unwrap_or(0) - purlay_contract::constants::REPLAY_OVERLAP_MS;
        let floor = floor_id(from);
        let ids: Vec<&Uuid> = chan
            .roots
            .range::<Uuid, _>((Bound::Excluded(&floor), Bound::Unbounded))
            .filter(|id| self.messages.get(*id).is_some_and(|m| m.deleted_ms.is_none() && !m.hidden_for.contains_key(user)))
            .collect();
        let capped = ids.len() > cap;
        let msgs: Vec<Message> = ids.iter().take(cap).filter_map(|id| self.message_view_by_id(id, Some(user))).collect();
        let next = if capped { msgs.last().map(|m| m.id.clone()) } else { None };
        (msgs, next)
    }

    // --- pins ----------------------------------------------------------------------------

    pub fn publish_pins(&self, channel_id: &str) {
        let frame = ServerFrame::PinsUpdated(PinsUpdatedFrame {
            data: PinsUpdatedFrameData { channel_id: Uuid::new(channel_id) },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_channel(channel_id, &frame);
    }

    pub fn list_pins(&self, user: &str, channel_id: &str) -> AppResult<PinList> {
        let chan = self.visible_channel(channel_id, user)?;
        let mut pins: Vec<&Pin> = chan.pins.iter().collect();
        pins.sort_by_key(|p| std::cmp::Reverse(p.at_ms));
        Ok(pins
            .into_iter()
            .filter_map(|p| {
                let m = self.messages.get(&p.message_id).filter(|m| m.deleted_ms.is_none())?;
                Some(PinnedMessage {
                    message: self.message_view(m, Some(user)),
                    pinned_by: Some(self.user_view(&p.by)),
                    pinned_at: ts(p.at_ms),
                })
            })
            .collect())
    }

    pub fn set_pin(&mut self, user: &str, message_id: &str, pin: bool) -> AppResult<()> {
        let m = self.readable_message(message_id, user, !pin)?;
        let channel_id = m.channel_id.clone();
        let chan = self.channels.get_mut(&channel_id).expect("channel");
        if !chan.members.contains_key(user) {
            return Err(AppError::not_a_member());
        }
        let has = chan.pins.iter().any(|p| p.message_id.as_str() == message_id);
        if pin == has {
            return Ok(());
        }
        if pin {
            if chan.pins.len() >= PINS_MAX as usize {
                return Err(AppError::new("PINS_LIMIT", "This channel has the most pins it may."));
            }
            chan.pins.push(Pin { message_id: Uuid::new(message_id), by: Uuid::new(user), at_ms: now_ms() });
        } else {
            chan.pins.retain(|p| p.message_id.as_str() != message_id);
        }
        self.publish_pins(&channel_id);
        Ok(())
    }

    pub fn preview_link(&self, user: &str, message_id: &str, url: &str) -> AppResult<Option<WebLinkPreview>> {
        let m = self.readable_message(message_id, user, false)?;
        if m.hidden_for.contains_key(user) {
            return Err(AppError::message_not_found());
        }
        if !url.starts_with("https://") || !m.content.contains(url) {
            return Ok(None);
        }
        let host = url.trim_start_matches("https://").split(['/', '?', '#']).next().unwrap_or_default();
        if host.is_empty() || host.eq_ignore_ascii_case("github.com") {
            return Ok(None);
        }
        Ok(Some(WebLinkPreview {
            title: format!("{host} — preview"),
            description: Some("A link preview made up by the Purlay dev server.".into()),
            image_url: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_skip_code() {
        let m = scan_mentions("hi @Ada and @grace-h, see `@linus` and\n```\n@mira\n```\nmail me@x.com @here");
        assert_eq!(m, vec!["ada", "grace-h", "here"]);
    }

    #[test]
    fn paging() {
        let ids: Vec<Uuid> = (1..=7).map(|i| Uuid::new(format!("{i}"))).collect();
        let p = |before: Option<&str>, after: Option<&str>, limit| Page {
            before: before.map(Uuid::new),
            after: after.map(Uuid::new),
            limit,
        };
        let (got, next) = page_ids(&ids, &p(None, None, 3), false);
        assert_eq!(got, vec![Uuid::new("7"), Uuid::new("6"), Uuid::new("5")]);
        assert_eq!(next, Some(Uuid::new("5")));
        let (got, next) = page_ids(&ids, &p(Some("3"), None, 3), false);
        assert_eq!(got, vec![Uuid::new("2"), Uuid::new("1")]);
        assert_eq!(next, None);
        let (got, next) = page_ids(&ids, &p(None, Some("2"), 3), false);
        assert_eq!(got, vec![Uuid::new("3"), Uuid::new("4"), Uuid::new("5")]);
        assert_eq!(next, Some(Uuid::new("5")));
    }
}
