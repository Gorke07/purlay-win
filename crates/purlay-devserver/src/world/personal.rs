//! What is the person's own: saved messages, reminders, scheduled messages, notifications,
//! the inbox, searches; and the issues module and uploads.

use super::chat::{Page, PostInput, check_content, page_ids};
use super::*;
use crate::ids::{is_uuid, new_id};
use purlay_contract::constants::{
    ATTACHMENT_BYTES_MAX, MESSAGE_REMINDERS_MAX, PENDING_UPLOADS_MAX, SAVED_MAX, SCHEDULE_AHEAD_MAX_DAYS,
    SCHEDULED_MESSAGES_MAX, SEARCH_QUERY_MAX, SEARCH_QUERY_MIN, THREAD_EXCERPT_MAX,
};

const FIND_HITS_MAX: usize = 10;
const SAVED_SEARCHES_MAX: usize = 50;

/// A time a reminder or a scheduled message may be set for: 5 s to 365 days ahead.
fn check_ahead(field: &str, at: &Timestamp) -> AppResult<i64> {
    let ms = at.unix_ms().ok_or_else(|| AppError::field(field, "is not a time"))?;
    let now = now_ms();
    if ms < now + 5_000 || ms > now + SCHEDULE_AHEAD_MAX_DAYS * 86_400_000 {
        return Err(AppError::field(field, "must be between five seconds and 365 days ahead"));
    }
    Ok(ms)
}

impl World {
    // --- saved ---------------------------------------------------------------------------

