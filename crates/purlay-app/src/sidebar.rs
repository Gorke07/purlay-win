//! The sidebar: the workspace, the destinations (Inbox, Search, Saved…), then starred
//! channels, the person's own groups, channels and conversations. Rows never move by
//! activity; unread is bold, and only mentions get a count (UX §2).

use crate::i18n::{t, tf};
use crate::nav::{Nav, Route};
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::contract::{Timestamp, Uuid};
use std::collections::HashSet;

pub fn channel_title(c: &Channel, me: &Uuid) -> String {
    if c.r#type == ChannelType::DirectMessage {
        let mut names: Vec<String> = c.peers.iter().filter(|p| p.id != *me).map(|p| p.display_name.clone()).collect();
        if names.is_empty() {
            if let Some(p) = &c.peer {
                names.push(p.display_name.clone());
            }
        }
        if names.is_empty() {
            return c.name.clone().unwrap_or_default();
        }
        return names.join(", ");
    }
    c.name.clone().unwrap_or_default()
}

pub struct SidebarView {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    folded: HashSet<String>,
    _subs: Vec<Subscription>,
}

impl SidebarView {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify()), cx.observe(&nav, |_, _, cx| cx.notify())];
        SidebarView { app, nav, folded: HashSet::new(), _subs: subs }
    }

    fn destination(&self, id: &'static str, ic: &'static str, label_key: &'static str, route: Route, badge: i64, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let selected = self.nav.read(cx).route == route;
        let nav = self.nav.clone();
        nav_row(id, selected, cx)
            .child(div().w(px(20.)).flex().justify_center().child(ico(ic, 18., k.accent_text)))
            .child(text_style(div(), purlay_tokens::typography::BODY, cx).flex_1().text_color(k.nav_ink).child(t(label_key)))
            .when(badge > 0, |d| d.child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.nav_ink_muted).child(if badge > 99 { "99+".to_string() } else { badge.to_string() })))
            .on_click(move |_, _, cx| nav.update(cx, |n, cx| n.go(route.clone(), cx)))
            .into_any_element()
    }

    fn section_header(&self, key: String, title: String, extra: Option<AnyElement>, cx: &mut Context<Self>) -> Stateful<Div> {
        let k = c(cx);
        let folded = self.folded.contains(&key);
        let key2 = key.clone();
        let group = SharedString::from(format!("secg-{key}"));
        h_flex()
            .id(SharedString::from(format!("sec-{key}")))
            .group(group.clone())
            .h(px(28.))
            .mt_3()
            .ml(px(12.))
            .mr(px(8.))
            .gap_1()
            .cursor_pointer()
            .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.nav_ink_muted).font_weight(FontWeight::MEDIUM).child(title))
            // The fold arrow shows on hover, and stays while folded.
            .child(div().when(!folded, |d| d.invisible().group_hover(group.clone(), |s| s.visible())).child(ico(if folded { "chevron-right" } else { "chevron-down" }, 12., k.nav_ink_muted)))
            .child(div().flex_1())
            .children(extra)
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.folded.remove(&key2) {
                    this.folded.insert(key2.clone());
                }
                cx.notify();
            }))
    }

    fn channel_row(&self, ch: &Channel, me: &Uuid, presence: Option<(bool, bool)>, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let selected = self.nav.read(cx).current_channel() == Some(&ch.id);
        let state = ch.state.clone();
        let unread = state.as_ref().is_some_and(|s| s.unread_count > 0);
        let mentions = state.as_ref().map(|s| if ch.r#type == ChannelType::DirectMessage { s.unread_count } else { s.unread_mention_count }).unwrap_or(0);
        let muted = ch.notification_level == Some(NotificationLevel::None);
        let title = channel_title(ch, me);
        let lead: AnyElement = match ch.r#type {
            ChannelType::DirectMessage => {
                let peer = ch.peers.iter().find(|p| p.id != *me).or(ch.peer.as_ref());
                let key = peer.map(|p| p.username.clone()).unwrap_or_else(|| title.clone());
                div()
                    .relative()
                    .child(avatar(key, &title, 22., cx))
                    .when_some(presence, |d, (on, away)| d.child(div().absolute().right(px(-3.)).bottom(px(-3.)).child(presence_dot(on, away, cx))))
                    .into_any_element()
            }
            ChannelType::Private => ico("lock", 17., k.accent_text).into_any_element(),
            _ => ico("hash", 18., k.accent_text).into_any_element(),
        };
        let nav = self.nav.clone();
        let id = ch.id.clone();
        let app = self.app.clone();
        let menu_ch = ch.clone();
        let row = nav_row(SharedString::from(format!("ch-{}", ch.id)), selected, cx)
            .child(div().w(px(20.)).flex().justify_center().child(lead))
            .child(
                text_style(div(), purlay_tokens::typography::BODY, cx)
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(k.nav_ink)
                    .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(muted, |d| d.opacity(0.55))
                    .child(title),
            )
            .when_some(ch.incident.as_ref(), |d, i| {
                d.child(div().px_1().rounded(px(4.)).bg(k.danger_subtle).text_color(k.danger_text).text_size(px(10.)).child(format!("SEV{}", i.severity)))
            })
            .when(mentions > 0 && !muted, |d| d.child(count_badge(mentions, cx)))
            .on_click(move |_, _, cx| nav.update(cx, |n, cx| n.open_channel(&id, cx)));
        // The menu wraps an outer box so the row keeps its hover style.
        div()
            .id(SharedString::from(format!("chmenu-{}", ch.id)))
            .child(row)
            .context_menu(move |menu, _, cx| channel_menu(menu, &menu_ch, &app, cx))
            .into_any_element()
    }
}

