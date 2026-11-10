//! The destinations beside channels: Inbox, Search, Saved, Open Topics, Issues, Drafts,
//! Scheduled and Reminders. Each declares what it reads; the engine keeps that live.

use crate::i18n::{t, tf};
use crate::markdown_view::render_markdown;
use crate::nav::{Nav, Route};
use crate::sidebar::channel_title;
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::api::ApiResult;
use purlay_core::contract::constants::{SEARCH_QUERY_MAX, SEARCH_QUERY_MIN};
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::session::Session;
use purlay_core::store::{Key, LoadState};

fn page_header(title_text: impl Into<SharedString>, right: Option<AnyElement>, cx: &App) -> Div {
    let k = c(cx);
    // A large title over the page, as on the Mac; the toolbar row above stays clear.
    h_flex()
        .flex_none()
        .px(px(24.))
        .pt(px(20.))
        .pb_3()
        .gap_2()
        .child(div().text_size(px(24.)).line_height(px(30.)).font_weight(FontWeight::BOLD).text_color(k.ink).child(title_text.into()))
        .child(div().flex_1())
        .children(right)
}

fn tabs<T: Clone + PartialEq + 'static>(id: &'static str, items: Vec<(T, &'static str)>, current: &T, on: impl Fn(T, &mut App) + 'static, cx: &App) -> Div {
    let k = c(cx);
    let on = std::rc::Rc::new(on);
    let mut row = h_flex().gap_1().p(px(2.)).rounded(px(purlay_tokens::radius::LG)).bg(k.surface_200);
    for (i, (v, key)) in items.into_iter().enumerate() {
        let selected = &v == current;
        let on = on.clone();
        row = row.child(
            div()
                .id((id, i))
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(purlay_tokens::radius::MD))
                .cursor_pointer()
                .when(selected, |d| d.bg(k.surface_100).shadow_sm())
                .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(if selected { k.ink } else { k.ink_muted }).child(t(key)))
                .on_click(move |_, _, cx| on(v.clone(), cx)),
        );
    }
    row
}

/// A message found somewhere: its channel, sender, time and body; opens where it is.
pub fn result_card(id: impl Into<ElementId>, m: &Message, session: &Session, nav: &Entity<Nav>, extra: Option<AnyElement>, cx: &App) -> AnyElement {
    let k = c(cx);
    let store = session.store().read_recursive();
    // A direct message's "channel" is the other person, already named as the sender.
    let ch_name = store.channel(&m.channel_id).filter(|ch| ch.r#type != ChannelType::DirectMessage).map(|ch| format!("#{}", channel_title(ch, &session.user_id)));
    drop(store);
    let name = m.sender.as_ref().map(|s| s.display_name.clone()).unwrap_or_default();
    let username = m.sender.as_ref().map(|s| s.username.clone()).unwrap_or_default();
    let ms = m.created_at.unix_ms().unwrap_or(0);
    let (nav, ch, mid, parent) = (nav.clone(), m.channel_id.clone(), m.id.clone(), m.parent_id.clone());
    let hover = k.surface_050;
    div()
        .id(id)
        .mx_4()
        .my_1()
        .p_3()
        .rounded(px(purlay_tokens::radius::XL))
        .border_1()
        .border_color(k.border_subtle)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(
            h_flex()
                .gap_2()
                .child(avatar(username, &name, 20., cx))
                .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.ink).font_weight(FontWeight::SEMIBOLD).child(name))
                .when_some(ch_name, |d, n| d.child(caption(n, cx)))
                .child(div().flex_1())
                .child(caption(short_datetime(ms), cx)),
        )
        .child(div().pt_1().max_h(px(160.)).overflow_hidden().child(render_markdown(&format!("rc-{}", m.id), &m.content, cx)))
        .children(extra)
        .on_click(move |_, _, cx| {
            nav.update(cx, |n, cx| match &parent {
                Some(root) => n.open_thread(root, &ch, cx),
                None => n.open_message(&ch, &mid, cx),
            })
        })
        .into_any_element()
}

fn list_body(state: Option<&LoadState>, empty: bool, empty_text: &'static str, items: Vec<AnyElement>, on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    match state {
        None | Some(LoadState::Loading) if empty => skeleton_rows(5, cx).into_any_element(),
        Some(LoadState::Failed(why)) if empty => error_state("page-retry", format!("{} {why}", t("error.load")), on_retry, cx).into_any_element(),
        _ if empty => empty_state(t(empty_text), cx).into_any_element(),
        _ => div().id("page-scroll").flex_1().min_h_0().overflow_y_scroll().child(v_flex().py_2().children(items)).into_any_element(),
    }
}

