//! The narrowed tree a message renders from, mirroring the server's
//! `packages/markdown/src/tree.ts` node for node and field for field.
//!
//! Serialising any of these with `serde` gives exactly the JSON shape of the
//! server's tree (and of the conformance corpus).

use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};

/// Inline (phrasing) content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Inline {
    Text {
        value: String,
    },
    InlineCode {
        value: String,
    },
    Emphasis {
        children: Vec<Inline>,
    },
    Strong {
        children: Vec<Inline>,
    },
    Delete {
        children: Vec<Inline>,
    },
    /// `url` is an address `safe_href` allowed (http, https or mailto), as
    /// the WHATWG URL parser serialises it.
    Link {
        url: String,
        children: Vec<Inline>,
    },
    Break,
}

/// Column alignment of a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Left,
    Right,
    Center,
}

/// A list item; `checked` is a task list item's box, `None` for an ordinary item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub children: Vec<Block>,
}

/// A table cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    pub children: Vec<Inline>,
}

/// A table row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    pub children: Vec<TableCell>,
}

/// Block (flow) content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph {
        children: Vec<Inline>,
    },
    /// `level` is 1, 2 or 3: `###` to `######` are all level 3.
    Heading {
        level: u8,
        children: Vec<Inline>,
    },
    /// Only a fence makes one; `lang` is the info string's first word.
    Code {
        lang: Option<String>,
        value: String,
    },
    Blockquote {
        children: Vec<Block>,
    },
    /// `start` is `Some` exactly when `ordered` is true.
    List {
        ordered: bool,
        start: Option<u32>,
        children: Vec<ListItem>,
    },
    /// The first row is the header.
    Table {
        align: Vec<Option<Align>>,
        children: Vec<TableRow>,
    },
    ThematicBreak,
}

/// The tree's root.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Root {
    pub children: Vec<Block>,
}

impl Serialize for ListItem {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("ListItem", 3)?;
        st.serialize_field("type", "listItem")?;
        st.serialize_field("checked", &self.checked)?;
        st.serialize_field("children", &self.children)?;
        st.end()
    }
}

impl Serialize for TableCell {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("TableCell", 2)?;
        st.serialize_field("type", "tableCell")?;
        st.serialize_field("children", &self.children)?;
        st.end()
    }
}

impl Serialize for TableRow {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("TableRow", 2)?;
        st.serialize_field("type", "tableRow")?;
        st.serialize_field("children", &self.children)?;
        st.end()
    }
}

impl Serialize for Root {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Root", 2)?;
        st.serialize_field("type", "root")?;
        st.serialize_field("children", &self.children)?;
        st.end()
    }
}

impl Serialize for Block {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        match self {
            Block::Paragraph { children } => {
                m.serialize_entry("type", "paragraph")?;
                m.serialize_entry("children", children)?;
            }
            Block::Heading { level, children } => {
                m.serialize_entry("type", "heading")?;
                m.serialize_entry("level", level)?;
                m.serialize_entry("children", children)?;
            }
            Block::Code { lang, value } => {
                m.serialize_entry("type", "code")?;
                m.serialize_entry("lang", lang)?;
                m.serialize_entry("value", value)?;
            }
            Block::Blockquote { children } => {
                m.serialize_entry("type", "blockquote")?;
                m.serialize_entry("children", children)?;
            }
            Block::List {
                ordered,
                start,
                children,
            } => {
                m.serialize_entry("type", "list")?;
                m.serialize_entry("ordered", ordered)?;
                if *ordered {
                    m.serialize_entry("start", &start.unwrap_or(1))?;
                }
                m.serialize_entry("children", children)?;
            }
            Block::Table { align, children } => {
                m.serialize_entry("type", "table")?;
                m.serialize_entry("align", align)?;
                m.serialize_entry("children", children)?;
            }
            Block::ThematicBreak => {
                m.serialize_entry("type", "thematicBreak")?;
            }
        }
        m.end()
    }
}

impl Root {
    /// The tree as the server's JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("the tree always serialises")
    }
}
