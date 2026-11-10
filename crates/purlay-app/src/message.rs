//! One message row, shared by the feed and the thread panel.

use crate::i18n::{t, tf};
use crate::images::images;
use crate::markdown_view::render_markdown;
use crate::nav::{Nav, NavEvent};
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::contract::{Timestamp, Uuid};
use purlay_core::session::Session;
use purlay_core::store::{Delivery, PendingMessage};
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Clone)]
pub struct MsgCtx {
    pub session: Session,
    pub nav: Entity<Nav>,
    pub me: Uuid,
    pub moderator: bool,
    pub in_thread: bool,
    pub open_thread: Option<Uuid>,
    pub saved: Rc<HashSet<Uuid>>,
    pub workspace_slug: Option<String>,
    pub compact_density: bool,
}

impl MsgCtx {
    pub fn permalink(&self, m: &Message) -> String {
        let slug = self.workspace_slug.clone().unwrap_or_default();
        self.session.web_url(&format!("/w/{slug}/c/{}?message={}", m.channel_id, m.id))
    }
}

fn sender_name(m: &Message) -> String {
    if let Some(s) = &m.sender {
        return s.display_name.clone();
    }
    if let Some(w) = &m.incoming_webhook {
        return w.name.clone();
    }
    if m.github_event.is_some() {
        return "GitHub".into();
    }
    "Purlay".into()
}

/// The row for a message the server has.
pub fn message_row(m: &Message, ctx: &MsgCtx, compact: bool, highlight: bool, cx: &mut App) -> AnyElement {
    let k = c(cx);
    let id = m.id.clone();
    let created = m.created_at.unix_ms().unwrap_or(0);
    let deleted = m.deleted_at.is_some();
    let name = sender_name(m);
    let avatar_key = m.sender.as_ref().map(|s| s.username.clone()).unwrap_or_else(|| name.clone());
    let compact = compact && !ctx.compact_density || (ctx.compact_density && compact);
    let group = SharedString::from(format!("msg-{id}"));
    let gutter = px(40.);

    let left: AnyElement = if compact {
        div()
            .w(gutter)
            .flex_none()
            .pt(px(4.))
            .invisible()
            .group_hover(group.clone(), |s| s.visible())
            .child(caption(clock_time(created), cx).text_size(px(10.)))
            .into_any_element()
    } else {
        div().w(gutter).flex_none().pt(px(2.)).child(avatar(avatar_key, &name, 38., cx)).into_any_element()
    };

    let mut content = v_flex().flex_1().min_w_0().gap(px(2.));
    if !compact {
        content = content.child(
            h_flex()
                .gap_2()
                .items_baseline()
                .child(text_style(div(), purlay_tokens::typography::BODY, cx).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(k.ink).child(name.clone()))
                .when(m.incoming_webhook.is_some(), |d| d.child(caption(t("msg.webhook"), cx)))
                .child(caption(clock_time(created), cx).id(SharedString::from(format!("ts-{id}"))).tooltip({
                    let full = short_datetime(created);
                    move |w, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(w, cx)
                })),
        );
    }
    if deleted {
        content = content.child(muted(t("msg.deleted"), cx).italic());
    } else {
        content = content.child(
            div()
                .text_color(k.ink)
                .child(render_markdown(&format!("md-{id}"), &m.content, cx))
                .when(m.edited_at.is_some(), |d| d.child(caption(t("msg.edited"), cx))),
        );
        if !m.attachments.is_empty() {
            content = content.child(attachments(&m.attachments, ctx, cx));
        }
        content = content.children(link_previews(m, ctx, cx));
        if !m.reactions.is_empty() {
            content = content.child(reactions(m, ctx, cx));
        }
    }
    if !ctx.in_thread {
        if let Some(th) = m.thread.as_ref().filter(|t| t.reply_count > 0 || t.knowledge.as_ref().is_some_and(|k| k.is_question)) {
            content = content.child(thread_line(m, th, ctx, cx));
        }
    }

    let toolbar = if deleted { None } else { Some(hover_toolbar(m, ctx, cx)) };
    let menu_m = m.clone();
    let menu_ctx = ctx.clone();
    let hover_bg = k.surface_050;
    // The context menu wraps an outer box: wrapping the row itself loses its hover styles.
    let row = div()
        .id(SharedString::from(format!("row-{id}")))
        .group(group.clone())
        .relative()
        .flex()
        .flex_row()
        .items_start()
        .gap_3()
        .px(px(24.))
        .py(px(if compact { 2. } else { 8. }))
        // A permalink's target: an accent tint with an accent edge, unlike the hover tone.
        .when(highlight, |d| d.bg(k.accent.opacity(0.14)).border_l_2().border_color(k.accent))
        .when(!highlight, |d| d.hover(move |s| s.bg(hover_bg)))
        .child(left)
        .child(content)
        .children(toolbar.map(|t| div().absolute().top(px(-16.)).right(px(24.)).invisible().group_hover(group.clone(), |s| s.visible()).child(t)));
    div()
        .id(SharedString::from(format!("rowmenu-{id}")))
        .child(row)
        .context_menu(move |menu, window, cx| message_menu(menu, &menu_m, &menu_ctx, window, cx))
        .into_any_element()
}

