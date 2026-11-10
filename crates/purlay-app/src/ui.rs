//! Small building blocks every view uses: text styles, avatars, states, times.

use crate::assets::icon;
use crate::i18n::{t, tf};
use crate::theme::{avatar_bg, c, Tokens};
use chrono::{Datelike, Local, TimeZone};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_tokens::typography as ty;

pub fn text_style(div: Div, style: purlay_tokens::TypeStyle, cx: &App) -> Div {
    let fam = if style.mono { Tokens::get(cx).mono } else { Tokens::get(cx).sans };
    div.font_family(fam)
        .text_size(px(style.size))
        .line_height(px((style.size * style.line_height).round()))
        .font_weight(FontWeight(style.weight))
}

pub fn title(s: impl Into<SharedString>, cx: &App) -> Div {
    text_style(div(), ty::TITLE, cx).text_color(c(cx).ink).child(s.into())
}

pub fn heading(s: impl Into<SharedString>, cx: &App) -> Div {
    text_style(div(), ty::HEADING, cx).text_color(c(cx).ink).child(s.into())
}

pub fn body(s: impl Into<SharedString>, cx: &App) -> Div {
    text_style(div(), ty::BODY, cx).text_color(c(cx).ink).child(s.into())
}

pub fn label(s: impl Into<SharedString>, cx: &App) -> Div {
    text_style(div(), ty::LABEL, cx).text_color(c(cx).ink_muted).child(s.into())
}

pub fn caption(s: impl Into<SharedString>, cx: &App) -> Div {
    text_style(div(), ty::CAPTION, cx).text_color(c(cx).ink_subtle).child(s.into())
}

pub fn muted(s: impl Into<SharedString>, cx: &App) -> Div {
    text_style(div(), ty::BODY, cx).text_color(c(cx).ink_muted).child(s.into())
}

/// A Lucide icon from Purlay's set at the given size and color.
pub fn ico(name: &'static str, size: f32, color: Hsla) -> impl IntoElement {
    icon(name).size(px(size)).text_color(color)
}

/// Initials on the avatar color, or the picture when the server gave one and it is loaded.
pub fn avatar(id: impl Into<SharedString>, name: &str, size: f32, cx: &App) -> Div {
    let key: SharedString = id.into();
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(|c| c.to_uppercase())
        .collect();
    let initials = if initials.is_empty() { "?".to_string() } else { initials };
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(avatar_bg(cx, &key))
        .flex()
        .items_center()
        .justify_center()
        .text_color(c(cx).ink)
        .text_size(px((size * 0.38).round()))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials)
}

/// The presence dot drawn on an avatar's corner.
pub fn presence_dot(online: bool, away: bool, cx: &App) -> Div {
    let k = c(cx);
    div()
        .size(px(purlay_tokens::size::DOT))
        .rounded_full()
        .border_2()
        .border_color(crate::theme::sidebar_tones(cx).0)
        .when(online && !away, |d| d.bg(k.success))
        .when(online && away, |d| d.bg(k.warning))
        .when(!online, |d| d.bg(k.border_strong))
}

pub fn count_badge(n: i64, cx: &App) -> Div {
    let k = c(cx);
    div()
        .flex_none()
        .min_w(px(purlay_tokens::size::BADGE))
        .h(px(purlay_tokens::size::BADGE))
        .px(px(5.))
        .rounded_full()
        .bg(k.accent)
        .text_color(k.on_accent)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(if n > 99 { "99+".to_string() } else { n.to_string() })
}

/// The view's loading state: skeleton rows, never a spinner (UX §8).
pub fn skeleton_rows(n: usize, cx: &App) -> Div {
    let k = c(cx);
    v_flex().gap_4().p_4().children((0..n).map(move |i| {
        h_flex()
            .gap_3()
            .child(div().size(px(28.)).rounded(px(8.)).bg(k.surface_200))
            .child(
                v_flex()
                    .gap_2()
                    .flex_1()
                    .child(div().h(px(10.)).w(px(120. + (i % 3) as f32 * 30.)).rounded(px(4.)).bg(k.surface_200))
                    .child(div().h(px(10.)).w(relative(0.6 + (i % 4) as f32 * 0.08)).rounded(px(4.)).bg(k.surface_200)),
            )
    }))
}

