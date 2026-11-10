//! A channel's loaded messages: one contiguous run of the feed, sorted by id.
//!
//! The window knows whether it reaches the newest message (so a live one belongs at its
//! end) and the oldest (so there is nothing more to page). A window opened around an older
//! message does not reach the newest, and a live message is not inserted into it; the UI
//! shows "Jump to recent" instead.

use purlay_contract::models::{Message, MessageRevisedFrameData, Reaction, ThreadSummary};
use purlay_contract::{Timestamp, Uuid};

/// At most this many rows are kept; the end away from the reader is trimmed.
pub const WINDOW_MAX: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct ChannelWindow {
    pub messages: Vec<Message>,
    pub reaches_newest: bool,
    pub reaches_oldest: bool,
    /// The cursor for the next older page (`before=`), when not at the oldest.
    pub older_cursor: Option<Uuid>,
    /// The cursor for the next newer page (`after=`), when not at the newest.
    pub newer_cursor: Option<Uuid>,
    /// The message the window was opened around (a permalink), if any.
    pub anchor: Option<Uuid>,
    pub loading_older: bool,
    pub loading_newer: bool,
}

impl ChannelWindow {
    pub fn latest(page: Vec<Message>, more_older: Option<Uuid>) -> Self {
        let mut w = ChannelWindow {
            reaches_newest: true,
            reaches_oldest: more_older.is_none(),
            older_cursor: more_older,
            ..Default::default()
        };
        w.merge(page);
        w
    }

    pub fn around(target: Uuid, page: Vec<Message>, older: Option<Uuid>, newer: Option<Uuid>) -> Self {
        let mut w = ChannelWindow {
            reaches_newest: newer.is_none(),
            reaches_oldest: older.is_none(),
            older_cursor: older,
            newer_cursor: newer,
            anchor: Some(target),
            ..Default::default()
        };
        w.merge(page);
        w
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.messages.binary_search_by(|m| m.id.as_str().cmp(id)).ok()
    }