fn nav_row(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let (_, hover, selected_bg) = crate::theme::sidebar_tones(cx);
    h_flex()
        .id(id)
        .h(px(34.))
        .mx_2()
        .px(px(10.))
        .gap(px(10.))
        .rounded(px(purlay_tokens::radius::XL))
        .cursor_pointer()
        .when(selected, |d| d.bg(selected_bg))
        .when(!selected, |d| d.hover(move |s| s.bg(hover)))
}

pub fn channel_menu(menu: gpui_kit::component::menu::PopupMenu, ch: &Channel, app: &Entity<AppModel>, cx: &App) -> gpui_kit::component::menu::PopupMenu {
    let Some(s) = app.read(cx).session().cloned() else { return menu };
    let (id, starred) = (ch.id.clone(), ch.starred);
    let level = ch.notification_level.clone();
    let mut menu = menu;
    if ch.r#type != ChannelType::DirectMessage {
        let s1 = s.clone();
        let i1 = id.clone();
        menu = menu.item(PopupMenuItem::new(if starred { t("channel.unstar") } else { t("channel.star") }).on_click(move |_, _, _| {
            s1.star(&i1, !starred);
        }));
    }
    // Marking read needs the newest id, which only a loaded feed knows.
    let unread = ch.state.as_ref().is_some_and(|st| st.unread_count > 0);
    let newest = s.store().read_recursive().windows.get(&id).filter(|w| w.reaches_newest).and_then(|w| w.newest_id().cloned());
    if let (true, Some(n)) = (unread, newest) {
        let (s2, i2) = (s.clone(), id.clone());
        menu = menu.item(PopupMenuItem::new(t("sidebar.markAllRead")).on_click(move |_, _, _| s2.mark_read(&i2, &n)));
    }
    menu = menu.separator().label(t("channel.notify"));
    for (lv, key) in [(NotificationLevel::All, "channel.notify.all"), (NotificationLevel::Mentions, "channel.notify.mentions"), (NotificationLevel::None, "channel.notify.none")] {
        let s3 = s.clone();
        let i3 = id.clone();
        let checked = level.as_ref() == Some(&lv) || (level.is_none() && lv == NotificationLevel::Mentions);
        menu = menu.item(PopupMenuItem::new(t(key)).checked(checked).on_click(move |_, _, _| {
            s3.set_notification_level(&i3, lv.clone());
        }));
    }
    if ch.r#type != ChannelType::DirectMessage {
        let s4 = s.clone();
        let i4 = id.clone();
        menu = menu.separator().item(PopupMenuItem::new(t("channel.leave")).on_click(move |_, _, _| {
            s4.leave(&i4);
        }));
    }
    menu
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(session) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let nav = self.nav.read(cx);
        let ws = nav.workspace.clone();
        let issues_on = nav.has_module("issues", cx);
        let chat_on = nav.has_module("chat", cx);
        let store = session.store().read_recursive();
        let me_id = session.user_id.clone();
        let me = store.me.clone();
        let membership = ws.as_ref().and_then(|w| store.membership(w).cloned());
        let ws_name = membership.as_ref().map(|m| m.workspace.name.clone()).unwrap_or_else(|| "Purlay".into());
        let deep_until = membership.as_ref().and_then(|m| m.deep_work_until.clone()).filter(|u| u.unix_ms().is_some_and(|ms| ms > now_ms()));
        let channels: Vec<Channel> = ws.as_ref().and_then(|w| store.channels.get(w)).map(|m| m.values().cloned().collect()).unwrap_or_default();
        let groups: Vec<ChannelGroup> = ws.as_ref().and_then(|w| store.cache.get::<ChannelGroupPage>(&purlay_core::store::Key::ChannelGroups(w.clone()))).map(|p| p.data.clone()).unwrap_or_default();
        let presence = store.presence.clone();
        let loaded = ws.as_ref().and_then(|w| store.channels_loaded.get(w)).cloned();
        let inbox_unread = ws
            .as_ref()
            .and_then(|w| store.cache.get::<NotificationPage>(&purlay_core::store::Key::Notifications(w.clone(), String::new(), true)))
            .map(|p| p.data.len() as i64)
            .unwrap_or(0);
        drop(store);
        let caps_sched = session.capability(|c| c.scheduled_messages);
        let caps_rem = session.capability(|c| c.message_reminders);

        let mut joined: Vec<Channel> = channels.iter().filter(|c| c.state.is_some() && c.r#type != ChannelType::DirectMessage).cloned().collect();
        joined.sort_by_key(|c| channel_title(c, &me_id).to_lowercase());
        let mut dms: Vec<Channel> = channels.iter().filter(|c| c.r#type == ChannelType::DirectMessage).cloned().collect();
        dms.sort_by_key(|c| channel_title(c, &me_id).to_lowercase());
        let starred: Vec<Channel> = joined.iter().chain(dms.iter()).filter(|c| c.starred).cloned().collect();

        let pres = |c: &Channel| -> Option<(bool, bool)> {
            if c.r#type != ChannelType::DirectMessage {
                return None;
            }
            let peer = c.peers.iter().find(|p| p.id != me_id).or(c.peer.as_ref())?;
            presence.get(&peer.id).map(|p| (p.online, p.away))
        };

        // Workspace header: its tile, name and switcher, at the window's top edge (so the
        // space around it moves the window).
        let memberships = me.as_ref().map(|m| m.memberships.clone()).unwrap_or_default();
        let nav_e = self.nav.clone();
        let ws_menu = ws.clone();
        let s_menu = session.clone();
        let initials: String = {
            let words: Vec<&str> = ws_name.split_whitespace().collect();
            let s: String = if words.len() >= 2 { words.iter().take(2).filter_map(|w| w.chars().next()).collect() } else { ws_name.chars().take(2).collect() };
            s.to_uppercase()
        };
        let header = h_flex()
            .h(px(crate::chrome::TOP_BAR))
            .flex_none()
            .pl_2()
            .pr(px(10.))
            .gap_1()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div().flex_1().min_w_0().occlude().child(
                    Button::new("ws-menu")
                        .ghost()
                        .w_full()
                        .h(px(46.))
                        .px_2()
                        .rounded(px(purlay_tokens::radius::XL))
                        .child(
                            h_flex()
                                .w_full()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex_none()
                                        .size(px(36.))
                                        .rounded(px(9.))
                                        .bg(k.accent)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(k.on_accent)
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::BOLD)
                                        .child(initials),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_start()
                                        .child(text_style(div(), purlay_tokens::typography::HEADING, cx).text_color(k.nav_ink).whitespace_nowrap().overflow_hidden().text_ellipsis().child(ws_name.clone()))
                                        .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.nav_ink_muted).child(t("chrome.workspace"))),
                                )
                                .child(ico("chevrons-up-down", 15., k.nav_ink_muted)),
                        )
                        .dropdown_menu(move |mut menu, _, _| {
                            for m in &memberships {
                                let nav = nav_e.clone();
                                let id = m.workspace.id.clone();
                                let checked = ws_menu.as_ref() == Some(&id);
                                menu = menu.item(PopupMenuItem::new(m.workspace.name.clone()).checked(checked).on_click(move |_, _, cx| {
                                    let id = id.clone();
                                    nav.update(cx, |n, cx| n.set_workspace(Some(id), cx));
                                }));
                            }
                            if let Some(w) = ws_menu.clone() {
                                let s = s_menu.clone();
                                menu = menu.separator().item(PopupMenuItem::new(t("sidebar.markAllRead")).on_click(move |_, _, _| {
                                    s.mark_all_read(&w);
                                }));
                            }
                            menu
                        }),
                ),
            )
            .child(div().occlude().child(crate::chrome::tool_button("hide-sidebar", "panel-left", t("chrome.hideSidebar"), false, cx).size(px(32.)).on_click(|_, window, cx| {
                window.dispatch_action(Box::new(crate::actions::ToggleSidebar), cx);
            })));

        // "Search Purlay", which opens the palette (Ctrl+K).
        let field_bg = if crate::theme::Tokens::get(cx).dark { k.ink.opacity(0.06) } else { k.nav_hover.opacity(0.7) };
        let field_hover = if crate::theme::Tokens::get(cx).dark { k.ink.opacity(0.1) } else { k.nav_hover };
        let search = h_flex()
            .id("sidebar-search")
            .flex_none()
            .h(px(36.))
            .mx_2()
            .mt_1()
            .mb_2()
            .px(px(10.))
            .gap_2()
            .rounded(px(purlay_tokens::radius::XL))
            .bg(field_bg)
            .cursor_pointer()
            .hover(move |s| s.bg(field_hover))
            .child(ico("search", 16., k.nav_ink_muted))
            .child(text_style(div(), purlay_tokens::typography::BODY, cx).flex_1().text_color(k.nav_ink_muted).child(t("chrome.search")))
            .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.nav_ink_muted).child("Ctrl K"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::OpenPalette), cx));
        let mut list = v_flex().pb_3().gap(px(1.));
        list = list
            .child(self.destination("d-inbox", "inbox", "nav.inbox", Route::Inbox, inbox_unread, cx))
            .child(self.destination("d-saved", "bookmark", "nav.saved", Route::Saved, 0, cx))
            .when(chat_on, |l| l.child(self.destination("d-topics", "circle-help", "nav.openTopics", Route::OpenTopics, 0, cx)))
            .when(issues_on, |l| l.child(self.destination("d-issues", "list-todo", "nav.issues", Route::Issues, 0, cx)))
            .child(self.destination("d-drafts", "pencil", "nav.drafts", Route::Drafts, 0, cx))
            .when(caps_sched, |l| l.child(self.destination("d-sched", "calendar-clock", "nav.scheduled", Route::Scheduled, 0, cx)))
            .when(caps_rem, |l| l.child(self.destination("d-rem", "alarm-clock", "nav.reminders", Route::Reminders, 0, cx)));

        if let Some(until) = &deep_until {
            let w = ws.clone();
            let s = session.clone();
            list = list.child(
                h_flex()
                    .id("deep-banner")
                    .mx_2()
                    .mt_2()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .rounded(px(purlay_tokens::radius::LG))
                    .bg(k.accent_subtle)
                    .child(ico("focus", 15., k.accent_text))
                    .child(text_style(div(), purlay_tokens::typography::LABEL, cx).flex_1().text_color(k.accent_text).child(tf("deep.on", &[&clock_time(until.unix_ms().unwrap_or(0))])))
                    .child(Button::new("deep-end").xsmall().ghost().label(t("deep.end")).on_click(move |_, _, _| {
                        if let Some(w) = &w {
                            s.set_deep_work(w, None);
                        }
                    })),
            );
        }

        if !chat_on {
            list = list.child(div().p_4().child(caption(t("sidebar.noChat"), cx)));
        } else if matches!(loaded, Some(purlay_core::store::LoadState::Loading) | None) && channels.is_empty() {
            list = list.child(skeleton_rows(4, cx));
        } else {
            if !starred.is_empty() {
                let key = "starred".to_string();
                list = list.child(self.section_header(key.clone(), t("sidebar.starred").into(), None, cx));
                if !self.folded.contains(&key) {
                    for ch in &starred {
                        list = list.child(self.channel_row(ch, &me_id, pres(ch), cx));
                    }
                }
            }
            for g in &groups {
                let members: Vec<&Channel> = joined.iter().filter(|c| !c.starred && c.sidebar_group_id.as_ref() == Some(&g.id)).collect();
                let key = format!("group-{}", g.id);
                list = list.child(self.section_header(key.clone(), g.name.clone(), None, cx));
                if !self.folded.contains(&key) {
                    for ch in members {
                        list = list.child(self.channel_row(ch, &me_id, None, cx));
                    }
                }
            }
            let ungrouped: Vec<&Channel> = joined.iter().filter(|c| !c.starred && c.sidebar_group_id.is_none()).collect();
            let key = "channels".to_string();
            let add = icon_button("add-channel", "plus", "sidebar.newChannel", cx)
                .size(px(22.))
                .on_click(cx.listener(|this, _, window, cx| crate::dialogs::new_channel(this.app.clone(), this.nav.clone(), window, cx)))
                .into_any_element();
            list = list.child(self.section_header(key.clone(), t("sidebar.channels").into(), Some(add), cx));
            if !self.folded.contains(&key) {
                if ungrouped.is_empty() && joined.is_empty() {
                    list = list.child(div().px_4().py_1().child(caption(t("sidebar.empty"), cx)));
                }
                for ch in ungrouped {
                    list = list.child(self.channel_row(ch, &me_id, None, cx));
                }
            }
            let key = "dms".to_string();
            let add_dm = icon_button("add-dm", "plus", "sidebar.newConversation", cx)
                .size(px(22.))
                .on_click(cx.listener(|this, _, window, cx| crate::dialogs::new_conversation(this.app.clone(), this.nav.clone(), window, cx)))
                .into_any_element();
            list = list.child(self.section_header(key.clone(), t("sidebar.conversations").into(), Some(add_dm), cx));
            if !self.folded.contains(&key) {
                for ch in dms.iter().filter(|c| !c.starred) {
                    list = list.child(self.channel_row(ch, &me_id, pres(ch), cx));
                }
            }
        }

        // The person at the bottom, with their menu: away, Deep Work, settings, sign out.
        let my_presence = presence.get(&me_id).copied();
        let app_e = self.app.clone();
        let nav_e = self.nav.clone();
        let s_menu = session.clone();
        let ws_menu = ws.clone();
        let away = me.as_ref().map(|m| m.away_chosen).unwrap_or(false);
        let deep_on = deep_until.is_some();
        let footer = me.as_ref().map(|m| {
            div().flex_none().px_2().py_2().child(
                Button::new("me-menu")
                    .ghost()
                    .w_full()
                    .h(px(48.))
                    .px_2()
                    .rounded(px(purlay_tokens::radius::XL))
                    .child(
                        h_flex()
                            .w_full()
                            .gap(px(10.))
                            .child(
                                div()
                                    .relative()
                                    .child(avatar(m.user.username.clone(), &m.user.display_name, 32., cx))
                                    .child(div().absolute().right(px(-2.)).bottom(px(-2.)).child(presence_dot(my_presence.map(|p| p.online).unwrap_or(true), m.away_chosen, cx))),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_start()
                                    .child(text_style(div(), purlay_tokens::typography::LABEL, cx).font_weight(FontWeight::SEMIBOLD).text_color(k.nav_ink).whitespace_nowrap().overflow_hidden().text_ellipsis().child(m.user.display_name.clone()))
                                    .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.nav_ink_muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(if m.away_chosen { t("presence.away").to_string() } else { format!("@{}", m.user.username) })),
                            )
                            .child(ico("chevrons-up-down", 15., k.nav_ink_muted)),
                    )
                    .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                        let s = s_menu.clone();
                        menu = menu.item(PopupMenuItem::new(if away { t("away.clear") } else { t("away.set") }).on_click(move |_, _, _| {
                            s.set_away(!away);
                        }));
                        if let Some(w) = ws_menu.clone() {
                            if deep_on {
                                let s = s_menu.clone();
                                let w2 = w.clone();
                                menu = menu.item(PopupMenuItem::new(t("deep.end")).on_click(move |_, _, _| {
                                    s.set_deep_work(&w2, None);
                                }));
                            } else {
                                for (key, mins) in [("deep.30m", 30i64), ("deep.1h", 60), ("deep.2h", 120), ("deep.tomorrow", -1)] {
                                    let s = s_menu.clone();
                                    let w2 = w.clone();
                                    menu = menu.item(PopupMenuItem::new(format!("{} · {}", t("deep.title"), t(key))).on_click(move |_, _, _| {
                                        s.set_deep_work(&w2, Some(deep_work_until(mins)));
                                    }));
                                }
                            }
                        }
                        let nav = nav_e.clone();
                        menu = menu.separator().item(PopupMenuItem::new(t("nav.settings")).on_click(move |_, _, cx| nav.update(cx, |n, cx| n.go(Route::Settings, cx))));
                        let app = app_e.clone();
                        menu.item(PopupMenuItem::new(t("settings.signOut")).on_click(move |_, _, cx| app.update(cx, |m, cx| m.sign_out(false, cx))))
                    }),
            )
        });

        v_flex()
            .w(px(purlay_tokens::size::SIDEBAR))
            .h_full()
            .flex_none()
            .bg(crate::theme::sidebar_tones(cx).0)
            .border_r_1()
            .border_color(k.nav_border)
            .child(header)
            .child(search)
            .child(div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().child(list))
            .children(footer)
            .into_any_element()
    }
}
/// Deep Work presets: minutes from now, or `-1` for tomorrow 09:00 (capped below 24 h).
pub fn deep_work_until(mins: i64) -> Timestamp {
    use chrono::{Duration, Local, TimeZone};
    let now = Local::now();
    let until = if mins < 0 {
        let tomorrow = now.date_naive().succ_opt().unwrap_or(now.date_naive());
        let nine = tomorrow.and_hms_opt(9, 0, 0).unwrap();
        let t = Local.from_local_datetime(&nine).single().unwrap_or(now + Duration::hours(12));
        let cap = now + Duration::hours(purlay_core::contract::constants::DEEP_WORK_MAX_HOURS) - Duration::minutes(1);
        if t > cap { cap } else { t }
    } else {
        now + Duration::minutes(mins)
    };
    Timestamp::from_unix_ms(until.timestamp_millis())
}
