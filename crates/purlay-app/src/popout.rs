//! A conversation in a window of its own (as Teams' pop-out chats): the feed and its
//! composer, threads opening in place, a pin that keeps the window above others, and a
//! way back to the main window. One window per conversation; asking again brings it up.

use crate::chrome::{caption_buttons, tool_button, tool_group, TOP_BAR};
use crate::conversation::ConversationView;
use crate::i18n::t;
use crate::nav::{Inspector, Nav};
use crate::sidebar::channel_title;
use crate::state::AppModel;
use crate::theme::c;
use crate::thread::ThreadView;
use crate::ui::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::ChannelType;
use purlay_core::contract::Uuid;
use std::collections::HashMap;

#[derive(Default)]
struct Popouts(HashMap<Uuid, AnyWindowHandle>);
impl Global for Popouts {}

/// Opens `channel` in its own window, or brings its window to the front.
pub fn open(app: Entity<AppModel>, channel: Uuid, cx: &mut App) {
    if !cx.has_global::<Popouts>() {
        cx.set_global(Popouts::default());
    }
    if let Some(h) = cx.global::<Popouts>().0.get(&channel).copied() {
        if h.update(cx, |_, w, cx| crate::tray::show_window(w, cx)).is_ok() {
            return;
        }
        cx.global_mut::<Popouts>().0.remove(&channel);
    }
    // Beside the main window when there is room, else centred.
    let size_ = size(px(560.), px(760.));
    let main = cx.try_global::<crate::banner::MainWindow>().map(|m| m.0);
    let bounds = main
        .and_then(|m| m.update(cx, |_, w, _| w.bounds()).ok())
        .map(|b| Bounds::new(point(b.origin.x + px(48.), b.origin.y + px(48.)), size_))
        .unwrap_or_else(|| Bounds::centered(None, size_, cx));
    let mut options = gpui_kit::component::TitleBar::window_options();
    options.window_bounds = Some(WindowBounds::Windowed(bounds));
    options.window_min_size = Some(size(px(380.), px(420.)));
    options.app_id = Some("dev.purlay.windows".into());
    let ch = channel.clone();
    let opened = cx.open_window(options, move |window, cx| cx.new(|cx| PopoutView::new(app, ch, window, cx)));
    match opened {
        Ok(h) => {
            cx.global_mut::<Popouts>().0.insert(channel, h.into());
        }
        Err(e) => tracing::warn!("conversation window: {e}"),
    }
}

pub struct PopoutView {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    channel: Uuid,
    conversation: Entity<ConversationView>,
    thread: Option<Entity<ThreadView>>,
    pinned: bool,
    _subs: Vec<Subscription>,
}

impl PopoutView {
    fn new(app: Entity<AppModel>, channel: Uuid, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if let Some(s) = app.read(cx).session() {
            s.open_channel(&channel);
        }
        let nav = cx.new(|cx| Nav::passive(app.clone(), &channel, cx));
        let conversation = cx.new(|cx| ConversationView::new(app.clone(), nav.clone(), channel.clone(), window, cx));
        let id = window.window_handle().window_id().as_u64();
        let ch = channel.clone();
        let subs = vec![
            cx.observe(&nav, |_, _, cx| cx.notify()),
            cx.observe(&app, |_, _, cx| cx.notify()),
            // Purlay is "in front" while this window is; it reads what it shows.
            cx.observe_window_activation(window, move |this, window, cx| {
                let active = window.is_window_active();
                this.app.update(cx, |m, cx| m.set_window_active(id, active, cx));
            }),
        ];
        window.on_window_should_close(cx, move |window, cx| {
            forget(&ch, window, cx);
            true
        });
        PopoutView { app, nav, channel, conversation, thread: None, pinned: false, _subs: subs }
    }