/// The row for a message still on its way (or refused).
pub fn pending_row(p: &PendingMessage, ctx: &MsgCtx, cx: &mut App) -> AnyElement {
    let k = c(cx);
    let me = ctx.session.store().read_recursive().me.clone();
    let name = me.as_ref().map(|m| m.user.display_name.clone()).unwrap_or_default();
    let username = me.as_ref().map(|m| m.user.username.clone()).unwrap_or_default();
    let failed = p.delivery == Delivery::Failed;
    let (id, s1, s2) = (p.id.clone(), ctx.session.clone(), ctx.session.clone());
    let id2 = id.clone();
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap_3()
        .px(px(24.))
        .py(px(8.))
        .when(failed, |d| d.border_l_2().border_color(k.danger).bg(k.danger_subtle))
        .child(div().w(px(40.)).flex_none().pt(px(2.)).child(avatar(username, &name, 38., cx)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    h_flex()
                        .gap_2()
                        .child(text_style(div(), purlay_tokens::typography::BODY, cx).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(ico("clock", 12., k.ink_subtle))
                        .when(!failed, |d| d.child(caption(t("msg.sending"), cx))),
                )
                .child(render_markdown(&format!("pmd-{}", p.id), &p.content, cx))
                .when(!p.attachments.is_empty(), |d| d.child(attachments(&p.attachments, ctx, cx)))
                .when(failed, |d| {
                    d.child(
                        h_flex()
                            .gap_2()
                            .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.danger_text).child(t("msg.failed")))
                            .child(Button::new(SharedString::from(format!("retry-{id}"))).xsmall().link().label(t("msg.retry")).on_click(move |_, _, _| s1.retry(&id)))
                            .child(caption("·", cx))
                            .child(Button::new(SharedString::from(format!("discard-{id2}"))).xsmall().link().label(t("msg.discard")).on_click(move |_, _, _| s2.discard(&id2))),
                    )
                }),
        )
        .into_any_element()
}

fn hover_toolbar(m: &Message, ctx: &MsgCtx, cx: &mut App) -> AnyElement {
    let k = c(cx);
    let id = m.id.clone();
    let mut bar = h_flex().gap(px(2.)).p(px(3.)).rounded(px(purlay_tokens::radius::RADIUS_2XL)).bg(if crate::theme::Tokens::get(cx).dark { k.surface_200 } else { k.surface_100 }).border_1().border_color(k.border_subtle).shadow_md();
    let (m1, c1) = (m.clone(), ctx.clone());
    bar = bar.child(icon_button(SharedString::from(format!("react-{id}")), "smile-plus", "msg.react", cx).on_click(move |_, window, cx| {
        open_reaction_picker(m1.clone(), c1.clone(), window, cx);
    }));
    if !ctx.in_thread && m.parent_id.is_none() {
        let (nav, root, ch) = (ctx.nav.clone(), m.id.clone(), m.channel_id.clone());
        bar = bar.child(icon_button(SharedString::from(format!("thread-{id}")), "message-square-text", "msg.reply", cx).on_click(move |_, _, cx| {
            nav.update(cx, |n, cx| n.open_thread(&root, &ch, cx));
        }));
    }
    let (m2, c2) = (m.clone(), ctx.clone());
    bar = bar.child(
        Button::new(SharedString::from(format!("more-{id}")))
            .ghost()
            .xsmall()
            .icon(crate::assets::icon("more-horizontal"))
            .dropdown_menu(move |menu, window, cx| message_menu(menu, &m2, &c2, window, cx)),
    );
    bar.into_any_element()
}

