//! Small things kept on this device: the servers signed in to, drafts, recent searches,
//! sidebar order, appearance. One JSON file under `%APPDATA%\Purlay`, written atomically.
//!
//! Drafts are per server, person, channel and thread (`draft.<user>.<channel>[.<parent>]`),
//! removed when empty, capped at 64 000 UTF-16 units (the server has no drafts API).

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const DRAFT_MAX_UTF16: usize = 64_000;
pub const RECENT_SEARCHES_MAX: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    /// The origin as typed and normalised (`https://chat.example.com`).
    pub address: String,
    /// The canonical origin from the handshake (`appUrl`).
    pub origin: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub last_workspace: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

/// How a notification looks: Purlay's own banner, or a Windows toast.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationStyle {
    #[default]
    Purlay,
    Windows,
}

/// The sound a notification makes: one of Purlay's tones, or Windows' own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationTone {
    #[default]
    Drop,
    Chime,
    Marimba,
    Glass,
    /// The toast's own sound, as set in Windows.
    System,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Data {
    pub servers: Vec<ServerEntry>,
    pub current_server: Option<String>,
    pub appearance: Appearance,
    pub density: Density,
    /// `en`, `tr`, or empty for the system's.
    pub language: String,
    pub launch_at_login: bool,
    pub notifications_enabled: bool,
    pub notification_sound: bool,
    pub notification_tone: NotificationTone,
    pub notification_style: NotificationStyle,
    /// No count on the taskbar button.
    pub hide_taskbar_badge: bool,
    /// Closing the window quits instead of leaving Purlay in the notification area.
    pub quit_on_close: bool,
    /// The one-time still running notice was shown.
    pub tray_hint_shown: bool,
    /// key → text
    pub drafts: BTreeMap<String, String>,
    /// `<server>|<user>|<workspace>` → newest first
    pub recent_searches: BTreeMap<String, Vec<String>>,
    /// `<server>|<user>|<workspace>` → section/channel order
    pub sidebar_order: BTreeMap<String, Vec<String>>,
    /// Emoji used recently, newest first.
    pub recent_emoji: Vec<String>,
    pub window: Option<WindowState>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

#[derive(Clone)]
pub struct Prefs {
    path: Option<PathBuf>,
    data: Arc<Mutex<Data>>,
}

impl Prefs {
    pub fn load(path: Option<PathBuf>) -> Self {
        let data = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Data>(&t).ok())
            .unwrap_or_else(|| Data { notifications_enabled: true, notification_sound: true, ..Default::default() });
        Prefs { path, data: Arc::new(Mutex::new(data)) }
    }

    pub fn in_memory() -> Self {
        Prefs::load(None)
    }

    pub fn get(&self) -> Data {
        self.data.lock().clone()
    }

    pub fn read<R>(&self, f: impl FnOnce(&Data) -> R) -> R {
        f(&self.data.lock())
    }

    pub fn update(&self, f: impl FnOnce(&mut Data)) {
        let bytes = {
            let mut d = self.data.lock();
            f(&mut d);
            serde_json::to_vec_pretty(&*d).unwrap_or_default()
        };
        if let Some(p) = &self.path {
            let _ = crate::outbox::write_atomic(p, &bytes);
        }
    }

    pub fn draft_key(server: &str, user: &str, channel: &str, parent: Option<&str>) -> String {
        match parent {
            Some(p) => format!("{server}|draft.{user}.{channel}.{p}"),
            None => format!("{server}|draft.{user}.{channel}"),
        }
    }

    pub fn draft(&self, key: &str) -> String {
        self.read(|d| d.drafts.get(key).cloned().unwrap_or_default())
    }

    pub fn set_draft(&self, key: &str, text: &str) {
        let text: String = {
            let mut units = 0;
            text.chars()
                .take_while(|c| {
                    units += c.len_utf16();
                    units <= DRAFT_MAX_UTF16
                })
                .collect()
        };
        self.update(|d| {
            if text.trim().is_empty() {
                d.drafts.remove(key);
            } else {
                d.drafts.insert(key.to_string(), text);
            }
        });
    }

    pub fn push_recent_search(&self, scope: &str, q: &str) {
        let q = q.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.update(|d| {
            let list = d.recent_searches.entry(scope.to_string()).or_default();
            list.retain(|x| *x != q);
            list.insert(0, q);
            list.truncate(RECENT_SEARCHES_MAX);
        });
    }

    pub fn push_recent_emoji(&self, emoji: &str) {
        self.update(|d| {
            d.recent_emoji.retain(|x| x != emoji);
            d.recent_emoji.insert(0, emoji.to_string());
            d.recent_emoji.truncate(24);
        });
    }

    /// Drops everything kept for a person on a server (sign-out).
    pub fn forget_account(&self, server: &str) {
        let prefix = format!("{server}|");
        self.update(|d| {
            d.drafts.retain(|k, _| !k.starts_with(&prefix));
            d.recent_searches.retain(|k, _| !k.starts_with(&prefix));
            d.sidebar_order.retain(|k, _| !k.starts_with(&prefix));
        });
    }
}

/// `%APPDATA%\Purlay` (roaming: settings) and `%LOCALAPPDATA%\Purlay` (outbox, caches).
pub fn data_dirs() -> Option<(PathBuf, PathBuf)> {
    let dirs = directories::ProjectDirs::from("dev", "Purlay", "Purlay")?;
    let (mut config, mut data) = (dirs.config_dir().to_path_buf(), dirs.data_local_dir().to_path_buf());
    // `PURLAY_PROFILE=name` keeps a separate set of settings, drafts and outboxes (for
    // development: a second instance against another server).
    if let Some(p) = profile() {
        config = config.join("profiles").join(&p);
        data = data.join("profiles").join(&p);
    }
    Some((config, data))
}

/// The profile named by `PURLAY_PROFILE`, cleaned to a safe folder name.
pub fn profile() -> Option<String> {
    let p = std::env::var("PURLAY_PROFILE").ok()?;
    let clean: String = p.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(32).collect();
    (!clean.is_empty()).then_some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_are_removed_when_empty_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let p = Prefs::load(Some(dir.path().join("prefs.json")));
        let k = Prefs::draft_key("s", "u", "c", None);
        p.set_draft(&k, "hello");
        assert_eq!(Prefs::load(Some(dir.path().join("prefs.json"))).draft(&k), "hello");
        p.set_draft(&k, "  ");
        assert_eq!(p.draft(&k), "");
        p.set_draft(&k, &"x".repeat(70_000));
        assert_eq!(p.draft(&k).len(), DRAFT_MAX_UTF16);
    }

    #[test]
    fn recent_searches_dedupe() {
        let p = Prefs::in_memory();
        p.push_recent_search("s", "a");
        p.push_recent_search("s", "b");
        p.push_recent_search("s", "a");
        assert_eq!(p.get().recent_searches["s"], vec!["a", "b"]);
    }
}
