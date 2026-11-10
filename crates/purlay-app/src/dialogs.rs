//! Dialogs: a new channel, a new conversation, edit history, a picture, saving a file.

use crate::i18n::{t, tf};
use crate::nav::Nav;
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::*;
use purlay_core::contract::constants::CONVERSATION_PEOPLE_MAX;
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::session::Session;
use purlay_core::store::Key;

// --- new channel ---------------------------------------------------------------------

struct NewChannel {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    name: Entity<InputState>,
    topic: Entity<InputState>,
    private: bool,
    busy: bool,
    error: Option<String>,
}

impl NewChannel {
    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.app.read(cx).session().cloned() else { return };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return };
        let name = self.name.read(cx).value().trim().trim_start_matches('#').to_string();
        if name.is_empty() {
            return;
        }
        let topic = self.topic.read(cx).value().trim().to_string();
        let req = CreateChannelRequest {
            name,
            topic: if topic.is_empty() { None } else { Some(topic) },
            r#type: Some(if self.private { CreateChannelRequestType::Private } else { CreateChannelRequestType::Public }),
        };
        self.busy = true;
        cx.notify();
        let h = s.create_channel(&w, req);
        let nav = self.nav.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = h.await;
            let _ = this.update_in(cx, |me, window, cx| {
                me.busy = false;
                match r {
                    Ok(Ok(ch)) => {
                        window.close_dialog(cx);
                        nav.update(cx, |n, cx| n.open_channel(&ch.id, cx));
                    }
                    Ok(Err(e)) => me.error = Some(tf("error.action", &[&e])),
                    Err(e) => me.error = Some(tf("error.action", &[&e])),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for NewChannel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        v_flex()
            .gap_3()
            .child(v_flex().gap_1().child(label(t("channel.create.name"), cx)).child(Input::new(&self.name).prefix(div().text_color(k.ink_muted).child("#"))))
            .child(v_flex().gap_1().child(label(t("channel.create.topic"), cx)).child(Input::new(&self.topic)))
            .child(Checkbox::new("private").label(t("channel.create.private")).checked(self.private).on_click(cx.listener(|this, v: &bool, _, cx| {
                this.private = *v;
                cx.notify();
            })))
            .when_some(self.error.clone(), |d, e| d.child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.danger_text).child(e)))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1())
                    .child(Button::new("nc-cancel").ghost().label(t("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("nc-create").primary().loading(self.busy).label(t("channel.create.create")).on_click(cx.listener(|this, _, window, cx| this.create(window, cx)))),
            )
    }
}

pub fn new_channel(app: Entity<AppModel>, nav: Entity<Nav>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("engineering"));
        let topic = cx.new(|cx| InputState::new(window, cx));
        NewChannel { app, nav, name, topic, private: false, busy: false, error: None }
    });
    let focus = view.read(cx).name.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |d, _, _| d.title(t("channel.create.title")).width(px(purlay_tokens::size::DIALOG)).child(view.clone()));
    window.focus(&focus, cx);
}

// --- new conversation ---------------------------------------------------------------

struct NewConversation {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    search: Entity<InputState>,
    chosen: Vec<User>,
    busy: bool,
    _subs: Vec<Subscription>,
}

impl NewConversation {
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chosen.is_empty() {
            return;
        }
        let Some(s) = self.app.read(cx).session().cloned() else { return };
        let Some(w) = self.nav.read(cx).workspace.clone() else { return };
        self.busy = true;
        let h = s.open_conversation(&w, self.chosen.iter().map(|u| u.id.clone()).collect());
        let nav = self.nav.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = h.await;
            let _ = this.update_in(cx, |me, window, cx| {
                me.busy = false;
                match r {
                    Ok(Ok(ch)) => {
                        window.close_dialog(cx);
                        nav.update(cx, |n, cx| n.open_channel(&ch.id, cx));
                    }
                    Ok(Err(e)) => window.push_notification(tf("error.action", &[&e]), cx),
                    Err(e) => window.push_notification(tf("error.action", &[&e]), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for NewConversation {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div() };
        let w = self.nav.read(cx).workspace.clone().unwrap_or_default();
        let q = self.search.read(cx).value().trim().to_lowercase();
        let members = s.store().read_recursive().cache.get::<WorkspaceMemberList>(&Key::Members(w)).cloned().unwrap_or_default();
        let max = (CONVERSATION_PEOPLE_MAX - 1) as usize;
        let mut found: Vec<User> = members
            .into_iter()
            .map(|m| m.user)
            .filter(|u| u.id != s.user_id && !self.chosen.iter().any(|c| c.id == u.id))
            .filter(|u| q.is_empty() || u.display_name.to_lowercase().contains(&q) || u.username.to_lowercase().contains(&q))
            .collect();
        found.sort_by_key(|u| u.display_name.to_lowercase());
        found.truncate(30);
        let hover = k.surface_050;
        v_flex()
            .gap_3()
            .child(h_flex().flex_wrap().gap_1().children(self.chosen.iter().enumerate().map(|(i, u)| {
                let id = u.id.clone();
                h_flex()
                    .id(("chosen", i))
                    .h(px(26.))
                    .pl_2()
                    .pr_1()
                    .gap_1()
                    .rounded_full()
                    .bg(k.accent_subtle)
                    .child(label(u.display_name.clone(), cx).text_color(k.accent_text))
                    .child(icon_button(("unchoose", i), "x", "delete", cx).size(px(20.)).on_click(cx.listener(move |this, _, _, cx| {
                        this.chosen.retain(|c| c.id != id);
                        cx.notify();
                    })))
            })))
            .child(Input::new(&self.search))
            .when(self.chosen.len() >= max, |d| d.child(caption(tf("dm.limit", &[&CONVERSATION_PEOPLE_MAX]), cx)))
            .child(div().id("dm-list").h(px(280.)).overflow_y_scroll().child(v_flex().gap(px(2.)).children(found.into_iter().enumerate().map(|(i, u)| {
                let u2 = u.clone();
                h_flex()
                    .id(("dm-person", i))
                    .h(px(36.))
                    .px_2()
                    .gap_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(avatar(u.username.clone(), &u.display_name, 24., cx))
                    .child(body(u.display_name.clone(), cx))
                    .child(caption(format!("@{}", u.username), cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.chosen.len() < max {
                            this.chosen.push(u2.clone());
                            this.search.update(cx, |s, cx| s.set_value("", window, cx));
                            cx.notify();
                        }
                    }))
            }))))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1())
                    .child(Button::new("dm-cancel").ghost().label(t("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("dm-start").primary().loading(self.busy).disabled(self.chosen.is_empty()).label(t("dm.start")).on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))),
            )
    }
}

pub fn new_conversation(app: Entity<AppModel>, nav: Entity<Nav>, window: &mut Window, cx: &mut App) {
    if let (Some(s), Some(w)) = (app.read(cx).session().cloned(), nav.read(cx).workspace.clone()) {
        s.send(purlay_core::engine::Command::Ensure(Key::Members(w)));
    }
    let view = cx.new(|cx| {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("dm.placeholder")));
        let subs = vec![cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        })];
        NewConversation { app, nav, search, chosen: vec![], busy: false, _subs: subs }
    });
    let focus = view.read(cx).search.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |d, _, _| d.title(t("dm.title")).width(px(purlay_tokens::size::DIALOG)).child(view.clone()));
    window.focus(&focus, cx);
}

