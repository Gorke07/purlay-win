//! Purlay's own notification banners, drawn in Purlay's look instead of a Windows toast.
//!
//! Each banner is a small window of its own: always on top, never taking the focus,
//! with Windows 11's rounded corners and shadow. Up to three stack in the lower right
//! corner above the taskbar, newest at the bottom free slot. A banner leaves after a few
//! seconds (not while the pointer rests on it); a click opens the message.

use crate::i18n::t;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::Uuid;
use std::time::Duration;

const WIDTH: f32 = 380.;
const HEIGHT: f32 = 104.;
const GAP: f32 = 10.;
const MARGIN: f32 = 14.;
const SLOTS: usize = 3;
/// How long a banner stays, in steps of [`TICK`].
const LIFETIME: Duration = Duration::from_millis(6500);
const TICK: Duration = Duration::from_millis(100);

/// What a banner says.
#[derive(Clone)]
pub struct Banner {
    /// The sender's name.
    pub title: String,
    /// The sender's username, for the avatar color.
    pub avatar_key: String,
    /// Where: `#general`, or "Direct message".
    pub place: String,
    /// The message, as one plain line (the view wraps it to two).
    pub body: String,
    /// Mentions the person.
    pub mention: bool,
    /// Purlay's logo instead of a person's avatar (the sample).
    pub logo: bool,
    /// `purlay://open?…`, opened on a click.
    pub link: String,
    pub channel: Uuid,
    pub message: Uuid,
    /// The thread the message is in; a reply goes there too.
    pub thread_root: Option<Uuid>,
    /// What the reply box says it answers (Reply in #general, Reply to Grace).
    pub reply_hint: String,
}

impl Banner {
    /// A banner showing what one looks like (from Settings); a click on it does nothing.
    pub fn sample() -> Self {
        Banner {
            title: "Purlay".into(),
            avatar_key: "purlay".into(),
            place: t("settings.notify.style").into(),
            body: t("settings.notify.sample").into(),
            mention: false,
            logo: true,
            link: String::new(),
            channel: Uuid::default(),
            message: Uuid::default(),
            thread_root: None,
            reply_hint: String::new(),
        }
    }
}

/// The window behind the app's main view, to bring forward on a click.
pub struct MainWindow(pub AnyWindowHandle);
impl Global for MainWindow {}

#[derive(Default)]
struct Slots {
    used: [Option<(u64, AnyWindowHandle)>; SLOTS],
    next: u64,
}
impl Global for Slots {}

/// Shows a banner in the lowest free slot (or in place of the oldest when all are taken).
pub fn show(b: Banner, cx: &mut App) {
    if !cx.has_global::<Slots>() {
        cx.set_global(Slots::default());
    }
    let (slot, id, evict) = cx.update_global::<Slots, _>(|s, _| {
        s.next += 1;
        let id = s.next;
        if let Some(i) = s.used.iter().position(|x| x.is_none()) {
            return (i, id, None);
        }
        // All taken: the oldest (smallest id) makes way.
        let (i, _) = s.used.iter().enumerate().filter_map(|(i, x)| x.as_ref().map(|(n, _)| (i, *n))).min_by_key(|(_, n)| *n).unwrap_or((0, 0));
        (i, id, s.used[i].take().map(|(_, h)| h))
    });
    if let Some(h) = evict {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }

    let Some(display) = cx.primary_display() else { return };
    let area = display.visible_bounds();
    let origin = point(
        area.origin.x + area.size.width - px(WIDTH + MARGIN),
        area.origin.y + area.size.height - px((HEIGHT + GAP) * (slot as f32 + 1.) - GAP + MARGIN),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(WIDTH), px(HEIGHT))))),
        titlebar: Some(TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: None }),
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: Some(display.id()),
        app_id: Some("dev.purlay.windows".into()),
        ..Default::default()
    };
    let opened = cx.open_window(options, move |window, cx| {
        round_corners(window);
        cx.new(|cx| BannerView::new(b, id, window, cx))
    });
    tracing::debug!("banner {id} in slot {slot}");
    match opened {
        Ok(h) => cx.update_global::<Slots, _>(|s, _| s.used[slot] = Some((id, h.into()))),
        Err(e) => tracing::warn!("banner: {e}"),
    }
}

fn release(id: u64, cx: &mut App) {
    if cx.has_global::<Slots>() {
        cx.update_global::<Slots, _>(|s, _| {
            for x in s.used.iter_mut() {
                if x.as_ref().is_some_and(|(n, _)| *n == id) {
                    *x = None;
                }
            }
        });
    }
}