// --- Inbox -----------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub enum InboxFilter {
    All,
    Mentions,
    Direct,
    Threads,
    Updates,
}

pub struct InboxPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    filter: InboxFilter,
    unread: bool,
    _subs: Vec<Subscription>,
}

impl InboxPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify()), cx.observe(&nav, |_, _, cx| cx.notify())];
        InboxPage { app, nav, filter: InboxFilter::All, unread: false, _subs: subs }
    }

    fn types(&self, server_filters: bool) -> String {
        if !server_filters {
            return String::new();
        }
        match self.filter {
            InboxFilter::Mentions => "mention".into(),
            InboxFilter::Direct => "direct_message".into(),
            InboxFilter::Threads => "thread_reply,thread_resolved".into(),
            _ => String::new(),
        }
    }

    pub fn keys(&self, cx: &App) -> Vec<Key> {
        let Some(s) = self.app.read(cx).session() else { return vec![] };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return vec![] };
        let server_filters = s.capability(|c| c.notification_type_filters);
        if self.filter == InboxFilter::Updates {
            vec![Key::Inbox(w, self.unread)]
        } else {
            vec![Key::Notifications(w, self.types(server_filters), self.unread)]
        }
    }
}

impl Render for InboxPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return div().into_any_element() };
        let keys = self.keys(cx);
        self.nav.update(cx, |n, cx| n.set_keys("inbox", keys.clone(), cx));
        let key = keys[0].clone();
        let this = cx.entity().downgrade();
        let this2 = this.clone();
        let filter_bar = h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .child(tabs(
                "inbox-tab",
                vec![(InboxFilter::All, "inbox.all"), (InboxFilter::Mentions, "inbox.mentions"), (InboxFilter::Direct, "inbox.direct"), (InboxFilter::Threads, "inbox.threads"), (InboxFilter::Updates, "inbox.updates")],
                &self.filter,
                move |f, cx| {
                    let _ = this.update(cx, |me, cx| {
                        me.filter = f;
                        cx.notify();
                    });
                },
                cx,
            ))
            .child(tabs(
                "inbox-unread",
                vec![(false, "inbox.all"), (true, "inbox.unread")],
                &self.unread,
                move |u, cx| {
                    let _ = this2.update(cx, |me, cx| {
                        me.unread = u;
                        cx.notify();
                    });
                },
                cx,
            ));
        let store = s.store().read_recursive();
        let state = store.cache.state(&key).cloned();
        let mut items: Vec<AnyElement> = vec![];
        if self.filter == InboxFilter::Updates {
            if let Some(page) = store.cache.get::<InboxPage2>(&key) {
                for e in &page.data {
                    let actor = e.actor_id.as_ref().and_then(|a| store.user(a)).map(|u| u.display_name.clone()).unwrap_or_else(|| e.module.clone());
                    let summary = format!("{} · {}", actor, describe_inbox_entry(e));
                    let ms = e.created_at.unix_ms().unwrap_or(0);
                    let unread = e.read_at.is_none();
                    items.push(
                        h_flex()
                            .mx_4()
                            .my_1()
                            .p_3()
                            .gap_3()
                            .rounded(px(purlay_tokens::radius::XL))
                            .border_1()
                            .border_color(k.border_subtle)
                            .when(unread, |d| d.bg(k.accent_subtle))
                            .child(ico(if e.module == "issues" { "list-todo" } else { "bell" }, 16., k.ink_muted))
                            .child(body(summary, cx).flex_1())
                            .child(caption(ago(ms), cx))
                            .into_any_element(),
                    );
                }
            }
        } else if let Some(page) = store.cache.get::<NotificationPage>(&key) {
            let server_filters = s.capability(|c| c.notification_type_filters);
            let mut last_group: Option<(Uuid, String)> = None;
            for (i, n) in page.data.iter().enumerate() {
                if !server_filters && !matches_filter(&self.filter, &n.r#type) {
                    continue;
                }
                // Replies under one root on one day are one row.
                let day = day_label(n.created_at.unix_ms().unwrap_or(0));
                if let (Some(th), NotificationType::ThreadReply) = (&n.thread, &n.r#type) {
                    if last_group.as_ref() == Some(&(th.root_id.clone(), day.clone())) {
                        continue;
                    }
                    last_group = Some((th.root_id.clone(), day));
                }
                let ch = store.channel(&n.channel_id).map(|c| {
                    let n = channel_title(c, &s.user_id);
                    if c.r#type == ChannelType::DirectMessage { n } else { format!("#{n}") }
                });
                let what = match n.r#type {
                    NotificationType::Mention => tf("inbox.mentioned", &[&ch.clone().unwrap_or_default()]),
                    NotificationType::DirectMessage => t("inbox.dm").to_string(),
                    NotificationType::ThreadReply => tf("inbox.replied", &[&ch.clone().unwrap_or_default()]),
                    NotificationType::ThreadResolved => tf("inbox.resolved", &[&ch.clone().unwrap_or_default()]),
                    NotificationType::Reminder => t("inbox.reminder").to_string(),
                    _ => tf("inbox.channelMessage", &[&ch.clone().unwrap_or_default()]),
                };
                let unread = n.read_at.is_none();
                let label_el = h_flex().gap_2().child(caption(what, cx)).when(unread, |d| d.child(div().size(px(6.)).rounded_full().bg(k.accent)));
                let s2 = s.clone();
                let nid = n.id.clone();
                drop_borrow_guard();
                items.push(
                    div()
                        .id(("notif", i))
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            if unread {
                                s2.mark_notifications_read(serde_json::json!({ "ids": [nid.to_string()] }));
                            }
                        })
                        .child(result_card(("notif-card", i), &n.message, &s, &self.nav, Some(label_el.into_any_element()), cx))
                        .into_any_element(),
                );
            }
        }
        let empty = items.is_empty();
        drop(store);
        let s3 = s.clone();
        let w2 = w.clone();
        let updates = self.filter == InboxFilter::Updates;
        let mark_all = Button::new("inbox-mark-all").small().ghost().label(t("inbox.markAllRead")).on_click(move |_, _, _| {
            if updates {
                s3.mark_inbox_read(&w2, serde_json::json!({ "all": true }));
            } else {
                s3.mark_notifications_read(serde_json::json!({ "workspaceId": w2.to_string() }));
            }
        });
        let s4 = s.clone();
        let k2 = key.clone();
        v_flex()
            .size_full()
            .child(page_header(t("nav.inbox"), Some(mark_all.into_any_element()), cx))
            .child(filter_bar)
            .child(list_body(state.as_ref(), empty, "inbox.empty", items, move |_, _, _| s4.refresh(k2.clone()), cx))
            .into_any_element()
    }
}

