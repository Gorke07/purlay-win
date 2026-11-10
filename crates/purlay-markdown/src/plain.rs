//! A message as one line a list can show (server `plain.ts`).

use crate::parse::parse_markdown;
use crate::tree::{Block, Inline, Root};

fn spell(nodes: &[Inline], out: &mut String) {
    for node in nodes {
        match node {
            Inline::Text { value } | Inline::InlineCode { value } => out.push_str(value),
            Inline::Emphasis { children }
            | Inline::Strong { children }
            | Inline::Delete { children }
            | Inline::Link { children, .. } => spell(children, out),
            Inline::Break => out.push(' '),
        }
    }
}

fn words(blocks: &[Block], out: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Paragraph { children } | Block::Heading { children, .. } => {
                let mut s = String::new();
                spell(children, &mut s);
                out.push(s);
            }
            Block::Code { value, .. } => out.push(value.clone()),
            Block::Blockquote { children } => words(children, out),
            Block::List { children, .. } => {
                for item in children {
                    words(&item.children, out);
                }
            }
            Block::Table { children, .. } => {
                for row in children {
                    for cell in &row.children {
                        let mut s = String::new();
                        spell(&cell.children, &mut s);
                        out.push(s);
                    }
                }
            }
            Block::ThematicBreak => {}
        }
    }
}

/// JavaScript's `\s`.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// Every run of white space is one space, trimmed.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending = false;
    for c in text.chars() {
        if is_js_space(c) {
            pending = true;
        } else {
            if pending && !out.is_empty() {
                out.push(' ');
            }
            pending = false;
            out.push(c);
        }
    }
    out
}

/// The tree's text, in reading order, on one line: a code span by its
/// contents, a link by its label, emphasis without its marks.
pub fn plain_text(root: &Root) -> String {
    let mut w = Vec::new();
    words(&root.children, &mut w);
    one_line(&w.join(" "))
}

/// A message's content as an excerpt; past the bounds, the content as typed on one line.
pub fn excerpt_of(content: &str) -> String {
    match parse_markdown(content) {
        Some(tree) => plain_text(&tree),
        None => one_line(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        plain_text(&parse_markdown(text).expect("within bounds"))
    }

    #[test]
    fn server_cases() {
        assert_eq!(
            plain("Deployed `api` `06f1700` to **staging** in _84s_."),
            "Deployed api 06f1700 to staging in 84s."
        );
        assert_eq!(
            plain("see [the runbook](https://example.com/runbook) first"),
            "see the runbook first"
        );
        assert_eq!(
            plain("First.\n\n- one\n- two\n\n> quoted\n\n```ts\nconst a = 1;\nconst b = 2;\n```"),
            "First. one two quoted const a = 1; const b = 2;"
        );
        assert_eq!(plain("@dogancan can you look?"), "@dogancan can you look?");
        assert_eq!(
            plain("| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\nline one  \nline two"),
            "a b 1 2 line one line two"
        );
        assert_eq!(excerpt_of("`code` and **bold**"), "code and bold");
        let deep = format!("{} deep", ">".repeat(200));
        assert!(parse_markdown(&deep).is_none());
        assert_eq!(excerpt_of(&deep), deep);
        assert_eq!(excerpt_of("a\n\nb"), "a b");
    }
}
