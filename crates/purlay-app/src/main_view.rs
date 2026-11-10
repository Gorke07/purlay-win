//! The signed-in window: sidebar, the page, and one inspector (thread, files, pins,
//! bookmarks or members). Palette and shortcuts float above.

use crate::actions::*;
use crate::conversation::ConversationView;
use crate::i18n::t;
use crate::nav::{Inspector, Nav, Route};
use crate::pages::*;
use crate::palette::Palette;
use crate::panels::PanelView;
use crate::settings::SettingsPage;
use crate::sidebar::SidebarView;
use crate::state::{link_target, AppEvent, AppModel};
use crate::theme::c;
use crate::thread::ThreadView;
use crate::ui::*;
use crate::i18n::tf;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::Uuid;
use std::collections::HashMap;
use std::time::Duration;

pub struct MainView {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    sidebar: Entity<SidebarView>,
    conversations: HashMap<Uuid, Entity<ConversationView>>,
    order: Vec<Uuid>,
    thread: Option<Entity<ThreadView>>,
    panel: Entity<PanelView>,
    inbox: Entity<InboxPage>,
    search: Entity<SearchPage>,
    saved: Entity<SavedPage>,
    topics: Entity<TopicsPage>,
    issues: Entity<IssuesPage>,
    drafts: Entity<DraftsPage>,
    scheduled: Entity<ScheduledPage>,
    reminders: Entity<RemindersPage>,
    settings: Entity<SettingsPage>,
    palette: Entity<Palette>,
    focus: FocusHandle,
    sidebar_hidden: bool,
    _active: Task<()>,
    _subs: Vec<Subscription>,
}