type InboxPage2 = purlay_core::contract::models::InboxPage;

fn drop_borrow_guard() {}

fn matches_filter(f: &InboxFilter, ty: &NotificationType) -> bool {
    match f {
        InboxFilter::All => true,
        InboxFilter::Mentions => *ty == NotificationType::Mention,
        InboxFilter::Direct => *ty == NotificationType::DirectMessage,
        InboxFilter::Threads => matches!(ty, NotificationType::ThreadReply | NotificationType::ThreadResolved),
        InboxFilter::Updates => false,
    }
}

fn describe_inbox_entry(e: &InboxEntry) -> String {
    let title = e.params.get("title").and_then(|v| v.as_ref()).and_then(|v| v.as_str()).map(String::from);
    match (e.kind.as_str(), title) {
        (_, Some(t)) => format!("{} — {}", e.kind.replace(['_', '.'], " "), t),
        (k, None) => k.replace(['_', '.'], " "),
    }
}

// --- Search ----------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum SearchKind {
    All,
    Questions,
    Code,
    Files,
}

pub struct SearchPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    pub input: Entity<InputState>,
    kind: SearchKind,
    results: Option<ApiResult<purlay_core::contract::models::SearchPage>>,
    elsewhere: Option<FindResponse>,
    loading: bool,
    task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl SearchPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("search.placeholder")));
        let subs = vec![
            cx.observe(&app, |_, _, cx| cx.notify()),
            cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::PressEnter { .. } => this.run(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
        ];
        SearchPage { app, nav, input, kind: SearchKind::All, results: None, elsewhere: None, loading: false, task: None, _subs: subs }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn run_query(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        let q = q.to_string();
        self.input.update(cx, |s, cx| s.set_value(q, window, cx));
        self.run(window, cx);
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().trim().to_string();
        let Some(s) = self.app.read(cx).session().cloned() else { return };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return };
        let n = q.chars().count();
        if n < SEARCH_QUERY_MIN as usize || n > SEARCH_QUERY_MAX as usize {
            window.push_notification(tf("search.min", &[&SEARCH_QUERY_MIN]), cx);
            return;
        }
        let scope = format!("{}|{}|{}", s.server_key(), s.user_id, w);
        s.prefs.push_recent_search(&scope, &q);
        let mut query = purlay_core::contract::models::SearchQuery { q: q.clone(), channel_id: None, sender_id: None, status: None, question: None, kind: None, since: None, until: None, repository: None, before: None, limit: Some(50) };
        match self.kind {
            SearchKind::Questions => query.question = Some("yes".into()),
            SearchKind::Code => query.kind = Some(SearchQueryKind::Code),
            SearchKind::Files => query.kind = Some(SearchQueryKind::File),
            SearchKind::All => {}
        }
        self.loading = true;
        let h1 = s.search(&w, query);
        let h2 = s.find(&w, q);
        self.task = Some(cx.spawn(async move |this, cx| {
            let r = h1.await.unwrap_or_else(|e| Err(purlay_core::api::ApiError::Transport(e.to_string())));
            let f = h2.await.ok().and_then(|r| r.ok());
            let _ = this.update(cx, |me, cx| {
                me.results = Some(r);
                me.elsewhere = f;
                me.loading = false;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for SearchPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        self.nav.update(cx, |n, cx| n.set_keys("search", vec![], cx));
        let w = self.nav.read(cx).workspace.clone().unwrap_or_default();
        let this = cx.entity().downgrade();
        let bar = v_flex().px_4().py_3().gap_2().child(Input::new(&self.input).prefix(crate::ui::ico("search", 14., c(cx).ink_muted))).child(tabs(
            "search-kind",
            vec![(SearchKind::All, "inbox.all"), (SearchKind::Questions, "search.questions"), (SearchKind::Code, "search.code"), (SearchKind::Files, "search.files")],
            &self.kind,
            move |k, cx| {
                let _ = this.update(cx, |me, cx| {
                    me.kind = k;
                    cx.notify();
                });
            },
            cx,
        ));
        let body_el: AnyElement = if self.loading {
            skeleton_rows(4, cx).into_any_element()
        } else {
            match &self.results {
                None => {
                    let scope = format!("{}|{}|{}", s.server_key(), s.user_id, w);
                    let recent = s.prefs.get().recent_searches.get(&scope).cloned().unwrap_or_default();
                    v_flex()
                        .px_4()
                        .gap_1()
                        .when(!recent.is_empty(), |d| d.child(caption(t("search.recent"), cx)))
                        .children(recent.into_iter().enumerate().map(|(i, q)| {
                            let q2 = q.clone();
                            row_hover(("recent-q", i), false, cx).px_2().py_1().child(body(q, cx)).on_click(cx.listener(move |this, _, window, cx| this.run_query(&q2, window, cx)))
                        }))
                        .into_any_element()
                }
                Some(Err(e)) => empty_state(tf("error.action", &[e]), cx).into_any_element(),
                Some(Ok(page)) if page.data.is_empty() && self.elsewhere.as_ref().is_none_or(|f| f.groups.iter().all(|g| g.hits.is_empty())) => empty_state(t("search.none"), cx).into_any_element(),
                Some(Ok(page)) => {
                    let mut col = v_flex().py_2();
                    for (i, m) in page.data.iter().enumerate() {
                        col = col.child(result_card(("search-hit", i), m, &s, &self.nav, None, cx));
                    }
                    if let Some(f) = &self.elsewhere {
                        let others: Vec<&FindGroup> = f.groups.iter().filter(|g| g.source != "chat" && !g.hits.is_empty()).collect();
                        if !others.is_empty() {
                            col = col.child(div().px_4().pt_4().child(caption(t("search.other"), cx)));
                            for g in others {
                                for (i, h) in g.hits.iter().enumerate() {
                                    col = col.child(
                                        v_flex()
                                            .id(SharedString::from(format!("find-{}-{i}", g.source)))
                                            .mx_4()
                                            .my_1()
                                            .p_3()
                                            .rounded(px(purlay_tokens::radius::XL))
                                            .border_1()
                                            .border_color(c(cx).border_subtle)
                                            .child(h_flex().gap_2().child(caption(g.source.clone(), cx)).child(label(h.title.clone(), cx)))
                                            .when_some(h.excerpt.clone(), |d, e| d.child(caption(e, cx))),
                                    );
                                }
                            }
                        }
                    }
                    div().id("search-scroll").flex_1().min_h_0().overflow_y_scroll().child(col).into_any_element()
                }
            }
        };
        v_flex().size_full().child(page_header(t("nav.search"), None, cx)).child(bar).child(body_el).into_any_element()
    }
}

// --- Saved -----------------------------------------------------------------------------

pub struct SavedPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    state: SavedState,
    _subs: Vec<Subscription>,
}

impl SavedPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        SavedPage { app, nav, state: SavedState::InProgress, _subs: subs }
    }
}

impl Render for SavedPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return div().into_any_element() };
        let key = Key::Saved(w, self.state.clone());
        self.nav.update(cx, |n, cx| n.set_keys("saved", vec![key.clone()], cx));
        let this = cx.entity().downgrade();
        let bar = h_flex().px_4().py_2().child(tabs(
            "saved-tab",
            vec![(SavedState::InProgress, "saved.inProgress"), (SavedState::Completed, "saved.completed"), (SavedState::Archived, "saved.archived")],
            &self.state,
            move |st, cx| {
                let _ = this.update(cx, |me, cx| {
                    me.state = st;
                    cx.notify();
                });
            },
            cx,
        ));
        let store = s.store().read_recursive();
        let state = store.cache.state(&key).cloned();
        let mut items = vec![];
        if let Some(page) = store.cache.get::<purlay_core::contract::models::SavedPage>(&key) {
            for (i, it) in page.data.iter().enumerate() {
                let (s1, m1) = (s.clone(), it.message.clone());
                let next = match self.state {
                    SavedState::InProgress => Some((SavedState::Completed, "saved.complete")),
                    _ => Some((SavedState::InProgress, "saved.restore")),
                };
                let mut actions = h_flex().pt_2().gap_2();
                if let Some((st, key2)) = next {
                    actions = actions.child(Button::new(("saved-next", i)).xsmall().outline().label(t(key2)).on_click(move |_, _, _| {
                        s1.save(&m1, Some(st.clone()));
                    }));
                }
                if self.state != SavedState::Archived {
                    let (s2, m2) = (s.clone(), it.message.clone());
                    actions = actions.child(Button::new(("saved-arch", i)).xsmall().ghost().label(t("saved.archive")).on_click(move |_, _, _| {
                        s2.save(&m2, Some(SavedState::Archived));
                    }));
                }
                let (s3, m3) = (s.clone(), it.message.clone());
                actions = actions.child(Button::new(("saved-rm", i)).xsmall().ghost().label(t("msg.unsave")).on_click(move |_, _, _| {
                    s3.unsave(&m3);
                }));
                items.push(result_card(("saved", i), &it.message, &s, &self.nav, Some(actions.into_any_element()), cx));
            }
        }
        let empty = items.is_empty();
        drop(store);
        let s4 = s.clone();
        v_flex()
            .size_full()
            .child(page_header(t("nav.saved"), None, cx))
            .child(bar)
            .child(list_body(state.as_ref(), empty, "saved.empty", items, move |_, _, _| s4.refresh(key.clone()), cx))
            .into_any_element()
    }
}

