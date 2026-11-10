//! Settings (UX §12): device settings here; account, workspace and server writes open
//! the web UI, where the person may need to sign in separately.

use purlay_core::prefs::NotificationStyle;
use crate::actions::SHORTCUTS;
use crate::i18n::{t, tf};
use crate::nav::Nav;
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::prefs::{Appearance, Density};
use purlay_core::store::Key;

#[derive(Clone, Copy, PartialEq)]
enum Section {
    General,
    Appearance,
    Notifications,
    Account,
    Workspace,
    Server,
    Shortcuts,
}

pub struct SettingsPage {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    section: Section,
    _subs: Vec<Subscription>,
}

impl SettingsPage {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&app, |_, _, cx| cx.notify())];
        SettingsPage { app, nav, section: Section::General, _subs: subs }
    }
}

fn row(title_text: impl Into<SharedString>, desc: Option<String>, control: AnyElement, cx: &App) -> Div {
    let k = c(cx);
    h_flex()
        .py_3()
        .gap_4()
        .border_b_1()
        .border_color(k.border_subtle)
        .child(v_flex().flex_1().gap(px(2.)).child(body(title_text, cx)).when_some(desc, |d, s| d.child(caption(s, cx))))
        .child(control)
}

fn choice<T: Copy + PartialEq + 'static>(id: &'static str, items: &[(T, &'static str)], current: T, on: impl Fn(T, &mut Window, &mut App) + 'static, cx: &App) -> AnyElement {
    let k = c(cx);
    let on = std::rc::Rc::new(on);
    let mut seg = h_flex().gap_1().p(px(2.)).rounded(px(purlay_tokens::radius::LG)).bg(k.surface_200);
    for (i, (v, key)) in items.iter().enumerate() {
        let sel = *v == current;
        let (v, on) = (*v, on.clone());
        seg = seg.child(
            div()
                .id((id, i))
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(purlay_tokens::radius::MD))
                .cursor_pointer()
                .when(sel, |d| d.bg(k.surface_100).shadow_sm())
                .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(if sel { k.ink } else { k.ink_muted }).child(t(key)))
                .on_click(move |_, w, cx| on(v, w, cx)),
        );
    }
    seg.into_any_element()
}

