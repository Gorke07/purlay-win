//! `@username` mentions: the shape (server `packages/markdown/src/mentions.ts`),
//! where they may be (`prose.ts`), and what a body mentions
//! (`apps/api/.../messages/mentions.ts` and `body.ts`).

use crate::parse::{Reading, read_markdown, without_fenced_code};
use crate::tree::{Block, Inline, Root};

/// More than this many distinct names in one message are ignored.
pub const MENTIONS_PER_MESSAGE_MAX: usize = 50;

/// The names that would address everybody, which the product does not have
/// (lower case; matching is case-insensitive).
pub const BROADCAST_NAMES: [&str; 3] = ["here", "channel", "everyone"];

/// Where a name is, in byte offsets of the text it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionSpan {
    /// Offset of the `@`.
    pub start: usize,
    /// Offset just past the name.
    pub end: usize,
    /// The name as written, without the `@`.
    pub name: String,
}

/// `[\p{L}\p{N}_]`. `is_alphabetic` is the Alphabetic property, a close
/// superset of `\p{L}` (it adds letter-like marks such as U+0345).
fn is_word(c: char) -> bool {
    c == '_' || c.is_alphabetic() || c.is_numeric()
}

/// Every mention-shaped name in a text, in order: the server's
/// `(?<![\p{L}\p{N}_@`])@([A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38})(?![\p{L}\p{N}_])`.
pub fn mentions_in(text: &str) -> Vec<MentionSpan> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = text[at..].find('@') {
        let start = at + found;
        at = start + 1;
        if let Some(prev) = text[..start].chars().next_back()
            && (is_word(prev) || prev == '@' || prev == '`')
        {
            continue;
        }
        // Greedy: the ends after each unit of the name.
        let mut ends: Vec<usize> = Vec::new();
        let mut i = start + 1;
        if i < b.len() && b[i].is_ascii_alphanumeric() {
            i += 1;
            ends.push(i);
            while ends.len() < 39 && i < b.len() {
                if b[i].is_ascii_alphanumeric()
                    || (b[i] == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_alphanumeric())
                {
                    i += 1;
                    ends.push(i);
                } else {
                    break;
                }
            }
        }
        // Backtrack until what follows is not a word character.
        let matched = ends
            .iter()
            .rev()
            .copied()
            .find(|&end| !text[end..].chars().next().is_some_and(is_word));
        if let Some(end) = matched {
            out.push(MentionSpan {
                start,
                end,
                name: text[start + 1..end].to_owned(),
            });
            at = end;
        }
    }
    out
}

/// The names a body mentions, and the refused broadcasts among them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedMentions {
    /// Distinct (case-insensitively), in order of first appearance, as written; at most 50.
    pub names: Vec<String>,
    /// The broadcast names that appeared, lower case. Empty when the body is acceptable.
    pub broadcast: Vec<String>,
}

/// The names found, distinct and bounded, and the refused broadcasts among them.
pub fn names_of<I, S>(found: I) -> ParsedMentions
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = ParsedMentions::default();
    let mut known: Vec<String> = Vec::new();
    for name in found {
        let name = name.as_ref();
        let key = name.to_lowercase();
        if BROADCAST_NAMES.contains(&key.as_str()) {
            if !out.broadcast.contains(&key) {
                out.broadcast.push(key);
            }
        } else if !known.contains(&key) && known.len() < MENTIONS_PER_MESSAGE_MAX {
            known.push(key);
            out.names.push(name.to_owned());
        }
    }
    out
}

/// Code and links are not prose: each stands for one backtick, which no name may follow.
fn spell(nodes: &[Inline], out: &mut String) {
    for node in nodes {
        match node {
            Inline::Text { value } => out.push_str(value),
            Inline::InlineCode { .. } | Inline::Link { .. } => out.push('`'),
            Inline::Emphasis { children }
            | Inline::Strong { children }
            | Inline::Delete { children } => spell(children, out),
            Inline::Break => out.push('\n'),
        }
    }
}

fn prose_of(blocks: &[Block], out: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Paragraph { children } | Block::Heading { children, .. } => {
                let mut s = String::new();
                spell(children, &mut s);
                out.push(s);
            }
            Block::Blockquote { children } => prose_of(children, out),
            Block::List { children, .. } => {
                for item in children {
                    prose_of(&item.children, out);
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
            Block::Code { .. } | Block::ThematicBreak => {}
        }
    }
}

/// Every mention-shaped name in the tree's prose, as written, in reading order
/// (server `proseNames`): never inside code or a link.
pub fn prose_names(tree: &Root) -> Vec<String> {
    let mut prose = Vec::new();
    prose_of(&tree.children, &mut prose);
    prose
        .iter()
        .flat_map(|p| mentions_in(p).into_iter().map(|s| s.name))
        .collect()
}