// --- Open topics -------------------------------------------------------------------------

pub struct TopicsPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    _subs: Vec<Subscription>,
}

impl TopicsPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        TopicsPage { app, nav, _subs: subs }
    }
}

impl Render for TopicsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return div().into_any_element() };
        let key = Key::Unresolved(w);
        self.nav.update(cx, |n, cx| n.set_keys("topics", vec![key.clone()], cx));
        let store = s.store().read_recursive();
        let state = store.cache.state(&key).cloned();
        let mut items = vec![];
        if let Some(page) = store.cache.get::<UnresolvedPage>(&key) {
            for (i, m) in page.data.iter().enumerate() {
                let th = m.thread.clone();
                let extra = th.map(|th| {
                    h_flex()
                        .pt_1()
                        .gap_2()
                        .child(ico("circle-dot", 12., c(cx).accent_text))
                        .child(caption(if th.reply_count == 1 { t("thread.reply1").to_string() } else { tf("thread.replies", &[&th.reply_count]) }, cx))
                        .when(th.knowledge.as_ref().is_some_and(|k| k.is_question), |d| d.child(caption(t("thread.question"), cx)))
                        .into_any_element()
                });
                let (nav, root, ch) = (self.nav.clone(), m.id.clone(), m.channel_id.clone());
                items.push(
                    div()
                        .id(("topic-wrap", i))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| nav.update(cx, |n, cx| n.open_thread(&root, &ch, cx)))
                        .child(result_card(("topic", i), m, &s, &self.nav, extra, cx))
                        .into_any_element(),
                );
            }
        }
        let empty = items.is_empty();
        drop(store);
        let s2 = s.clone();
        v_flex()
            .size_full()
            .child(page_header(t("nav.openTopics"), None, cx))
            .child(list_body(state.as_ref(), empty, "topics.empty", items, move |_, _, _| s2.refresh(key.clone()), cx))
            .into_any_element()
    }
}

