//! Actions and the keyboard (UX §7). Ctrl stands for the Mac's ⌘; bare letters are never
//! global shortcuts.

use gpui_kit::*;

gpui_kit::actions!(
    purlay,
    [
        OpenPalette,
        OpenSearch,
        ToggleSidebar,
        OpenInbox,
        ToggleDeepWork,
        NextUnread,
        PrevUnread,
        NextChannel,
        PrevChannel,
        JumpToUnread,
        PopOut,
        MarkAllRead,
        OpenSettings,
        ShowShortcuts,
        NewConversation,
        CloseInspector,
        FocusComposer,
        GoBack,
        Quit,
        // composer
        MentionNext,
        MentionPrev,
        MentionPick,
        MentionDismiss,
        CancelEdit,
        EditLast,
        SendNow,
        // palette
        PaletteNext,
        PalettePrev,
        PalettePick,
        PaletteClose,
    ]
);

pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-k", OpenPalette, Some("Main")),
        KeyBinding::new("ctrl-shift-f", OpenSearch, Some("Main")),
        KeyBinding::new("ctrl-shift-i", OpenInbox, Some("Main")),
        KeyBinding::new("ctrl-shift-d", ToggleDeepWork, Some("Main")),
        // As in Slack and Discord: Alt+arrows walk the list, with Shift only the unread.
        KeyBinding::new("alt-down", NextChannel, Some("Main")),
        KeyBinding::new("alt-up", PrevChannel, Some("Main")),
        KeyBinding::new("alt-shift-down", NextUnread, Some("Main")),
        KeyBinding::new("alt-shift-up", PrevUnread, Some("Main")),
        KeyBinding::new("ctrl-j", JumpToUnread, Some("Main")),
        KeyBinding::new("shift-pageup", JumpToUnread, Some("Main")),
        KeyBinding::new("ctrl-.", ShowShortcuts, Some("Main")),
        KeyBinding::new("ctrl-o", PopOut, Some("Main")),
        KeyBinding::new("shift-escape", MarkAllRead, Some("Main")),
        KeyBinding::new("ctrl-,", OpenSettings, Some("Main")),
        KeyBinding::new("ctrl-/", ShowShortcuts, Some("Main")),
        KeyBinding::new("ctrl-n", NewConversation, Some("Main")),
        KeyBinding::new("escape", CloseInspector, Some("Main")),
        KeyBinding::new("ctrl-l", FocusComposer, Some("Main")),
        KeyBinding::new("alt-left", GoBack, Some("Main")),
        KeyBinding::new("ctrl-q", Quit, None),
        // The composer's own keys win over the text field's (they are bound later, at the same depth).
        KeyBinding::new("down", MentionNext, Some("ComposerMentions > Input")),
        KeyBinding::new("up", MentionPrev, Some("ComposerMentions > Input")),
        KeyBinding::new("tab", MentionPick, Some("ComposerMentions > Input")),
        KeyBinding::new("enter", MentionPick, Some("ComposerMentions > Input")),
        KeyBinding::new("escape", MentionDismiss, Some("ComposerMentions > Input")),
        KeyBinding::new("escape", CancelEdit, Some("ComposerEditing > Input")),
        KeyBinding::new("up", EditLast, Some("ComposerEmpty > Input")),
        KeyBinding::new("ctrl-enter", SendNow, Some("Composer > Input")),
        KeyBinding::new("ctrl-enter", SendNow, Some("ComposerEmpty > Input")),
        KeyBinding::new("ctrl-enter", SendNow, Some("ComposerEditing > Input")),
        KeyBinding::new("down", PaletteNext, Some("Palette")),
        KeyBinding::new("up", PalettePrev, Some("Palette")),
        KeyBinding::new("enter", PalettePick, Some("Palette")),
        KeyBinding::new("escape", PaletteClose, Some("Palette")),
    ]);
}

/// (keys, i18n key) for the shortcuts sheet.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl K", "shortcut.palette"),
    ("Ctrl Shift F", "shortcut.search"),
    ("Ctrl Shift I", "shortcut.inbox"),
    ("Ctrl Shift D", "shortcut.deepWork"),
    ("Ctrl N", "shortcut.newConversation"),
    ("Alt ↓", "shortcut.nextChannel"),
    ("Alt ↑", "shortcut.prevChannel"),
    ("Alt Shift ↓", "shortcut.nextUnread"),
    ("Alt Shift ↑", "shortcut.prevUnread"),
    ("Ctrl J", "shortcut.jumpToUnread"),
    ("Ctrl O", "shortcut.popOut"),
    ("Esc", "shortcut.markRead"),
    ("Shift Esc", "shortcut.markAllRead"),
    ("Ctrl L", "shortcut.focusComposer"),
    ("↑", "shortcut.editLast"),
    ("Ctrl ,", "shortcut.settings"),
    ("Ctrl /", "shortcut.shortcuts"),
];