pub fn empty_state(text: impl Into<SharedString>, cx: &App) -> Div {
    v_flex().flex_1().items_center().justify_center().p_8().child(muted(text, cx).text_center().max_w(px(420.)))
}

pub fn error_state(id: &'static str, text: impl Into<SharedString>, on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, cx: &App) -> Div {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(muted(text, cx).text_center())
        .child(gpui_kit::component::button::Button::new(id).small().label(t("retry")).on_click(on_retry))
}

pub fn v_flex() -> Div {
    div().flex().flex_col()
}

pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// A rounded hover row used in lists and the sidebar.
pub fn row_hover(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let k = c(cx);
    let hover = k.surface_050;
    div()
        .id(id)
        .rounded(px(purlay_tokens::radius::LG))
        .cursor_pointer()
        .when(selected, |d| d.bg(k.accent_subtle))
        .when(!selected, |d| d.hover(move |s| s.bg(hover)))
}

pub fn icon_button(id: impl Into<ElementId>, name: &'static str, tip: &'static str, cx: &App) -> Stateful<Div> {
    let k = c(cx);
    let hover = k.surface_200;
    div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .rounded(px(purlay_tokens::radius::MD))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(t(tip)).build(window, cx))
        .child(ico(name, 16., k.ink_muted))
}

pub fn divider(cx: &App) -> Div {
    div().h(px(1.)).w_full().bg(c(cx).border_subtle)
}

pub fn kbd(s: &str, cx: &App) -> Div {
    let k = c(cx);
    div()
        .px(px(5.))
        .h(px(18.))
        .rounded(px(4.))
        .border_1()
        .border_color(k.border_subtle)
        .bg(k.surface_050)
        .text_size(px(11.))
        .text_color(k.ink_muted)
        .flex()
        .items_center()
        .child(s.to_string())
}

// --- time ------------------------------------------------------------------------

pub fn local(ms: i64) -> chrono::DateTime<Local> {
    Local.timestamp_millis_opt(ms).single().unwrap_or_else(Local::now)
}

pub fn clock_time(ms: i64) -> String {
    local(ms).format("%H:%M").to_string()
}

pub fn day_label(ms: i64) -> String {
    let d = local(ms).date_naive();
    let today = Local::now().date_naive();
    if d == today {
        return t("msg.today").into();
    }
    if Some(d) == today.pred_opt() {
        return t("msg.yesterday").into();
    }
    let months_en = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let months_tr = ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];
    let days_en = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let days_tr = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];
    let m = d.month0() as usize;
    let wd = d.weekday().num_days_from_monday() as usize;
    let same_year = d.year() == today.year();
    if crate::i18n::is_turkish() {
        let y = if same_year { String::new() } else { format!(" {}", d.year()) };
        format!("{} {}{}, {}", d.day(), months_tr[m], y, days_tr[wd])
    } else {
        let y = if same_year { String::new() } else { format!(", {}", d.year()) };
        format!("{}, {} {}{}", days_en[wd], months_en[m], d.day(), y)
    }
}

pub fn same_day(a: i64, b: i64) -> bool {
    local(a).date_naive() == local(b).date_naive()
}

/// "now", "5m", "3h", "2d" — for thread lines and the inbox.
pub fn ago(ms: i64) -> String {
    let now = Local::now().timestamp_millis();
    let s = ((now - ms) / 1000).max(0);
    if s < 60 {
        t("time.now").into()
    } else if s < 3600 {
        tf("time.minutes", &[&(s / 60)])
    } else if s < 86400 {
        tf("time.hours", &[&(s / 3600)])
    } else {
        tf("time.days", &[&(s / 86400)])
    }
}

pub fn short_datetime(ms: i64) -> String {
    format!("{} {}", day_label(ms), clock_time(ms))
}

pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}