pub fn message_menu(menu: PopupMenu, m: &Message, ctx: &MsgCtx, _window: &mut Window, _cx: &mut Context<PopupMenu>) -> PopupMenu {
    let mut menu = menu;
    let mine = m.sender.as_ref().is_some_and(|s| s.id == ctx.me);
    let deleted = m.deleted_at.is_some();
    let s = ctx.session.clone();
    if !deleted {
        let text = m.content.clone();
        menu = menu.item(PopupMenuItem::new(t("msg.copyText")).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))));
    }
    let link = ctx.permalink(m);
    menu = menu.item(PopupMenuItem::new(t("msg.copyLink")).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))));
    if !ctx.in_thread && m.parent_id.is_none() {
        let (nav, root, ch) = (ctx.nav.clone(), m.id.clone(), m.channel_id.clone());
        menu = menu.item(PopupMenuItem::new(t("msg.reply")).on_click(move |_, _, cx| nav.update(cx, |n, cx| n.open_thread(&root, &ch, cx))));
    }
    if !ctx.in_thread {
        let (s1, ch, id) = (s.clone(), m.channel_id.clone(), m.id.clone());
        menu = menu.item(PopupMenuItem::new(t("msg.markUnread")).on_click(move |_, _, _| {
            s1.mark_unread(&ch, &id);
        }));
    }
    menu = menu.separator();
    let saved = ctx.saved.contains(&m.id);
    let (s2, m2) = (s.clone(), m.clone());
    menu = menu.item(PopupMenuItem::new(if saved { t("msg.unsave") } else { t("msg.save") }).on_click(move |_, _, _| {
        if saved {
            s2.unsave(&m2);
        } else {
            s2.save(&m2, None);
        }
    }));
    if m.parent_id.is_none() {
        let (s3, m3) = (s.clone(), m.clone());
        menu = menu.item(PopupMenuItem::new(t("msg.pin")).on_click(move |_, _, _| {
            s3.pin(&m3, true);
        }));
    }
    if s.capability(|c| c.message_reminders) {
        menu = menu.separator().label(t("msg.remind"));
        for (key, mins) in [("msg.remind.20m", 20i64), ("msg.remind.1h", 60), ("msg.remind.3h", 180), ("msg.remind.tomorrow", -1)] {
            let (s4, m4) = (s.clone(), m.clone());
            menu = menu.item(PopupMenuItem::new(t(key)).on_click(move |_, window, cx| {
                let at = remind_at(mins);
                s4.remind(&m4, at);
                window.push_notification(t("msg.remind.set"), cx);
            }));
        }
    }
    menu = menu.separator();
    if mine && !deleted {
        let (nav, m5, in_thread) = (ctx.nav.clone(), m.clone(), ctx.in_thread);
        menu = menu.item(PopupMenuItem::new(t("msg.edit")).on_click(move |_, _, cx| {
            let m5 = m5.clone();
            nav.update(cx, |_, cx| cx.emit(NavEvent::Edit { message: Box::new(m5), in_thread }));
        }));
    }
    if m.edited_at.is_some() {
        let (s6, id6) = (s.clone(), m.id.clone());
        menu = menu.item(PopupMenuItem::new(t("msg.history")).on_click(move |_, window, cx| {
            crate::dialogs::edit_history(s6.clone(), id6.clone(), window, cx);
        }));
    }
    if !deleted && s.capability(|c| c.personal_message_deletion) {
        let (s7, m7) = (s.clone(), m.clone());
        menu = menu.item(PopupMenuItem::new(t("msg.hide")).on_click(move |_, _, _| {
            s7.hide(&m7);
        }));
    }
    if !deleted && (mine || ctx.moderator) {
        let (s8, m8) = (s.clone(), m.clone());
        menu = menu.item(PopupMenuItem::new(t("msg.delete")).on_click(move |_, window, cx| {
            let (s9, m9) = (s8.clone(), m8.clone());
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (s10, m10) = (s9.clone(), m9.clone());
                alert
                    .title(t("msg.deleteConfirm"))
                    .description(t("msg.deleteConfirmBody"))
                    .confirm()
                    .ok_text(t("delete"))
                    .cancel_text(t("cancel"))
                    .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                    .on_ok(move |_, _, _| {
                        s10.delete(&m10);
                        true
                    })
            });
        }));
    }
    menu
}

