//! The inspector's other faces: a channel's files, pins, bookmarks and members.

use crate::i18n::t;
use crate::nav::{Inspector, Nav};
use crate::pages::result_card;
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::store::{Key, LoadState};

pub struct PanelView {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    bm_url: Entity<InputState>,
    bm_title: Entity<InputState>,
    adding: bool,
    _subs: Vec<Subscription>,
}

impl PanelView {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify()), cx.observe(&nav, |_, _, cx| cx.notify())];
        let bm_url = cx.new(|cx| InputState::new(window, cx).placeholder("https://"));
        let bm_title = cx.new(|cx| InputState::new(window, cx).placeholder(t("issues.title")));
        PanelView { app, nav, bm_url, bm_title, adding: false, _subs: subs }
    }
}

fn panel_shell(title_key: &'static str, nav: Entity<Nav>, body_el: AnyElement, cx: &App) -> AnyElement {
    let k = c(cx);
    v_flex()
        .w(px(purlay_tokens::size::THREAD))
        .h_full()
        .flex_none()
        .bg(k.surface_100)
        .border_l_1()
        .border_color(k.border_subtle)
        .child(
            h_flex()
                .h(px(purlay_tokens::size::HEADER))
                .px_4()
                .gap_2()
                .border_b_1()
                .border_color(k.border_subtle)
                .child(heading(t(title_key), cx))
                .child(div().flex_1())
                .child(icon_button("close-panel", "x", "close", cx).on_click(move |_, _, cx| nav.update(cx, |n, cx| n.close_inspector(cx)))),
        )
        .child(div().id("panel-scroll").flex_1().min_h_0().overflow_y_scroll().child(body_el))
        .into_any_element()
}

fn state_or<T>(state: Option<&LoadState>, v: Option<&T>, empty: bool, empty_key: &'static str, f: impl FnOnce(&T) -> AnyElement, cx: &App) -> AnyElement {
    match (state, v) {
        (_, Some(v)) if !empty => f(v),
        (Some(LoadState::Failed(why)), None) => empty_state(format!("{} {why}", t("error.load")), cx).into_any_element(),
        (_, Some(_)) => empty_state(t(empty_key), cx).into_any_element(),
        _ => skeleton_rows(3, cx).into_any_element(),
    }
}

