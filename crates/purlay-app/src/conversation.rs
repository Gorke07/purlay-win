//! A channel: its header, the feed and the composer (UX §3, §6).
//!
//! The feed keeps the reader's place: it follows the bottom only when already there, and
//! otherwise counts what arrived in an "N new messages" pill. The "New" marker sits at the
//! last-read position for as long as the channel is open. The channel is marked read when
//! its bottom is reached while the window is in front.

use crate::composer::{Composer, ComposerEvent};
use crate::i18n::{t, tf};
use crate::message::{message_row, pending_row, MsgCtx};
use crate::nav::{Inspector, Nav, NavEvent};
use crate::sidebar::channel_title;
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::Sizable as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::realtime::connection::Status;
use purlay_core::session::Session;
use purlay_core::store::Key;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Clone, PartialEq)]
enum Row {
    Older,
    Beginning,
    Day(String),
    NewMarker,
    Msg { id: Uuid, compact: bool },
    Pending(Uuid),
    Newer,
}

impl Row {
    fn key(&self) -> String {
        match self {
            Row::Older => "older".into(),
            Row::Beginning => "beginning".into(),
            Row::Day(d) => format!("day:{d}"),
            Row::NewMarker => "new".into(),
            Row::Msg { id, .. } => format!("m:{id}"),
            Row::Pending(id) => format!("p:{id}"),
            Row::Newer => "newer".into(),
        }
    }
}

const COMPACT_WINDOW_MS: i64 = 5 * 60 * 1000;

pub struct ConversationView {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    session: Session,
    pub channel: Uuid,
    workspace: Uuid,
    list: ListState,
    rows: Rc<Vec<Row>>,
    hashes: Vec<u64>,
    last_rev: u64,
    pub composer: Entity<Composer>,
    marker_after: Option<Uuid>,
    unseen: usize,
    last_marked: Option<Uuid>,
    pending_reveal: Option<Uuid>,
    _subs: Vec<Subscription>,
}