impl MainView {
    pub fn new(app: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| Nav::new(app.clone(), cx));
        let sidebar = cx.new(|cx| SidebarView::new(app.clone(), nav.clone(), cx));
        let panel = cx.new(|cx| PanelView::new(app.clone(), nav.clone(), window, cx));
        let inbox = cx.new(|cx| InboxPage::new(app.clone(), nav.clone(), cx));
        let search = cx.new(|cx| SearchPage::new(app.clone(), nav.clone(), window, cx));
        let saved = cx.new(|cx| SavedPage::new(app.clone(), nav.clone(), cx));
        let topics = cx.new(|cx| TopicsPage::new(app.clone(), nav.clone(), cx));
        let issues = cx.new(|cx| IssuesPage::new(app.clone(), nav.clone(), window, cx));
        let drafts = cx.new(|_| DraftsPage::new(app.clone(), nav.clone()));
        let scheduled = cx.new(|cx| ScheduledPage::new(app.clone(), nav.clone(), cx));
        let reminders = cx.new(|cx| RemindersPage::new(app.clone(), nav.clone(), cx));
        let settings = cx.new(|cx| SettingsPage::new(app.clone(), nav.clone(), cx));
        let palette = cx.new(|cx| Palette::new(app.clone(), nav.clone(), window, cx));
        let subs = vec![
            cx.observe(&nav, |_, _, cx| cx.notify()),
            cx.observe(&app, |this, _, cx| {
                if this.nav.read(cx).workspace.is_none() {
                    this.nav.update(cx, |n, cx| n.pick_workspace(cx));
                }
                cx.notify();
            }),
            cx.subscribe_in(&app, window, |this, _, e: &AppEvent, _, cx| {
                match e {
                    AppEvent::OpenLink(l) => this.open_link(l, cx),
                    AppEvent::OpenSettings => this.nav.update(cx, |n, cx| n.go(Route::Settings, cx)),
                    _ => {}
                }
            }),
        ];
        // ACTIVE every minute while the window is in front: it holds pushes and drives away.
        let active = cx.spawn(async move |this, cx| loop {
            let ok = this
                .update(cx, |me, cx| {
                    if me.app.read(cx).window_active {
                        if let (Some(s), Some(w)) = (me.app.read(cx).session().cloned(), me.nav.read(cx).workspace.clone()) {
                            s.active(&w);
                        }
                    }
                })
                .is_ok();
            if !ok {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(20)).await;
        });
        let mut me = MainView {
            app,
            nav,
            sidebar,
            conversations: HashMap::new(),
            order: vec![],
            thread: None,
            panel,
            inbox,
            search,
            saved,
            topics,
            issues,
            drafts,
            scheduled,
            reminders,
            settings,
            palette,
            focus: cx.focus_handle(),
            sidebar_hidden: false,
            _active: active,
            _subs: subs,
        };
        // Shortcuts bound to "Main" need the focus inside this view from the start.
        // Created while the window draws: focus after the frame, not during it.
        let focus = me.focus.clone();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
        let pending = me.app.update(cx, |m, _| m.take_pending_links());
        for l in pending {
            me.open_link(&l, cx);
        }
        me
    }

    fn open_link(&mut self, raw: &str, cx: &mut Context<Self>) {
        if let Some((_slug, ch, msg)) = link_target(raw) {
            self.nav.update(cx, |n, cx| match msg {
                Some(m) => n.open_message(&ch, &m, cx),
                None => n.open_channel(&ch, cx),
            });
        }
    }

    fn conversation(&mut self, ch: &Uuid, window: &mut Window, cx: &mut Context<Self>) -> Entity<ConversationView> {
        if let Some(v) = self.conversations.get(ch) {
            self.order.retain(|x| x != ch);
            self.order.push(ch.clone());
            return v.clone();
        }
        let v = cx.new(|cx| ConversationView::new(self.app.clone(), self.nav.clone(), ch.clone(), window, cx));
        self.conversations.insert(ch.clone(), v.clone());
        self.order.push(ch.clone());
        // Keep a few open channels' places; let the rest go.
        while self.order.len() > 8 {
            let old = self.order.remove(0);
            self.conversations.remove(&old);
        }
        v
    }

    fn thread_view(&mut self, root: &Uuid, channel: &Uuid, window: &mut Window, cx: &mut Context<Self>) -> Entity<ThreadView> {
        if let Some(t) = &self.thread {
            if t.read(cx).root == *root {
                return t.clone();
            }
        }
        let v = cx.new(|cx| ThreadView::new(self.app.clone(), self.nav.clone(), root.clone(), channel.clone(), window, cx));
        self.thread = Some(v.clone());
        v
    }

    fn home(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let me = s.store().read_recursive().me.clone();
        let Some(me) = me else { return skeleton_rows(4, cx).into_any_element() };
        if me.memberships.is_empty() {
            let url = s.web_url("/");
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(muted(t("workspace.none"), cx).max_w(px(420.)).text_center())
                .child(Button::new("open-web").primary().label(t("workspace.openBrowser")).on_click(move |_, _, cx| cx.open_url(&url)))
                .into_any_element();
        }
        // Open the first unread channel, or the first one.
        let list = self.nav.read(cx).ordered_channels(cx);
        let pick = list.iter().find(|c| c.state.as_ref().is_some_and(|s| s.unread_count > 0)).or(list.first()).map(|c| c.id.clone());
        if let Some(id) = pick {
            let nav = self.nav.clone();
            cx.defer(move |cx| nav.update(cx, |n, cx| n.open_channel(&id, cx)));
        }
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(img("purlay/logo.png").size(px(64.)).rounded(px(16.)).opacity(0.9))
            .when(me.must_enrol_two_factor, |d| {
                let url = s.web_url("/");
                d.child(muted(t("workspace.twoFactor"), cx).max_w(px(420.)).text_center())
                    .child(Button::new("twofa").outline().label(t("workspace.openBrowser")).on_click(move |_, _, cx| cx.open_url(&url)))
            })
            .child(caption(t("palette.placeholder"), cx))
            .child(h_flex().gap_1().child(kbd("Ctrl", cx)).child(kbd("K", cx)))
            .bg(k.surface_100)
            .into_any_element()
    }

    /// The row across the top, right of the sidebar: what is open on the left, its tools
    /// on the right, then the caption buttons. Its empty parts move the window.
    fn top_bar(&self, route: &Route, inspector: &Option<Inspector>, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let me = s.user_id.clone();
        let ws = self.nav.read(cx).workspace.clone();
        let (ch, inbox_unread) = {
            let store = s.store().read_recursive();
            let ch = match route {
                Route::Channel(id) => store.channel(id).cloned(),
                _ => None,
            };
            let n = ws
                .as_ref()
                .and_then(|w| store.cache.get::<purlay_core::contract::models::NotificationPage>(&purlay_core::store::Key::Notifications(w.clone(), String::new(), true)))
                .map(|p| p.data.len() as i64)
                .unwrap_or(0);
            (ch, n)
        };

        // What is open: a channel's name over its topic, or nothing (pages carry a large title).
        let left: Option<AnyElement> = ch.as_ref().map(|ch| {
            let name = crate::sidebar::channel_title(ch, &me);
            let dm = ch.r#type == purlay_core::contract::models::ChannelType::DirectMessage;
            let lead: AnyElement = if dm {
                let peer = ch.peers.iter().find(|p| p.id != me).or(ch.peer.as_ref());
                let key = peer.map(|p| p.username.clone()).unwrap_or_else(|| name.clone());
                avatar(key, &name, 26., cx).into_any_element()
            } else {
                ico(if ch.r#type == purlay_core::contract::models::ChannelType::Private { "lock" } else { "hash" }, 16., k.ink).into_any_element()
            };
            let topic = ch.topic.clone().filter(|t| !t.is_empty());
            // Muted: says so, with when it ends; a click ends a timed mute now.
            let muted = ch.notification_level == Some(purlay_core::contract::models::NotificationLevel::None);
            let until = s.muted_until(&ch.id);
            let mute_chip = muted.then(|| {
                let (s2, id2) = (s.clone(), ch.id.clone());
                let text = match until {
                    Some(u) => tf("mute.until", &[&crate::sidebar::mute_end_text(u)]),
                    None => t("mute.muted").to_string(),
                };
                let hover = k.ink.opacity(0.07);
                h_flex()
                    .id("mute-chip")
                    .flex_none()
                    .gap_1()
                    .px_2()
                    .h(px(22.))
                    .rounded_full()
                    .bg(k.surface_200)
                    .occlude()
                    .when(until.is_some(), |d| d.cursor_pointer().hover(move |st| st.bg(hover)))
                    .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(if until.is_some() { t("mute.end") } else { t("channel.notify.none") }).build(w, cx))
                    .child(ico("bell-off", 12., k.ink_muted))
                    .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).whitespace_nowrap().child(text))
                    .on_click(move |_, _, _| {
                        if until.is_some() {
                            s2.unmute(&id2);
                        }
                    })
            });
            h_flex()
                .min_w_0()
                .gap(px(6.))
                .child(lead)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(text_style(div(), purlay_tokens::typography::HEADING, cx).text_color(k.ink).whitespace_nowrap().overflow_hidden().text_ellipsis().child(name))
                        .when_some(topic, |d, tp| {
                            d.child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(tp))
                        }),
                )
                .children(mute_chip)
                .into_any_element()
        });

        let nav = self.nav.clone();
        let mut tools = h_flex().flex_none().gap_2();
        tools = tools.child(crate::chrome::theme_button(s.prefs.clone(), cx));
        if let Some(ch) = &ch {
            let id = ch.id.clone();
            let is_dm = ch.r#type == purlay_core::contract::models::ChannelType::DirectMessage;
            let open = |i: Inspector| inspector.as_ref() == Some(&i);
            let toggle = |i: Inspector| {
                let nav = nav.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let i = i.clone();
                    nav.update(cx, |n, cx| n.toggle_inspector(i, cx));
                }
            };
            let s_star = s.clone();
            let (sid, starred) = (ch.id.clone(), ch.starred);
            let mut g = crate::chrome::tool_group(cx).child(
                crate::chrome::tool_button("tb-members", "users", tf("channel.members", &[&ch.member_count]), open(Inspector::Members(id.clone())), cx).on_click(toggle(Inspector::Members(id.clone()))),
            );
            if !is_dm {
                g = g.child(
                    crate::chrome::tool_button_tinted("tb-star", "star", if starred { t("channel.unstar") } else { t("channel.star") }, false, starred.then_some(k.warning), cx)
                        .on_click(move |_, _, _| {
                            s_star.star(&sid, !starred);
                        }),
                );
            }
            g = g.child(crate::chrome::tool_button("tb-pins", "pin", t("channel.pins"), open(Inspector::Pins(id.clone())), cx).on_click(toggle(Inspector::Pins(id.clone()))));
            tools = tools.child(g);

            let panel_open = inspector.is_some();
            let nav_p = nav.clone();
            let id_p = id.clone();
            tools = tools.child(
                crate::chrome::tool_group(cx)
                    .child(self.inbox_tool(inbox_unread, cx))
                    .child(crate::chrome::tool_button("tb-search", "search", t("nav.search"), false, cx).on_click({
                        let nav = nav.clone();
                        move |_, window, cx| {
                            nav.update(cx, |n, cx| n.go(Route::Search, cx));
                            window.dispatch_action(Box::new(OpenSearch), cx);
                        }
                    }))
                    .child(crate::chrome::tool_button("tb-panel", "panel-right", t("chrome.sidePanel"), panel_open, cx).on_click(move |_, _, cx| {
                        let id = id_p.clone();
                        nav_p.update(cx, |n, cx| if n.inspector.is_some() { n.close_inspector(cx) } else { n.toggle_inspector(Inspector::Files(id), cx) });
                    })),
            );

            // The rest of what a channel offers, in one menu.
            let app = self.app.clone();
            let menu_ch = ch.clone();
            let nav_m = nav.clone();
            tools = tools.child(
                crate::chrome::tool_group(cx).child(
                    Button::new("tb-more")
                        .ghost()
                        .tooltip(t("chrome.more"))
                        .size(px(30.))
                        .rounded(px(15.))
                        .child(ico("more-horizontal", 18., k.ink))
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                            let (n1, n2, i1, i2) = (nav_m.clone(), nav_m.clone(), menu_ch.id.clone(), menu_ch.id.clone());
                            let menu = menu
                                .item(gpui_kit::component::menu::PopupMenuItem::new(t("channel.files")).on_click(move |_, _, cx| {
                                    let i = i1.clone();
                                    n1.update(cx, |n, cx| n.toggle_inspector(Inspector::Files(i), cx));
                                }))
                                .item(gpui_kit::component::menu::PopupMenuItem::new(t("channel.bookmarks")).on_click(move |_, _, cx| {
                                    let i = i2.clone();
                                    n2.update(cx, |n, cx| n.toggle_inspector(Inspector::Bookmarks(i), cx));
                                }))
                                .separator();
                            crate::sidebar::channel_menu(menu, &menu_ch, &app, cx)
                        }),
                ),
            );
        } else {
            tools = tools.child(
                crate::chrome::tool_group(cx).child(self.inbox_tool(inbox_unread, cx)).child(crate::chrome::tool_button("tb-search", "search", t("nav.search"), matches!(route, Route::Search), cx).on_click({
                    let nav = nav.clone();
                    move |_, window, cx| {
                        nav.update(cx, |n, cx| n.go(Route::Search, cx));
                        window.dispatch_action(Box::new(OpenSearch), cx);
                    }
                })),
            );
        }

        let sidebar_hidden = self.sidebar_hidden;
        h_flex()
            .id("top-bar")
            .h(px(crate::chrome::TOP_BAR))
            .flex_none()
            .items_start()
            .bg(k.surface_100)
            .border_b_1()
            .border_color(k.border_subtle)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl(px(if sidebar_hidden { 12. } else { 20. }))
                    .pr_2()
                    .gap_3()
                    .window_control_area(WindowControlArea::Drag)
                    .when(sidebar_hidden, |d| {
                        d.child(crate::chrome::tool_group(cx).child(crate::chrome::tool_button("tb-sidebar", "panel-left", t("chrome.showSidebar"), false, cx).on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar_hidden = false;
                            cx.notify();
                        }))))
                    })
                    .children(left)
                    .child(div().flex_1())
                    .child(tools),
            )
            .child(crate::chrome::caption_buttons(window, cx))
            .into_any_element()
    }

    fn inbox_tool(&self, unread: i64, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let nav = self.nav.clone();
        let open = self.nav.read(cx).route == Route::Inbox;
        div()
            .relative()
            .child(crate::chrome::tool_button("tb-inbox", "inbox", t("nav.inbox"), open, cx).on_click(move |_, _, cx| nav.update(cx, |n, cx| n.go(Route::Inbox, cx))))
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-1.))
                        .right(px(-1.))
                        .min_w(px(16.))
                        .h(px(16.))
                        .px(px(4.))
                        .rounded_full()
                        .bg(k.danger)
                        .border_2()
                        .border_color(k.surface_100)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui_kit::white())
                        .child(if unread > 99 { "99+".to_string() } else { unread.to_string() }),
                )
            })
            .into_any_element()
    }

    fn shortcuts_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        v_flex()
            .w(px(480.))
            .p_5()
            .gap_2()
            .rounded(px(purlay_tokens::radius::RADIUS_2XL))
            .bg(k.surface_100)
            .border_1()
            .border_color(k.border_subtle)
            .shadow_xl()
            .child(title(t("settings.shortcuts"), cx))
            .children(SHORTCUTS.iter().map(|(keys, key)| {
                h_flex().py_1().child(body(t(key), cx).flex_1()).child(h_flex().gap_1().children(keys.split(' ').map(|k2| kbd(k2, cx))))
            }))
            .into_any_element()
    }
}