/// Windows 11 rounds the banner's corners and gives it the system shadow.
fn round_corners(window: &Window) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
        if let Ok(h) = HasWindowHandle::window_handle(window) {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                let pref = DWMWCP_ROUND;
                unsafe {
                    let _ = DwmSetWindowAttribute(HWND(w.hwnd.get() as *mut _), DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, std::mem::size_of_val(&pref) as u32);
                }
            }
        }
    }
}

struct BannerView {
    b: Banner,
    id: u64,
    hovered: bool,
    left: Duration,
    closing: bool,
    /// The reply box, while answering from the banner.
    reply: Option<Entity<InputState>>,
    sent: bool,
    _timer: Task<()>,
    _subs: Vec<Subscription>,
}

impl BannerView {
    fn new(b: Banner, id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Counts down while the pointer is away; a banner being read stays.
        let timer = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let done = this
                .update_in(cx, |me, window, cx| {
                    // Paused while read or answered; a sent reply leaves at once.
                    if !me.hovered && me.reply.is_none() || me.sent {
                        me.left = me.left.saturating_sub(TICK);
                    }
                    if me.left.is_zero() {
                        me.close(window, cx);
                        return true;
                    }
                    false
                })
                .unwrap_or_else(|e| {
                    tracing::debug!("banner timer stopped: {e}");
                    true
                });
            if done {
                break;
            }
        });
        BannerView { b, id, hovered: false, left: LIFETIME, closing: false, reply: None, sent: false, _timer: timer, _subs: vec![] }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        tracing::debug!("banner {} closes", self.id);
        release(self.id, cx);
        window.remove_window();
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let link = self.b.link.clone();
        if link.is_empty() {
            self.close(window, cx);
            return;
        }
        let app = crate::state::app(cx);
        app.update(cx, |m, cx| m.handle_url(link, cx));
        if let Some(main) = cx.try_global::<MainWindow>().map(|m| m.0) {
            let _ = main.update(cx, |_, w, cx| crate::tray::show_window(w, cx));
        }
        self.close(window, cx);
    }

    /// Opens the reply box: the banner takes the keyboard (it never does otherwise).
    fn start_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hint = self.b.reply_hint.clone();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        self._subs.push(cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.send_reply(window, cx);
            }
        }));
        window.activate_window();
        input.update(cx, |s, cx| s.focus(window, cx));
        self.reply = Some(input);
        cx.notify();
    }

    fn cancel_reply(&mut self, cx: &mut Context<Self>) {
        self.reply = None;
        self._subs.clear();
        self.left = self.left.max(Duration::from_secs(3));
        cx.notify();
    }

    /// Sends the reply where the message was (its thread, or the conversation) and
    /// counts the message as read.
    fn send_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.reply.clone() else { return };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let app = crate::state::app(cx);
        let Some(s) = app.read(cx).session().cloned() else { return };
        drop(s.send_message(&self.b.channel, self.b.thread_root.as_ref(), text, vec![]));
        if self.b.thread_root.is_none() {
            s.mark_read(&self.b.channel, &self.b.message);
        }
        input.update(cx, |st, cx| st.set_value("", window, cx));
        self.reply = None;
        self.sent = true;
        self.left = Duration::from_millis(1400);
        cx.notify();
    }

    fn mark_read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = crate::state::app(cx);
        if let Some(s) = app.read(cx).session().cloned().filter(|_| !self.b.link.is_empty()) {
            s.mark_read(&self.b.channel, &self.b.message);
        }
        self.close(window, cx);
    }
}

