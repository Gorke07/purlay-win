//! The thread inspector beside the feed (UX §4): Follow, the root, Resolve/Reopen, the
//! replies and a reply box with its own draft. Only one thread is open at a time.

use crate::composer::{Composer, ComposerEvent};
use crate::i18n::{t, tf};
use crate::message::{message_row, pending_row, MsgCtx};
use crate::nav::{Nav, NavEvent};
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::session::Session;
use purlay_core::store::Key;
use std::collections::HashSet;
use std::rc::Rc;

pub struct ThreadView {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    session: Session,
    pub root: Uuid,
    pub channel: Uuid,
    workspace: Uuid,
    composer: Entity<Composer>,
    scroll: ScrollHandle,
    last_count: usize,
    last_read: Option<Uuid>,
    _subs: Vec<Subscription>,
}

impl ThreadView {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, root: Uuid, channel: Uuid, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session = app.read(cx).session().cloned().expect("session");
        let workspace = session.store().read_recursive().workspace_of_channel(&channel).unwrap_or_default();
        let composer = cx.new(|cx| Composer::new(session.clone(), workspace.clone(), channel.clone(), Some(root.clone()), t("thread.replyPlaceholder").into(), window, cx));
        let subs = vec![
            cx.observe(&app, |this, _, cx| {
                this.after_change(cx);
                cx.notify();
            }),
            cx.subscribe_in(&composer, window, |this, _, e: &ComposerEvent, window, cx| match e {
                ComposerEvent::Sent => this.scroll.scroll_to_bottom(),
                ComposerEvent::EditLast => {
                    let me = this.session.user_id.clone();
                    let last = this.session.store().read_recursive().threads.get(&this.root).and_then(|t| {
                        t.replies.messages.iter().rev().find(|m| m.sender.as_ref().is_some_and(|s| s.id == me) && m.deleted_at.is_none()).cloned()
                    });
                    if let Some(m) = last {
                        this.composer.update(cx, |c, cx| c.edit(m, window, cx));
                    }
                }
            }),
            cx.subscribe_in(&nav, window, |this, _, e: &NavEvent, window, cx| {
                if let NavEvent::Edit { message, in_thread: true } = e {
                    if message.parent_id.as_ref() == Some(&this.root) {
                        let m = (**message).clone();
                        this.composer.update(cx, |c, cx| c.edit(m, window, cx));
                    }
                }
            }),
        ];
        let v = ThreadView { app, nav, session, root, channel, workspace, composer, scroll: ScrollHandle::new(), last_count: 0, last_read: None, _subs: subs };
        v.composer.update(cx, |c, cx| c.focus(window, cx));
        v
    }

    fn after_change(&mut self, cx: &mut Context<Self>) {
        let (count, newest) = {
            let store = self.session.store().read_recursive();
            let t = store.threads.get(&self.root);
            (t.map(|t| t.replies.messages.len()).unwrap_or(0), t.and_then(|t| t.replies.newest_id().cloned()))
        };
        if count != self.last_count {
            self.last_count = count;
            self.scroll.scroll_to_bottom();
        }
        if self.app.read(cx).window_active {
            if let Some(n) = newest {
                if self.last_read.as_ref() != Some(&n) {
                    self.last_read = Some(n.clone());
                    self.session.mark_thread_read(&self.root, &n);
                }
            }
        }
    }

    fn ctx(&self, cx: &App) -> MsgCtx {
        let store = self.session.store().read_recursive();
        let membership = store.membership(&self.workspace).cloned();
        let saved: HashSet<Uuid> = store.cache.get::<SavedIds>(&Key::SavedIds(self.workspace.clone())).map(|s| s.iter().map(|x| x.message_id.clone()).collect()).unwrap_or_default();
        let _ = cx;
        MsgCtx {
            session: self.session.clone(),
            nav: self.nav.clone(),
            me: self.session.user_id.clone(),
            moderator: membership.as_ref().is_some_and(|m| matches!(m.role, UserRole::Owner | UserRole::Admin)),
            in_thread: true,
            open_thread: Some(self.root.clone()),
            saved: Rc::new(saved),
            workspace_slug: membership.map(|m| m.workspace.slug.clone()),
            compact_density: false,
        }
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let (root, replies, loaded, following) = {
            let store = self.session.store().read_recursive();
            let t = store.threads.get(&self.root);
            let root = t.and_then(|t| t.root.clone()).or_else(|| store.find_message(&self.channel, &self.root).cloned());
            let following = store.cache.get::<ThreadFollow>(&Key::ThreadFollow(self.root.clone())).map(|f| f.following).or(t.and_then(|t| t.following));
            (root, t.map(|t| t.replies.messages.clone()).unwrap_or_default(), t.is_some_and(|t| t.loaded), following)
        };
        let pending: Vec<_> = self.session.store().read_recursive().pending_for(&self.channel, Some(&self.root)).into_iter().cloned().collect();
        let ctx = self.ctx(cx);
        let resolved = root.as_ref().and_then(|r| r.thread.as_ref()).is_some_and(|t| t.resolved_at.is_some());
        let question = root.as_ref().and_then(|r| r.thread.as_ref()).and_then(|t| t.knowledge.clone());

        let nav = self.nav.clone();
        let s1 = self.session.clone();
        let root_id = self.root.clone();
        let follow_label = if following == Some(true) { t("thread.unfollow") } else { t("thread.follow") };
        let header = h_flex()
            .h(px(purlay_tokens::size::HEADER))
            .flex_none()
            .px_4()
            .gap_2()
            .border_b_1()
            .border_color(k.border_subtle)
            .child(heading(t("thread.title"), cx))
            .when(resolved, |d| d.child(div().px_2().rounded_full().bg(k.success_subtle).child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.success_text).child(t("thread.resolved")))))
            .child(div().flex_1())
            .child(Button::new("follow").xsmall().ghost().label(follow_label).on_click(move |_, _, _| {
                s1.follow(&root_id, following != Some(true));
            }))
            .when_some(root.clone(), |d, r| {
                let s = self.session.clone();
                let nav = self.nav.clone();
                d.child(Button::new("resolve").xsmall().outline().label(if resolved { t("thread.reopen") } else { t("thread.resolve") }).on_click(move |_, _, cx| {
                    s.resolve(&r, !resolved);
                    // Resolving folds the thread away and closes the panel (UX §4.3).
                    if !resolved {
                        nav.update(cx, |n, cx| n.close_inspector(cx));
                    }
                }))
            })
            .child(icon_button("close-thread", "x", "close", cx).on_click(move |_, _, cx| nav.update(cx, |n, cx| n.close_inspector(cx))));

        let mut list = v_flex().py_2();
        if let Some(r) = &root {
            list = list.child(message_row(r, &ctx, false, false, cx));
            if let Some(q) = &question {
                if q.is_question {
                    list = list.child(
                        h_flex()
                            .mx_4()
                            .px_3()
                            .py_1()
                            .gap_2()
                            .rounded(px(8.))
                            .bg(k.warning_subtle)
                            .child(ico("circle-help", 14., k.warning_text))
                            .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.warning_text).child(t("thread.question")))
                            .when(q.accepted_answer.is_some(), |d| d.child(caption(format!("· {}", t("thread.accepted")), cx))),
                    );
                }
            }
            let n = r.thread.as_ref().map(|t| t.reply_count).unwrap_or(replies.len() as i64);
            list = list.child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .child(caption(if n == 1 { t("thread.reply1").to_string() } else { tf("thread.replies", &[&n]) }, cx))
                    .child(div().flex_1().h(px(1.)).bg(k.border_subtle)),
            );
        }
        if !loaded {
            list = list.child(skeleton_rows(3, cx));
        }
        let mut prev: Option<&Message> = None;
        for m in &replies {
            let compact = prev.is_some_and(|p| {
                p.sender.as_ref().map(|s| &s.id) == m.sender.as_ref().map(|s| &s.id)
                    && m.created_at.unix_ms().unwrap_or(0) - p.created_at.unix_ms().unwrap_or(0) < 5 * 60 * 1000
            });
            list = list.child(message_row(m, &ctx, compact, false, cx));
            prev = Some(m);
        }
        for p in &pending {
            list = list.child(pending_row(p, &ctx, cx));
        }

        v_flex()
            .w(px(purlay_tokens::size::THREAD))
            .h_full()
            .flex_none()
            .bg(k.surface_100)
            .border_l_1()
            .border_color(k.border_subtle)
            .child(header)
            .child(div().id("thread-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).child(list))
            .child(div().px_3().pb_3().pt_1().child(self.composer.clone()))
    }
}