impl ConversationView {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, channel: Uuid, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session = app.read(cx).session().cloned().expect("session");
        let (workspace, title, last_read) = {
            let store = session.store().read_recursive();
            let ch = store.channel(&channel).cloned();
            let w = store.workspace_of_channel(&channel).unwrap_or_default();
            let title = ch.as_ref().map(|c| channel_title(c, &session.user_id)).unwrap_or_default();
            let last = ch.and_then(|c| c.state).filter(|s| s.unread_count > 0).and_then(|s| s.last_read_message_id);
            (w, title, last)
        };
        let is_dm = session.store().read_recursive().channel(&channel).is_some_and(|c| c.r#type == ChannelType::DirectMessage);
        let placeholder = if is_dm { tf("composer.placeholderDm", &[&title]) } else { tf("composer.placeholder", &[&format!("#{title}")]) };
        let composer = cx.new(|cx| Composer::new(session.clone(), workspace.clone(), channel.clone(), None, placeholder, window, cx));
        let list = ListState::new(0, ListAlignment::Bottom, px(800.));
        list.set_follow_mode(FollowMode::Tail);
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |ev, _, cx| {
            let range = ev.visible_range.clone();
            let following = ev.is_following_tail;
            let _ = weak.update(cx, |me, cx| me.on_scroll(range, following, cx));
        });
        let mut subs = vec![
            cx.observe(&app, |this, _, cx| {
                this.refresh(cx);
                // The window coming to the front is when a feed already at its bottom is read.
                let following = this.list.is_following_tail();
                this.mark_read_if_seen(following, cx);
            }),
            cx.subscribe_in(&composer, window, |this, _, e: &ComposerEvent, window, cx| match e {
                ComposerEvent::Sent => {
                    this.list.set_follow_mode(FollowMode::Tail);
                    this.unseen = 0;
                    cx.notify();
                }
                ComposerEvent::EditLast => this.edit_last(window, cx),
            }),
            cx.subscribe_in(&nav, window, |this, _, e: &NavEvent, window, cx| match e {
                NavEvent::Reveal(id) => {
                    this.pending_reveal = Some(id.clone());
                    this.refresh(cx);
                }
                NavEvent::FocusComposer => {
                    if this.nav.read(cx).current_channel() == Some(&this.channel) {
                        this.composer.update(cx, |c, cx| c.focus(window, cx));
                    }
                }
                NavEvent::Edit { message, in_thread: false } if message.channel_id == this.channel => {
                    let m = (**message).clone();
                    this.composer.update(cx, |c, cx| c.edit(m, window, cx));
                }
                _ => {}
            }),
        ];
        subs.push(cx.observe(&crate::images::images(cx), |this, _, cx| {
            let n = this.rows.len();
            this.list.remeasure_items(0..n);
            cx.notify();
        }));
        let mut me = ConversationView {
            app,
            nav,
            session,
            channel,
            workspace,
            list,
            rows: Rc::new(vec![]),
            hashes: vec![],
            last_rev: u64::MAX,
            composer,
            marker_after: last_read,
            unseen: 0,
            last_marked: None,
            pending_reveal: None,
            _subs: subs,
        };
        me.refresh(cx);
        // Opened by choosing it: the caret goes to its message box (the nav event that asks
        // for this was sent before this view existed).
        let composer = me.composer.clone();
        window.defer(cx, move |window, cx| composer.update(cx, |c, cx| c.focus(window, cx)));
        me
    }

    /// Ctrl+J: brings the first unread message (the "New" line) into view.
    pub fn jump_to_unread(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.rows.iter().position(|r| *r == Row::NewMarker) {
            self.list.set_follow_mode(FollowMode::Normal);
            self.list.scroll_to_reveal_item(ix);
            cx.notify();
        }
    }

    /// Esc: the channel is read up to its newest message, and the "New" line goes.
    pub fn mark_read_now(&mut self, cx: &mut Context<Self>) {
        let newest = self.session.store().read_recursive().windows.get(&self.channel).filter(|w| w.reaches_newest).and_then(|w| w.newest_id().cloned());
        if let Some(n) = newest {
            self.last_marked = Some(n.clone());
            self.session.mark_read(&self.channel, &n);
        }
        if self.marker_after.take().is_some() {
            self.last_rev = u64::MAX;
            self.refresh(cx);
        }
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    fn edit_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = self.session.user_id.clone();
        let last = self.session.store().read_recursive().windows.get(&self.channel).and_then(|w| {
            w.messages.iter().rev().find(|m| m.sender.as_ref().is_some_and(|s| s.id == me) && m.deleted_at.is_none()).cloned()
        });
        if let Some(m) = last {
            self.composer.update(cx, |c, cx| c.edit(m, window, cx));
        }
    }

    fn on_scroll(&mut self, range: std::ops::Range<usize>, following: bool, cx: &mut Context<Self>) {
        if range.start == 0 && self.rows.first() == Some(&Row::Older) {
            self.session.send(purlay_core::engine::Command::LoadOlder(self.channel.clone()));
        }
        if range.end >= self.rows.len() && self.rows.last() == Some(&Row::Newer) {
            self.session.send(purlay_core::engine::Command::LoadNewer(self.channel.clone()));
        }
        if following && self.unseen > 0 {
            self.unseen = 0;
            cx.notify();
        }
        self.mark_read_if_seen(following, cx);
    }

    fn mark_read_if_seen(&mut self, at_bottom: bool, cx: &mut Context<Self>) {
        // Only the channel on screen, in a window in front, is read.
        if !at_bottom || !self.app.read(cx).window_active || self.nav.read(cx).current_channel() != Some(&self.channel) {
            return;
        }
        let newest = {
            let store = self.session.store().read_recursive();
            store.windows.get(&self.channel).filter(|w| w.reaches_newest).and_then(|w| w.newest_id().cloned())
        };
        if let Some(n) = newest {
            if self.last_marked.as_ref() != Some(&n) {
                self.last_marked = Some(n.clone());
                self.session.mark_read(&self.channel, &n);
            }
        }
    }

    /// Rebuilds the rows when the store moved, keeping the reader's place.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let rev = self.session.store().read_recursive().revision;
        if rev == self.last_rev && self.pending_reveal.is_none() {
            return;
        }
        self.last_rev = rev;
        let (rows, hashes) = self.build_rows();
        let old_keys: Vec<String> = self.rows.iter().map(Row::key).collect();
        let new_keys: Vec<String> = rows.iter().map(Row::key).collect();
        let mut pre = 0;
        while pre < old_keys.len() && pre < new_keys.len() && old_keys[pre] == new_keys[pre] {
            pre += 1;
        }
        let mut suf = 0;
        while suf < old_keys.len() - pre && suf < new_keys.len() - pre && old_keys[old_keys.len() - 1 - suf] == new_keys[new_keys.len() - 1 - suf] {
            suf += 1;
        }
        let old_mid = pre..old_keys.len() - suf;
        let new_mid_len = new_keys.len() - suf - pre;
        let appended_messages = suf == 0 && old_mid.is_empty() && new_mid_len > 0 && pre == old_keys.len();
        if !old_mid.is_empty() || new_mid_len > 0 {
            if self.rows.is_empty() {
                self.list.reset(rows.len());
            } else {
                self.list.splice(old_mid.clone(), new_mid_len);
            }
        }
        // Rows that stayed but changed (an edit, a reaction, a thread line) are measured again.
        for (i, (old, new)) in self.hashes.iter().zip(hashes.iter()).take(pre).enumerate() {
            if old != new {
                self.list.remeasure_items(i..i + 1);
            }
        }
        for j in 0..suf {
            let (oi, ni) = (old_keys.len() - 1 - j, new_keys.len() - 1 - j);
            if self.hashes.get(oi) != hashes.get(ni) {
                self.list.remeasure_items(ni..ni + 1);
            }
        }
        if appended_messages && !self.list.is_following_tail() && !self.rows.is_empty() {
            self.unseen += rows[pre..].iter().filter(|r| matches!(r, Row::Msg { .. })).count();
        }
        self.rows = Rc::new(rows);
        self.hashes = hashes;
        if let Some(target) = self.pending_reveal.clone() {
            let key = format!("m:{target}");
            if let Some(ix) = self.rows.iter().position(|r| r.key() == key) {
                self.list.scroll_to_reveal_item(ix);
                self.pending_reveal = None;
            }
        }
        let following = self.list.is_following_tail();
        self.mark_read_if_seen(following, cx);
        cx.notify();
    }

    fn build_rows(&self) -> (Vec<Row>, Vec<u64>) {
        let store = self.session.store().read_recursive();
        let mut rows = Vec::new();
        let mut hashes = Vec::new();
        let push = |rows: &mut Vec<Row>, hashes: &mut Vec<u64>, r: Row, h: u64| {
            rows.push(r);
            hashes.push(h);
        };
        let Some(w) = store.windows.get(&self.channel) else { return (rows, hashes) };
        push(&mut rows, &mut hashes, if w.reaches_oldest { Row::Beginning } else { Row::Older }, 0);
        let mut prev: Option<&Message> = None;
        let mut marker_done = self.marker_after.is_none();
        for m in &w.messages {
            let ms = m.created_at.unix_ms().unwrap_or(0);
            let new_day = prev.is_none_or(|p| !same_day(p.created_at.unix_ms().unwrap_or(0), ms));
            if new_day {
                push(&mut rows, &mut hashes, Row::Day(day_label(ms)), 0);
            }
            let mut marker_here = false;
            if !marker_done {
                if let Some(after) = &self.marker_after {
                    if m.id > *after && m.sender.as_ref().is_none_or(|s| s.id != self.session.user_id) {
                        push(&mut rows, &mut hashes, Row::NewMarker, 0);
                        marker_done = true;
                        marker_here = true;
                    }
                }
            }
            let compact = !new_day
                && !marker_here
                && prev.is_some_and(|p| {
                    p.sender.as_ref().map(|s| &s.id) == m.sender.as_ref().map(|s| &s.id)
                        && m.sender.is_some()
                        && ms - p.created_at.unix_ms().unwrap_or(0) < COMPACT_WINDOW_MS
                        && p.thread.as_ref().is_none_or(|t| t.reply_count == 0)
                });
            push(&mut rows, &mut hashes, Row::Msg { id: m.id.clone(), compact }, hash_message(m));
            prev = Some(m);
        }
        if w.reaches_newest {
            for p in store.pending_for(&self.channel, None) {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (p.delivery as u8).hash(&mut h);
                p.reason.hash(&mut h);
                push(&mut rows, &mut hashes, Row::Pending(p.id.clone()), h.finish());
            }
        } else {
            push(&mut rows, &mut hashes, Row::Newer, 0);
        }
        (rows, hashes)
    }

    fn msg_ctx(&self, cx: &App) -> MsgCtx {
        let store = self.session.store().read_recursive();
        let membership = store.membership(&self.workspace).cloned();
        let moderator = membership.as_ref().is_some_and(|m| matches!(m.role, UserRole::Owner | UserRole::Admin));
        let saved: HashSet<Uuid> = store.cache.get::<SavedIds>(&Key::SavedIds(self.workspace.clone())).map(|s| s.iter().map(|x| x.message_id.clone()).collect()).unwrap_or_default();
        let open_thread = match &self.nav.read(cx).inspector {
            Some(Inspector::Thread { root, .. }) => Some(root.clone()),
            _ => None,
        };
        MsgCtx {
            session: self.session.clone(),
            nav: self.nav.clone(),
            me: self.session.user_id.clone(),
            moderator,
            in_thread: false,
            open_thread,
            saved: Rc::new(saved),
            workspace_slug: membership.map(|m| m.workspace.slug.clone()),
            compact_density: self.session.prefs.get().density == purlay_core::prefs::Density::Compact,
        }
    }

    fn render_row(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let Some(row) = self.rows.get(ix).cloned() else { return div().into_any_element() };
        match row {
            Row::Older => div().h(px(48.)).flex().items_center().justify_center().child(caption(t("channel.loadOlder"), cx)).into_any_element(),
            Row::Newer => div().h(px(48.)).flex().items_center().justify_center().child(caption(t("loading"), cx)).into_any_element(),
            Row::Beginning => {
                let info = self.session.store().read_recursive().channel(&self.channel).cloned();
                let Some(ch) = info else { return div().into_any_element() };
                let me = &self.session.user_id;
                let name = channel_title(&ch, me);
                let dm = ch.r#type == ChannelType::DirectMessage;
                let tile: AnyElement = if dm {
                    let peer = ch.peers.iter().find(|p| p.id != *me).or(ch.peer.as_ref());
                    let key = peer.map(|p| p.username.clone()).unwrap_or_else(|| name.clone());
                    avatar(key, &name, 64., cx).into_any_element()
                } else {
                    div()
                        .size(px(64.))
                        .rounded(px(16.))
                        .bg(k.accent.opacity(if crate::theme::Tokens::get(cx).dark { 0.22 } else { 0.10 }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ico(if ch.r#type == ChannelType::Private { "lock" } else { "hash" }, 32., k.accent_text))
                        .into_any_element()
                };
                let title_text = if dm { name.clone() } else { format!("# {name}") };
                let sub = if dm {
                    Some(t("channel.emptyDm").to_string())
                } else {
                    ch.topic.clone().filter(|s| !s.is_empty()).or_else(|| Some(tf("channel.empty", &[&format!("#{name}")])))
                };
                v_flex()
                    .px(px(24.))
                    .pt(px(40.))
                    .pb_4()
                    .gap_3()
                    .child(tile)
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().font_family(crate::theme::Tokens::get(cx).sans).text_size(px(28.)).line_height(px(34.)).font_weight(FontWeight::BOLD).text_color(k.ink).child(title_text))
                            .children(sub.map(|s| text_style(div(), purlay_tokens::typography::BODY, cx).text_size(px(15.)).text_color(k.ink_muted).child(s))),
                    )
                    .into_any_element()
            }
            Row::Day(label_text) => h_flex()
                .w_full()
                .px(px(24.))
                .pt_4()
                .pb_2()
                .gap_4()
                .child(div().flex_1().h(px(1.)).bg(k.border_subtle))
                .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).font_weight(FontWeight::SEMIBOLD).child(label_text))
                .child(div().flex_1().h(px(1.)).bg(k.border_subtle))
                .into_any_element(),            Row::NewMarker => h_flex()
                .w_full()
                .px(px(24.))
                .py_1()
                .gap_2()
                .child(div().flex_1().h(px(1.)).bg(k.danger))
                .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.danger_text).font_weight(FontWeight::SEMIBOLD).child(t("channel.new")))
                .into_any_element(),
            Row::Msg { id, compact } => {
                let m = self.session.store().read_recursive().windows.get(&self.channel).and_then(|w| w.get(&id).cloned());
                let Some(m) = m else { return div().into_any_element() };
                let ctx = self.msg_ctx(cx);
                let highlight = self.nav.read(cx).highlight.as_ref() == Some(&id);
                message_row(&m, &ctx, compact, highlight, cx)
            }
            Row::Pending(id) => {
                let p = self.session.store().read_recursive().pending.iter().find(|p| p.id == id).cloned();
                let Some(p) = p else { return div().into_any_element() };
                let ctx = self.msg_ctx(cx);
                pending_row(&p, &ctx, cx)
            }
        }
    }

    fn connection_bar(&self, cx: &App) -> Option<AnyElement> {
        let k = c(cx);
        let store = self.session.store().read_recursive();
        let reconnecting = store.connected_once && !matches!(store.status, Some(Status::Open));
        let degraded = store.degraded.contains_key(self.channel.as_str()) || store.degraded.contains_key(purlay_core::store::UNNAMED);
        drop(store);
        if reconnecting {
            return Some(
                h_flex()
                    .px_4()
                    .py_1()
                    .gap_2()
                    .bg(k.warning_subtle)
                    .child(ico("wifi-off", 14., k.warning_text))
                    .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.warning_text).child(t("conn.reconnecting")))
                    .into_any_element(),
            );
        }
        if degraded {
            let s = self.session.clone();
            return Some(
                h_flex()
                    .px_4()
                    .py_1()
                    .gap_2()
                    .bg(k.warning_subtle)
                    .child(text_style(div(), purlay_tokens::typography::LABEL, cx).flex_1().text_color(k.warning_text).child(t("conn.degraded")))
                    .child(Button::new("retry-degraded").xsmall().ghost().label(t("retry")).on_click(move |_, _, _| s.send(purlay_core::engine::Command::RetryDegraded)))
                    .into_any_element(),
            );
        }
        None
    }

    fn typing_line(&self, cx: &App) -> AnyElement {
        let store = self.session.store().read_recursive();
        let names: Vec<String> = store.typing.get(&self.channel).map(|v| v.iter().filter_map(|u| store.user(u).map(|x| x.display_name.clone())).collect()).unwrap_or_default();
        drop(store);
        let text = match names.len() {
            0 => String::new(),
            1 => tf("composer.typing1", &[&names[0]]),
            2 => tf("composer.typing2", &[&names[0], &names[1]]),
            _ => t("composer.typingMany").to_string(),
        };
        // Who is typing, else how to send (under the box, as on the Mac).
        let line = if text.is_empty() { t("composer.hint").to_string() } else { text };
        div().h(px(24.)).px(px(6.)).pt(px(5.)).child(caption(line, cx).text_color(c(cx).ink_subtle)).into_any_element()
    }
}