impl Render for BannerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let dark = crate::theme::Tokens::get(cx).dark;
        let bg = if dark { k.surface_050 } else { k.surface_100 };
        let hover_btn = k.ink.opacity(0.08);
        let b = &self.b;

        let header = h_flex()
            .gap(px(6.))
            .child(img("purlay/logo.png").size(px(14.)).rounded(px(3.)))
            .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).font_weight(FontWeight::MEDIUM).child("Purlay"))
            .child(caption("·", cx))
            .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(b.place.clone()))
            .when(b.mention, |d| {
                d.child(
                    div()
                        .px(px(6.))
                        .rounded_full()
                        .bg(k.accent.opacity(0.14))
                        .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_size(px(11.)).text_color(k.accent_text).font_weight(FontWeight::SEMIBOLD).child(t("banner.mention"))),
                )
            })
            .child(div().flex_1())
            .child(caption(t("time.now"), cx).group_hover("banner", |s| s.invisible()))
            .child(
                // Close: shown on hover, over the time.
                div()
                    .id("banner-close")
                    .absolute()
                    .right(px(8.))
                    .top(px(6.))
                    .size(px(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .invisible()
                    .group_hover("banner", |s| s.visible())
                    .hover(move |s| s.bg(hover_btn))
                    .child(ico("x", 14., k.ink_muted))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(window, cx);
                    })),
            );

        // On hover the action buttons take the second line.
        let lines = if self.hovered && !b.link.is_empty() { 1 } else { 2 };
        let face = if b.logo { img("purlay/logo.png").size(px(36.)).rounded(px(9.)).into_any_element() } else { avatar(b.avatar_key.clone(), &b.title, 36., cx).into_any_element() };
        let body: AnyElement = if self.sent {
            // Sent: a short confirmation, then the banner leaves.
            h_flex()
                .gap_2()
                .pt_2()
                .child(div().size(px(22.)).rounded_full().bg(k.success).flex().items_center().justify_center().child(ico("check", 13., k.on_status)))
                .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_color(k.ink).child(t("banner.sent")))
                .into_any_element()
        } else if let Some(input) = &self.reply {
            // Answering: the message on one line, the box under it.
            v_flex()
                .gap(px(6.))
                .child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).font_weight(FontWeight::SEMIBOLD).text_color(k.ink).whitespace_nowrap().child(format!("{}:", b.title)))
                        .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(b.body.clone())),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(Input::new(input).small()))
                        .child(
                            div()
                                .id("banner-send")
                                .flex_none()
                                .size(px(28.))
                                .rounded_full()
                                .bg(k.accent)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(move |s| s.bg(k.accent_hover))
                                .child(ico("arrow-up", 15., k.on_accent))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.send_reply(window, cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            h_flex()
                .items_start()
                .gap_3()
                .child(face)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(text_style(div(), purlay_tokens::typography::LABEL, cx).text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(k.ink).whitespace_nowrap().overflow_hidden().text_ellipsis().child(b.title.clone()))
                        .child(text_style(div(), purlay_tokens::typography::LABEL, cx).font_weight(FontWeight::NORMAL).text_color(k.ink_muted).line_clamp(lines).child(b.body.clone())),
                )
                .into_any_element()
        };

        // Reply and Mark as read, on hover (not on the sample, not while answering).
        let pill = |id: &'static str, label_key: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(10.))
                .h(px(24.))
                .rounded_full()
                .flex()
                .items_center()
                .when(primary, |d| d.bg(k.accent).hover(move |s| s.bg(k.accent_hover)))
                .when(!primary, |d| d.bg(bg).border_1().border_color(k.border_subtle).hover(move |s| s.bg(hover_btn)))
                .child(text_style(div(), purlay_tokens::typography::CAPTION, cx).font_weight(FontWeight::MEDIUM).text_color(if primary { k.on_accent } else { k.ink }).child(t(label_key)))
        };
        let actions = (!b.link.is_empty() && self.reply.is_none() && !self.sent).then(|| {
            h_flex()
                .absolute()
                .right(px(10.))
                .bottom(px(8.))
                .gap(px(6.))
                .invisible()
                .group_hover("banner", |s| s.visible())
                .child(pill("banner-read", "banner.markRead", false).on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.mark_read(window, cx);
                })))
                .child(pill("banner-reply", "banner.reply", true).on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.start_reply(window, cx);
                })))
        });
        div()
            .id("banner")
            .group("banner")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(bg)
            .border_1()
            .border_color(k.border_subtle)
            .text_color(k.ink)
            .font_family(crate::theme::Tokens::get(cx).sans)
            .cursor_pointer()
            .on_hover(cx.listener(|this, h: &bool, _, cx| {
                this.hovered = *h;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" && this.reply.is_some() {
                    this.cancel_reply(cx);
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                // While answering, a click on the banner does not open the conversation.
                if this.reply.is_none() && !this.sent {
                    this.open(window, cx);
                }
            }))
            .child(
                v_flex()
                    .size_full()
                    .px(px(14.))
                    .pt(px(10.))
                    .gap(px(8.))
                    .child(header)
                    .child(body)
                    .with_animation("banner-in", Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()), |el, d| {
                        el.ml(px(28. * (1. - d))).opacity(d)
                    }),
            )
            .children(actions)
    }
}
