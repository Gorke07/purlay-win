//! The highest message id held per channel: what a reconnect resumes from.
//!
//! UUIDv7s sort as text in the order they were made, so "the highest" is a string
//! comparison (D-8), made case-insensitive. A cursor is not a watermark (D-65): it only
//! moves up, and the server's replay starts `REPLAY_OVERLAP_MS` beneath it.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Cursors {
    highest: HashMap<String, String>,
}

impl Cursors {
    pub fn observe(&mut self, channel_id: &str, message_id: &str) {
        let candidate = message_id.to_ascii_lowercase();
        match self.highest.get(channel_id) {
            Some(current) if candidate <= *current => {}
            _ => {
                self.highest.insert(channel_id.to_string(), candidate);
            }
        }
    }

    pub fn get(&self, channel_id: &str) -> Option<&str> {
        self.highest.get(channel_id).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_moves_up() {
        let mut c = Cursors::default();
        assert_eq!(c.get("a"), None);
        c.observe("a", "0190-2");
        c.observe("a", "0190-1");
        assert_eq!(c.get("a"), Some("0190-2"));
        c.observe("a", "0190-3");
        assert_eq!(c.get("a"), Some("0190-3"));
    }

    #[test]
    fn case_insensitive() {
        let mut c = Cursors::default();
        c.observe("a", "0190-B");
        c.observe("a", "0190-a");
        assert_eq!(c.get("a"), Some("0190-b"));
    }

    #[test]
    fn per_channel() {
        let mut c = Cursors::default();
        c.observe("a", "2");
        c.observe("b", "1");
        assert_eq!(c.get("a"), Some("2"));
        assert_eq!(c.get("b"), Some("1"));
    }
}
