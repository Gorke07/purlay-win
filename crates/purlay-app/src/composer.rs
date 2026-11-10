//! The message box (UX §5): Enter sends unless the caret is inside an open code fence,
//! Shift+Enter is a new line; `@` suggests people; files upload as soon as they are
//! picked, pasted or dropped, and a send waits for them; drafts are kept per channel and
//! thread; ↑ in an empty box edits the person's last message.

use crate::actions::*;
use crate::i18n::{t, tf};
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::constants::{ATTACHMENTS_PER_MESSAGE_MAX, ATTACHMENT_BYTES_MAX, MESSAGE_CONTENT_MAX};
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::prefs::Prefs;
use purlay_core::session::Session;
use std::time::Duration;

pub enum ComposerEvent {
    Sent,
    /// ↑ in an empty box: the owner finds the person's last message and calls `edit`.
    EditLast,
}

#[derive(Clone)]
enum UploadState {
    Uploading,
    Done(Attachment),
    Failed(String),
}

#[derive(Clone)]
struct Upload {
    key: u64,
    name: String,
    state: UploadState,
}

struct Mentions {
    start: usize,
    query: String,
    selected: usize,
    people: Vec<User>,
}

pub struct Composer {
    session: Session,
    workspace: Uuid,
    pub channel: Uuid,
    pub parent: Option<Uuid>,
    pub text: Entity<TextareaState>,
    uploads: Vec<Upload>,
    next_key: u64,
    editing: Option<Message>,
    mentions: Option<Mentions>,
    draft_key: String,
    save_draft: Option<Task<()>>,
    placeholder: String,
    disabled_reason: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(session: Session, workspace: Uuid, channel: Uuid, parent: Option<Uuid>, placeholder: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let server = session.server_key();
        let draft_key = Prefs::draft_key(&server, &session.user_id, &channel, parent.as_deref());
        let draft = session.prefs.draft(&draft_key);
        let ph = placeholder.clone();
        let text = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 12).submit_on_enter(true).placeholder(ph).default_value(draft));
        let subs = vec![cx.subscribe_in(&text, window, |this, _, e: &InputEvent, window, cx| match e {
            InputEvent::Change => this.changed(window, cx),
            InputEvent::PressEnter { shift: false, .. } => this.enter(window, cx),
            _ => {}
        })];
        Composer {
            session,
            workspace,
            channel,
            parent,
            text,
            uploads: vec![],
            next_key: 0,
            editing: None,
            mentions: None,
            draft_key,
            save_draft: None,
            placeholder,
            disabled_reason: None,
            _subs: subs,
        }
    }

    pub fn set_disabled(&mut self, reason: Option<String>, cx: &mut Context<Self>) {
        if self.disabled_reason != reason {
            self.disabled_reason = reason;
            cx.notify();
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.text.update(cx, |s, cx| s.focus(window, cx));
    }

    fn value(&self, cx: &App) -> String {
        self.text.read(cx).value().to_string()
    }

    fn set_value(&mut self, v: &str, window: &mut Window, cx: &mut Context<Self>) {
        let v = v.to_string();
        self.text.update(cx, |s, cx| s.set_value(v, window, cx));
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let v = self.value(cx);
        // Enter inserts a line while the caret is inside an open fence.
        let cursor = self.text.read(cx).cursor().min(v.len());
        let before = &v[..floor_char(&v, cursor)];
        let in_fence = purlay_core::markdown::ends_in_open_code(before);
        self.text.update(cx, |s, cx| s.set_submit_on_enter(!in_fence, cx));
        if !v.trim().is_empty() && self.editing.is_none() {
            self.session.typing(&self.channel);
        }
        self.update_mentions(before, cx);
        if self.editing.is_none() {
            let key = self.draft_key.clone();
            let prefs = self.session.prefs.clone();
            self.save_draft = Some(cx.spawn_in(window, async move |_, cx| {
                cx.background_executor().timer(Duration::from_millis(400)).await;
                prefs.set_draft(&key, &v);
            }));
        }
        cx.notify();
    }

    fn update_mentions(&mut self, before: &str, cx: &mut Context<Self>) {
        let found = mention_query(before);
        match found {
            Some((start, query)) => {
                let people = self.people_matching(&query, cx);
                if people.is_empty() {
                    self.mentions = None;
                } else {
                    let selected = self.mentions.as_ref().filter(|m| m.start == start).map(|m| m.selected.min(people.len() - 1)).unwrap_or(0);
                    self.mentions = Some(Mentions { start, query, selected, people });
                }
            }
            None => self.mentions = None,
        }
    }

    fn people_matching(&self, q: &str, _cx: &App) -> Vec<User> {
        let store = self.session.store().read_recursive();
        let q = q.to_lowercase();
        let members = store.cache.get::<WorkspaceMemberList>(&purlay_core::store::Key::Members(self.workspace.clone()));
        let mut out: Vec<User> = members
            .map(|m| m.iter().map(|x| x.user.clone()).collect())
            .unwrap_or_else(|| store.people.values().cloned().collect());
        out.retain(|u| u.id != self.session.user_id && (u.username.to_lowercase().starts_with(&q) || u.display_name.to_lowercase().split_whitespace().any(|w| w.starts_with(&q))));
        out.sort_by_key(|u| u.username.to_lowercase());
        out.truncate(8);
        out
    }

    fn pick_mention(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.mentions.take() else { return };
        let Some(u) = m.people.get(ix.unwrap_or(m.selected)).cloned() else { return };
        let cursor = self.text.read(cx).cursor();
        let insert = format!("@{} ", u.username);
        self.text.update(cx, |s, cx| {
            s.set_selected_range(m.start..cursor, cx);
            s.replace(insert, window, cx);
        });
        cx.notify();
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mentions.is_some() {
            self.pick_mention(None, window, cx);
            return;
        }
        self.send(window, cx);
    }

    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled_reason.is_some() {
            return;
        }
        let raw = self.value(cx);
        let mut content = raw.trim_end().to_string();
        if self.editing.is_none() && self.run_command(&content, window, cx) {
            self.set_value("", window, cx);
            self.session.prefs.set_draft(&self.draft_key, "");
            cx.notify();
            return;
        }
        if let Some(rest) = content.strip_prefix("/shrug") {
            content = format!("{} ¯\\\\_(ツ)_/¯", rest.trim()).trim().to_string();
        } else if let Some(rest) = content.strip_prefix("/me ") {
            content = format!("_{}_", rest.trim());
        }
        if let Some(m) = self.editing.take() {
            if !content.trim().is_empty() && content != m.content {
                let h = self.session.edit(&m, content);
                spawn_report(h, window, cx);
            }
            self.set_value("", window, cx);
            let draft = self.session.prefs.draft(&self.draft_key);
            self.set_value(&draft, window, cx);
            cx.notify();
            return;
        }
        if self.uploads.iter().any(|u| matches!(u.state, UploadState::Uploading)) {
            window.push_notification(t("composer.uploading"), cx);
            return;
        }
        let attachments: Vec<Attachment> = self.uploads.iter().filter_map(|u| if let UploadState::Done(a) = &u.state { Some(a.clone()) } else { None }).collect();
        if content.trim().is_empty() && attachments.is_empty() {
            return;
        }
        if content.encode_utf16().count() > MESSAGE_CONTENT_MAX as usize {
            window.push_notification(tf("error.action", &[&format!("> {MESSAGE_CONTENT_MAX}")]), cx);
            return;
        }
        // The id comes back on the receiver; nothing here needs it.
        drop(self.session.send_message(&self.channel, self.parent.as_ref(), content, attachments));
        self.uploads.clear();
        self.mentions = None;
        self.session.prefs.set_draft(&self.draft_key, "");
        self.save_draft = None;
        self.set_value("", window, cx);
        cx.emit(ComposerEvent::Sent);
        cx.notify();
    }

    /// Sends later instead of now (`capabilities.scheduledMessages`).
    fn schedule(&mut self, at: purlay_core::contract::Timestamp, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.value(cx).trim_end().to_string();
        if content.trim().is_empty() || self.uploads.iter().any(|u| matches!(u.state, UploadState::Uploading)) {
            return;
        }
        let ids: Vec<Uuid> = self.uploads.iter().filter_map(|u| if let UploadState::Done(a) = &u.state { Some(a.id.clone()) } else { None }).collect();
        let h = self.session.schedule(&self.channel, self.parent.as_ref(), content, at.clone(), ids);
        let when = crate::ui::short_datetime(at.unix_ms().unwrap_or(0));
        spawn_report(h, window, cx);
        window.push_notification(tf("scheduled.at", &[&when]), cx);
        self.uploads.clear();
        self.session.prefs.set_draft(&self.draft_key, "");
        self.set_value("", window, cx);
        cx.notify();
    }

    /// Slash commands that act instead of sending (UX §5). Returns whether one ran.
    fn run_command(&mut self, content: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(cmd) = content.strip_prefix('/') else { return false };
        let (name, arg) = cmd.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((cmd, ""));
        let s = self.session.clone();
        let ch = self.channel.clone();
        match name {
            "star" => {
                s.star(&ch, true);
            }
            "unstar" => {
                s.star(&ch, false);
            }
            "mute" => {
                s.set_notification_level(&ch, NotificationLevel::None);
            }
            "unmute" => {
                s.set_notification_level(&ch, NotificationLevel::All);
            }
            "leave" => {
                s.leave(&ch);
            }
            "deepwork" => {
                let mins = arg.trim_end_matches('m').parse::<i64>().unwrap_or(60).clamp(5, 24 * 60 - 1);
                s.set_deep_work(&self.workspace, Some(crate::sidebar::deep_work_until(mins)));
                window.push_notification(t("deep.body"), cx);
            }
            _ => return false,
        }
        true
    }

    /// Puts a message in the box to edit it.
    pub fn edit(&mut self, m: Message, window: &mut Window, cx: &mut Context<Self>) {
        let content = m.content.clone();
        self.editing = Some(m);
        self.set_value(&content, window, cx);
        self.focus(window, cx);
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            let draft = self.session.prefs.draft(&self.draft_key);
            self.set_value(&draft, window, cx);
            cx.notify();
        }
    }

    pub fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            match std::fs::metadata(&p) {
                Ok(md) if md.len() > ATTACHMENT_BYTES_MAX as u64 => {
                    window.push_notification(tf("composer.tooLarge", &[&name]), cx);
                    continue;
                }
                Err(_) => continue,
                _ => {}
            }
            let path = p.clone();
            self.start_upload(name, move || std::fs::read(&path).map_err(|e| e.to_string()), window, cx);
        }
    }

    fn start_upload(&mut self, name: String, read: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploads.len() >= ATTACHMENTS_PER_MESSAGE_MAX as usize {
            window.push_notification(tf("composer.tooMany", &[&ATTACHMENTS_PER_MESSAGE_MAX]), cx);
            return;
        }
        let key = self.next_key;
        self.next_key += 1;
        self.uploads.push(Upload { key, name: name.clone(), state: UploadState::Uploading });
        let session = self.session.clone();
        let ws = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            let rt = session.rt.clone();
            let bytes = rt.spawn_blocking(read).await.map_err(|e| e.to_string()).and_then(|r| r);
            let result = match bytes {
                Ok(b) => session.upload(&ws, name, b).await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string())),
                Err(e) => Err(e),
            };
            let _ = this.update(cx, |me, cx| {
                if let Some(u) = me.uploads.iter_mut().find(|u| u.key == key) {
                    u.state = match result {
                        Ok(a) => UploadState::Done(a),
                        Err(e) => UploadState::Failed(e),
                    };
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_upload(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(pos) = self.uploads.iter().position(|u| u.key == key) {
            let u = self.uploads.remove(pos);
            if let UploadState::Done(a) = u.state {
                self.session.discard_upload(&a.id);
            }
            cx.notify();
        }
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |me, window, cx| me.attach_paths(paths, window, cx));
            }
        })
        .detach();
    }

    fn paste(&mut self, item: &ClipboardItem, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut handled = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let ext = match image.format {
                        ImageFormat::Jpeg => "jpg",
                        ImageFormat::Gif => "gif",
                        ImageFormat::Webp => "webp",
                        _ => "png",
                    };
                    let bytes = image.bytes.clone();
                    self.start_upload(format!("pasted-image.{ext}"), move || Ok(bytes), window, cx);
                    handled = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.attach_paths(paths.paths().to_vec(), window, cx);
                    handled = true;
                }
                _ => {}
            }
        }
        handled
    }

    fn insert_emoji(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.new(|cx| crate::emoji_picker::EmojiPicker::new(self.session.prefs.clone(), window, cx));
        let text = self.text.clone();
        picker.update(cx, |p, _| {
            p.on_pick(move |e, window, cx| {
                let e = e.to_string();
                text.update(cx, |s, cx| s.insert(e, window, cx));
                window.close_dialog(cx);
                text.update(cx, |s, cx| s.focus(window, cx));
            })
        });
        let focus = picker.read(cx).focus_handle(cx);
        window.open_dialog(cx, move |d, _, _| d.title(t("composer.emoji")).width(px(380.)).child(picker.clone()));
        window.focus(&focus, cx);
    }

    fn key_context(&self, cx: &App) -> &'static str {
        if self.mentions.is_some() {
            "ComposerMentions"
        } else if self.editing.is_some() {
            "ComposerEditing"
        } else if self.text.read(cx).value().is_empty() {
            "ComposerEmpty"
        } else {
            "Composer"
        }
    }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `…@ad|` → (offset of `@`, "ad") when the caret ends a mention being typed.
