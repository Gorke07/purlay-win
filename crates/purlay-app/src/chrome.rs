//! The window's own frame. Purlay draws its title bar into the page, as Windows 11 apps
//! do: the sidebar reaches the top edge, and the toolbar shares a row with the caption
//! buttons. Windows itself answers the caption buttons (so snap layouts work) and the
//! drag areas (so a double click maximizes and Aero Snap works).

use crate::i18n::t;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::prefs::{Appearance, Prefs};

/// Height of the top row: the sidebar's workspace header and the content toolbar.
pub const TOP_BAR: f32 = 56.;

/// Minimize, maximize or restore, and close, at the window's top right corner.
pub fn caption_buttons(window: &Window, cx: &App) -> impl IntoElement {
    let k = c(cx);
    let maximized = window.is_maximized();
    let hover = k.ink.opacity(0.07);
    let pressed = k.ink.opacity(0.11);
    let button = |id: &'static str, icon_name: &'static str, area: WindowControlArea| {
        div()
            .id(id)
            .w(px(46.))
            .h(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(k.ink_muted)
            // Above the drag area: the hit test stops here and Windows sees the button.
            .occlude()
            .window_control_area(area)
            .hover(move |s| s.bg(hover).text_color(k.ink))
            .active(move |s| s.bg(pressed))
            .child(crate::assets::icon(icon_name).size(px(16.)))
    };
    let close_bg = gpui_kit::rgb(0xC42B1C);
    h_flex()
        .flex_none()
        .items_start()
        .h_full()
        .child(button("cap-min", "win-min", WindowControlArea::Min))
        .child(button("cap-max", if maximized { "win-restore" } else { "win-max" }, WindowControlArea::Max))
        .child(
            div()
                .id("cap-close")
                .w(px(46.))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(k.ink_muted)
                .occlude()
                .window_control_area(WindowControlArea::Close)
                .when(!maximized, |d| d.rounded_tr(px(8.)))
                .hover(move |s| s.bg(close_bg).text_color(gpui_kit::white()))
                .active(move |s| s.bg(close_bg.opacity(0.85)).text_color(gpui_kit::white()))
                .child(crate::assets::icon("win-close").size(px(16.))),
        )
}

/// A thin draggable strip with the caption buttons, for screens without a toolbar
/// (connecting, signing in).
pub fn bare_title_bar(window: &Window, cx: &App) -> impl IntoElement {
    h_flex()
        .id("bare-title")
        .flex_none()
        .w_full()
        .h(px(36.))
        .items_start()
        .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag))
        .child(caption_buttons(window, cx))
}

/// A rounded group of toolbar buttons, raised a little from the bar.
pub fn tool_group(cx: &App) -> Div {
    let k = c(cx);
    let dark = crate::theme::Tokens::get(cx).dark;
    h_flex()
        .flex_none()
        .h(px(36.))
        .px(px(3.))
        .gap(px(1.))
        .rounded_full()
        .bg(if dark { k.surface_200 } else { k.surface_100 })
        .border_1()
        .border_color(k.border_subtle)
        .shadow_xs()
        .occlude()
}

/// One button of a [`tool_group`]: an icon in a circle that lights on hover, tinted
/// while what it opens is open.
pub fn tool_button(id: impl Into<ElementId>, icon_name: &'static str, tip: impl Into<SharedString>, active: bool, cx: &App) -> Stateful<Div> {
    tool_button_tinted(id, icon_name, tip, active, None, cx)
}

/// A [`tool_button`] whose icon takes the given color (a starred star).
pub fn tool_button_tinted(id: impl Into<ElementId>, icon_name: &'static str, tip: impl Into<SharedString>, active: bool, tint: Option<Hsla>, cx: &App) -> Stateful<Div> {
    let tip: SharedString = tip.into();
    let k = c(cx);
    let hover = k.ink.opacity(0.07);
    div()
        .id(id)
        .flex_none()
        .size(px(30.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(active, |d| d.bg(k.accent_subtle))
        .hover(move |s| s.bg(hover))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .child(ico(icon_name, 18., tint.unwrap_or(if active { k.accent_text } else { k.ink })))
}

/// The theme button: cycles light → dark → the system's, and says which is on.
pub fn theme_button(prefs: Prefs, cx: &App) -> impl IntoElement {
    let current = prefs.get().appearance;
    let (icon_name, tip) = match current {
        Appearance::Light => ("sun", "settings.theme.light"),
        Appearance::Dark => ("moon", "settings.theme.dark"),
        Appearance::System => ("sun-moon", "settings.theme.system"),
    };
    tool_group(cx).child(tool_button("theme", icon_name, t(tip), false, cx).on_click(move |_, window, cx| {
        let next = match prefs.get().appearance {
            Appearance::System => Appearance::Light,
            Appearance::Light => Appearance::Dark,
            Appearance::Dark => Appearance::System,
        };
        prefs.update(|d| d.appearance = next);
        let dark = match next {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
        };
        crate::theme::apply(dark, cx);
        cx.refresh_windows();
    }))
}