fn remind_at(mins: i64) -> Timestamp {
    use chrono::{Duration, Local, TimeZone};
    let now = Local::now();
    let at = if mins < 0 {
        let tomorrow = now.date_naive().succ_opt().unwrap_or(now.date_naive());
        Local.from_local_datetime(&tomorrow.and_hms_opt(9, 0, 0).unwrap()).single().unwrap_or(now + Duration::hours(12))
    } else {
        now + Duration::minutes(mins)
    };
    Timestamp::from_unix_ms(at.timestamp_millis())
}

fn reactions(m: &Message, ctx: &MsgCtx, cx: &mut App) -> AnyElement {
    let k = c(cx);
    let mut row = h_flex().flex_wrap().gap_1().pt_1();
    for r in &m.reactions {
        let mine = r.user_ids.contains(&ctx.me);
        let (s, msg, emoji) = (ctx.session.clone(), m.clone(), r.emoji.clone());
        let names: Vec<String> = {
            let store = ctx.session.store().read_recursive();
            r.user_ids.iter().filter_map(|u| store.user(u).map(|x| x.display_name.clone())).collect()
        };
        let tip = format!("{} · {}", r.emoji, names.join(", "));
        let (bg, border) = if mine { (k.accent.opacity(0.12), k.accent.opacity(0.7)) } else { (k.surface_200, k.surface_200) };
        row = row.child(
            h_flex()
                .id(SharedString::from(format!("rx-{}-{}", m.id, r.emoji)))
                .h(px(26.))
                .px(px(9.))
                .gap(px(5.))
                .rounded_full()
                .border_1()
                .border_color(border)
                .bg(bg)
                .cursor_pointer()
                .child(div().text_size(px(14.)).child(r.emoji.clone()))
                .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(if mine { k.accent_text } else { k.ink_muted }).child(r.user_ids.len().to_string()))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
                .on_click(move |_, window, cx| {
                    react_checked(&s, &msg, &emoji, !mine, window, cx);
                }),
        );
    }
    let (m2, c2) = (m.clone(), ctx.clone());
    row = row.child(
        div()
            .id(SharedString::from(format!("rx-add-{}", m.id)))
            .h(px(26.))
            .w(px(34.))
            .rounded_full()
            .border_1()
            .border_color(k.border_subtle)
            .invisible()
            .group_hover(SharedString::from(format!("msg-{}", m.id)), |s| s.visible())
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(ico("smile-plus", 14., k.ink_muted))
            .on_click(move |_, window, cx| open_reaction_picker(m2.clone(), c2.clone(), window, cx)),
    );
    row.into_any_element()
}