    fn title(&self, cx: &App) -> (String, Option<String>, bool, String) {
        let Some(s) = self.app.read(cx).session() else { return (String::new(), None, false, String::new()) };
        let store = s.store().read_recursive();
        let Some(ch) = store.channel(&self.channel) else { return (String::new(), None, false, String::new()) };
        let name = channel_title(ch, &s.user_id);
        let dm = ch.r#type == ChannelType::DirectMessage;
        let key = if dm { ch.peers.iter().find(|p| p.id != s.user_id).or(ch.peer.as_ref()).map(|p| p.username.clone()).unwrap_or_else(|| name.clone()) } else { name.clone() };
        (name, ch.topic.clone().filter(|t| !t.is_empty()), dm, key)
    }
}

impl Render for PopoutView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let (name, topic, dm, avatar_key) = self.title(cx);
        window.set_window_title(&if name.is_empty() { "Purlay".to_string() } else { format!("{name} · Purlay") });

        // A thread opens in place of the feed; its close button comes back.
        let page: AnyElement = match self.nav.read(cx).inspector.clone() {
            Some(Inspector::Thread { root, channel }) => {
                let reuse = self.thread.as_ref().filter(|t| t.read(cx).root == root).cloned();
                let v = reuse.unwrap_or_else(|| {
                    let v = cx.new(|cx| ThreadView::new(self.app.clone(), self.nav.clone(), root.clone(), channel.clone(), window, cx));
                    self.thread = Some(v.clone());
                    v
                });
                div().size_full().child(v).into_any_element()
            }
            _ => {
                self.thread = None;
                self.conversation.clone().into_any_element()
            }
        };

        let lead: AnyElement = if dm { avatar(avatar_key, &name, 24., cx).into_any_element() } else { ico("hash", 16., k.ink).into_any_element() };
        let pinned = self.pinned;
        let app = self.app.clone();
        let ch = self.channel.clone();
        let bar = h_flex()
            .id("popout-bar")
            .h(px(TOP_BAR))
            .flex_none()
            .items_start()
            .border_b_1()
            .border_color(k.border_subtle)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pl_4()
                    .pr_2()
                    .gap(px(8.))
                    .window_control_area(WindowControlArea::Drag)
                    .child(lead)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(text_style(div(), purlay_tokens::typography::HEADING, cx).text_color(k.ink).whitespace_nowrap().overflow_hidden().text_ellipsis().child(name))
                            .when_some(topic, |d, tp| d.child(text_style(div(), purlay_tokens::typography::CAPTION, cx).text_color(k.ink_muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(tp))),
                    )
                    .child(
                        tool_group(cx)
                            .child(tool_button("pop-pin", "pin", if pinned { t("popout.unpin") } else { t("popout.pin") }, pinned, cx).on_click(cx.listener(|this, _, window, cx| {
                                this.pinned = !this.pinned;
                                crate::tray::set_topmost(window, this.pinned);
                                cx.notify();
                            })))
                            .child(tool_button("pop-main", "panel-left", t("popout.inMain"), false, cx).on_click(move |_, window, cx| {
                                // Back to the main window, showing this conversation there.
                                let url = format!("purlay://open?channel={ch}");
                                app.update(cx, |m, cx| m.handle_url(url, cx));
                                if let Some(main) = cx.try_global::<crate::banner::MainWindow>().map(|m| m.0) {
                                    let _ = main.update(cx, |_, w, cx| crate::tray::show_window(w, cx));
                                }
                                forget(&ch, window, cx);
                                window.remove_window();
                            })),
                    ),
            )
            .child(caption_buttons(window, cx));

        v_flex()
            .size_full()
            .bg(k.surface_100)
            .text_color(k.ink)
            .font_family(crate::theme::Tokens::get(cx).sans)
            .child(bar)
            .child(div().flex_1().min_h_0().child(page))
    }
}

/// A conversation window is going: no longer listed, and no longer holding Purlay "in front".
fn forget(channel: &Uuid, window: &Window, cx: &mut App) {
    if cx.has_global::<Popouts>() {
        cx.global_mut::<Popouts>().0.remove(channel);
    }
    let id = window.window_handle().window_id().as_u64();
    crate::state::app(cx).update(cx, |m, cx| m.set_window_active(id, false, cx));
}