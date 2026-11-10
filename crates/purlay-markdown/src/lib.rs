//! Purlay's message markdown, read exactly as the server's `@purlay/markdown`
//! reads it (D-101): CommonMark through micromark (here its Rust port,
//! `markdown-rs`) with GFM autolink literals, strikethrough, tables and task
//! lists; no raw HTML, definitions/references, images, indented code, setext
//! headings, character references or footnotes; then narrowed to the node
//! kinds the product renders, with link addresses WHATWG-normalised and
//! restricted to http, https and mailto.
//!
//! [`read_markdown`] is the entry point; the tree serialises (serde) to the
//! server's JSON byte for byte.

mod engine;
mod href;
mod mentions;
mod parse;
mod plain;
mod scan;
mod tree;

pub use engine::ends_in_open_code;
pub use href::safe_href;
pub use mentions::{
    BROADCAST_NAMES, MENTIONS_PER_MESSAGE_MAX, MentionSpan, ParsedMentions, mentions, mentions_in,
    names_of, parse_mentions, prose_names, read_mentions,
};
pub use parse::{
    MARKDOWN_DEPTH_MAX, Reading, TypedReason, parse_markdown, read_markdown, without_fenced_code,
};
pub use plain::{excerpt_of, plain_text};
pub use scan::{MARKDOWN_DELIMITERS_MAX, MARKDOWN_NESTING_MAX, MARKDOWN_RUN_MAX, nests_too_deep};
pub use tree::{Align, Block, Inline, ListItem, Root, TableCell, TableRow};
