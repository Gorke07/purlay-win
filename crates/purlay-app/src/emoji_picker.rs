//! The emoji picker: a search field, the person's recent emoji, then the catalogue.

use crate::i18n::t;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::prefs::Prefs;
use std::rc::Rc;

type OnPick = Rc<dyn Fn(&str, &mut Window, &mut App)>;

pub struct EmojiPicker {
    prefs: Prefs,
    search: Entity<InputState>,
    on_pick: Option<OnPick>,
    _subs: Vec<Subscription>,
}

impl EmojiPicker {
    pub fn new(prefs: Prefs, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("emoji.search")));
        let subs = vec![cx.subscribe_in(&search, window, |this, state, e, window, cx| match e {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => {
                let q = state.read(cx).value().to_string();
                if let Some(first) = crate::emoji::search(&q).first() {
                    this.pick(first, window, cx);
                }
            }
            _ => {}
        })];
        EmojiPicker { prefs, search, on_pick: None, _subs: subs }
    }

    pub fn on_pick(&mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) {
        self.on_pick = Some(Rc::new(f));
    }

    fn pick(&mut self, e: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.prefs.push_recent_emoji(e);
        if let Some(f) = self.on_pick.clone() {
            f(e, window, cx);
        }
    }

    fn grid(&self, id: &'static str, list: Vec<String>, cx: &mut Context<Self>) -> Div {
        let k = c(cx);
        let hover = k.surface_200;
        let mut grid = div().flex().flex_row().flex_wrap().gap(px(2.));
        for (i, e) in list.into_iter().enumerate() {
            let e2 = e.clone();
            grid = grid.child(
                div()
                    .id((id, i))
                    .size(px(36.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(20.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(e)
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(&e2, window, cx))),
            );
        }
        grid
    }
}

impl Focusable for EmojiPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }
}

impl Render for EmojiPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let q = self.search.read(cx).value().to_string();
        let recent = self.prefs.get().recent_emoji;
        let found: Vec<String> = crate::emoji::search(&q).into_iter().map(String::from).collect();
        v_flex()
            .gap_2()
            .w_full()
            .child(Input::new(&self.search))
            .child(
                div()
                    .id("emoji-scroll")
                    .h(px(280.))
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap_2()
                            .when(q.is_empty() && !recent.is_empty(), |d| d.child(caption(t("emoji.recent"), cx)).child(self.grid("recent", recent.clone(), cx)))
                            .when(q.is_empty(), |d| d.child(caption(t("emoji.all"), cx)))
                            .child(self.grid("all", found, cx)),
                    ),
            )
    }
}
