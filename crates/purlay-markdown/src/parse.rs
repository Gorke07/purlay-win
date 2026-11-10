//! A message's markdown, read once into the tree the product renders (server
//! `parse.ts`).

use markdown::mdast::{AlignKind, Node};

use crate::engine::{block_tree_of, code_ranges_of, syntax_tree_of};
use crate::href::safe_href;
use crate::scan::{MARKDOWN_DELIMITERS_MAX, MARKDOWN_RUN_MAX, measure_outside, nests_too_deep};
use crate::tree::{Align, Block, Inline, ListItem, Root, TableCell, TableRow};

/// Nesting, on the tree, beyond which a body is shown as typed.
pub const MARKDOWN_DEPTH_MAX: usize = 100;

/// Why a body is shown as typed rather than formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypedReason {
    /// A line opens more than `MARKDOWN_NESTING_MAX` containers.
    Nesting,
    /// More than `MARKDOWN_DELIMITERS_MAX` inline delimiters outside code.
    Delimiters,
    /// An address-like run longer than `MARKDOWN_RUN_MAX` outside code.
    Run,
    /// The tree is deeper than `MARKDOWN_DEPTH_MAX`.
    Depth,
    /// The engine failed inside the bounds: a defect, never expected.
    Engine,
}

impl TypedReason {
    /// The server's (and corpus') name for the reason.
    pub const fn as_str(self) -> &'static str {
        match self {
            TypedReason::Nesting => "nesting",
            TypedReason::Delimiters => "delimiters",
            TypedReason::Run => "run",
            TypedReason::Depth => "depth",
            TypedReason::Engine => "engine",
        }
    }
}

impl serde::Serialize for TypedReason {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// A body's tree, or why it is shown as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Tree(Root),
    Typed(TypedReason),
}

impl Reading {
    /// The tree, when the body is formatted.
    pub fn tree(&self) -> Option<&Root> {
        match self {
            Reading::Tree(root) => Some(root),
            Reading::Typed(_) => None,
        }
    }

    pub fn into_tree(self) -> Option<Root> {
        match self {
            Reading::Tree(root) => Some(root),
            Reading::Typed(_) => None,
        }
    }

    /// `{ "tree": … }` or `{ "tree": null, "reason": … }`, as the server's `Reading`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Reading::Tree(root) => serde_json::json!({ "tree": root.to_json() }),
            Reading::Typed(reason) => {
                serde_json::json!({ "tree": null, "reason": reason.as_str() })
            }
        }
    }
}

struct TooDeep;

/// Adjacent text runs, joined.
fn joined(nodes: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(nodes.len());
    for node in nodes {
        if let Inline::Text { value } = &node
            && let Some(Inline::Text { value: last }) = out.last_mut()
        {
            last.push_str(value);
            continue;
        }
        out.push(node);
    }
    out
}

fn source_of<'a>(node: &Node, text: &'a str) -> &'a str {
    match node.position() {
        Some(p) => text.get(p.start.offset..p.end.offset).unwrap_or(""),
        None => "",
    }
}

fn guard(depth: usize) -> Result<(), TooDeep> {
    if depth > MARKDOWN_DEPTH_MAX {
        Err(TooDeep)
    } else {
        Ok(())
    }
}

fn inlines(nodes: &[Node], depth: usize, text: &str) -> Result<Vec<Inline>, TooDeep> {
    guard(depth)?;
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            Node::Text(t) => out.push(Inline::Text {
                value: t.value.clone(),
            }),
            Node::InlineCode(c) => out.push(Inline::InlineCode {
                value: c.value.clone(),
            }),
            Node::Break(_) => out.push(Inline::Break),
            Node::Emphasis(e) => out.push(Inline::Emphasis {
                children: inlines(&e.children, depth + 1, text)?,
            }),
            Node::Strong(e) => out.push(Inline::Strong {
                children: inlines(&e.children, depth + 1, text)?,
            }),
            Node::Delete(e) => out.push(Inline::Delete {
                children: inlines(&e.children, depth + 1, text)?,
            }),
            Node::Link(l) => {
                let children = inlines(&l.children, depth + 1, text)?;
                match safe_href(&l.url) {
                    Some(url) => out.push(Inline::Link { url, children }),
                    None => out.extend(children),
                }
            }
            Node::Html(_)
            | Node::Image(_)
            | Node::ImageReference(_)
            | Node::LinkReference(_)
            | Node::FootnoteReference(_) => out.push(Inline::Text {
                value: source_of(node, text).to_owned(),
            }),
            // Nothing else is phrasing content the configured engine makes.
            _ => {}
        }
    }
    Ok(joined(out))
}