pub fn mention_query(before: &str) -> Option<(usize, String)> {
    let at = before.rfind('@')?;
    let q = &before[at + 1..];
    if q.len() > 39 || !q.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let prev = before[..at].chars().last();
    if prev.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '@' || c == '`') {
        return None;
    }
    Some((at, q.to_string()))
}

fn spawn_report<T: Send + 'static>(h: tokio::task::JoinHandle<purlay_core::api::ApiResult<T>>, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            if let Ok(Err(e)) = h.await {
                let msg = tf("error.action", &[&e]);
                let _ = cx.update(|window, cx| window.push_notification(msg, cx));
            }
        })
        .detach();
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let ctx = self.key_context(cx);
        let disabled = self.disabled_reason.clone();
        let this = cx.entity().downgrade();

        let mentions = self.mentions.as_ref().map(|m| {
            let hover = k.surface_050;
            v_flex()
                .absolute()
                // Above the box, never over what is being typed.
                .bottom(relative(1.))
                .mb_1()
                .left(px(8.))
                .w(px(320.))
                .p_1()
                .rounded(px(purlay_tokens::radius::RADIUS_2XL))
                .bg(k.surface_100)
                .border_1()
                .border_color(k.border_subtle)
                .shadow_lg()
                .children(m.people.iter().enumerate().map(|(i, u)| {
                    let sel = i == m.selected;
                    h_flex()
                        .id(("mention", i))
                        .h(px(32.))
                        .px_2()
                        .gap_2()
                        .rounded(px(purlay_tokens::radius::LG))
                        .cursor_pointer()
                        .when(sel, |d| d.bg(k.accent_subtle))
                        .when(!sel, |d| d.hover(move |s| s.bg(hover)))
                        .child(avatar(u.username.clone(), &u.display_name, 20., cx))
                        .child(body(u.display_name.clone(), cx))
                        .child(caption(format!("@{}", u.username), cx))
                        .on_click(cx.listener(move |this, _, window, cx| this.pick_mention(Some(i), window, cx)))
                }))
                .child(div().px_2().pt_1().child(caption(format!("@{}", m.query), cx)))
        });

        let uploads = (!self.uploads.is_empty()).then(|| {
            h_flex().flex_wrap().gap_2().px_3().pt_3().children(self.uploads.iter().map(|u| {
                let key = u.key;
                let (label_text, color) = match &u.state {
                    UploadState::Uploading => (format!("{} · {}", u.name, t("composer.uploading")), k.ink_muted),
                    UploadState::Done(a) => (format!("{} · {}", u.name, crate::message::human_size(a.size_bytes)), k.ink),
                    UploadState::Failed(e) => (tf("composer.uploadFailed", &[e]), k.danger_text),
                };
                h_flex()
                    .h(px(28.))
                    .pl_2()
                    .pr_1()
                    .gap_1()
                    .rounded(px(purlay_tokens::radius::MD))
                    .bg(k.surface_200)
                    .child(ico("paperclip", 13., k.ink_muted))
                    .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(color).max_w(px(240.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(label_text))
                    .child(icon_button(("rm-upload", key as usize), "x", "delete", cx).size(px(22.)).on_click(cx.listener(move |this, _, _, cx| this.remove_upload(key, cx))))
            }))
        });

        let editing_banner = self.editing.as_ref().map(|_| {
            h_flex()
                .px_3()
                .pt_2()
                .gap_2()
                .child(ico("pencil", 13., k.accent_text))
                .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.accent_text).child(t("composer.editing")))
                .child(caption("· Esc", cx))
        });

        let dark = crate::theme::Tokens::get(cx).dark;
        let edge = if dark { k.border_strong.opacity(0.35) } else { k.border_strong.opacity(0.45) };
        let has_text = !self.text.read(cx).value().trim().is_empty() || self.uploads.iter().any(|u| matches!(u.state, UploadState::Done(_)));
        v_flex()
            .relative()
            .key_context(ctx)
            .on_action(cx.listener(|this, _: &MentionNext, _, cx| {
                if let Some(m) = &mut this.mentions {
                    m.selected = (m.selected + 1) % m.people.len().max(1);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &MentionPrev, _, cx| {
                if let Some(m) = &mut this.mentions {
                    m.selected = (m.selected + m.people.len().max(1) - 1) % m.people.len().max(1);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &MentionPick, window, cx| this.pick_mention(None, window, cx)))
            .on_action(cx.listener(|this, _: &MentionDismiss, _, cx| {
                this.mentions = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CancelEdit, window, cx| this.cancel_edit(window, cx)))
            .on_action(cx.listener(|_, _: &EditLast, _, cx| cx.emit(ComposerEvent::EditLast)))
            .on_action(cx.listener(|this, _: &SendNow, window, cx| this.send(window, cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| this.attach_paths(paths.paths().to_vec(), window, cx)))
            .child(
                v_flex()
                    .rounded(px(16.))
                    .border_1()
                    .border_color(edge)
                    .bg(if dark { k.surface_050 } else { k.surface_100 })
                    .shadow_md()
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_color(k.accent).bg(k.accent_subtle))
                    .children(editing_banner)
                    .children(uploads)
                    .child(
                        div().px(px(6.)).pt(px(6.)).min_h(px(44.)).child(
                            Textarea::new(&self.text)
                                .appearance(false)
                                .disabled(disabled.is_some())
                                .on_paste(move |item, window, cx| this.update(cx, |me, cx| me.paste(item, window, cx)).unwrap_or(false)),
                        ),
                    )
                    .child(
                        h_flex()
                            .px(px(10.))
                            .pb(px(10.))
                            .gap(px(2.))
                            .child(icon_button("attach", "plus", "composer.attach", cx).on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))))
                            .child(icon_button("mention", "at-sign", "composer.mention", cx).on_click(cx.listener(|this, _, window, cx| {
                                this.text.update(cx, |s, cx| {
                                    s.insert("@", window, cx);
                                    s.focus(window, cx);
                                });
                            })))
                            .child(icon_button("emoji", "smile", "composer.emoji", cx).on_click(cx.listener(|this, _, window, cx| this.insert_emoji(window, cx))))
                            .when(self.session.capability(|c| c.scheduled_messages) && self.editing.is_none(), |d| {
                                let me = cx.entity().downgrade();
                                d.child(
                                    Button::new("schedule")
                                        .ghost()
                                        .small()
                                        .icon(crate::assets::icon("calendar-clock"))
                                        .tooltip(t("composer.schedule"))
                                        .dropdown_menu(move |mut menu, _, _| {
                                            menu = menu.label(t("scheduled.pick"));
                                            for (key, mins) in [("deep.30m", 30i64), ("deep.1h", 60), ("deep.2h", 120), ("msg.remind.tomorrow", -1)] {
                                                let me = me.clone();
                                                menu = menu.item(PopupMenuItem::new(t(key)).on_click(move |_, window, cx| {
                                                    let at = crate::sidebar::deep_work_until(mins);
                                                    let _ = me.update(cx, |c, cx| c.schedule(at, window, cx));
                                                }));
                                            }
                                            menu
                                        }),
                                )
                            })
                            .child(div().flex_1().children(disabled.clone().map(|d| caption(d, cx))))
                            .child({
                                // A round send button that fills with the accent once there is something to send.
                                let ready = has_text && disabled.is_none();
                                let (bg, fg) = if ready { (k.accent, k.on_accent) } else { (k.surface_200, k.ink_subtle) };
                                let hover = if ready { k.accent_hover } else { k.surface_200 };
                                div()
                                    .id("send")
                                    .size(px(32.))
                                    .rounded_full()
                                    .bg(bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(ready, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
                                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t("composer.send")).build(w, cx))
                                    .child(ico("arrow-up", 17., fg))
                                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                            }),
                    ),
            )
            .children(mentions)
            .when(self.placeholder.is_empty(), |d| d)
    }
}

#[cfg(test)]
mod tests {
    use super::mention_query;

    #[test]
    fn finds_the_mention_being_typed() {
        assert_eq!(mention_query("hi @ad"), Some((3, "ad".into())));
        assert_eq!(mention_query("@"), Some((0, String::new())));
        assert_eq!(mention_query("mail@ad"), None);
        assert_eq!(mention_query("hi @ad min"), None);
        assert_eq!(mention_query("`@x"), None);
    }
}