// --- Issues ----------------------------------------------------------------------------

pub struct IssuesPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    status: IssueQueryStatus,
    title_in: Entity<InputState>,
    body_in: Entity<TextareaState>,
    creating: bool,
    _subs: Vec<Subscription>,
}

impl IssuesPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title_in = cx.new(|cx| InputState::new(window, cx).placeholder(t("issues.title")));
        let body_in = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 10).placeholder(t("issues.body")));
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        IssuesPage { app, nav, status: IssueQueryStatus::Open, title_in, body_in, creating: false, _subs: subs }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.app.read(cx).session().cloned() else { return };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return };
        let title = self.title_in.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let body_text = self.body_in.read(cx).value().to_string();
        let h = s.create_issue(&w, CreateIssueRequest { title, body: if body_text.trim().is_empty() { None } else { Some(body_text) }, assignee_id: None });
        self.creating = false;
        self.title_in.update(cx, |st, cx| st.set_value("", window, cx));
        self.body_in.update(cx, |st, cx| st.set_value("", window, cx));
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Err(e)) = h.await {
                let msg = tf("error.action", &[&e]);
                let _ = cx.update(|window, cx| window.push_notification(msg, cx));
            }
        })
        .detach();
        cx.notify();
    }
}

impl Render for IssuesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return div().into_any_element() };
        if !self.nav.read(cx).has_module("issues", cx) {
            return v_flex().size_full().child(page_header(t("nav.issues"), None, cx)).child(empty_state(t("issues.off"), cx)).into_any_element();
        }
        let key = Key::Issues(w.clone(), self.status.clone());
        self.nav.update(cx, |n, cx| n.set_keys("issues", vec![key.clone(), Key::Members(w.clone())], cx));
        let this = cx.entity().downgrade();
        let bar = h_flex().px_4().py_2().child(tabs(
            "issue-tab",
            vec![(IssueQueryStatus::Open, "issues.open"), (IssueQueryStatus::Done, "issues.done")],
            &self.status,
            move |st, cx| {
                let _ = this.update(cx, |me, cx| {
                    me.status = st;
                    cx.notify();
                });
            },
            cx,
        ));
        let form = self.creating.then(|| {
            v_flex()
                .mx_4()
                .my_2()
                .p_3()
                .gap_2()
                .rounded(px(purlay_tokens::radius::XL))
                .border_1()
                .border_color(k.border_subtle)
                .child(Input::new(&self.title_in))
                .child(Textarea::new(&self.body_in))
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1())
                        .child(Button::new("issue-cancel").small().ghost().label(t("cancel")).on_click(cx.listener(|this, _, _, cx| {
                            this.creating = false;
                            cx.notify();
                        })))
                        .child(Button::new("issue-create").small().primary().label(t("issues.create")).on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))),
                )
        });
        let store = s.store().read_recursive();
        let state = store.cache.state(&key).cloned();
        let mut items = vec![];
        if let Some(page) = store.cache.get::<IssuePage>(&key) {
            for (i, is) in page.data.iter().enumerate() {
                let assignee = is.assignee_id.as_ref().and_then(|a| store.user(a)).map(|u| u.display_name.clone()).unwrap_or_else(|| t("issues.unassigned").into());
                let done = is.status == IssueStatus::Done;
                let (s1, w1, id1) = (s.clone(), w.clone(), is.id.clone());
                items.push(
                    v_flex()
                        .mx_4()
                        .my_1()
                        .p_3()
                        .gap_1()
                        .rounded(px(purlay_tokens::radius::XL))
                        .border_1()
                        .border_color(k.border_subtle)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(ico(if done { "circle-check-big" } else { "circle-dot" }, 15., if done { k.success_text } else { k.accent_text }))
                                .child(body(is.title.clone(), cx).font_weight(FontWeight::SEMIBOLD).flex_1())
                                .child(caption(assignee, cx))
                                .child(Button::new(("issue-toggle", i)).xsmall().outline().label(if done { t("issues.reopen") } else { t("issues.markDone") }).on_click(move |_, _, _| {
                                    let st = if done { UpdateIssueRequestStatus::Open } else { UpdateIssueRequestStatus::Done };
                                    s1.update_issue(&w1, &id1, UpdateIssueRequest { status: Some(st), ..Default::default() });
                                })),
                        )
                        .when(!is.body.trim().is_empty(), |d| d.child(div().pl(px(23.)).child(render_markdown(&format!("issue-{}", is.id), &is.body, cx))))
                        .child(div().pl(px(23.)).child(caption(short_datetime(is.updated_at.unix_ms().unwrap_or(0)), cx)))
                        .into_any_element(),
                );
            }
        }
        let empty = items.is_empty();
        drop(store);
        let new_btn = Button::new("issue-new").small().primary().label(t("issues.new")).on_click(cx.listener(|this, _, window, cx| {
            this.creating = true;
            this.title_in.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
        }));
        let s2 = s.clone();
        v_flex()
            .size_full()
            .child(page_header(t("nav.issues"), Some(new_btn.into_any_element()), cx))
            .child(bar)
            .children(form)
            .child(list_body(state.as_ref(), empty, "issues.empty", items, move |_, _, _| s2.refresh(key.clone()), cx))
            .into_any_element()
    }
}

