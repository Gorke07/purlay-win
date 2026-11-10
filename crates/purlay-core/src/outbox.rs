//! Messages sent and not yet known to have arrived, kept across a restart
//! (the web client's `pending-store.ts`).
//!
//! Written atomically on every change to `<data>/outbox/<server>/<userId>.json` and read back
//! at start as **failed**, with the Retry the row already has: the same id, so a message that
//! did reach the server comes back as itself (D-8). Per person; bounded to
//! `OUTBOX_MAX_MESSAGES` and `OUTBOX_MAX_CHARS`, newest kept. A failure to remember is a
//! message that is not remembered, never a send that does not work.

use crate::ids::is_message_id_shape;
use crate::store::{Delivery, PendingMessage};
use purlay_contract::constants::{OUTBOX_MAX_CHARS, OUTBOX_MAX_MESSAGES};
use purlay_contract::models::Attachment;
use purlay_contract::{Timestamp, Uuid};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub content: String,
    pub created_at: Timestamp,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone)]
pub struct Outbox {
    path: Option<PathBuf>,
}

impl Outbox {
    pub fn new(dir: Option<&Path>, user_id: &str) -> Self {
        let path = dir.map(|d| d.join(format!("{}.json", sanitize(user_id))));
        Outbox { path }
    }

    pub fn none() -> Self {
        Outbox { path: None }
    }

    /// What was kept, as failed rows; anything that is not what this wrote is ignored.
    pub fn load(&self) -> Vec<PendingMessage> {
        let Some(path) = &self.path else { return vec![] };
        let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
        let parsed: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                // Unreadable: moved aside, never trusted, never lost silently.
                let aside = path.with_extension(format!("corrupt-{}", crate::clock::Clock::now_ms(&crate::clock::SystemClock)));
                let _ = std::fs::rename(path, aside);
                return vec![];
            }
        };
        let mut out: Vec<PendingMessage> = parsed
            .into_iter()
            .filter_map(|v| serde_json::from_value::<StoredMessage>(v).ok())
            .filter(|m| is_message_id_shape(&m.id) && (!m.content.is_empty() || !m.attachments.is_empty()))
            .map(|m| PendingMessage {
                id: m.id,
                channel_id: m.channel_id,
                parent_id: m.parent_id,
                content: m.content,
                created_at: m.created_at,
                delivery: Delivery::Failed,
                reason: None,
                attachments: m.attachments,
            })
            .collect();
        let max = OUTBOX_MAX_MESSAGES as usize;
        if out.len() > max {
            out.drain(..out.len() - max);
        }
        out
    }

    /// The newest that fit; nothing in flight removes the file.
    pub fn save(&self, pending: &[PendingMessage]) {
        let Some(path) = &self.path else { return };
        let mut kept: Vec<StoredMessage> = Vec::new();
        let mut chars = 0usize;
        for p in pending.iter().rev().filter(|p| p.delivery != Delivery::Sent) {
            chars += p.content.encode_utf16().count();
            if kept.len() >= OUTBOX_MAX_MESSAGES as usize || chars > OUTBOX_MAX_CHARS as usize {
                break;
            }
            kept.insert(
                0,
                StoredMessage {
                    id: p.id.clone(),
                    channel_id: p.channel_id.clone(),
                    parent_id: p.parent_id.clone(),
                    content: p.content.clone(),
                    created_at: p.created_at.clone(),
                    attachments: p.attachments.clone(),
                },
            );
        }
        if kept.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        let _ = write_atomic(path, &serde_json::to_vec(&kept).unwrap_or_default());
    }

    pub fn clear(&self) {
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(n: u32, content: &str, delivery: Delivery) -> PendingMessage {
        PendingMessage {
            id: Uuid::new(format!("0190a000-0000-7000-8000-{n:012}")),
            channel_id: Uuid::new("0190a000-0000-7000-8000-00000000ffff"),
            parent_id: None,
            content: content.into(),
            created_at: Timestamp::new("2026-11-10T00:00:00.000Z"),
            delivery,
            reason: None,
            attachments: vec![],
        }
    }

    #[test]
    fn round_trip_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let o = Outbox::new(Some(dir.path()), "u1");
        o.save(&[pending(1, "a", Delivery::Sending), pending(2, "b", Delivery::Sent), pending(3, "c", Delivery::Failed)]);
        let back = o.load();
        assert_eq!(back.len(), 2);
        assert!(back.iter().all(|p| p.delivery == Delivery::Failed));
        assert_eq!(back[0].content, "a");
        o.save(&[]);
        assert!(o.load().is_empty());
    }

    #[test]
    fn bounded_newest_kept() {
        let dir = tempfile::tempdir().unwrap();
        let o = Outbox::new(Some(dir.path()), "u1");
        let all: Vec<_> = (0..60).map(|n| pending(n, "x", Delivery::Sending)).collect();
        o.save(&all);
        let back = o.load();
        assert_eq!(back.len(), OUTBOX_MAX_MESSAGES as usize);
        assert_eq!(back.last().unwrap().id.as_str(), "0190a000-0000-7000-8000-000000000059");
    }

    #[test]
    fn per_person() {
        let dir = tempfile::tempdir().unwrap();
        Outbox::new(Some(dir.path()), "u1").save(&[pending(1, "a", Delivery::Sending)]);
        assert!(Outbox::new(Some(dir.path()), "u2").load().is_empty());
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let o = Outbox::new(Some(dir.path()), "u1");
        std::fs::write(dir.path().join("u1.json"), b"{not json").unwrap();
        assert!(o.load().is_empty());
        assert!(!dir.path().join("u1.json").exists());
    }
}