/// The text with every code span and fenced block replaced by a single
/// backtick (the server's mention parser for a body shown as typed).
fn without_code(text: &str) -> String {
    let b = text.as_bytes();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'`' {
            let s = i;
            while i < b.len() && b[i] == b'`' {
                i += 1;
            }
            runs.push((s, i - s));
        } else {
            i += 1;
        }
    }
    if runs.is_empty() {
        return text.to_owned();
    }
    let mut next: Vec<Option<usize>> = vec![None; runs.len()];
    let mut seen = std::collections::HashMap::<usize, usize>::new();
    for index in (0..runs.len()).rev() {
        next[index] = seen.get(&runs[index].1).copied();
        seen.insert(runs[index].1, index);
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut index = 0usize;
    while index < runs.len() {
        let (at, len) = runs[index];
        let closer = next[index];
        if closer.is_none() && len < 3 {
            index += 1;
            continue;
        }
        out.push_str(&text[cursor..at]);
        out.push('`');
        match closer {
            None => {
                cursor = text.len();
                break;
            }
            Some(c) => {
                cursor = runs[c].0 + runs[c].1;
                index = c + 1;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out
}

/// Mentions in a body as typed, outside code (the server's `parseMentions`).
pub fn parse_mentions(content: &str) -> ParsedMentions {
    names_of(
        mentions_in(&without_code(content))
            .into_iter()
            .map(|s| s.name),
    )
}

/// What a body mentions, as the server reads it when the message is written:
/// the prose names of its tree, or, for a body shown as typed, the names
/// outside code in the text as typed.
pub fn read_mentions(input: &str) -> ParsedMentions {
    match read_markdown(input) {
        Reading::Tree(tree) => names_of(prose_names(&tree)),
        Reading::Typed(_) => parse_mentions(&without_fenced_code(input)),
    }
}

/// The usernames a body mentions (distinct, as written, at most 50; broadcast
/// names are never included — see [`read_mentions`] for those).
pub fn mentions(input: &str) -> Vec<String> {
    read_mentions(input).names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        mentions_in(text).into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn spans_as_written() {
        assert_eq!(
            mentions_in("hi @Bob and @a-b."),
            vec![
                MentionSpan {
                    start: 3,
                    end: 7,
                    name: "Bob".into()
                },
                MentionSpan {
                    start: 12,
                    end: 16,
                    name: "a-b".into()
                },
            ]
        );
    }

    #[test]
    fn at_most_39() {
        assert_eq!(names(&format!("@{}", "a".repeat(39))), vec!["a".repeat(39)]);
        assert!(names(&format!("@{}", "a".repeat(40))).is_empty());
    }

    #[test]
    fn not_after() {
        for text in [
            "mail a@bob.example",
            "@@bob",
            "`x`@bob",
            "Ac@bob",
            "Ğ@bob",
            "1@bob",
            "_@bob",
        ] {
            assert!(names(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn hyphens_and_surroundings() {
        assert_eq!(names("@bob-"), vec!["bob"]);
        assert_eq!(names("@bob--x"), vec!["bob"]);
        assert!(names("@-bob").is_empty());
        assert!(names("@bobĞ").is_empty());
        assert_eq!(names("@bob-xĞ"), vec!["bob"]);
        assert_eq!(names("(**@bob**), @carol!"), vec!["bob", "carol"]);
        assert_eq!(names("@a @b"), vec!["a", "b"]);
    }

    #[test]
    fn body_mentions_skip_code_and_links() {
        assert_eq!(
            mentions("ping @ada and @bob-2, not @ here"),
            vec!["ada", "bob-2"]
        );
        assert!(mentions("`@ada`").is_empty());
        assert!(mentions("```\n@ada\n```").is_empty());
        assert!(mentions("~~~\n@ada\n~~~").is_empty());
        assert!(mentions("[@ada](https://example.com)").is_empty());
        assert!(mentions("`x`@ada").is_empty());
        assert_eq!(
            mentions("**@ada** _@bob_ ~~@cy~~"),
            vec!["ada", "bob", "cy"]
        );
        assert_eq!(
            mentions("> @ada\n\n- @bob\n\n| @cy |\n|---|"),
            vec!["ada", "bob", "cy"]
        );
    }

    #[test]
    fn distinct_case_insensitive_and_bounded() {
        assert_eq!(mentions("@Ada @ada @ADA @bob"), vec!["Ada", "bob"]);
        let many: String = (0..60).map(|i| format!("@u{i} ")).collect();
        assert_eq!(mentions(&many).len(), MENTIONS_PER_MESSAGE_MAX);
    }

    #[test]
    fn broadcasts_are_refused_not_names() {
        let read = read_mentions("@here @Channel @everyone @ada @here");
        assert_eq!(read.names, vec!["ada"]);
        assert_eq!(read.broadcast, vec!["here", "channel", "everyone"]);
    }

    #[test]
    fn typed_body_reads_outside_code() {
        let deep = format!("{} @ada `@bob` @cy\n```\n@dan\n```", ">".repeat(40));
        assert_eq!(mentions(&deep), vec!["ada", "cy"]);
        assert_eq!(
            parse_mentions("```\n@ada never closed").names,
            Vec::<String>::new()
        );
        assert_eq!(parse_mentions("a `` @ada").names, vec!["ada"]);
    }
}