// --- Drafts, scheduled, reminders ---------------------------------------------------------

pub struct DraftsPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
}

impl DraftsPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>) -> Self {
        DraftsPage { app, nav }
    }
}

impl Render for DraftsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let prefix = format!("{}|draft.{}.", s.server_key(), s.user_id);
        let drafts: Vec<(String, String)> = s.prefs.get().drafts.into_iter().filter(|(k, _)| k.starts_with(&prefix)).collect();
        let store = s.store().read_recursive();
        let mut items = vec![];
        for (i, (key, text)) in drafts.iter().enumerate() {
            let rest = &key[prefix.len()..];
            let mut parts = rest.split('.');
            let ch = Uuid::new(parts.next().unwrap_or_default());
            let parent = parts.next().map(Uuid::new);
            let name = store.channel(&ch).map(|c| channel_title(c, &s.user_id)).unwrap_or_default();
            let (nav, ch2, p2) = (self.nav.clone(), ch.clone(), parent.clone());
            let hover = k.surface_050;
            items.push(
                v_flex()
                    .id(("draft", i))
                    .mx_4()
                    .my_1()
                    .p_3()
                    .rounded(px(purlay_tokens::radius::XL))
                    .border_1()
                    .border_color(k.border_subtle)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(h_flex().gap_2().child(ico("pencil", 13., k.ink_muted)).child(label(format!("#{name}"), cx)).when(parent.is_some(), |d| d.child(caption(t("thread.title"), cx))))
                    .child(div().pt_1().child(muted(text.chars().take(280).collect::<String>(), cx)))
                    .on_click(move |_, _, cx| {
                        nav.update(cx, |n, cx| match &p2 {
                            Some(root) => n.open_thread(root, &ch2, cx),
                            None => n.open_channel(&ch2, cx),
                        })
                    })
                    .into_any_element(),
            );
        }
        drop(store);
        let empty = items.is_empty();
        v_flex()
            .size_full()
            .child(page_header(t("nav.drafts"), None, cx))
            .child(list_body(Some(&LoadState::Loaded), empty, "drafts.empty", items, |_, _, _| {}, cx))
            .into_any_element()
    }
}