/// Adds or takes back a reaction, holding the per-person limit (`REACTIONS_PER_PERSON_MAX`).
pub fn react_checked(s: &Session, m: &Message, emoji: &str, add: bool, window: &mut Window, cx: &mut App) {
    if add {
        let me = &s.user_id;
        let mine = m.reactions.iter().filter(|r| r.user_ids.iter().any(|u| u == me)).count();
        let max = purlay_core::contract::constants::REACTIONS_PER_PERSON_MAX as usize;
        if mine >= max && !m.reactions.iter().any(|r| r.emoji == emoji && r.user_ids.iter().any(|u| u == me)) {
            window.push_notification(tf("msg.reactionLimit", &[&max]), cx);
            return;
        }
    }
    s.react(m, emoji, add);
}

pub fn open_reaction_picker(m: Message, ctx: MsgCtx, window: &mut Window, cx: &mut App) {
    let s = ctx.session.clone();
    let picker = cx.new(|cx| crate::emoji_picker::EmojiPicker::new(s.prefs.clone(), window, cx));
    let msg = m.clone();
    let session = s.clone();
    picker.update(cx, |p, _| {
        p.on_pick(move |emoji, window, cx| {
            react_checked(&session, &msg, emoji, true, window, cx);
            window.close_dialog(cx);
        })
    });
    let focus = picker.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| dialog.title(t("msg.react")).width(px(380.)).child(picker.clone()));
    window.focus(&focus, cx);
}

fn thread_line(m: &Message, th: &ThreadSummary, ctx: &MsgCtx, cx: &mut App) -> AnyElement {
    let k = c(cx);
    let open = ctx.open_thread.as_ref() == Some(&m.id);
    let resolved = th.resolved_at.is_some();
    let question = th.knowledge.as_ref().is_some_and(|q| q.is_question);
    let replies = if th.reply_count == 1 { t("thread.reply1").to_string() } else { tf("thread.replies", &[&th.reply_count]) };
    let last = th.last_reply_at.as_ref().and_then(|t| t.unix_ms()).map(|ms| tf("thread.last", &[&ago(ms)]));
    let (nav, root, ch) = (ctx.nav.clone(), m.id.clone(), m.channel_id.clone());
    let state_text = if open {
        t("thread.open").to_string()
    } else if resolved {
        t("thread.resolved").to_string()
    } else {
        t("thread.openState").to_string()
    };
    let hover = k.surface_200;
    h_flex()
        .id(SharedString::from(format!("tl-{}", m.id)))
        .mt_1()
        .px_2()
        .py_1()
        .gap_2()
        .rounded(px(purlay_tokens::radius::MD))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(ico(if resolved { "circle-check-big" } else { "circle-dot" }, 13., if resolved { k.success_text } else { k.accent_text }))
        .when(question, |d| d.child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.warning_text).child(t("thread.question"))))
        .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(if resolved { k.success_text } else { k.accent_text }).child(state_text))
        .when(th.reply_count > 0, |d| d.child(caption("·", cx)).child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.accent_text).child(replies)))
        .when_some(last, |d, l| d.child(caption("·", cx)).child(caption(l, cx)))
        .on_click(move |_, _, cx| nav.update(cx, |n, cx| n.open_thread(&root, &ch, cx)))
        .into_any_element()
}