impl Focusable for MainView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let route = self.nav.read(cx).route.clone();
        let inspector = self.nav.read(cx).inspector.clone();
        let palette_open = self.nav.read(cx).palette;
        let shortcuts_open = self.nav.read(cx).shortcuts;
        if !matches!(route, Route::Inbox) {
            self.nav.update(cx, |n, cx| n.set_keys("inbox", vec![], cx));
        }
        for (view, r) in [("saved", Route::Saved), ("topics", Route::OpenTopics), ("issues", Route::Issues), ("scheduled", Route::Scheduled), ("reminders", Route::Reminders), ("settings", Route::Settings)] {
            if route != r {
                self.nav.update(cx, |n, cx| n.set_keys(view, vec![], cx));
            }
        }

        let page: AnyElement = match &route {
            Route::Channel(ch) => self.conversation(ch, window, cx).into_any_element(),
            Route::Inbox => self.inbox.clone().into_any_element(),
            Route::Search => self.search.clone().into_any_element(),
            Route::Saved => self.saved.clone().into_any_element(),
            Route::OpenTopics => self.topics.clone().into_any_element(),
            Route::Issues => self.issues.clone().into_any_element(),
            Route::Drafts => self.drafts.clone().into_any_element(),
            Route::Scheduled => self.scheduled.clone().into_any_element(),
            Route::Reminders => self.reminders.clone().into_any_element(),
            Route::Settings => self.settings.clone().into_any_element(),
            Route::Home => self.home(cx),
        };
        let side: Option<AnyElement> = match &inspector {
            Some(Inspector::Thread { root, channel }) => Some(self.thread_view(root, channel, window, cx).into_any_element()),
            Some(_) => Some(self.panel.clone().into_any_element()),
            None => {
                self.thread = None;
                None
            }
        };
        let narrow = window.viewport_size().width < px(purlay_tokens::size::NARROW);

        let overlay = if palette_open {
            Some(self.palette.clone().into_any_element())
        } else if shortcuts_open {
            Some(self.shortcuts_overlay(cx))
        } else {
            None
        };

        div()
            .key_context("Main")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_row()
            .bg(k.surface_100)
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.nav.update(cx, |n, cx| {
                    n.palette = !n.palette;
                    cx.notify();
                });
                if this.nav.read(cx).palette {
                    this.palette.update(cx, |p, cx| p.open(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| {
                this.nav.update(cx, |n, cx| n.go(Route::Search, cx));
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_hidden = !this.sidebar_hidden;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenInbox, _, cx| this.nav.update(cx, |n, cx| n.go(Route::Inbox, cx))))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.nav.update(cx, |n, cx| n.go(Route::Settings, cx))))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.nav.update(cx, |n, cx| n.go_back(cx))))
            .on_action(cx.listener(|this, _: &NextUnread, _, cx| this.nav.update(cx, |n, cx| n.step_unread(true, cx))))
            .on_action(cx.listener(|this, _: &PrevUnread, _, cx| this.nav.update(cx, |n, cx| n.step_unread(false, cx))))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _, cx| {
                this.nav.update(cx, |n, cx| {
                    n.shortcuts = !n.shortcuts;
                    cx.notify();
                })
            }))
            .on_action(cx.listener(|this, _: &NewConversation, window, cx| crate::dialogs::new_conversation(this.app.clone(), this.nav.clone(), window, cx)))
            .on_action(cx.listener(|this, _: &CloseInspector, _, cx| {
                this.nav.update(cx, |n, cx| {
                    if n.palette || n.shortcuts {
                        n.palette = false;
                        n.shortcuts = false;
                        cx.notify();
                    } else {
                        n.close_inspector(cx);
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                if let Some(ch) = this.nav.read(cx).current_channel().cloned() {
                    if let Some(v) = this.conversations.get(&ch).cloned() {
                        v.update(cx, |v, cx| v.focus_composer(window, cx));
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &MarkAllRead, _, cx| {
                if let (Some(s), Some(w)) = (this.app.read(cx).session().cloned(), this.nav.read(cx).workspace.clone()) {
                    s.mark_all_read(&w);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleDeepWork, _, cx| {
                let Some(s) = this.app.read(cx).session().cloned() else { return };
                let Some(w) = this.nav.read(cx).workspace.clone() else { return };
                let on = s.store().read_recursive().membership(&w).and_then(|m| m.deep_work_until.clone()).and_then(|u| u.unix_ms()).is_some_and(|ms| ms > now_ms());
                s.set_deep_work(&w, if on { None } else { Some(crate::sidebar::deep_work_until(60)) });
            }))
            .when((!narrow || inspector.is_none()) && !self.sidebar_hidden, |d| d.child(self.sidebar.clone()))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.top_bar(&route, &inspector, window, cx))
                    .child(div().flex().flex_row().flex_1().min_h_0().child(div().flex_1().min_w_0().h_full().child(page)).children(side)),
            )
            .children(overlay.map(|o| {
                div()
                    .id("overlay")
                    .absolute()
                    .inset_0()
                    .bg(k.scrim)
                    .flex()
                    .justify_center()
                    .pt(px(96.))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.nav.update(cx, |n, cx| {
                            n.palette = false;
                            n.shortcuts = false;
                            cx.notify();
                        })
                    }))
                    .child(div().id("overlay-inner").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(o))
            }))
    }
}