pub struct ScheduledPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    _subs: Vec<Subscription>,
    _poll: Task<()>,
}

impl ScheduledPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        // The list refreshes every 15 s while open.
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
            let ok = this
                .update(cx, |me, cx| {
                    if let (Some(s), Some(w)) = (me.app.read(cx).session().cloned(), me.nav.read(cx).workspace.clone()) {
                        if me.nav.read(cx).route == Route::Scheduled {
                            s.refresh(Key::Scheduled(w));
                        }
                    }
                })
                .is_ok();
            if !ok {
                break;
            }
        });
        ScheduledPage { app, nav, _subs: subs, _poll: poll }
    }
}

impl Render for ScheduledPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return div().into_any_element() };
        let key = Key::Scheduled(w);
        self.nav.update(cx, |n, cx| n.set_keys("scheduled", vec![key.clone()], cx));
        let store = s.store().read_recursive();
        let state = store.cache.state(&key).cloned();
        let mut items = vec![];
        if let Some(list) = store.cache.get::<ScheduledMessageList>(&key) {
            for (i, m) in list.iter().filter(|m| m.state == ScheduledMessageState::Pending).enumerate() {
                let name = store.channel(&m.channel_id).map(|c| channel_title(c, &s.user_id)).unwrap_or_default();
                let (s1, ch, id) = (s.clone(), m.channel_id.clone(), m.id.clone());
                let key2 = key.clone();
                items.push(
                    v_flex()
                        .mx_4()
                        .my_1()
                        .p_3()
                        .gap_1()
                        .rounded(px(purlay_tokens::radius::XL))
                        .border_1()
                        .border_color(k.border_subtle)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(ico("calendar-clock", 14., k.accent_text))
                                .child(label(format!("#{name}"), cx))
                                .child(caption(tf("scheduled.at", &[&short_datetime(m.send_at.unix_ms().unwrap_or(0))]), cx))
                                .child(div().flex_1())
                                .child(Button::new(("sched-cancel", i)).xsmall().ghost().label(t("scheduled.cancel")).on_click(move |_, _, _| {
                                    let h = s1.cancel_scheduled(&ch, &id);
                                    let s2 = s1.clone();
                                    let k3 = key2.clone();
                                    s1.rt.spawn(async move {
                                        let _ = h.await;
                                        s2.refresh(k3);
                                    });
                                })),
                        )
                        .child(render_markdown(&format!("sched-{}", m.id), &m.content, cx))
                        .into_any_element(),
                );
            }
        }
        let empty = items.is_empty();
        drop(store);
        let s2 = s.clone();
        v_flex()
            .size_full()
            .child(page_header(t("nav.scheduled"), None, cx))
            .child(list_body(state.as_ref(), empty, "scheduled.empty", items, move |_, _, _| s2.refresh(key.clone()), cx))
            .into_any_element()
    }
}