fn web_link(id: &'static str, key: &'static str, url: String) -> AnyElement {
    Button::new(id).small().outline().label(t(key)).icon(crate::assets::icon("external-link")).on_click(move |_, _, cx| cx.open_url(&url)).into_any_element()
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let Some(s) = self.app.read(cx).session().cloned() else { return div().into_any_element() };
        let prefs = s.prefs.get();
        let ws = self.nav.read(cx).workspace.clone();
        let keys = if self.section == Section::Account { vec![Key::Sessions] } else { vec![] };
        self.nav.update(cx, |n, cx| n.set_keys("settings", keys, cx));
        let (me, membership) = {
            let store = s.store().read_recursive();
            (store.me.clone(), ws.as_ref().and_then(|w| store.membership(w).cloned()))
        };
        let role = membership.as_ref().map(|m| m.role.clone());
        let is_admin = matches!(role, Some(UserRole::Owner) | Some(UserRole::Admin));
        let is_owner = matches!(role, Some(UserRole::Owner));
        let server_admin = me.as_ref().is_some_and(|m| m.is_server_admin);
        let slug = membership.as_ref().map(|m| m.workspace.slug.clone()).unwrap_or_default();

        let mut sections = vec![
            (Section::General, "settings.general"),
            (Section::Appearance, "settings.appearance"),
            (Section::Notifications, "settings.notifications"),
            (Section::Account, "settings.account"),
            (Section::Workspace, "settings.workspace"),
        ];
        if server_admin {
            sections.push((Section::Server, "settings.server"));
        }
        sections.push((Section::Shortcuts, "settings.shortcuts"));
        let nav_col = v_flex().w(px(200.)).flex_none().p_2().gap(px(2.)).border_r_1().border_color(k.border_subtle).children(sections.into_iter().enumerate().map(|(i, (sec, key))| {
            row_hover(("set-sec", i), self.section == sec, cx).px_3().py_2().child(body(t(key), cx)).on_click(cx.listener(move |this, _, _, cx| {
                this.section = sec;
                cx.notify();
            }))
        }));

        let p1 = s.prefs.clone();
        let content: AnyElement = match self.section {
            Section::General => {
                let p2 = p1.clone();
                let lang = match prefs.language.as_str() {
                    "en" => 1,
                    "tr" => 2,
                    _ => 0,
                };
                v_flex()
                    .child(row(
                        t("settings.language"),
                        None,
                        choice("lang", &[(0, "settings.language.system"), (1, "English"), (2, "Türkçe")], lang, move |v, w, cx| {
                            let code = ["", "en", "tr"][v as usize];
                            p2.update(|d| d.language = code.into());
                            crate::i18n::set_language(code);
                            w.refresh();
                            cx.refresh_windows();
                        }, cx),
                        cx,
                    ))
                    .child(row(
                        t("settings.launch"),
                        None,
                        Switch::new("launch").checked(purlay_platform::launch_at_login()).on_change(move |on, _, cx| {
                            let _ = purlay_platform::set_launch_at_login(*on);
                            p1.update(|d| d.launch_at_login = *on);
                            cx.refresh_windows();
                        }).into_any_element(),
                        cx,
                    ))
                    .child(row(
                        t("settings.closeToTray"),
                        None,
                        Switch::new("close-to-tray").checked(!prefs.quit_on_close).on_change({
                            let p = s.prefs.clone();
                            move |on, _, cx| {
                                p.update(|d| d.quit_on_close = !*on);
                                cx.refresh_windows();
                            }
                        }).into_any_element(),
                        cx,
                    ))
                    .child(row(tf("settings.version", &[&env!("CARGO_PKG_VERSION"), &s.info.server.version]), None, div().into_any_element(), cx))
                    .into_any_element()
            }
            Section::Appearance => {
                let p2 = p1.clone();
                v_flex()
                    .child(row(
                        t("settings.theme"),
                        None,
                        choice("theme", &[(Appearance::System, "settings.theme.system"), (Appearance::Light, "settings.theme.light"), (Appearance::Dark, "settings.theme.dark")], prefs.appearance, move |v, w, cx| {
                            p1.update(|d| d.appearance = v);
                            let dark = match v {
                                Appearance::Dark => true,
                                Appearance::Light => false,
                                Appearance::System => matches!(w.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
                            };
                            crate::theme::apply(dark, cx);
                        }, cx),
                        cx,
                    ))
                    .child(row(
                        t("settings.density"),
                        None,
                        choice("density", &[(Density::Comfortable, "settings.density.comfortable"), (Density::Compact, "settings.density.compact")], prefs.density, move |v, _, cx| {
                            p2.update(|d| d.density = v);
                            cx.refresh_windows();
                        }, cx),
                        cx,
                    ))
                    .into_any_element()
            }
            Section::Notifications => {
                let p2 = p1.clone();
                let p3 = p1.clone();
                v_flex()
                    .child(div().py_2().child(muted(t("settings.notify.body"), cx)))
                    .child(row(t("settings.notify.enabled"), None, Switch::new("notify").checked(prefs.notifications_enabled).on_change(move |on, _, cx| {
                        p1.update(|d| d.notifications_enabled = *on);
                        cx.refresh_windows();
                    }).into_any_element(), cx))
                    .child(row(
                        t("settings.notify.style"),
                        None,
                        choice(
                            "notify-style",
                            &[(NotificationStyle::Purlay, "settings.notify.style.purlay"), (NotificationStyle::Windows, "settings.notify.style.windows")],
                            prefs.notification_style,
                            {
                                let p = p3.clone();
                                move |v, _, cx| {
                                    p.update(|d| d.notification_style = v);
                                    // A sample, so the choice can be seen at once.
                                    if v == NotificationStyle::Purlay {
                                        crate::banner::show(crate::banner::Banner::sample(), cx);
                                    } else {
                                        purlay_platform::toast("Purlay", crate::i18n::t("settings.notify.sample"), None, true);
                                    }
                                    cx.refresh_windows();
                                }
                            },
                            cx,
                        ),
                        cx,
                    ))
                    .when(prefs.notification_style == NotificationStyle::Purlay, |d| {
                        use purlay_core::prefs::BannerCorner as BC;
                        let sample = |cx: &mut App| crate::banner::show(crate::banner::Banner::sample(), cx);
                        d.child(row(
                            t("settings.notify.position"),
                            None,
                            choice(
                                "banner-corner",
                                &[(BC::BottomRight, "settings.notify.corner.br"), (BC::BottomLeft, "settings.notify.corner.bl"), (BC::TopRight, "settings.notify.corner.tr"), (BC::TopLeft, "settings.notify.corner.tl")],
                                prefs.banner_corner,
                                {
                                    let p = p3.clone();
                                    move |v, _, cx| {
                                        p.update(|d| d.banner_corner = v);
                                        sample(cx);
                                        cx.refresh_windows();
                                    }
                                },
                                cx,
                            ),
                            cx,
                        ))
                        .child(row(
                            t("settings.notify.compact"),
                            None,
                            Switch::new("banner-compact").checked(prefs.banner_compact).on_change({
                                let p = p3.clone();
                                move |on, _, cx| {
                                    p.update(|d| d.banner_compact = *on);
                                    sample(cx);
                                    cx.refresh_windows();
                                }
                            }).into_any_element(),
                            cx,
                        ))
                        .child(row(
                            t("settings.notify.hideText"),
                            Some(t("settings.notify.hideTextHint").to_string()),
                            Switch::new("banner-hide-text").checked(prefs.banner_hide_text).on_change({
                                let p = p3.clone();
                                move |on, _, cx| {
                                    p.update(|d| d.banner_hide_text = *on);
                                    cx.refresh_windows();
                                }
                            }).into_any_element(),
                            cx,
                        ))
                        .child(row(
                            t("settings.notify.dnd"),
                            Some(t("settings.notify.dndHint").to_string()),
                            Switch::new("respect-dnd").checked(!prefs.notify_during_dnd).on_change({
                                let p = p3.clone();
                                move |on, _, cx| {
                                    p.update(|d| d.notify_during_dnd = !*on);
                                    cx.refresh_windows();
                                }
                            }).into_any_element(),
                            cx,
                        ))
                    })
                    .child(row(
                        t("settings.notify.badge"),
                        None,
                        Switch::new("taskbar-badge").checked(!prefs.hide_taskbar_badge).on_change({
                            let p = p3.clone();
                            move |on, _, cx| {
                                p.update(|d| d.hide_taskbar_badge = !*on);
                                crate::tray::sync_attention(&crate::state::app(cx), cx);
                                cx.refresh_windows();
                            }
                        }).into_any_element(),
                        cx,
                    ))
                    .child(row(t("settings.notify.sound"), None, Switch::new("sound").checked(prefs.notification_sound).on_change(move |on, _, cx| {
                        p2.update(|d| d.notification_sound = *on);
                        cx.refresh_windows();
                    }).into_any_element(), cx))
                    .when(prefs.notification_sound, |d| d.child(tone_picker(p3, prefs.notification_tone, cx)))
                    .into_any_element()
            }
            Section::Account => {
                let sessions = s.store().read_recursive().cache.get::<DeviceSessionList>(&Key::Sessions).cloned();
                let app = self.app.clone();
                let app2 = self.app.clone();
                let mut col = v_flex();
                if let Some(m) = &me {
                    col = col.child(
                        h_flex()
                            .py_3()
                            .gap_3()
                            .child(avatar(m.user.username.clone(), &m.user.display_name, 40., cx))
                            .child(v_flex().flex_1().child(title(m.user.display_name.clone(), cx)).child(caption(format!("@{} · {}", m.user.username, m.user.email), cx)))
                            .child(web_link("edit-profile", "settings.profile", s.web_url(&format!("/w/{slug}/settings/profile")))),
                    );
                }
                col = col.child(div().pt_4().pb_2().child(label(t("settings.sessions"), cx)));
                if let Some(list) = sessions {
                    for (i, d) in list.data.iter().enumerate() {
                        let (s2, id) = (s.clone(), d.id.clone());
                        col = col.child(row(
                            d.name.clone().unwrap_or_else(|| d.kind.as_str().to_string()),
                            Some(format!("{}{}", short_datetime(d.last_used_at.unix_ms().unwrap_or(0)), if d.current { format!(" · {}", t("settings.thisDevice")) } else { String::new() })),
                            if d.current {
                                div().into_any_element()
                            } else {
                                Button::new(("end-session", i)).xsmall().ghost().label(t("settings.endSession")).on_click(move |_, _, _| {
                                    let api = s2.api.clone();
                                    let id = id.clone();
                                    let s3 = s2.clone();
                                    s2.rt.spawn(async move {
                                        let _ = api.end_my_session(&id).await;
                                        s3.refresh(Key::Sessions);
                                    });
                                }).into_any_element()
                            },
                            cx,
                        ));
                    }
                } else {
                    col = col.child(skeleton_rows(2, cx));
                }
                col = col.child(
                    h_flex()
                        .pt_4()
                        .gap_2()
                        .child(Button::new("sign-out").outline().label(t("settings.signOut")).on_click(move |_, _, cx| app.update(cx, |m, cx| m.sign_out(false, cx))))
                        .child(Button::new("sign-out-all").danger().label(t("settings.signOutAll")).on_click(move |_, _, cx| app2.update(cx, |m, cx| m.sign_out(true, cx)))),
                );
                col.into_any_element()
            }
            Section::Workspace => {
                let mut col = v_flex().child(div().py_2().child(caption(t("settings.inBrowser"), cx)));
                let base = format!("/w/{slug}/settings");
                if is_admin {
                    col = col
                        .child(row(t("settings.workspaceGeneral"), None, web_link("ws-general", "open", s.web_url(&format!("{base}/general"))), cx))
                        .child(row(t("settings.members"), None, web_link("ws-members", "open", s.web_url(&format!("{base}/members"))), cx))
                        .child(row(t("settings.invitations"), None, web_link("ws-inv", "open", s.web_url(&format!("{base}/invitations"))), cx))
                        .child(row(t("settings.integrations"), None, web_link("ws-int", "open", s.web_url(&format!("{base}/integrations"))), cx))
                        .child(row(t("settings.audit"), None, web_link("ws-audit", "open", s.web_url(&format!("{base}/audit"))), cx));
                }
                if is_owner {
                    col = col.child(row(t("settings.data"), None, web_link("ws-data", "open", s.web_url(&format!("{base}/data"))), cx));
                }
                let members = ws.as_ref().and_then(|w| s.store().read_recursive().cache.get::<WorkspaceMemberList>(&Key::Members(w.clone())).cloned()).unwrap_or_default();
                col = col.child(div().pt_4().pb_2().child(label(t("settings.members"), cx)));
                let mut members = members;
                members.sort_by_key(|m| m.user.display_name.to_lowercase());
                for m in members {
                    let role_key = match m.role {
                        UserRole::Owner => "settings.role.owner",
                        UserRole::Admin => "settings.role.admin",
                        _ => "settings.role.member",
                    };
                    col = col.child(
                        h_flex()
                            .py_2()
                            .gap_3()
                            .child(avatar(m.user.username.clone(), &m.user.display_name, 28., cx))
                            .child(v_flex().flex_1().child(body(m.user.display_name.clone(), cx)).child(caption(format!("@{}{}", m.user.username, m.profile.title.clone().map(|t| format!(" · {t}")).unwrap_or_default()), cx)))
                            .child(caption(t(role_key), cx)),
                    );
                }
                col.into_any_element()
            }
            Section::Server => v_flex()
                .child(div().py_2().child(caption(t("settings.inBrowser"), cx)))
                .child(row(t("settings.serverSettings"), None, web_link("server", "open", s.web_url(&format!("/w/{slug}/settings/server"))), cx))
                .into_any_element(),
            Section::Shortcuts => v_flex()
                .children(SHORTCUTS.iter().map(|(keys, key)| {
                    row(t(key), None, h_flex().gap_1().children(keys.split(' ').map(|k2| kbd(k2, cx))).into_any_element(), cx)
                }))
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .child(h_flex().px(px(24.)).pt(px(20.)).pb_3().child(div().text_size(px(24.)).line_height(px(30.)).font_weight(FontWeight::BOLD).text_color(k.ink).child(t("nav.settings"))))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(nav_col)
                    .child(div().id("settings-scroll").flex_1().overflow_y_scroll().child(div().max_w(px(720.)).px_6().py_4().child(content))),
            )
            .into_any_element()
    }
}

