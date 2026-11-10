//! Draws a message from the narrowed markdown tree the server reads (D-101): paragraphs,
//! headings at body size, fenced code with a language badge, quotes, lists and task
//! lists, tables, rules; emphasis, strong, strikethrough, inline code, links and
//! `@mentions`. A body the reader refuses is shown as typed.

use crate::theme::{c, Tokens};
use crate::ui::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use purlay_core::markdown::{self as md, Block, Inline, Reading};
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Copy, Default, PartialEq)]
struct Marks {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
    mention: bool,
}

struct Flat {
    text: String,
    segs: Vec<(Range<usize>, Marks)>,
    links: Vec<(Range<usize>, String)>,
}

impl Flat {
    fn push(&mut self, s: &str, m: Marks) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        self.segs.push((start..self.text.len(), m));
    }
}

fn flatten(inlines: &[Inline], m: Marks, out: &mut Flat, mentions: bool) {
    for i in inlines {
        match i {
            Inline::Text { value } => {
                if mentions && !m.link && !m.code {
                    let spans = md::mentions_in(value);
                    let mut at = 0;
                    for sp in spans {
                        out.push(&value[at..sp.start], m);
                        out.push(&value[sp.start..sp.end], Marks { mention: true, ..m });
                        at = sp.end;
                    }
                    out.push(&value[at..], m);
                } else {
                    out.push(value, m);
                }
            }
            Inline::InlineCode { value } => out.push(value, Marks { code: true, ..m }),
            Inline::Emphasis { children } => flatten(children, Marks { italic: true, ..m }, out, mentions),
            Inline::Strong { children } => flatten(children, Marks { bold: true, ..m }, out, mentions),
            Inline::Delete { children } => flatten(children, Marks { strike: true, ..m }, out, mentions),
            Inline::Link { url, children } => {
                let start = out.text.len();
                flatten(children, Marks { link: true, ..m }, out, false);
                if out.text.len() > start {
                    out.links.push((start..out.text.len(), url.clone()));
                }
            }
            Inline::Break => out.push("\n", m),
        }
    }
}

/// Renders inline content as one wrapped, clickable text run.
pub fn inline_text(id: impl Into<ElementId>, inlines: &[Inline], base_bold: bool, cx: &App) -> AnyElement {
    let k = c(cx);
    let mono: SharedString = Tokens::get(cx).mono.into();
    let mut flat = Flat { text: String::new(), segs: vec![], links: vec![] };
    flatten(inlines, Marks { bold: base_bold, ..Default::default() }, &mut flat, true);
    if flat.text.is_empty() {
        return div().into_any_element();
    }
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut fonts: Vec<(Range<usize>, SharedString)> = Vec::new();
    for (r, m) in &flat.segs {
        let mut h = HighlightStyle::default();
        if m.bold {
            h.font_weight = Some(FontWeight::SEMIBOLD);
        }
        if m.italic {
            h.font_style = Some(FontStyle::Italic);
        }
        if m.strike {
            h.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: Some(k.ink_muted) });
        }
        if m.code {
            h.background_color = Some(k.code_bg);
            h.color = Some(k.ink);
            fonts.push((r.clone(), mono.clone()));
        }
        if m.link {
            h.color = Some(k.accent_text);
            h.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(k.accent_text), wavy: false });
        }
        if m.mention {
            h.color = Some(k.accent_text);
            h.background_color = Some(k.accent_subtle);
            h.font_weight = Some(FontWeight::MEDIUM);
        }
        if h != HighlightStyle::default() {
            highlights.push((r.clone(), h));
        }
    }
    let styled = StyledText::new(flat.text.clone()).with_highlights(highlights).with_font_family_overrides(fonts);
    if flat.links.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<Range<usize>> = flat.links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Rc<Vec<String>> = Rc::new(flat.links.iter().map(|(_, u)| u.clone()).collect());
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, _, cx| {
            if let Some(u) = urls.get(ix) {
                cx.open_url(u);
            }
        })
        .into_any_element()
}

/// The whole body of a message.
pub fn render_markdown(id: &str, content: &str, cx: &App) -> AnyElement {
    match md::read_markdown(content) {
        Reading::Tree(root) => {
            let mut col = v_flex().gap_1().min_w_0();
            for (i, b) in root.children.iter().enumerate() {
                col = col.child(block(&format!("{id}-{i}"), b, 0, cx));
            }
            col.into_any_element()
        }
        Reading::Typed(_) => text_style(div(), purlay_tokens::typography::BODY, cx).text_color(c(cx).ink).child(content.to_string()).into_any_element(),
    }
}

