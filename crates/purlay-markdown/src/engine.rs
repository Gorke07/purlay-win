//! The engine, configured as the server's `engine.ts` (D-101): micromark (here
//! its Rust port, `markdown-rs`) with the GFM extensions the product renders and
//! nothing else.

use markdown::mdast::Node;
use markdown::{Constructs, ParseOptions};

use crate::scan::{CodeRange, nests_too_deep};

/// CommonMark with raw HTML, definitions, images, indented code, setext
/// headings and character references never recognised; no footnotes.
fn base_constructs() -> Constructs {
    Constructs {
        html_flow: false,
        html_text: false,
        definition: false,
        label_start_image: false,
        code_indented: false,
        heading_setext: false,
        character_reference: false,
        gfm_footnote_definition: false,
        gfm_label_start_footnote: false,
        frontmatter: false,
        math_flow: false,
        math_text: false,
        ..Constructs::default()
    }
}

fn full_options() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            gfm_autolink_literal: true,
            gfm_strikethrough: true,
            gfm_table: true,
            gfm_task_list_item: true,
            ..base_constructs()
        },
        gfm_strikethrough_single_tilde: true,
        ..ParseOptions::default()
    }
}

fn blocks_options() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            gfm_table: true,
            attention: false,
            autolink: false,
            character_escape: false,
            code_text: false,
            hard_break_escape: false,
            hard_break_trailing: false,
            label_start_link: false,
            label_end: false,
            ..base_constructs()
        },
        ..ParseOptions::default()
    }
}

/// The full reading. Only ever run inside the bounds `parse` checks first.
pub(crate) fn syntax_tree_of(text: &str) -> Result<Node, String> {
    markdown::to_mdast(text, &full_options()).map_err(|e| e.to_string())
}

/// The block structure alone: where the code is, read cheaply.
pub(crate) fn block_tree_of(text: &str) -> Result<Node, String> {
    markdown::to_mdast(text, &blocks_options()).map_err(|e| e.to_string())
}

/// The byte ranges of fenced code in a tree the engine made, in order.
pub(crate) fn code_ranges_of(tree: &Node) -> Vec<CodeRange> {
    let mut ranges = Vec::new();
    let mut stack = vec![tree];
    while let Some(node) = stack.pop() {
        if let Node::Code(_) = node {
            if let Some(p) = node.position() {
                ranges.push(CodeRange {
                    start: p.start.offset,
                    end: p.end.offset,
                });
            }
            continue;
        }
        if let Some(children) = node.children() {
            stack.extend(children.iter().rev());
        }
    }
    ranges
}

/// Whether the caret at the end of `text` is in unfinished fenced code (the
/// composer's question): the last code block runs to the end and has only its
/// opening fence.
pub fn ends_in_open_code(text: &str) -> bool {
    if (!text.contains("```") && !text.contains("~~~")) || nests_too_deep(text) {
        return false;
    }
    let Ok(tree) = block_tree_of(text) else {
        return false;
    };
    let Some(last) = code_ranges_of(&tree).last().copied() else {
        return false;
    };
    if last.end != text.len() {
        return false;
    }
    // A closing fence, when there is one, is the block's last line (any earlier
    // line that could close it would have): the block is open unless its last
    // line, past its container prefix, is a run of the opening marker at least
    // as long as the opening fence.
    let block = &text[last.start..last.end];
    let opening = block.trim_start_matches([' ', '\t']);
    let marker = opening.as_bytes().first().copied().unwrap_or(b'`');
    let open_len = opening.bytes().take_while(|&b| b == marker).count();
    let mut lines = block.split(['\n', '\r']);
    lines.next();
    let Some(last_line) = lines.next_back() else {
        return true;
    };
    let l = last_line
        .trim_start_matches([' ', '\t', '>'])
        .trim_end_matches([' ', '\t']);
    let n = l.bytes().take_while(|&b| b == marker).count();
    !(n >= open_len && n == l.len())
}

#[cfg(test)]
mod tests {
    use super::ends_in_open_code;

    #[test]
    fn open_code_at_the_end() {
        assert!(ends_in_open_code("```"));
        assert!(ends_in_open_code("```ts\nlet a"));
        assert!(ends_in_open_code("text\n~~~\nx"));
        assert!(ends_in_open_code("> ```\n> x"));
        assert!(ends_in_open_code("````\n```"));
        assert!(!ends_in_open_code("```\nx\n```"));
        assert!(!ends_in_open_code("> ```\n> x\n> ```"));
        assert!(!ends_in_open_code("```\nx\n```\nafter"));
        assert!(!ends_in_open_code("no code"));
        assert!(!ends_in_open_code("`inline`"));
    }
}