impl Render for PanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let Some(ins) = self.nav.read(cx).inspector.clone() else { return div().into_any_element() };
        let nav = self.nav.clone();
        let store = s.store().read_recursive();
        match ins {
            Inspector::Files(ch) => {
                let key = Key::Files(ch);
                let page = store.cache.get::<FilePage>(&key).cloned();
                let state = store.cache.state(&key).cloned();
                drop(store);
                let empty = page.as_ref().is_none_or(|p| p.data.is_empty());
                let body_el = state_or(
                    state.as_ref(),
                    page.as_ref(),
                    empty,
                    "files.empty",
                    |p| {
                        v_flex()
                            .p_2()
                            .gap_1()
                            .children(p.data.iter().enumerate().map(|(i, f)| {
                                let (s2, a) = (s.clone(), f.attachment.clone());
                                row_hover(("file", i), false, cx)
                                    .p_2()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(ico(if f.attachment.kind == AttachmentKind::Image { "image" } else { "file-text" }, 16., k.ink_muted))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(label(f.attachment.file_name.clone(), cx).text_color(k.ink).overflow_hidden().text_ellipsis().whitespace_nowrap())
                                            .child(caption(format!("{} · {} · {}", f.sender.display_name, crate::message::human_size(f.attachment.size_bytes), short_datetime(f.sent_at.unix_ms().unwrap_or(0))), cx)),
                                    )
                                    .on_click(move |_, window, cx| crate::dialogs::save_attachment(s2.clone(), a.clone(), window, cx))
                            }))
                            .into_any_element()
                    },
                    cx,
                );
                panel_shell("channel.files", nav, body_el, cx)
            }
            Inspector::Pins(ch) => {
                let key = Key::Pins(ch);
                let list = store.cache.get::<PinList>(&key).cloned();
                let state = store.cache.state(&key).cloned();
                drop(store);
                let empty = list.as_ref().is_none_or(|l| l.is_empty());
                let body_el = state_or(
                    state.as_ref(),
                    list.as_ref(),
                    empty,
                    "pins.empty",
                    |l| {
                        v_flex()
                            .py_2()
                            .children(l.iter().enumerate().map(|(i, p)| {
                                let (s2, m) = (s.clone(), p.message.clone());
                                let extra = h_flex()
                                    .pt_1()
                                    .child(div().flex_1())
                                    .child(Button::new(("unpin", i)).xsmall().ghost().label(t("msg.unpin")).on_click(move |_, _, _| {
                                        s2.pin(&m, false);
                                    }))
                                    .into_any_element();
                                result_card(("pin", i), &p.message, &s, &self.nav, Some(extra), cx)
                            }))
                            .into_any_element()
                    },
                    cx,
                );
                panel_shell("channel.pins", nav, body_el, cx)
            }
            Inspector::Bookmarks(ch) => {
                let key = Key::Bookmarks(ch.clone());
                let list = store.cache.get::<BookmarkList>(&key).cloned();
                let state = store.cache.state(&key).cloned();
                drop(store);
                let empty = list.as_ref().is_none_or(|l| l.is_empty());
                let adder = if self.adding {
                    v_flex()
                        .p_3()
                        .gap_2()
                        .child(Input::new(&self.bm_url).small())
                        .child(Input::new(&self.bm_title).small())
                        .child(
                            h_flex().gap_2().child(div().flex_1()).child(Button::new("bm-cancel").xsmall().ghost().label(t("cancel")).on_click(cx.listener(|this, _, _, cx| {
                                this.adding = false;
                                cx.notify();
                            }))).child(Button::new("bm-save").xsmall().primary().label(t("save")).on_click(cx.listener({
                                let ch = ch.clone();
                                move |this, _, window, cx| {
                                    let url = this.bm_url.read(cx).value().trim().to_string();
                                    let title = this.bm_title.read(cx).value().trim().to_string();
                                    if url.is_empty() {
                                        return;
                                    }
                                    if let Some(s) = this.app.read(cx).session().cloned() {
                                        let title = if title.is_empty() { url.clone() } else { title };
                                        let h = s.add_bookmark(&ch, url, title);
                                        let s2 = s.clone();
                                        let k2 = Key::Bookmarks(ch.clone());
                                        s.rt.spawn(async move {
                                            let _ = h.await;
                                            s2.refresh(k2);
                                        });
                                    }
                                    this.adding = false;
                                    this.bm_url.update(cx, |s, cx| s.set_value("", window, cx));
                                    this.bm_title.update(cx, |s, cx| s.set_value("", window, cx));
                                    cx.notify();
                                }
                            }))),
                        )
                        .into_any_element()
                } else {
                    div()
                        .p_3()
                        .child(Button::new("bm-add").small().outline().label(t("bookmarks.add")).on_click(cx.listener(|this, _, _, cx| {
                            this.adding = true;
                            cx.notify();
                        })))
                        .into_any_element()
                };
                let list_el = state_or(
                    state.as_ref(),
                    list.as_ref(),
                    empty,
                    "bookmarks.empty",
                    |l| {
                        v_flex()
                            .px_2()
                            .gap_1()
                            .children(l.iter().enumerate().map(|(i, b)| {
                                let u = b.url.clone();
                                let (s2, ch2, id2) = (s.clone(), ch.clone(), b.id.clone());
                                row_hover(("bm", i), false, cx)
                                    .p_2()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(ico("link", 14., k.accent_text))
                                    .child(v_flex().flex_1().min_w_0().child(label(b.title.clone(), cx).text_color(k.ink)).child(caption(b.url.clone(), cx).overflow_hidden().text_ellipsis().whitespace_nowrap()))
                                    .child(icon_button(("bm-rm", i), "x", "delete", cx).on_click(move |_, _, _| {
                                        let h = s2.remove_bookmark(&ch2, &id2);
                                        let s3 = s2.clone();
                                        let k3 = Key::Bookmarks(ch2.clone());
                                        s2.rt.spawn(async move {
                                            let _ = h.await;
                                            s3.refresh(k3);
                                        });
                                    }))
                                    .on_click(move |_, _, cx| cx.open_url(&u))
                            }))
                            .into_any_element()
                    },
                    cx,
                );
                panel_shell("channel.bookmarks", nav, v_flex().child(adder).child(list_el).into_any_element(), cx)
            }
            Inspector::Members(ch) => {
                let key = Key::ChannelMembers(ch);
                let list = store.cache.get::<ChannelMemberList>(&key).cloned();
                let state = store.cache.state(&key).cloned();
                let presence = store.presence.clone();
                drop(store);
                let empty = list.as_ref().is_none_or(|l| l.is_empty());
                let ws = self.nav.read(cx).workspace.clone();
                let body_el = state_or(
                    state.as_ref(),
                    list.as_ref(),
                    empty,
                    "members.empty",
                    |l| {
                        let mut l = l.clone();
                        l.sort_by_key(|m| m.user.display_name.to_lowercase());
                        v_flex()
                            .p_2()
                            .gap_1()
                            .children(l.into_iter().enumerate().map(|(i, m)| {
                                let p = presence.get(&m.user.id).copied();
                                let (app, nav2, uid, w) = (self.app.clone(), self.nav.clone(), m.user.id.clone(), ws.clone());
                                row_hover(("member", i), false, cx)
                                    .p_2()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(div().relative().child(avatar(m.user.username.clone(), &m.user.display_name, 28., cx)).when_some(p, |d, p| d.child(div().absolute().right(px(-2.)).bottom(px(-2.)).child(presence_dot(p.online, p.away, cx)))))
                                    .child(v_flex().flex_1().child(body(m.user.display_name.clone(), cx)).child(caption(format!("@{}", m.user.username), cx)))
                                    .on_click(move |_, _, cx| {
                                        let Some(s) = app.read(cx).session().cloned() else { return };
                                        let Some(w) = w.clone() else { return };
                                        if uid == s.user_id {
                                            return;
                                        }
                                        let h = s.open_conversation(&w, vec![uid.clone()]);
                                        let nav3 = nav2.clone();
                                        cx.spawn(async move |cx| {
                                            if let Ok(Ok(ch)) = h.await {
                                                nav3.update(cx, |n, cx| n.open_channel(&ch.id, cx));
                                            }
                                        })
                                        .detach();
                                    })
                            }))
                            .into_any_element()
                    },
                    cx,
                );
                panel_shell("settings.members", nav, body_el, cx)
            }
            Inspector::Thread { .. } => div().into_any_element(),
        }
    }
}