fn hash_message(m: &Message) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.content.hash(&mut h);
    m.edited_at.hash(&mut h);
    m.deleted_at.hash(&mut h);
    for r in &m.reactions {
        r.emoji.hash(&mut h);
        r.user_ids.len().hash(&mut h);
    }
    if let Some(t) = &m.thread {
        t.reply_count.hash(&mut h);
        t.resolved_at.hash(&mut h);
        t.last_reply_at.hash(&mut h);
    }
    m.attachments.len().hash(&mut h);
    h.finish()
}

impl Render for ConversationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        self.refresh(cx);
        let (ch, gone, loading, failed, role) = {
            let store = self.session.store().read_recursive();
            let ch = store.channel(&self.channel).cloned();
            let gone = store.gone.get(&self.channel).cloned();
            let loading = store.window_loading.contains(&self.channel) && !store.windows.contains_key(&self.channel);
            let failed = store.window_failed.get(&self.channel).cloned();
            let role = store.membership(&self.workspace).map(|m| m.role.clone());
            (ch, gone, loading, failed, role)
        };
        let Some(ch) = ch else {
            let text = match gone.as_deref() {
                Some("archived") => t("channel.gone.archived"),
                Some("left") => t("channel.gone.left"),
                Some("removed") => t("channel.gone.removed"),
                _ if gone.is_some() => t("channel.gone"),
                _ => t("loading"),
            };
            return v_flex().size_full().child(empty_state(text, cx)).into_any_element();
        };
        let joined = ch.state.is_some() || ch.r#type == ChannelType::DirectMessage;
        let admin_only = ch.only_admins_post && !matches!(role, Some(UserRole::Owner) | Some(UserRole::Admin));
        self.composer.update(cx, |c, cx| c.set_disabled(if admin_only { Some(t("channel.onlyAdmins").into()) } else { None }, cx));

        let feed: AnyElement = if let Some(why) = failed.filter(|_| self.rows.is_empty()) {
            let s = self.session.clone();
            let c2 = self.channel.clone();
            error_state("retry-feed", format!("{} {}", t("error.load"), why), move |_, _, _| s.send(purlay_core::engine::Command::ReloadChannel(c2.clone())), cx).into_any_element()
        } else if loading || self.rows.is_empty() {
            skeleton_rows(6, cx).into_any_element()
        } else {
            list(self.list.clone(), cx.processor(|this, ix, window, cx| this.render_row(ix, window, cx)))
                .size_full()
                .pb_3()
                .into_any_element()
        };

        let pill = (self.unseen > 0).then(|| {
            let accent = k.accent;
            div().absolute().bottom(px(12.)).w_full().flex().justify_center().child(
                h_flex()
                    .id("new-pill")
                    .px_4()
                    .h(px(32.))
                    .gap_2()
                    .rounded_full()
                    .bg(accent)
                    .text_color(k.on_accent)
                    .shadow_md()
                    .cursor_pointer()
                    .child(ico("arrow-down", 13., k.on_accent))
                    .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.on_accent).child(tf("channel.newMessages", &[&self.unseen])))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.list.set_follow_mode(FollowMode::Tail);
                        this.unseen = 0;
                        cx.notify();
                    })),
            )
        });
        let jump = {
            let reaches = self.session.store().read_recursive().windows.get(&self.channel).map(|w| w.reaches_newest).unwrap_or(true);
            (!reaches).then(|| {
                let s = self.session.clone();
                let c2 = self.channel.clone();
                div().absolute().bottom(px(12.)).w_full().flex().justify_center().child(
                    h_flex()
                        .id("jump-recent")
                        .px_4()
                        .h(px(32.))
                        .gap_2()
                        .rounded_full()
                        .bg(k.accent)
                        .shadow_md()
                        .cursor_pointer()
                        .child(ico("arrow-down", 14., k.on_accent))
                        .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.on_accent).child(t("channel.jumpRecent")))
                        .on_click(move |_, _, _| s.send(purlay_core::engine::Command::ReloadChannel(c2.clone()))),
                )
            })
        };

        let bottom: AnyElement = if joined {
            v_flex().px(px(20.)).pb_2().child(self.composer.clone()).child(self.typing_line(cx)).into_any_element()
        } else {
            let s = self.session.clone();
            let c2 = self.channel.clone();
            h_flex()
                .px_4()
                .py_3()
                .gap_3()
                .border_t_1()
                .border_color(k.border_subtle)
                .child(muted(t("channel.notMember"), cx).flex_1())
                .child(Button::new("join").primary().label(t("channel.join")).on_click(move |_, _, _| {
                    s.join(&c2);
                }))
                .into_any_element()
        };
        let _ = window;
        v_flex()
            .size_full()
            .min_w_0()
            .bg(k.surface_100)
            .children(self.connection_bar(cx))
            .child(div().relative().flex_1().min_h_0().child(feed).children(pill).children(jump))
            .child(bottom)
            .into_any_element()
    }
}