    pub fn get(&self, id: &str) -> Option<&Message> {
        self.position(id).map(|i| &self.messages[i])
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Message> {
        self.position(id).map(move |i| &mut self.messages[i])
    }

    pub fn newest_id(&self) -> Option<&Uuid> {
        self.messages.last().map(|m| &m.id)
    }

    pub fn oldest_id(&self) -> Option<&Uuid> {
        self.messages.first().map(|m| &m.id)
    }

    /// Inserts or replaces messages by id; returns how many were new.
    pub fn merge(&mut self, page: Vec<Message>) -> usize {
        let mut added = 0;
        for m in page {
            match self.messages.binary_search_by(|x| x.id.cmp(&m.id)) {
                Ok(i) => self.messages[i] = m,
                Err(i) => {
                    self.messages.insert(i, m);
                    added += 1;
                }
            }
        }
        added
    }

    /// A live message: kept only when the window reaches the newest (or it falls inside
    /// what is loaded).
    pub fn live(&mut self, m: Message) -> bool {
        let inside = match (self.oldest_id(), self.newest_id()) {
            (Some(lo), Some(hi)) => m.id >= *lo && m.id <= *hi,
            _ => false,
        };
        if !(self.reaches_newest || inside) {
            return false;
        }
        self.merge(vec![m]);
        self.trim_oldest();
        true
    }

    pub fn prepend_older(&mut self, page: Vec<Message>, next: Option<Uuid>) {
        self.merge(page);
        self.older_cursor = next.clone();
        self.reaches_oldest = next.is_none();
        self.loading_older = false;
        self.trim_newest();
    }

    pub fn append_newer(&mut self, page: Vec<Message>, next: Option<Uuid>) {
        self.merge(page);
        self.newer_cursor = next.clone();
        self.reaches_newest = next.is_none();
        self.loading_newer = false;
        self.trim_oldest();
    }

    fn trim_oldest(&mut self) {
        if self.messages.len() > WINDOW_MAX {
            let cut = self.messages.len() - WINDOW_MAX;
            self.messages.drain(..cut);
            self.reaches_oldest = false;
            self.older_cursor = self.messages.first().map(|m| m.id.clone());
        }
    }

    fn trim_newest(&mut self) {
        if self.messages.len() > WINDOW_MAX {
            self.messages.truncate(WINDOW_MAX);
            self.reaches_newest = false;
            self.newer_cursor = self.messages.last().map(|m| m.id.clone());
        }
    }

    pub fn revise(&mut self, r: &MessageRevisedFrameData) -> bool {
        if let Some(m) = self.get_mut(&r.message_id) {
            apply_revision(m, r);
            true
        } else {
            false
        }
    }

    pub fn set_reactions(&mut self, id: &str, reactions: &[Reaction]) -> bool {
        if let Some(m) = self.get_mut(id) {
            m.reactions = reactions.to_vec();
            true
        } else {
            false
        }
    }

    pub fn set_thread(&mut self, id: &str, thread: &ThreadSummary) -> bool {
        if let Some(m) = self.get_mut(id) {
            m.thread = Some(thread.clone());
            true
        } else {
            false
        }
    }

    pub fn hide(&mut self, id: &str) -> bool {
        if let Some(i) = self.position(id) {
            self.messages.remove(i);
            true
        } else {
            false
        }
    }
}

pub fn apply_revision(m: &mut Message, r: &MessageRevisedFrameData) {
    m.content = r.content.clone();
    m.edited_at = r.edited_at.clone();
    m.deleted_at = r.deleted_at.clone();
    m.mentions = r.mentions.clone();
    if r.deleted_at.is_some() {
        m.attachments.clear();
        m.reactions.clear();
    }
}

/// A thread: its root and the replies loaded so far (oldest first, paged forward).
#[derive(Debug, Clone, Default)]
pub struct ThreadWindow {
    pub channel_id: Uuid,
    pub root: Option<Message>,
    pub replies: ChannelWindow,
    pub loaded: bool,
    pub following: Option<bool>,
}

impl ThreadWindow {
    pub fn replies_newest_at(&self) -> Option<&Timestamp> {
        self.replies.messages.last().map(|m| &m.created_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn msg(id: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "channelId": "0190a000-0000-7000-8000-000000000001", "sender": null, "githubEvent": null,
            "content": "x", "editedAt": null, "deletedAt": null, "createdAt": "2026-11-10T00:00:00.000Z",
            "parentId": null, "thread": null, "reactions": [], "mentions": [], "attachments": []
        }))
        .unwrap()
    }

    fn id(n: u32) -> String {
        format!("0190a000-0000-7000-8000-{n:012}")
    }

    #[test]
    fn merges_in_order_without_duplicates() {
        let mut w = ChannelWindow::latest(vec![msg(&id(3)), msg(&id(1))], None);
        assert_eq!(w.merge(vec![msg(&id(2)), msg(&id(3))]), 1);
        let ids: Vec<_> = w.messages.iter().map(|m| m.id.to_string()).collect();
        assert_eq!(ids, vec![id(1), id(2), id(3)]);
    }

    #[test]
    fn live_only_when_reaching_newest() {
        let mut w = ChannelWindow::around(Uuid::new(id(5)), vec![msg(&id(4)), msg(&id(5)), msg(&id(6))], Some(Uuid::new(id(4))), Some(Uuid::new(id(6))));
        assert!(!w.live(msg(&id(9))));
        assert!(w.live(msg(&id(5))));
        let mut w = ChannelWindow::latest(vec![msg(&id(1))], None);
        assert!(w.live(msg(&id(2))));
        assert_eq!(w.messages.len(), 2);
    }

    #[test]
    fn trims_to_the_bound() {
        let mut w = ChannelWindow::latest(vec![], None);
        for n in 0..(WINDOW_MAX as u32 + 10) {
            w.live(msg(&id(n)));
        }
        assert_eq!(w.messages.len(), WINDOW_MAX);
        assert!(!w.reaches_oldest);
        assert_eq!(w.messages[0].id.as_str(), id(10));
    }
}