fn attachments(list: &[Attachment], ctx: &MsgCtx, cx: &mut App) -> AnyElement {
    let k = c(cx);
    let imgs = images(cx);
    let mut col = h_flex().flex_wrap().gap_2().pt_1().items_start();
    for a in list {
        if a.kind == AttachmentKind::Image {
            let pic = imgs.update(cx, |i, cx| i.get(&a.id, true, cx));
            let (w, h) = fit(a.width, a.height, purlay_tokens::size::ATTACHMENT);
            let (s, aid, name) = (ctx.session.clone(), a.id.clone(), a.file_name.clone());
            col = col.child(
                div()
                    .id(SharedString::from(format!("att-{}", a.id)))
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(purlay_tokens::radius::LG))
                    .overflow_hidden()
                    .bg(k.surface_200)
                    .cursor_pointer()
                    .when_some(pic, |d, p| d.child(img(p).size_full().object_fit(ObjectFit::Cover)))
                    .on_click(move |_, window, cx| crate::dialogs::image_viewer(s.clone(), aid.clone(), name.clone(), window, cx)),
            );
        } else {
            let (s, a2) = (ctx.session.clone(), a.clone());
            let hover = k.surface_050;
            col = col.child(
                h_flex()
                    .id(SharedString::from(format!("att-{}", a.id)))
                    .w(px(280.))
                    .p_2()
                    .gap_2()
                    .rounded(px(purlay_tokens::radius::LG))
                    .border_1()
                    .border_color(k.border_subtle)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(div().size(px(36.)).rounded(px(8.)).bg(k.accent_subtle).flex().items_center().justify_center().child(ico("file-text", 18., k.accent_text)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.ink).overflow_hidden().text_ellipsis().whitespace_nowrap().child(a.file_name.clone()))
                            .child(caption(human_size(a.size_bytes), cx)),
                    )
                    .child(ico("download", 16., k.ink_muted))
                    .on_click(move |_, window, cx| crate::dialogs::save_attachment(s.clone(), a2.clone(), window, cx)),
            );
        }
    }
    col.into_any_element()
}

fn fit(w: Option<i64>, h: Option<i64>, max: f32) -> (f32, f32) {
    match (w, h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let (w, h) = (w as f32, h as f32);
            let s = (max / w).min(max / h).min(1.0);
            ((w * s).max(48.), (h * s).max(48.))
        }
        _ => (max, max * 0.66),
    }
}

pub fn human_size(b: i64) -> String {
    let b = b as f64;
    if b < 1024.0 {
        format!("{b} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    }
}

/// Link cards for the first two public `https` links outside code (never the server's own).
fn link_previews(m: &Message, ctx: &MsgCtx, cx: &mut App) -> Vec<AnyElement> {
    let k = c(cx);
    let Some(root) = purlay_core::markdown::parse_markdown(&m.content) else { return vec![] };
    let mut urls = Vec::new();
    collect_links(&root.children, &mut urls);
    let own = url::Url::parse(&ctx.session.info.app_url).ok().and_then(|u| u.host_str().map(String::from));
    let urls: Vec<String> = urls
        .into_iter()
        .filter(|u| u.starts_with("https://"))
        .filter(|u| url::Url::parse(u).ok().and_then(|x| x.host_str().map(String::from)) != own)
        .take(2)
        .collect();
    let mut out = vec![];
    for u in urls {
        ctx.session.link_preview(m, &u);
        let preview = ctx.session.store().read_recursive().link_previews.get(&(m.id.clone(), u.clone())).cloned().flatten();
        if let Some(p) = preview {
            let u2 = u.clone();
            out.push(
                v_flex()
                    .id(SharedString::from(format!("lp-{}-{}", m.id, u)))
                    .mt_1()
                    .max_w(px(480.))
                    .pl_3()
                    .py_1()
                    .border_l_2()
                    .border_color(k.border_strong)
                    .cursor_pointer()
                    .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.accent_text).child(p.title.clone()))
                    .when_some(p.description.clone(), |d, desc| d.child(caption(desc, cx)))
                    .on_click(move |_, _, cx| cx.open_url(&u2))
                    .into_any_element(),
            );
        }
    }
    out
}

fn collect_links(blocks: &[purlay_core::markdown::Block], out: &mut Vec<String>) {
    use purlay_core::markdown::{Block, Inline};
    fn inl(i: &[Inline], out: &mut Vec<String>) {
        for x in i {
            match x {
                Inline::Link { url, .. } => out.push(url.clone()),
                Inline::Emphasis { children } | Inline::Strong { children } | Inline::Delete { children } => inl(children, out),
                _ => {}
            }
        }
    }
    for b in blocks {
        match b {
            Block::Paragraph { children } | Block::Heading { children, .. } => inl(children, out),
            Block::Blockquote { children } => collect_links(children, out),
            Block::List { children, .. } => {
                for it in children {
                    collect_links(&it.children, out);
                }
            }
            _ => {}
        }
    }
}