/// The notification sounds as a list: a row is chosen and heard with one click; the
/// speaker button replays it without choosing.
fn tone_picker(prefs: purlay_core::prefs::Prefs, current: purlay_core::prefs::NotificationTone, cx: &App) -> AnyElement {
    use purlay_core::prefs::NotificationTone as N;
    let k = c(cx);
    let items = [
        (N::Drop, "tone.drop", "tone.drop.desc"),
        (N::Chime, "tone.chime", "tone.chime.desc"),
        (N::Marimba, "tone.marimba", "tone.marimba.desc"),
        (N::Glass, "tone.glass", "tone.glass.desc"),
        (N::System, "tone.system", "tone.system.desc"),
    ];
    let hover = k.surface_050;
    let mut list = v_flex().gap_1();
    for (i, (tone, name, desc)) in items.into_iter().enumerate() {
        let sel = tone == current;
        let p = prefs.clone();
        list = list.child(
            h_flex()
                .id(("tone", i))
                .px_3()
                .py_2()
                .gap_3()
                .rounded(px(purlay_tokens::radius::XL))
                .border_1()
                .border_color(if sel { k.accent } else { k.border_subtle })
                .when(sel, |d| d.bg(k.accent.opacity(0.08)))
                .when(!sel, |d| d.hover(move |s| s.bg(hover)))
                .cursor_pointer()
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if sel { k.accent } else { k.surface_200 })
                        .child(ico(if sel { "check" } else { "volume-2" }, 16., if sel { k.on_accent } else { k.ink_muted })),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.ink).font_weight(FontWeight::SEMIBOLD).child(t(name)))
                        .child(caption(t(desc), cx)),
                )
                .child(
                    icon_button(("tone-play", i), "play", "settings.notify.toneHint", cx).on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        crate::state::play_tone(tone);
                    }),
                )
                .on_click(move |_, _, cx| {
                    p.update(|d| d.notification_tone = tone);
                    crate::state::play_tone(tone);
                    cx.refresh_windows();
                }),
        );
    }
    v_flex()
        .py_3()
        .gap_2()
        .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.ink).child(t("settings.notify.tone")))
        .child(caption(t("settings.notify.toneHint"), cx))
        .child(list)
        .into_any_element()
}