fn block(id: &str, b: &Block, depth: usize, cx: &App) -> AnyElement {
    let k = c(cx);
    match b {
        Block::Paragraph { children } => div().min_w_0().child(inline_text(SharedString::from(id.to_string()), children, false, cx)).into_any_element(),
        Block::Heading { children, .. } => div().pt_1().child(inline_text(SharedString::from(id.to_string()), children, true, cx)).into_any_element(),
        Block::Code { lang, value } => code_block(id, lang.as_deref(), value, cx),
        Block::Blockquote { children } => {
            let mut col = v_flex().gap_1().pl_3().border_l_2().border_color(k.border_strong).text_color(k.ink_muted);
            for (i, c2) in children.iter().enumerate() {
                col = col.child(block(&format!("{id}-q{i}"), c2, depth + 1, cx));
            }
            col.into_any_element()
        }
        Block::List { ordered, start, children } => {
            let mut col = v_flex().gap(px(2.));
            for (i, item) in children.iter().enumerate() {
                let marker: AnyElement = match item.checked {
                    Some(true) => div().text_color(k.success).child("☑").into_any_element(),
                    Some(false) => div().text_color(k.ink_muted).child("☐").into_any_element(),
                    None if *ordered => div().text_color(k.ink_muted).child(format!("{}.", start.unwrap_or(1) as usize + i)).into_any_element(),
                    None => div().text_color(k.ink_muted).child(if depth.is_multiple_of(2) { "•" } else { "◦" }).into_any_element(),
                };
                let mut inner = v_flex().flex_1().min_w_0().gap(px(2.));
                for (j, c2) in item.children.iter().enumerate() {
                    inner = inner.child(block(&format!("{id}-l{i}-{j}"), c2, depth + 1, cx));
                }
                col = col.child(div().flex().flex_row().items_start().gap_2().child(div().min_w(px(16.)).flex().justify_end().child(marker)).child(inner));
            }
            col.into_any_element()
        }
        Block::Table { align, children } => {
            let mut table = v_flex().rounded(px(purlay_tokens::radius::LG)).border_1().border_color(k.border_subtle).overflow_hidden().max_w_full();
            for (r, row) in children.iter().enumerate() {
                let mut line = div().flex().flex_row().when(r > 0, |d| d.border_t_1().border_color(k.border_subtle)).when(r == 0, |d| d.bg(k.surface_050));
                for (ci, cell) in row.children.iter().enumerate() {
                    let a = align.get(ci).copied().flatten();
                    let cell_div = div()
                        .flex_1()
                        .min_w(px(60.))
                        .px_2()
                        .py_1()
                        .when(ci > 0, |d| d.border_l_1().border_color(k.border_subtle))
                        .when(a == Some(md::Align::Right), |d| d.flex().justify_end())
                        .when(a == Some(md::Align::Center), |d| d.flex().justify_center())
                        .child(inline_text(SharedString::from(format!("{id}-t{r}-{ci}")), &cell.children, r == 0, cx));
                    line = line.child(cell_div);
                }
                table = table.child(line);
            }
            div().id(SharedString::from(format!("{id}-tbl"))).overflow_x_scroll().child(table).into_any_element()
        }
        Block::ThematicBreak => div().py_2().child(divider(cx)).into_any_element(),
    }
}

pub fn code_block(id: &str, lang: Option<&str>, value: &str, cx: &App) -> AnyElement {
    let k = c(cx);
    let code = value.to_string();
    let copy_text = code.clone();
    v_flex()
        .group("code")
        .relative()
        .my_1()
        .rounded(px(purlay_tokens::radius::XL))
        .border_1()
        .border_color(k.border_subtle)
        .bg(k.code_bg)
        .max_w_full()
        .when_some(lang.filter(|l| !l.is_empty()), |d, l| {
            d.child(h_flex().px_3().pt_2().child(caption(l.to_string(), cx).font_family(Tokens::get(cx).mono)))
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-code")))
                .overflow_x_scroll()
                .max_h(px(purlay_tokens::size::CODE_CLAMP * 2.))
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .child(text_style(div(), purlay_tokens::typography::CODE, cx).text_color(k.ink).whitespace_nowrap().child(code)),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id}-copy")))
                .absolute()
                .top(px(6.))
                .right(px(6.))
                .invisible()
                .group_hover("code", |s| s.visible())
                .child(icon_button(SharedString::from(format!("{id}-copybtn")), "copy", "copy", cx).on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                })),
        )
        .into_any_element()
}
