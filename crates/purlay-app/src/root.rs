//! The window's content: a title bar, then whichever phase the app is in.

use crate::i18n::{t, tf};
use crate::main_view::MainView;
use crate::state::{app, AppEvent, AppModel, Phase};
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub struct RootView {
    app: Entity<AppModel>,
    address: Entity<InputState>,
    paste: Entity<InputState>,
    main: Option<Entity<MainView>>,
    _subs: Vec<Subscription>,
}

impl RootView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = app(cx);
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(t("connect.placeholder")));
        let paste = cx.new(|cx| InputState::new(window, cx).placeholder("purlay://auth?code=…"));
        let mut subs = vec![cx.observe(&app, |this, app, cx| {
            let ready = app.read(cx).session().is_some();
            if !ready {
                this.main = None;
            }
            cx.notify();
        })];
        subs.push(cx.subscribe_in(&address, window, |this, _, e, _, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.submit_address(cx);
            }
        }));
        subs.push(cx.subscribe_in(&paste, window, |this, state, e, window, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                let v = state.read(cx).value().to_string();
                state.update(cx, |s, cx| s.set_value("", window, cx));
                this.app.update(cx, |m, cx| m.handle_url(v, cx));
            }
        }));
        subs.push(cx.subscribe_in(&app, window, |_this, _, e: &AppEvent, window, cx| match e {
            AppEvent::Notice(s) => window.push_notification(s.clone(), cx),
            AppEvent::SessionEnded => window.push_notification(t("session.ended"), cx),
            AppEvent::OpenLink(_) | AppEvent::OpenSettings => {}
        }));
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            crate::tray::refresh_badge();
            this.app.update(cx, |m, cx| m.set_window_active(active, cx));
        }));
        // Prefill the last server.
        let last = app.read(cx).prefs.get().servers.last().map(|s| s.address.clone());
        if let Some(l) = last {
            let l = l.trim_start_matches("https://").to_string();
            address.update(cx, |s, cx| s.set_value(l, window, cx));
        }
        // The address is the only thing to do on this screen: the caret starts there.
        address.update(cx, |s, cx| s.focus(window, cx));
        RootView { app, address, paste, main: None, _subs: subs }
    }

    fn submit_address(&mut self, cx: &mut Context<Self>) {
        let v = self.address.read(cx).value().to_string();
        self.app.update(cx, |m, cx| m.connect(v, cx));
    }

    fn render_connect(&mut self, error: Option<String>, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        let recent: Vec<String> = self.app.read(cx).prefs.get().servers.iter().map(|s| s.address.clone()).collect();
        centered_card(cx)
            .child(img("purlay/logo.png").size(px(56.)).rounded(px(14.)))
            .child(title(t("connect.title"), cx).text_size(px(22.)))
            .child(muted(t("connect.subtitle"), cx))
            .child(div().w_full().child(Input::new(&self.address).large().disabled(busy)))
            .when_some(error, |d, e| d.child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.danger_text).child(e)))
            .child(
                Button::new("connect")
                    .primary()
                    .large()
                    .w_full()
                    .loading(busy)
                    .label(if busy { t("connect.checking") } else { t("connect.continue") })
                    .on_click(cx.listener(|this, _, _, cx| this.submit_address(cx))),
            )
            .when(!recent.is_empty(), |d| {
                d.child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .pt_2()
                        .child(caption(t("connect.recent"), cx))
                        .children(recent.into_iter().enumerate().map(|(i, a)| {
                            let a2 = a.clone();
                            row_hover(("recent", i), false, cx)
                                .px_3()
                                .py_2()
                                .child(label(a.trim_start_matches("https://").to_string(), cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let v = a2.clone();
                                    this.app.update(cx, |m, cx| m.connect(v, cx));
                                }))
                        })),
                )
            })
            .into_any_element()
    }

    fn render_sign_in(&mut self, host: String, waiting: bool, error: Option<String>, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let k = c(cx);
        centered_card(cx)
            .child(img("purlay/logo.png").size(px(56.)).rounded(px(14.)))
            .child(title(tf("signin.title", &[&host]), cx).text_size(px(20.)).text_center())
            .child(muted(t("signin.body"), cx).text_center())
            .when_some(error, |d, e| d.child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.danger_text).text_center().child(e)))
            .child(
                Button::new("open-browser")
                    .primary()
                    .large()
                    .w_full()
                    .loading(busy)
                    .label(if waiting { t("signin.again") } else { t("signin.open") })
                    .on_click(cx.listener(|this, _, _, cx| this.app.update(cx, |m, cx| m.open_sign_in(cx)))),
            )
            .when(waiting && !busy, |d| {
                d.child(caption(t("signin.waiting"), cx)).child(
                    v_flex().w_full().gap_1().pt_2().child(caption(t("signin.paste"), cx)).child(Input::new(&self.paste).small()),
                )
            })
            .child(
                Button::new("other-server")
                    .ghost()
                    .small()
                    .label(t("signin.other"))
                    .on_click(cx.listener(|this, _, _, cx| this.app.update(cx, |m, cx| m.back_to_connect(cx)))),
            )
            .into_any_element()
    }
}

fn centered_card(cx: &App) -> Div {
    let k = c(cx);
    v_flex()
        .w(px(400.))
        .items_center()
        .gap_4()
        .p(px(32.))
        .rounded(px(purlay_tokens::radius::RADIUS_2XL))
        .bg(k.surface_100)
        .border_1()
        .border_color(k.border_subtle)
        .shadow_lg()
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let ready = matches!(self.app.read(cx).phase, Phase::Ready(_));
        if ready && self.main.is_none() {
            let app = self.app.clone();
            self.main = Some(cx.new(|cx| MainView::new(app, window, cx)));
        }
        let content: AnyElement = match &self.app.read(cx).phase {
            Phase::Connect { error, busy } => {
                let (e, b) = (error.clone(), *busy);
                self.render_connect(e, b, cx)
            }
            Phase::SignIn { origin, handoff, error, busy, .. } => {
                let host = origin.host_str().unwrap_or_default().to_string();
                let (w, e, b) = (handoff.is_some(), error.clone(), *busy);
                self.render_sign_in(host, w, e, b, cx)
            }
            Phase::Opening { address } => v_flex()
                .items_center()
                .gap_3()
                .child(img("purlay/logo.png").size(px(48.)).rounded(px(12.)))
                .child(muted(address.trim_start_matches("https://").to_string(), cx))
                .child(caption(t("loading"), cx))
                .into_any_element(),
            Phase::Ready(_) => self.main.clone().map(|m| m.into_any_element()).unwrap_or_else(|| div().into_any_element()),
        };
        // Signed in, the main view draws the frame (sidebar to the top, toolbar beside the
        // caption buttons); before that, a bare draggable strip is enough.
        v_flex()
            .size_full()
            .bg(if ready { k.surface_100 } else { k.surface_000 })
            .text_color(k.ink)
            .font_family(crate::theme::Tokens::get(cx).sans)
            .when(!ready, |d| d.child(crate::chrome::bare_title_bar(window, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(!ready, |d| d.flex().items_center().justify_center())
                    .child(content),
            )
    }
}