pub struct RemindersPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    _subs: Vec<Subscription>,
}

impl RemindersPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        RemindersPage { app, nav, _subs: subs }
    }
}

impl Render for RemindersPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return div().into_any_element() };
        let key = Key::Reminders(w);
        self.nav.update(cx, |n, cx| n.set_keys("reminders", vec![key.clone()], cx));
        let store = s.store().read_recursive();
        let state = store.cache.state(&key).cloned();
        let mut items = vec![];
        if let Some(list) = store.cache.get::<MessageReminderList>(&key) {
            for (i, r) in list.iter().enumerate() {
                let (s1, mid) = (s.clone(), r.message.id.clone());
                let extra = h_flex()
                    .pt_1()
                    .gap_2()
                    .child(ico("alarm-clock", 12., c(cx).accent_text))
                    .child(caption(tf("reminders.at", &[&short_datetime(r.remind_at.unix_ms().unwrap_or(0))]), cx))
                    .child(div().flex_1())
                    .child(Button::new(("rem-cancel", i)).xsmall().ghost().label(t("cancel")).on_click(move |_, _, _| {
                        s1.cancel_reminder(&mid);
                    }))
                    .into_any_element();
                items.push(result_card(("rem", i), &r.message, &s, &self.nav, Some(extra), cx));
            }
        }
        let empty = items.is_empty();
        drop(store);
        let s2 = s.clone();
        v_flex()
            .size_full()
            .child(page_header(t("nav.reminders"), None, cx))
            .child(list_body(state.as_ref(), empty, "reminders.empty", items, move |_, _, _| s2.refresh(key.clone()), cx))
            .into_any_element()
    }
}
