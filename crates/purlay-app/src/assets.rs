//! Assets: Purlay's own icons (Lucide, ISC) and logo, then gpui-kit's defaults.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/icons/", $name, ".svg")) as &[u8])),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "hash", "lock", "at-sign", "paperclip", "smile", "smile-plus", "send-horizontal", "pin", "pin-off", "bookmark",
    "bookmark-plus", "clock", "message-square", "message-square-text", "reply", "pencil", "trash-2", "check-check",
    "circle-dot", "circle-check-big", "list-todo", "bell-off", "users", "user-plus", "link", "image", "download", "x",
    "archive", "alarm-clock", "calendar-clock", "log-out", "focus", "bold", "italic", "strikethrough", "code", "list",
    "list-ordered", "quote", "eye", "history", "mail", "inbox", "search", "plus", "chevron-down", "chevron-right",
    "arrow-down", "wifi-off", "refresh-cw", "square-pen", "circle-help", "lightbulb", "flag", "siren",
    "git-pull-request", "bug", "house", "sticker", "file-text", "external-link", "copy", "more-horizontal", "sun-moon",
    "keyboard", "settings", "folder-plus", "star", "bell", "chevrons-up-down", "panel-left", "panel-right", "arrow-up",
    "moon", "sun", "list-filter", "gallery-vertical-end", "win-min", "win-max", "win-restore", "win-close", "volume-2", "play", "check",
];

pub const LOGO_PNG: &[u8] = include_bytes!("../assets/purlay-logo-128.png");

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix("purlay/").and_then(|p| p.strip_suffix(".svg")) {
            if let Some((_, bytes)) = ICONS.iter().find(|(n, _)| *n == name) {
                return Ok(Some(Cow::Borrowed(bytes)));
            }
        }
        if path == "purlay/logo.png" {
            return Ok(Some(Cow::Borrowed(LOGO_PNG)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = Vec::new();
        if "purlay/".starts_with(path) || path.starts_with("purlay") {
            out.extend(ICONS.iter().map(|(n, _)| SharedString::from(format!("purlay/{n}.svg"))));
        }
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}

/// An icon from Purlay's own set, by Lucide name.
pub fn icon(name: &'static str) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::default().path(format!("purlay/{name}.svg"))
}