// --- edit history ---------------------------------------------------------------------

pub fn edit_history(s: Session, message: Uuid, window: &mut Window, cx: &mut App) {
    let h = s.versions(&message);
    window
        .spawn(cx, async move |cx| {
            let r = h.await;
            let _ = cx.update(|window, cx| match r {
                Ok(Ok(page)) => {
                    let versions = page.data;
                    window.open_dialog(cx, move |d, _, cx| {
                        let mut col = v_flex().gap_3().max_h(px(480.));
                        for (i, v) in versions.iter().enumerate() {
                            col = col.child(
                                v_flex()
                                    .gap_1()
                                    .child(caption(short_datetime(v.replaced_at.unix_ms().unwrap_or(0)), cx))
                                    .child(crate::markdown_view::render_markdown(&format!("ver-{i}"), &v.content, cx)),
                            );
                        }
                        d.title(t("msg.history")).width(px(purlay_tokens::size::DIALOG)).child(div().id("versions").overflow_y_scroll().child(col))
                    });
                }
                Ok(Err(e)) => window.push_notification(tf("error.action", &[&e]), cx),
                Err(e) => window.push_notification(tf("error.action", &[&e]), cx),
            });
        })
        .detach();
}

// --- a picture ------------------------------------------------------------------------

pub fn image_viewer(s: Session, attachment: Uuid, name: String, window: &mut Window, cx: &mut App) {
    let images = crate::images::images(cx);
    let id = attachment.clone();
    let _ = s;
    window.open_dialog(cx, move |d, _, cx| {
        let pic = images.update(cx, |i, cx| i.get(&id, false, cx).or_else(|| i.get(&id, true, cx)));
        d.title(name.clone()).width(px(purlay_tokens::size::DIALOG_LG)).child(
            div()
                .w_full()
                .h(px(520.))
                .flex()
                .items_center()
                .justify_center()
                .child(match pic {
                    Some(p) => img(p).max_w_full().max_h_full().object_fit(ObjectFit::Contain).into_any_element(),
                    None => caption(t("loading"), cx).into_any_element(),
                }),
        )
    });
}

// --- saving a file ---------------------------------------------------------------------

/// Asks where to save, then downloads. The name is cleaned of control and bidi characters.
pub fn save_attachment(s: Session, a: Attachment, window: &mut Window, cx: &mut App) {
    let clean: String = a.file_name.chars().filter(|c| !c.is_control() && !matches!(*c as u32, 0x200b..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff)).collect();
    let dir = directories::UserDirs::new().and_then(|d| d.download_dir().map(|p| p.to_path_buf())).unwrap_or_else(std::env::temp_dir);
    let rx = cx.prompt_for_new_path(&dir, Some(&clean));
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let r = s.attachment_bytes(&a.id, false).await;
            let msg = match r {
                Ok(Ok(bytes)) => match std::fs::write(&path, &bytes) {
                    Ok(()) => None,
                    Err(e) => Some(tf("error.action", &[&e])),
                },
                Ok(Err(e)) => Some(tf("error.action", &[&e])),
                Err(e) => Some(tf("error.action", &[&e])),
            };
            if let Some(m) = msg {
                let _ = cx.update(|window, cx| window.push_notification(m, cx));
            }
        })
        .detach();
}