    pub fn publish_saved_updated(&self, user: &str) {
        let frame = ServerFrame::SavedUpdated(SavedUpdatedFrame {
            data: SavedUpdatedFrameData { workspace_id: self.workspace.id.clone() },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    pub fn save_message(&mut self, user: &str, message_id: &str, state: Option<SaveMessageRequestState>) -> AppResult<()> {
        let m = self.readable_message(message_id, user, false)?;
        if m.hidden_for.contains_key(user) {
            return Err(AppError::message_not_found());
        }
        let state = state.map(|s| SavedState::parse(s.as_str()));
        if let Some(SavedState::Unknown(_)) = state {
            return Err(AppError::field("state", "is not a saved state"));
        }
        let changed = match self.saved.iter_mut().find(|s| s.user.as_str() == user && s.message_id.as_str() == message_id) {
            Some(row) => match state {
                Some(s) if s != row.state => {
                    row.state = s;
                    true
                }
                _ => false,
            },
            None => {
                if self.saved.iter().filter(|s| s.user.as_str() == user).count() >= SAVED_MAX as usize {
                    return Err(AppError::new("SAVED_LIMIT", "You have saved the most you may."));
                }
                self.saved.push(SavedRow {
                    id: new_id(),
                    user: Uuid::new(user),
                    message_id: Uuid::new(message_id),
                    state: state.unwrap_or(SavedState::InProgress),
                    saved_ms: now_ms(),
                });
                true
            }
        };
        if changed {
            self.publish_saved_updated(user);
        }
        Ok(())
    }

    pub fn unsave_message(&mut self, user: &str, message_id: &str) {
        let before = self.saved.len();
        self.saved.retain(|s| !(s.user.as_str() == user && s.message_id.as_str() == message_id));
        if self.saved.len() != before {
            self.publish_saved_updated(user);
        }
    }

    /// Saves whose message the person can still see.
    fn visible_saves(&self, user: &str) -> Vec<&SavedRow> {
        self.saved
            .iter()
            .filter(|s| s.user.as_str() == user)
            .filter(|s| {
                self.readable_message(&s.message_id, user, false).is_ok_and(|m| !m.hidden_for.contains_key(user))
            })
            .collect()
    }

    pub fn list_saved(&self, user: &str, q: &SavedQuery) -> AppResult<SavedPage> {
        let page = Page::from_query(&PageQuery { before: q.before.clone(), after: None, limit: q.limit })?;
        let rows: Vec<&SavedRow> = self.visible_saves(user).into_iter().filter(|s| s.state == q.state).collect();
        let mut ids: Vec<Uuid> = rows.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        let (chosen, next) = page_ids(&ids, &page, false);
        Ok(SavedPage {
            data: chosen
                .iter()
                .filter_map(|id| rows.iter().find(|s| s.id == *id))
                .map(|s| SavedItem {
                    id: s.id.clone(),
                    state: s.state.clone(),
                    saved_at: ts(s.saved_ms),
                    message: self.message_view_by_id(&s.message_id, Some(user)).expect("visible"),
                })
                .collect(),
            next_cursor: next,
        })
    }

    pub fn saved_ids(&self, user: &str) -> SavedIds {
        self.visible_saves(user)
            .iter()
            .map(|s| SavedIdsItemItem {
                message_id: s.message_id.clone(),
                channel_id: self.messages[&s.message_id].channel_id.clone(),
                state: s.state.clone(),
            })
            .collect()
    }

    // --- reminders -----------------------------------------------------------------------

    pub fn publish_reminders_updated(&self, user: &str) {
        let frame = ServerFrame::RemindersUpdated(RemindersUpdatedFrame {
            data: RemindersUpdatedFrameData { workspace_id: self.workspace.id.clone() },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    fn reminder_view(&self, r: &ReminderRow, user: &str) -> Option<MessageReminder> {
        Some(MessageReminder {
            id: r.id.clone(),
            message: self.message_view_by_id(&r.message_id, Some(user))?,
            remind_at: ts(r.remind_ms),
            created_at: ts(r.created_ms),
        })
    }

    pub fn set_reminder(&mut self, user: &str, message_id: &str, at: &Timestamp) -> AppResult<MessageReminder> {
        let m = self.readable_message(message_id, user, false)?;
        if m.hidden_for.contains_key(user) {
            return Err(AppError::message_not_found());
        }
        let remind_ms = check_ahead("remindAt", at)?;
        let now = now_ms();
        let idx = self.reminders.iter().position(|r| r.user.as_str() == user && r.message_id.as_str() == message_id);
        let row = match idx {
            Some(i) => {
                self.reminders[i].remind_ms = remind_ms;
                self.reminders[i].clone()
            }
            None => {
                if self.reminders.iter().filter(|r| r.user.as_str() == user).count() >= MESSAGE_REMINDERS_MAX as usize {
                    return Err(AppError::field("reminders", "you have the most reminders you may"));
                }
                let row = ReminderRow {
                    id: new_id(),
                    user: Uuid::new(user),
                    message_id: Uuid::new(message_id),
                    remind_ms,
                    created_ms: now,
                };
                self.reminders.push(row.clone());
                row
            }
        };
        self.publish_reminders_updated(user);
        Ok(self.reminder_view(&row, user).expect("readable"))
    }

    pub fn cancel_reminder(&mut self, user: &str, message_id: &str) {
        let before = self.reminders.len();
        self.reminders.retain(|r| !(r.user.as_str() == user && r.message_id.as_str() == message_id));
        if self.reminders.len() != before {
            self.publish_reminders_updated(user);
        }
    }

    pub fn list_reminders(&self, user: &str) -> MessageReminderList {
        let mut rows: Vec<&ReminderRow> = self
            .reminders
            .iter()
            .filter(|r| r.user.as_str() == user && self.readable_message(&r.message_id, user, false).is_ok())
            .collect();
        rows.sort_by_key(|r| (r.remind_ms, r.id.clone()));
        rows.into_iter().take(100).filter_map(|r| self.reminder_view(r, user)).collect()
    }

    // --- scheduled messages ----------------------------------------------------------------

    fn scheduled_view(&self, s: &ScheduledRow) -> ScheduledMessage {
        ScheduledMessage {
            id: s.id.clone(),
            channel_id: s.channel_id.clone(),
            parent_id: s.parent_id.clone(),
            content: s.content.clone(),
            attachments: self.attachment_views(&s.attachment_ids),
            send_at: ts(s.send_ms),
            state: s.state.clone(),
            sent_message_id: s.sent_message_id.clone(),
            failure_code: s.failure_code.clone(),
            created_at: ts(s.created_ms),
            updated_at: ts(s.updated_ms),
        }
    }

    pub fn schedule_message(
        &mut self,
        user: &str,
        channel_id: &str,
        scheduled_id: &str,
        req: &ScheduleMessageRequest,
    ) -> AppResult<ScheduledMessage> {
        if !is_uuid(scheduled_id) {
            return Err(AppError::field("scheduledId", "is not a UUID"));
        }
        self.member_channel(channel_id, user)?;
        let attachment_ids = req.attachment_ids.clone().unwrap_or_default();
        check_content(&req.content, !attachment_ids.is_empty())?;
        let send_ms = check_ahead("sendAt", &req.send_at)?;
        if let Some(p) = &req.parent_id {
            match self.messages.get(p) {
                Some(r) if r.channel_id.as_str() == channel_id && r.parent_id.is_none() => {}
                _ => return Err(AppError::message_not_found()),
            }
        }
        let existing = self.scheduled.iter().position(|s| s.id.as_str() == scheduled_id);
        if let Some(i) = existing {
            let s = &self.scheduled[i];
            if s.user.as_str() != user {
                return Err(AppError::new("SCHEDULED_MESSAGE_NOT_FOUND", "No such scheduled message."));
            }
            if s.state == ScheduledMessageState::Sent {
                return Err(AppError::new("SCHEDULED_MESSAGE_SENT", "That message was sent already."));
            }
        } else {
            let open = self
                .scheduled
                .iter()
                .filter(|s| s.user.as_str() == user && s.state != ScheduledMessageState::Sent)
                .count();
            if open >= SCHEDULED_MESSAGES_MAX as usize {
                return Err(AppError::new("SCHEDULED_MESSAGE_LIMIT", "You have the most scheduled messages you may."));
            }
        }
        let held: Vec<Uuid> = existing.map(|i| self.scheduled[i].attachment_ids.clone()).unwrap_or_default();
        for a in &attachment_ids {
            match self.uploads.get(a) {
                Some(u) if u.uploader.as_str() == user && u.message_id.is_none() && (!u.reserved || held.contains(a)) => {}
                _ => return Err(AppError::new("ATTACHMENT_NOT_FOUND", "No such upload.")),
            }
        }
        for a in &held {
            if let Some(u) = self.uploads.get_mut(a) {
                u.reserved = false;
            }
        }
        for a in &attachment_ids {
            if let Some(u) = self.uploads.get_mut(a) {
                u.reserved = true;
            }
        }
        let now = now_ms();
        let row = ScheduledRow {
            id: Uuid::new(scheduled_id),
            user: Uuid::new(user),
            workspace_id: self.workspace.id.clone(),
            channel_id: Uuid::new(channel_id),
            parent_id: req.parent_id.clone(),
            content: req.content.clone(),
            attachment_ids,
            send_ms,
            state: ScheduledMessageState::Pending,
            sent_message_id: None,
            failure_code: None,
            created_ms: existing.map(|i| self.scheduled[i].created_ms).unwrap_or(now),
            updated_ms: now,
        };
        match existing {
            Some(i) => self.scheduled[i] = row.clone(),
            None => self.scheduled.push(row.clone()),
        }
        Ok(self.scheduled_view(&row))
    }

    pub fn cancel_scheduled(&mut self, user: &str, channel_id: &str, scheduled_id: &str) -> AppResult<()> {
        let Some(i) = self
            .scheduled
            .iter()
            .position(|s| s.id.as_str() == scheduled_id && s.user.as_str() == user && s.channel_id.as_str() == channel_id)
        else {
            return Err(AppError::new("SCHEDULED_MESSAGE_NOT_FOUND", "No such scheduled message."));
        };
        if self.scheduled[i].state == ScheduledMessageState::Sent {
            return Err(AppError::new("SCHEDULED_MESSAGE_SENT", "That message was sent already."));
        }
        let row = self.scheduled.remove(i);
        for a in &row.attachment_ids {
            if let Some(u) = self.uploads.get_mut(a) {
                u.reserved = false;
            }
        }
        Ok(())
    }

    pub fn list_scheduled(&self, user: &str) -> ScheduledMessageList {
        let mut rows: Vec<&ScheduledRow> = self
            .scheduled
            .iter()
            .filter(|s| s.user.as_str() == user && s.state != ScheduledMessageState::Sent)
            .filter(|s| self.channels.get(&s.channel_id).is_some_and(|c| Self::can_see(c, user)))
            .collect();
        rows.sort_by_key(|s| (s.send_ms, s.id.clone()));
        rows.into_iter().map(|s| self.scheduled_view(s)).collect()
    }

    /// Sends what is due and delivers due reminders. Called about once a second.
    pub fn run_due(&mut self) {
        let now = now_ms();
        let due: Vec<usize> = (0..self.scheduled.len())
            .filter(|i| self.scheduled[*i].state == ScheduledMessageState::Pending && self.scheduled[*i].send_ms <= now)
            .collect();
        for i in due {
            let s = self.scheduled[i].clone();
            let refused = match self.member_channel(&s.channel_id, &s.user) {
                Err(e) => Some(e.code),
                Ok(_) => match &s.parent_id {
                    Some(p) if !self.messages.get(p).is_some_and(|m| m.channel_id == s.channel_id) => Some("MESSAGE_NOT_FOUND"),
                    _ => None,
                },
            };
            let row = &mut self.scheduled[i];
            row.updated_ms = now;
            if let Some(code) = refused {
                row.state = ScheduledMessageState::Failed;
                row.failure_code = Some(code.to_string());
                continue;
            }
            let id = new_id();
            row.state = ScheduledMessageState::Sent;
            row.sent_message_id = Some(id.clone());
            self.post(PostInput {
                id,
                channel_id: s.channel_id.clone(),
                sender: s.user.clone(),
                content: s.content.clone(),
                parent_id: s.parent_id.clone(),
                attachment_ids: s.attachment_ids.clone(),
                at_ms: now,
            });
        }
        let (due, keep): (Vec<ReminderRow>, Vec<ReminderRow>) =
            std::mem::take(&mut self.reminders).into_iter().partition(|r| r.remind_ms <= now);
        self.reminders = keep;
        for r in due {
            let Ok(m) = self.readable_message(&r.message_id, &r.user, false) else { continue };
            let channel_id = m.channel_id.clone();
            self.notes.push(Note {
                id: new_id(),
                user: r.user.clone(),
                workspace_id: self.workspace.id.clone(),
                kind: NotificationType::Reminder,
                channel_id,
                message_id: r.message_id.clone(),
                created_ms: now,
                read_ms: None,
            });
            self.publish_reminders_updated(&r.user);
            self.publish_notifications_updated(&r.user);
        }
    }

    // --- notifications ---------------------------------------------------------------------

    fn notification_view(&self, n: &Note) -> Option<Notification> {
        let m = self.messages.get(&n.message_id)?;
        let thread = m.parent_id.as_ref().and_then(|p| self.messages.get(p)).map(|root| NotificationThread {
            root_id: root.id.clone(),
            excerpt: if root.deleted_ms.is_some() { None } else { excerpt(&root.content, THREAD_EXCERPT_MAX as usize) },
            author_id: root.sender.clone(),
        });
        Some(Notification {
            id: n.id.clone(),
            r#type: n.kind.clone(),
            channel_id: n.channel_id.clone(),
            message: self.message_view(m, Some(&n.user)),
            thread,
            created_at: ts(n.created_ms),
            read_at: n.read_ms.map(ts),
        })
    }

    fn note_visible(&self, n: &Note, user: &str) -> bool {
        n.user.as_str() == user
            && self
                .messages
                .get(&n.message_id)
                .is_some_and(|m| m.deleted_ms.is_none() && !m.hidden_for.contains_key(user))
            && self.channels.get(&n.channel_id).is_some_and(|c| Self::can_see(c, user))
    }

    pub fn list_notifications(&self, user: &str, q: &NotificationQuery) -> AppResult<NotificationPage> {
        self.check_workspace(&q.workspace_id, user)?;
        let types: Option<Vec<NotificationType>> = match &q.types {
            None => None,
            Some(csv) => {
                let mut out: Vec<NotificationType> = Vec::new();
                for part in csv.split(',') {
                    let t = NotificationType::parse(part);
                    if matches!(t, NotificationType::Unknown(_)) || out.contains(&t) {
                        return Err(AppError::field("types", "is not a list of distinct notification types"));
                    }
                    out.push(t);
                }
                Some(out)
            }
        };
        let limit = page_limit(q.limit)?;
        let mut rows: Vec<&Note> = self
            .notes
            .iter()
            .filter(|n| self.note_visible(n, user))
            .filter(|n| types.as_ref().is_none_or(|t| t.contains(&n.kind)))
            .filter(|n| q.unread_only != Some(true) || n.read_ms.is_none())
            .collect();
        rows.sort_by(|a, b| (b.created_ms, &b.message_id, &b.id).cmp(&(a.created_ms, &a.message_id, &a.id)));
        let start = match &q.before {
            Some(b) => {
                let Some(anchor) = self.notes.iter().find(|n| n.id == *b && n.user.as_str() == user) else {
                    return Err(AppError::field("before", "is not one of your notifications"));
                };
                let key = (anchor.created_ms, &anchor.message_id, &anchor.id);
                rows.iter().position(|n| (n.created_ms, &n.message_id, &n.id) < key).unwrap_or(rows.len())
            }
            None => 0,
        };
        let rest = &rows[start..];
        let chosen: Vec<&&Note> = rest.iter().take(limit).collect();
        let next = if rest.len() > limit { chosen.last().map(|n| n.id.clone()) } else { None };
        Ok(NotificationPage { data: chosen.iter().filter_map(|n| self.notification_view(n)).collect(), next_cursor: next })
    }

    pub fn mark_notifications_read(&mut self, user: &str, body: &serde_json::Value) -> AppResult<()> {
        let obj = body.as_object().ok_or_else(|| AppError::validation("The body is not an object."))?;
        let now = now_ms();
        let mut channels: BTreeSet<Uuid> = BTreeSet::new();
        if let Some(ids) = obj.get("ids") {
            if obj.len() != 1 {
                return Err(AppError::field("ids", "comes alone"));
            }
            let ids: Vec<Uuid> = serde_json::from_value(ids.clone()).map_err(|_| AppError::field("ids", "is not a list of ids"))?;
            if ids.is_empty() || ids.len() > 100 {
                return Err(AppError::field("ids", "must name 1 to 100 notifications"));
            }
            for n in self.notes.iter_mut().filter(|n| n.user.as_str() == user && ids.contains(&n.id) && n.read_ms.is_none()) {
                n.read_ms = Some(now);
                channels.insert(n.channel_id.clone());
            }
        } else if let Some(ws) = obj.get("workspaceId").and_then(|v| v.as_str()) {
            if obj.keys().any(|k| k != "workspaceId" && k != "before") {
                return Err(AppError::validation("Unknown field."));
            }
            self.check_workspace(ws, user)?;
            let bound = match obj.get("before").and_then(|v| v.as_str()) {
                Some(b) => {
                    let Some(anchor) = self.notes.iter().find(|n| n.id.as_str() == b && n.user.as_str() == user) else {
                        return Err(AppError::field("before", "is not one of your notifications"));
                    };
                    Some((anchor.created_ms, anchor.message_id.clone(), anchor.id.clone()))
                }
                None => None,
            };
            for n in self.notes.iter_mut().filter(|n| n.user.as_str() == user && n.read_ms.is_none()) {
                let within = bound
                    .as_ref()
                    .is_none_or(|(c, m, i)| (n.created_ms, &n.message_id, &n.id) <= (*c, m, i));
                if within {
                    n.read_ms = Some(now);
                    channels.insert(n.channel_id.clone());
                }
            }
        } else {
            return Err(AppError::field("ids", "or workspaceId is required"));
        }
        for c in channels {
            self.publish_channel_state(&c, user);
        }
        Ok(())
    }

    // --- the core inbox ----------------------------------------------------------------------

    pub fn publish_inbox_updated(&self, user: &str) {
        let frame = ServerFrame::InboxUpdated(InboxUpdatedFrame {
            data: InboxUpdatedFrameData { workspace_id: self.workspace.id.clone() },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
    }

    pub fn add_inbox_notice(&mut self, user: &Uuid, kind: &str, actor: Option<&Uuid>, subject: InboxEntrySubject, title: &str) {
        let mut params = BTreeMap::new();
        params.insert("title".to_string(), Some(serde_json::Value::String(title.to_string())));
        self.inbox.push(InboxRow {
            user: user.clone(),
            workspace_id: self.workspace.id.clone(),
            entry: InboxEntry {
                id: new_id(),
                module: "issues".into(),
                kind: kind.into(),
                actor_id: actor.cloned(),
                subject,
                params,
                read_at: None,
                created_at: ts(now_ms()),
            },
        });
        self.publish_inbox_updated(user);
    }

    pub fn list_inbox(&self, user: &str, q: &InboxQuery) -> AppResult<InboxPage> {
        let page = Page::from_query(&PageQuery { before: q.before.clone(), after: None, limit: q.limit })?;
        let rows: Vec<&InboxRow> = self.inbox.iter().filter(|r| r.user.as_str() == user).collect();
        let unread_count = rows.iter().filter(|r| r.entry.read_at.is_none()).count() as i64;
        let mut ids: Vec<Uuid> = rows
            .iter()
            .filter(|r| q.unread_only != Some(true) || r.entry.read_at.is_none())
            .map(|r| r.entry.id.clone())
            .collect();
        ids.sort();
        let (chosen, next) = page_ids(&ids, &page, false);
        Ok(InboxPage {
            data: chosen.iter().filter_map(|id| rows.iter().find(|r| r.entry.id == *id)).map(|r| r.entry.clone()).collect(),
            next_cursor: next,
            unread_count,
        })
    }

    pub fn mark_inbox_read(&mut self, user: &str, body: &serde_json::Value) -> AppResult<()> {
        let obj = body.as_object().ok_or_else(|| AppError::validation("The body is not an object."))?;
        let all = match (obj.get("ids"), obj.get("all")) {
            (Some(_), Some(_)) => return Err(AppError::field("ids", "and all cannot be sent together")),
            (None, Some(serde_json::Value::Bool(true))) => None,
            (Some(ids), None) => Some(
                serde_json::from_value::<Vec<Uuid>>(ids.clone()).map_err(|_| AppError::field("ids", "is not a list of ids"))?,
            ),
            _ => return Err(AppError::field("ids", "or all: true is required")),
        };
        let now = ts(now_ms());
        let mut changed = false;
        for r in self.inbox.iter_mut().filter(|r| r.user.as_str() == user && r.entry.read_at.is_none()) {
            if all.as_ref().is_none_or(|ids| ids.contains(&r.entry.id)) {
                r.entry.read_at = Some(now.clone());
                changed = true;
            }
        }
        if changed {
            self.publish_inbox_updated(user);
        }
        Ok(())
    }

    // --- search ----------------------------------------------------------------------------

    pub fn search(&self, user: &str, q: &SearchQuery) -> AppResult<SearchPage> {
        let text = q.q.trim().to_lowercase();
        let has_filter = q.channel_id.is_some()
            || q.sender_id.is_some()
            || q.status.is_some()
            || q.question.is_some()
            || q.kind.is_some()
            || q.since.is_some()
            || q.until.is_some()
            || q.repository.is_some();
        let n = text.chars().count();
        if (n == 0 && !has_filter) || (n > 0 && n < SEARCH_QUERY_MIN as usize) || n > SEARCH_QUERY_MAX as usize {
            return Err(AppError::field("q", "must be 3 to 200 characters"));
        }
        if let Some(c) = &q.channel_id {
            self.visible_channel(c, user)?;
        }
        let since = q.since.as_ref().and_then(|t| t.unix_ms());
        let until = q.until.as_ref().and_then(|t| t.unix_ms());
        let limit = page_limit(q.limit)?;
        let mut hits: Vec<&Msg> = self
            .messages
            .values()
            .filter(|m| m.deleted_ms.is_none() && !m.hidden_for.contains_key(user))
            .filter(|m| self.channels.get(&m.channel_id).is_some_and(|c| Self::can_see(c, user)))
            .filter(|m| text.is_empty() || m.content.to_lowercase().contains(&text))
            .filter(|m| q.channel_id.as_ref().is_none_or(|c| *c == m.channel_id))
            .filter(|m| q.sender_id.as_ref().is_none_or(|s| m.sender.as_ref() == Some(s)))
            .filter(|m| match &q.status {
                Some(SearchQueryStatus::Resolved) => m.resolved.is_some(),
                Some(SearchQueryStatus::Open) => m.parent_id.is_none() && !m.replies.is_empty() && m.resolved.is_none(),
                _ => true,
            })
            .filter(|m| match &q.kind {
                Some(SearchQueryKind::Code) => m.content.contains("```") || m.content.contains('`'),
                Some(SearchQueryKind::File) => !m.attachments.is_empty(),
                _ => true,
            })
            .filter(|m| q.question.is_none() || m.knowledge.as_ref().is_some_and(|k| k.is_question))
            .filter(|m| since.is_none_or(|s| m.created_ms >= s) && until.is_none_or(|u| m.created_ms < u))
            .filter(|m| q.before.as_ref().is_none_or(|b| m.id < *b))
            .collect();
        hits.sort_by(|a, b| b.id.cmp(&a.id));
        let more = hits.len() > limit;
        hits.truncate(limit);
        Ok(SearchPage {
            data: hits.iter().map(|m| self.message_view(m, Some(user))).collect(),
            next_cursor: if more { hits.last().map(|m| m.id.clone()) } else { None },
        })
    }

    pub fn find(&self, user: &str, q: &FindQuery) -> AppResult<FindResponse> {
        let text = q.q.trim().to_lowercase();
        let n = text.chars().count();
        if n < SEARCH_QUERY_MIN as usize || n > SEARCH_QUERY_MAX as usize {
            return Err(AppError::field("q", "must be 3 to 200 characters"));
        }
        let limit = match q.limit {
            None => FIND_HITS_MAX,
            Some(l) if l >= 1 && l as usize <= FIND_HITS_MAX => l as usize,
            Some(_) => return Err(AppError::field("limit", "is out of range")),
        };
        let chat = self.search(user, &SearchQuery { q: q.q.clone(), limit: Some(limit as i64), ..SearchQuery::default() })?;
        let chat_hits: Vec<FindHit> = chat
            .data
            .iter()
            .map(|m| {
                let where_ = self
                    .channels
                    .get(&m.channel_id)
                    .and_then(|c| c.name.clone())
                    .map(|n| format!("#{n}"))
                    .unwrap_or_else(|| "a direct message".into());
                let who = m.sender.as_ref().map(|s| s.display_name.clone()).unwrap_or_else(|| "Somebody".into());
                FindHit {
                    id: m.id.clone(),
                    title: format!("{who} in {where_}"),
                    excerpt: excerpt(&m.content, 160),
                    at: m.created_at.clone(),
                    target: FindTarget::Message(FindTargetMessage { channel_id: m.channel_id.clone(), message_id: m.id.clone() }),
                }
            })
            .collect();
        let mut issues: Vec<&IssueRow> = self
            .issues
            .iter()
            .filter(|r| r.issue.title.to_lowercase().contains(&text) || r.issue.body.to_lowercase().contains(&text))
            .collect();
        issues.sort_by(|a, b| b.issue.id.cmp(&a.issue.id));
        let issue_hits: Vec<FindHit> = issues
            .iter()
            .take(limit)
            .map(|r| FindHit {
                id: r.issue.id.clone(),
                title: r.issue.title.clone(),
                excerpt: excerpt(&r.issue.body, 160),
                at: r.issue.updated_at.clone(),
                target: FindTarget::Issue(FindTargetIssue { issue_id: r.issue.id.clone() }),
            })
            .collect();
        Ok(FindResponse {
            groups: vec![
                FindGroup { source: "chat".into(), hits: chat_hits },
                FindGroup { source: "issues".into(), hits: issue_hits },
            ],
            skipped: Vec::new(),
        })
    }

    pub fn list_unresolved(&self, user: &str, q: &UnresolvedQuery) -> AppResult<UnresolvedPage> {
        if let Some(c) = &q.channel_id {
            self.visible_channel(c, user)?;
        }
        if q.before.is_some() && q.after.is_some() {
            return Err(AppError::field("before", "before and after cannot be asked together"));
        }
        let limit = page_limit(q.limit)?;
        let oldest = matches!(q.order, Some(UnresolvedQueryOrder::Oldest));
        let mut roots: Vec<&Msg> = self
            .messages
            .values()
            .filter(|m| m.parent_id.is_none() && m.deleted_ms.is_none() && m.resolved.is_none())
            .filter(|m| !m.replies.is_empty() || m.knowledge.as_ref().is_some_and(|k| k.is_question))
            .filter(|m| self.channels.get(&m.channel_id).is_some_and(|c| Self::can_see(c, user)))
            .filter(|m| q.channel_id.as_ref().is_none_or(|c| *c == m.channel_id))
            .filter(|m| q.unanswered.as_deref() != Some("yes") || m.replies.is_empty())
            .filter(|m| {
                q.assignee_id.as_ref().is_none_or(|a| m.knowledge.as_ref().is_some_and(|k| k.assignee_id.as_ref() == Some(a)))
            })
            .collect();
        roots.sort_by(|a, b| if oldest { a.id.cmp(&b.id) } else { b.id.cmp(&a.id) });
        if let Some(b) = &q.before {
            roots.retain(|m| if oldest { m.id > *b } else { m.id < *b });
        }
        if let Some(a) = &q.after {
            roots.retain(|m| if oldest { m.id < *a } else { m.id > *a });
        }
        let more = roots.len() > limit;
        roots.truncate(limit);
        Ok(UnresolvedPage {
            data: roots.iter().map(|m| self.message_view(m, Some(user))).collect(),
            next_cursor: if more { roots.last().map(|m| m.id.clone()) } else { None },
        })
    }

    // --- saved searches ----------------------------------------------------------------------

    pub fn list_saved_searches(&self, user: &str, q: &OlderPageQuery) -> AppResult<SavedSearchPage> {
        let page = Page::from_query(&PageQuery { before: q.before.clone(), after: None, limit: q.limit })?;
        let mut ids: Vec<Uuid> =
            self.saved_searches.iter().filter(|s| s.user.as_str() == user).map(|s| s.search.id.clone()).collect();
        ids.sort();
        let (chosen, next) = page_ids(&ids, &page, false);
        Ok(SavedSearchPage {
            data: chosen
                .iter()
                .filter_map(|id| self.saved_searches.iter().find(|s| s.search.id == *id))
                .map(|s| s.search.clone())
                .collect(),
            next_cursor: next,
        })
    }

    pub fn save_search(&mut self, user: &str, search_id: &str, req: &SaveSearchRequest) -> AppResult<SavedSearch> {
        if !is_uuid(search_id) {
            return Err(AppError::field("searchId", "is not a UUID"));
        }
        let name = req.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(AppError::field("name", "is not a name"));
        }
        if let Some(row) = self.saved_searches.iter_mut().find(|s| s.search.id.as_str() == search_id) {
            if row.user.as_str() != user {
                return Err(AppError::new("SAVED_SEARCH_NOT_FOUND", "No such saved search."));
            }
            row.search.name = name.to_string();
            row.search.query = req.query.clone();
            return Ok(row.search.clone());
        }
        if self.saved_searches.iter().filter(|s| s.user.as_str() == user).count() >= SAVED_SEARCHES_MAX {
            return Err(AppError::new("SAVED_SEARCH_LIMIT", "You have the most saved searches you may."));
        }
        let search =
            SavedSearch { id: Uuid::new(search_id), name: name.to_string(), query: req.query.clone(), created_at: ts(now_ms()) };
        self.saved_searches.push(SavedSearchRow {
            user: Uuid::new(user),
            workspace_id: self.workspace.id.clone(),
            search: search.clone(),
        });
        Ok(search)
    }

    pub fn delete_saved_search(&mut self, user: &str, search_id: &str) {
        self.saved_searches.retain(|s| !(s.user.as_str() == user && s.search.id.as_str() == search_id));
    }

    // --- issues ------------------------------------------------------------------------------

    pub fn list_issues(&self, q: &IssueQuery) -> AppResult<IssuePage> {
        let page = Page::from_query(&PageQuery { before: q.before.clone(), after: None, limit: q.limit })?;
        let mut ids: Vec<Uuid> = self
            .issues
            .iter()
            .filter(|r| match &q.status {
                Some(IssueQueryStatus::Open) => r.issue.status == IssueStatus::Open,
                Some(IssueQueryStatus::Done) => r.issue.status == IssueStatus::Done,
                _ => true,
            })
            .map(|r| r.issue.id.clone())
            .collect();
        ids.sort();
        let (chosen, next) = page_ids(&ids, &page, false);
        Ok(IssuePage {
            data: chosen.iter().filter_map(|id| self.issues.iter().find(|r| r.issue.id == *id)).map(|r| r.issue.clone()).collect(),
            next_cursor: next,
        })
    }

    fn check_issue_title(title: &str) -> AppResult<String> {
        let t = title.trim();
        if t.is_empty() || t.chars().count() > 200 || t.chars().any(|c| c.is_control()) {
            return Err(AppError::field("title", "is not a title"));
        }
        Ok(t.to_string())
    }

    fn notify_assignee(&mut self, issue: &Issue, actor: &str) {
        if let Some(a) = &issue.assignee_id
            && a.as_str() != actor {
                let subject = InboxEntrySubject { r#type: "issue".into(), id: issue.id.clone() };
                self.add_inbox_notice(&a.clone(), "issue_assigned", Some(&Uuid::new(actor)), subject, &issue.title.clone());
            }
    }

    pub fn create_issue(&mut self, user: &str, req: &CreateIssueRequest) -> AppResult<Issue> {
        let title = Self::check_issue_title(&req.title)?;
        let assignee = req.assignee_id.clone().flatten();
        if let Some(a) = &assignee
            && !self.people.contains_key(a) {
                return Err(AppError::field("assigneeId", "is not a member of this workspace"));
            }
        let now = ts(now_ms());
        let issue = Issue {
            id: new_id(),
            title,
            body: req.body.clone().unwrap_or_default(),
            status: IssueStatus::Open,
            assignee_id: assignee,
            created_by: Some(Uuid::new(user)),
            created_at: now.clone(),
            updated_at: now,
        };
        self.issues.push(IssueRow { workspace_id: self.workspace.id.clone(), issue: issue.clone() });
        self.notify_assignee(&issue, user);
        Ok(issue)
    }

    pub fn update_issue(&mut self, user: &str, issue_id: &str, req: &UpdateIssueRequest) -> AppResult<Issue> {
        if req.title.is_none() && req.body.is_none() && req.status.is_none() && req.assignee_id.is_none() {
            return Err(AppError::validation("Change at least one thing."));
        }
        let title = req.title.as_deref().map(Self::check_issue_title).transpose()?;
        if let Some(Some(a)) = &req.assignee_id
            && !self.people.contains_key(a) {
                return Err(AppError::field("assigneeId", "is not a member of this workspace"));
            }
        let status = match &req.status {
            Some(UpdateIssueRequestStatus::Open) => Some(IssueStatus::Open),
            Some(UpdateIssueRequestStatus::Done) => Some(IssueStatus::Done),
            Some(UpdateIssueRequestStatus::Unknown(_)) => return Err(AppError::field("status", "is not open or done")),
            None => None,
        };
        let row = self
            .issues
            .iter_mut()
            .find(|r| r.issue.id.as_str() == issue_id)
            .ok_or_else(|| AppError::new("ISSUE_NOT_FOUND", "No such issue."))?;
        let before = row.issue.assignee_id.clone();
        if let Some(t) = title {
            row.issue.title = t;
        }
        if let Some(b) = &req.body {
            row.issue.body = b.clone();
        }
        if let Some(s) = status {
            row.issue.status = s;
        }
        if let Some(a) = &req.assignee_id {
            row.issue.assignee_id = a.clone();
        }
        row.issue.updated_at = ts(now_ms());
        let issue = row.issue.clone();
        if issue.assignee_id != before {
            self.notify_assignee(&issue, user);
        }
        Ok(issue)
    }

    pub fn delete_issue(&mut self, user: &str, issue_id: &str) -> AppResult<()> {
        let i = self
            .issues
            .iter()
            .position(|r| r.issue.id.as_str() == issue_id)
            .ok_or_else(|| AppError::new("ISSUE_NOT_FOUND", "No such issue."))?;
        if self.issues[i].issue.created_by.as_deref() != Some(user) && !self.is_admin(user) {
            return Err(AppError::forbidden());
        }
        self.issues.remove(i);
        Ok(())
    }

    // --- uploads -----------------------------------------------------------------------------

    /// `POST …/uploads`: the bool says whether it was made now (`201`) or is a retry (`200`).
    pub fn upload(&mut self, user: &str, q: &UploadQuery, bytes: Vec<u8>) -> AppResult<(Attachment, bool)> {
        if !crate::ids::is_v7_shape(&q.id) {
            return Err(AppError::field("id", "is not a UUIDv7"));
        }
        let name = q.name.trim();
        if name.is_empty() || name.chars().count() > 255 || name.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
            return Err(AppError::field("name", "is not a file name"));
        }
        if bytes.len() as i64 > ATTACHMENT_BYTES_MAX {
            return Err(AppError::new("PAYLOAD_TOO_LARGE", "The file is too large."));
        }
        if let Some(u) = self.uploads.get(&q.id) {
            if u.uploader.as_str() == user && u.attachment.file_name == name && u.attachment.size_bytes == bytes.len() as i64 {
                return Ok((u.attachment.clone(), false));
            }
            return Err(AppError::new("ATTACHMENT_ID_CONFLICT", "That upload id is taken."));
        }
        let pending = self.uploads.values().filter(|u| u.uploader.as_str() == user && u.message_id.is_none()).count();
        if pending >= PENDING_UPLOADS_MAX as usize {
            return Err(AppError::new("TOO_MANY_PENDING_UPLOADS", "Send or discard some uploads first."));
        }
        let attachment = self.store_upload(user, &q.id, name, bytes);
        Ok((attachment, true))
    }

    pub fn store_upload(&mut self, user: &str, id: &str, name: &str, bytes: Vec<u8>) -> Attachment {
        let (mime, kind, width, height) = crate::media::classify(&bytes, name);
        let attachment = Attachment {
            id: Uuid::new(id),
            file_name: name.to_string(),
            mime_type: mime,
            size_bytes: bytes.len() as i64,
            kind,
            width,
            height,
        };
        self.uploads.insert(
            Uuid::new(id),
            Upload {
                attachment: attachment.clone(),
                bytes: Arc::new(bytes),
                uploader: Uuid::new(user),
                workspace_id: self.workspace.id.clone(),
                message_id: None,
                reserved: false,
            },
        );
        attachment
    }

    /// The upload if the person may download it.
    pub fn downloadable(&self, user: &str, attachment_id: &str) -> AppResult<&Upload> {
        let not_found = || AppError::new("ATTACHMENT_NOT_FOUND", "No such attachment.");
        let u = self.uploads.get(attachment_id).ok_or_else(not_found)?;
        match &u.message_id {
            None if u.uploader.as_str() == user => Ok(u),
            None => Err(not_found()),
            Some(m) => match self.readable_message(m, user, false) {
                Ok(_) => Ok(u),
                Err(_) => Err(not_found()),
            },
        }
    }

    pub fn discard_upload(&mut self, user: &str, attachment_id: &str) -> AppResult<()> {
        match self.uploads.get(attachment_id) {
            Some(u) if u.uploader.as_str() == user && u.message_id.is_none() && !u.reserved => {
                self.uploads.remove(attachment_id);
                Ok(())
            }
            _ => Err(AppError::new("ATTACHMENT_NOT_FOUND", "No such attachment.")),
        }
    }
}