fn align_of(kind: &AlignKind) -> Option<Align> {
    match kind {
        AlignKind::Left => Some(Align::Left),
        AlignKind::Right => Some(Align::Right),
        AlignKind::Center => Some(Align::Center),
        AlignKind::None => None,
    }
}

fn blocks(nodes: &[Node], depth: usize, text: &str) -> Result<Vec<Block>, TooDeep> {
    guard(depth)?;
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            Node::Paragraph(p) => out.push(Block::Paragraph {
                children: inlines(&p.children, depth + 1, text)?,
            }),
            Node::Heading(h) => out.push(Block::Heading {
                level: h.depth.clamp(1, 3),
                children: inlines(&h.children, depth + 1, text)?,
            }),
            Node::Code(c) => out.push(Block::Code {
                lang: c.lang.clone(),
                value: c.value.clone(),
            }),
            Node::Blockquote(q) => out.push(Block::Blockquote {
                children: blocks(&q.children, depth + 1, text)?,
            }),
            Node::ThematicBreak(_) => out.push(Block::ThematicBreak),
            Node::List(l) => {
                let mut children = Vec::with_capacity(l.children.len());
                for item in &l.children {
                    if let Node::ListItem(item) = item {
                        children.push(ListItem {
                            checked: item.checked,
                            children: blocks(&item.children, depth + 2, text)?,
                        });
                    }
                }
                out.push(Block::List {
                    ordered: l.ordered,
                    start: if l.ordered {
                        Some(l.start.unwrap_or(1))
                    } else {
                        None
                    },
                    children,
                });
            }
            Node::Table(t) => {
                let mut rows = Vec::with_capacity(t.children.len());
                for row in &t.children {
                    if let Node::TableRow(row) = row {
                        let mut cells = Vec::with_capacity(row.children.len());
                        for cell in &row.children {
                            if let Node::TableCell(cell) = cell {
                                cells.push(TableCell {
                                    children: inlines(&cell.children, depth + 3, text)?,
                                });
                            }
                        }
                        rows.push(TableRow { children: cells });
                    }
                }
                out.push(Block::Table {
                    align: t.align.iter().map(align_of).collect(),
                    children: rows,
                });
            }
            // Never recognised at this level: shown as the characters typed.
            _ => out.push(Block::Paragraph {
                children: vec![Inline::Text {
                    value: source_of(node, text).to_owned(),
                }],
            }),
        }
    }
    Ok(out)
}

/// A body's tree, or why it is shown as typed: the server's `readMarkdown`.
pub fn read_markdown(text: &str) -> Reading {
    // micromark reads U+0000 as U+FFFD; markdown-rs keeps it. Neither character
    // counts for the bounds, so substituting first changes nothing else.
    let replaced;
    let text = if text.contains('\0') {
        replaced = text.replace('\0', "\u{FFFD}");
        replaced.as_str()
    } else {
        text
    };
    if nests_too_deep(text) {
        return Reading::Typed(TypedReason::Nesting);
    }
    let Ok(block_tree) = block_tree_of(text) else {
        return Reading::Typed(TypedReason::Engine);
    };
    let prose = measure_outside(text, &code_ranges_of(&block_tree));
    drop(block_tree);
    if prose.delimiters > MARKDOWN_DELIMITERS_MAX {
        return Reading::Typed(TypedReason::Delimiters);
    }
    if prose.longest_run > MARKDOWN_RUN_MAX {
        return Reading::Typed(TypedReason::Run);
    }
    let Ok(root) = syntax_tree_of(text) else {
        return Reading::Typed(TypedReason::Engine);
    };
    let children = match &root {
        Node::Root(r) => &r.children[..],
        _ => &[],
    };
    match blocks(children, 1, text) {
        Ok(children) => Reading::Tree(Root { children }),
        Err(TooDeep) => Reading::Typed(TypedReason::Depth),
    }
}

/// A body's tree, or `None` when it is shown as typed.
pub fn parse_markdown(text: &str) -> Option<Root> {
    read_markdown(text).into_tree()
}

/// The body with every fenced block blanked to spaces (one per UTF-16 unit,
/// line breaks kept), so mentions are looked for where the reader sees prose.
pub fn without_fenced_code(text: &str) -> String {
    if nests_too_deep(text) {
        return text.to_owned();
    }
    let Ok(tree) = block_tree_of(text) else {
        return text.to_owned();
    };
    let mut out = String::with_capacity(text.len());
    let mut from = 0usize;
    for range in code_ranges_of(&tree) {
        out.push_str(&text[from..range.start]);
        for c in text[range.start..range.end].chars() {
            if c == '\r' || c == '\n' {
                out.push(c);
            } else {
                for _ in 0..c.len_utf16() {
                    out.push(' ');
                }
            }
        }
        from = range.end;
    }
    out.push_str(&text[from..]);
    out
}
