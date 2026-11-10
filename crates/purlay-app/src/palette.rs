//! Ctrl+K: jump to a channel or a person.

use crate::actions::*;
use crate::i18n::t;
use crate::nav::Nav;
use crate::sidebar::channel_title;
use crate::state::AppModel;
use crate::theme::c;
use crate::ui::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::contract::models::*;
use purlay_core::contract::Uuid;
use purlay_core::store::Key;

#[derive(Clone)]
enum Target {
    Channel(Uuid),
    Person(Uuid),
}

#[derive(Clone)]
struct Item {
    title: String,
    detail: String,
    icon: &'static str,
    avatar: Option<String>,
    target: Target,
}

pub struct Palette {
    app: Entity<AppModel>,
    nav: Entity<Nav>,
    pub input: Entity<InputState>,
    selected: usize,
    _subs: Vec<Subscription>,
}

impl Palette {
    pub fn new(app: Entity<AppModel>, nav: Entity<Nav>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("palette.placeholder")));
        let subs = vec![cx.subscribe_in(&input, window, |this, _, e: &InputEvent, window, cx| match e {
            InputEvent::Change => {
                this.selected = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.pick(None, window, cx),
            _ => {}
        })];
        Palette { app, nav, input, selected: 0, _subs: subs }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0;
        self.input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
    }

    fn items(&self, cx: &App) -> Vec<Item> {
        let Some(s) = self.app.read(cx).session() else { return vec![] };
        let q = self.input.read(cx).value().trim().to_lowercase();
        let nav = self.nav.read(cx);
        let store = s.store().read_recursive();
        let mut out = Vec::new();
        for ch in nav.ordered_channels(cx) {
            let title = channel_title(&ch, &s.user_id);
            if !q.is_empty() && !title.to_lowercase().contains(&q) {
                continue;
            }
            let dm = ch.r#type == ChannelType::DirectMessage;
            out.push(Item {
                title: if dm { title.clone() } else { format!("#{title}") },
                detail: ch.topic.clone().unwrap_or_default(),
                icon: match ch.r#type {
                    ChannelType::Private => "lock",
                    ChannelType::DirectMessage => "at-sign",
                    _ => "hash",
                },
                avatar: if dm { Some(title) } else { None },
                target: Target::Channel(ch.id.clone()),
            });
        }
        if let Some(w) = &nav.workspace {
            // Public channels not joined yet are found too.
            if let Some(map) = store.channels.get(w) {
                for ch in map.values().filter(|c| c.state.is_none() && c.r#type == ChannelType::Public) {
                    let title = ch.name.clone().unwrap_or_default();
                    if !q.is_empty() && !title.to_lowercase().contains(&q) {
                        continue;
                    }
                    out.push(Item { title: format!("#{title}"), detail: ch.topic.clone().unwrap_or_default(), icon: "hash", avatar: None, target: Target::Channel(ch.id.clone()) });
                }
            }
            if let Some(members) = store.cache.get::<WorkspaceMemberList>(&Key::Members(w.clone())) {
                for m in members {
                    if m.user.id == s.user_id {
                        continue;
                    }
                    let hay = format!("{} {}", m.user.display_name, m.user.username).to_lowercase();
                    if q.is_empty() || !hay.contains(&q) {
                        continue;
                    }
                    out.push(Item {
                        title: m.user.display_name.clone(),
                        detail: format!("@{}", m.user.username),
                        icon: "at-sign",
                        avatar: Some(m.user.display_name.clone()),
                        target: Target::Person(m.user.id.clone()),
                    });
                }
            }
        }
        out.truncate(50);
        out
    }

    fn pick(&mut self, ix: Option<usize>, _window: &mut Window, cx: &mut Context<Self>) {
        let items = self.items(cx);
        let Some(it) = items.get(ix.unwrap_or(self.selected)).cloned() else { return };
        self.nav.update(cx, |n, cx| {
            n.palette = false;
            cx.notify();
        });
        match it.target {
            Target::Channel(id) => self.nav.update(cx, |n, cx| n.open_channel(&id, cx)),
            Target::Person(uid) => {
                let Some(s) = self.app.read(cx).session().cloned() else { return };
                let Some(w) = self.nav.read(cx).workspace.clone() else { return };
                let h = s.open_conversation(&w, vec![uid]);
                let nav = self.nav.clone();
                cx.spawn(async move |_, cx| {
                    if let Ok(Ok(ch)) = h.await {
                        nav.update(cx, |n, cx| n.open_channel(&ch.id, cx));
                    }
                })
                .detach();
            }
        }
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = c(cx);
        let items = self.items(cx);
        let n = items.len();
        let hover = k.surface_050;
        v_flex()
            .key_context("Palette")
            .on_action(cx.listener(move |this, _: &PaletteNext, _, cx| {
                if n > 0 {
                    this.selected = (this.selected + 1) % n;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(move |this, _: &PalettePrev, _, cx| {
                if n > 0 {
                    this.selected = (this.selected + n - 1) % n;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PalettePick, window, cx| this.pick(None, window, cx)))
            .on_action(cx.listener(|this, _: &PaletteClose, _, cx| {
                this.nav.update(cx, |n, cx| {
                    n.palette = false;
                    cx.notify();
                })
            }))
            .w(px(purlay_tokens::size::PALETTE))
            .rounded(px(purlay_tokens::radius::RADIUS_2XL))
            .bg(k.surface_100)
            .border_1()
            .border_color(k.border_subtle)
            .shadow_xl()
            .overflow_hidden()
            .child(div().p_2().child(Input::new(&self.input).large().appearance(false)))
            .child(crate::ui::divider(cx))
            .child(
                div().id("palette-list").max_h(px(purlay_tokens::size::PALETTE_LIST)).overflow_y_scroll().p_1().children(if items.is_empty() {
                    vec![div().p_4().child(caption(t("palette.none"), cx)).into_any_element()]
                } else {
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(i, it)| {
                            let sel = i == self.selected;
                            h_flex()
                                .id(("pal", i))
                                .h(px(36.))
                                .px_3()
                                .gap_2()
                                .rounded(px(purlay_tokens::radius::LG))
                                .cursor_pointer()
                                .when(sel, |d| d.bg(k.accent_subtle))
                                .when(!sel, |d| d.hover(move |s| s.bg(hover)))
                                .child(match &it.avatar {
                                    Some(name) => avatar(name.clone(), name, 20., cx).into_any_element(),
                                    None => ico(it.icon, 15., k.ink_muted).into_any_element(),
                                })
                                .child(body(it.title.clone(), cx))
                                .child(caption(it.detail.clone(), cx).flex_1().overflow_hidden().text_ellipsis().whitespace_nowrap())
                                .on_click(cx.listener(move |this, _, window, cx| this.pick(Some(i), window, cx)))
                                .into_any_element()
                        })
                        .collect()
                }),
            )
    }
}
