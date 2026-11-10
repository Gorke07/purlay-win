//! Schemas → serde types.
//!
//! Rules (kept in step with the macOS client's PurlayGen):
//! - A string enum keeps an `Unknown(String)` case: the server may add values.
//! - A union whose members share a constant property (`op`, `event`, `code`, …) is a
//!   tagged enum with an `Unknown(Value)` case; members that share a tag value are told
//!   apart by structure (the three `ACK` shapes).
//! - A required nullable field is `Option<T>`; an optional field is `Option<T>` that is
//!   left out when `None`; an optional *nullable* field is `Option<Option<T>>` so
//!   "leave it" (`None`) and "clear it" (`Some(None)`) differ.
//! - Unknown response fields are ignored (serde's default).

use crate::naming::{doc, field_ident, pascal, rust_str, shouty, snake};
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;

pub struct Output {
    pub models: String,
    pub schema_count: usize,
}

/// Schemas written by hand in `primitives.rs`.
const PRIMITIVES: &[&str] = &["Uuid", "Timestamp"];

/// Schemas whose generated shape would be worse than a hand-picked one.
fn override_for(name: &str) -> Option<&'static str> {
    match name {
        // A closed set of codes, each with its own `details`; a client reads the code and
        // treats `details` as data (it never shows the server's `message`).
        "AppErrorBody" => Some(
            "/// The one error envelope's body (`{ error: AppErrorBody }` over HTTP, `data` of an `ERROR` frame).\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub struct AppErrorBody {\n    pub code: ErrorCode,\n    #[serde(default)]\n    pub message: String,\n    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub details: Option<Value>,\n}\n",
        ),
        _ => None,
    }
}

struct Gen<'a> {
    schemas: &'a Map<String, Value>,
    out: String,
    /// Type names already taken (named schemas + generated anonymous ones).
    taken: BTreeSet<String>,
    queue: VecDeque<(String, Value)>,
}

#[derive(Clone)]
struct Disc {
    prop: String,
    /// (variant name, member schema (resolved), tag value, member type name)
    members: Vec<(String, Value, Value, String)>,
}

pub fn generate(contract: &Value) -> Result<Output> {
    let schemas = contract["schemas"].as_object().context("schemas")?;
    let mut g = Gen {
        schemas,
        out: String::new(),
        taken: schemas.keys().cloned().collect(),
        queue: VecDeque::new(),
    };
    g.out.push_str("#![allow(clippy::all, non_camel_case_types, unused_imports)]\n\n");
    g.out.push_str("use crate::{EmptyObject, JsonObject, Timestamp, Uuid};\nuse serde::{Deserialize, Serialize};\nuse serde_json::Value;\nuse std::collections::BTreeMap;\n\n");

    for (name, schema) in schemas {
        if PRIMITIVES.contains(&name.as_str()) {
            continue;
        }
        if let Some(code) = override_for(name) {
            g.out.push_str(code);
            g.out.push('\n');
            continue;
        }
        g.emit_named(name, schema)?;
        while let Some((n, s)) = g.queue.pop_front() {
            g.emit_named(&n, &s)?;
        }
    }
    Ok(Output { models: g.out, schema_count: schemas.len() })
}

impl<'a> Gen<'a> {
    fn resolve<'b>(&'b self, s: &'b Value) -> &'b Value {
        let mut cur = s;
        for _ in 0..16 {
            match cur.get("$ref").and_then(Value::as_str) {
                Some(r) => cur = &self.schemas[r],
                None => break,
            }
        }
        cur
    }

    fn fresh_name(&mut self, want: &str) -> String {
        let mut name = want.to_string();
        let mut i = 2;
        while self.taken.contains(&name) {
            name = format!("{want}{i}");
            i += 1;
        }
        self.taken.insert(name.clone());
        name
    }

    fn emit_named(&mut self, name: &str, schema: &Value) -> Result<()> {
        let description = schema.get("description").and_then(Value::as_str);
        let docs = description.map(|d| doc(d, "")).unwrap_or_default();

        if let Some(members) = schema.get("anyOf").and_then(Value::as_array) {
            let (members, nullable) = split_null(members);
            if nullable {
                let ty = self.type_of(schema, name)?;
                write!(self.out, "{docs}pub type {name} = {ty};\n\n")?;
                return Ok(());
            }
            if let Some(values) = self.string_consts(&members) {
                self.out.push_str(&docs);
                self.emit_string_enum(name, &values);
                return Ok(());
            }
            if let Some(disc) = self.discriminator(name, &members) {
                self.out.push_str(&docs);
                self.emit_tagged_union(name, &disc)?;
                return Ok(());
            }
            if members.len() == 1 {
                let ty = self.type_of(&members[0], name)?;
                write!(self.out, "{docs}pub type {name} = {ty};\n\n")?;
                return Ok(());
            }
            let ty = self.type_of(schema, name)?;
            write!(self.out, "{docs}pub type {name} = {ty};\n\n")?;
            return Ok(());
        }
        if is_object_with_props(schema) {
            self.emit_struct(name, schema, &docs, None)?;
            return Ok(());
        }
        // An alias for a primitive, an array or a map.
        let ty = self.type_of_inner(schema, &format!("{name}Item"))?;
        write!(self.out, "{docs}pub type {name} = {ty};\n\n")?;
        Ok(())
    }

    fn string_consts(&self, members: &[Value]) -> Option<Vec<String>> {
        let mut values = Vec::new();
        for m in members {
            let m = self.resolve(m);
            if let Some(inner) = m.get("anyOf").and_then(Value::as_array) {
                values.extend(self.string_consts(inner)?);
                continue;
            }
            values.push(m.get("const")?.as_str()?.to_string());
        }
        if values.is_empty() { None } else { Some(values) }
    }

    fn discriminator(&self, parent: &str, members: &[Value]) -> Option<Disc> {
        if members.len() < 2 {
            return None;
        }
        let resolved: Vec<&Value> = members.iter().map(|m| self.resolve(m)).collect();
        if !resolved.iter().all(|m| m.get("properties").is_some()) {
            return None;
        }
        let first = resolved[0]["properties"].as_object()?;
        let mut candidates: Vec<&String> = first
            .keys()
            .filter(|k| {
                resolved.iter().all(|m| {
                    m["properties"].get(k.as_str()).and_then(|p| p.get("const")).is_some()
                        && m["required"]
                            .as_array()
                            .is_some_and(|r| r.iter().any(|x| x.as_str() == Some(k.as_str())))
                })
            })
            .collect();
        let pref = ["event", "op", "code", "type", "kind", "status", "active", "state"];
        candidates.sort_by_key(|k| pref.iter().position(|p| p == k).unwrap_or(99));
        let prop = candidates.first()?.to_string();

        let mut used = BTreeSet::new();
        let strip = if parent.ends_with("Frame") { "Frame" } else { "" };
        let mut out = Vec::new();
        for (raw, m) in members.iter().zip(resolved.iter()) {
            let tag = m["properties"][&prop]["const"].clone();
            let mut variant = match raw.get("$ref").and_then(Value::as_str) {
                Some(r) => {
                    let base = r.strip_suffix(strip).filter(|s| !s.is_empty()).unwrap_or(r);
                    base.strip_prefix(parent.trim_end_matches("Frame")).filter(|s| !s.is_empty() && s.chars().next().unwrap().is_ascii_uppercase()).unwrap_or(base).to_string()
                }
                None => match &tag {
                    Value::String(s) => pascal(s),
                    Value::Bool(b) => format!("{}{}", pascal(&prop), if *b { "On" } else { "Off" }),
                    other => pascal(&other.to_string()),
                },
            };
            if variant == "Unknown" {
                variant = "UnknownValue".into();
            }
            while !used.insert(variant.clone()) {
                variant.push('2');
            }
            let ty = match raw.get("$ref").and_then(Value::as_str) {
                Some(r) => r.to_string(),
                None => String::new(),
            };
            out.push((variant, (*m).clone(), tag, ty));
        }
        Some(Disc { prop, members: out })
    }

    fn emit_string_enum(&mut self, name: &str, values: &[String]) {
        let mut variants: Vec<(String, String)> = Vec::new();
        let mut used = BTreeSet::new();
        for v in values {
            let mut id = pascal(v);
            if id == "Unknown" {
                id = "UnknownValue".into();
            }
            while !used.insert(id.clone()) {
                id.push('_');
            }
            variants.push((id, v.clone()));
        }
        let o = &mut self.out;
        writeln!(o, "#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]").ok();
        writeln!(o, "pub enum {name} {{").ok();
        for (id, v) in &variants {
            writeln!(o, "    /// `{v}`\n    {id},").ok();
        }
        writeln!(o, "    /// A value this build does not know (the server may add values).\n    Unknown(String),\n}}\n").ok();
        writeln!(o, "impl {name} {{").ok();
        writeln!(o, "    pub const ALL: &'static [{name}] = &[{}];", variants.iter().map(|(i, _)| format!("{name}::{i}")).collect::<Vec<_>>().join(", ")).ok();
        writeln!(o, "    pub fn as_str(&self) -> &str {{\n        match self {{").ok();
        for (id, v) in &variants {
            writeln!(o, "            {name}::{id} => {},", rust_str(v)).ok();
        }
        writeln!(o, "            {name}::Unknown(s) => s,\n        }}\n    }}").ok();
        writeln!(o, "    pub fn parse(s: &str) -> Self {{\n        match s {{").ok();
        for (id, v) in &variants {
            writeln!(o, "            {} => {name}::{id},", rust_str(v)).ok();
        }
        writeln!(o, "            other => {name}::Unknown(other.to_string()),\n        }}\n    }}\n}}\n").ok();
        writeln!(o, "impl std::fmt::Display for {name} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ f.write_str(self.as_str()) }}\n}}\n").ok();
        writeln!(o, "impl Serialize for {name} {{\n    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {{ s.serialize_str(self.as_str()) }}\n}}\n").ok();
        writeln!(o, "impl<'de> Deserialize<'de> for {name} {{\n    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {{\n        let s = std::borrow::Cow::<'de, str>::deserialize(d)?;\n        Ok({name}::parse(&s))\n    }}\n}}\n").ok();
    }

    fn emit_tagged_union(&mut self, name: &str, disc: &Disc) -> Result<()> {
        // Anonymous members become named structs first.
        let mut members = disc.members.clone();
        for m in members.iter_mut() {
            if m.3.is_empty() {
                let ty = self.fresh_name(&format!("{name}{}", m.0));
                self.emit_struct(&ty, &m.1, "", Some(&disc.prop))?;
                m.3 = ty;
            }
        }
        let o = &mut self.out;
        writeln!(o, "/// Tagged by `{}`.", disc.prop)?;
        writeln!(o, "#[derive(Debug, Clone, PartialEq)]\npub enum {name} {{")?;
        for (variant, _, tag, ty) in &members {
            writeln!(o, "    /// `{}: {}`\n    {variant}({ty}),", disc.prop, tag)?;
        }
        writeln!(o, "    /// A member this build does not know; kept whole.\n    Unknown(Value),\n}}\n")?;

        writeln!(o, "impl {name} {{\n    pub const TAG: &'static str = {};\n}}\n", rust_str(&disc.prop))?;

        // Deserialize: read the tag; the first member of that tag that reads wins.
        writeln!(o, "impl<'de> Deserialize<'de> for {name} {{")?;
        writeln!(o, "    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {{")?;
        writeln!(o, "        use serde::de::Error;\n        let v = Value::deserialize(d)?;\n        let tag = v.get({}).cloned().unwrap_or(Value::Null);", rust_str(&disc.prop))?;
        let mut by_tag: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        for (variant, _, tag, ty) in &members {
            let key = tag.to_string();
            if !by_tag.contains_key(&key) {
                order.push(key.clone());
            }
            by_tag.entry(key).or_default().push((variant.clone(), ty.clone()));
        }
        for key in &order {
            let list = &by_tag[key];
            writeln!(o, "        if tag == serde_json::json!({key}) {{")?;
            writeln!(o, "            let mut errs: Vec<String> = Vec::new();")?;
            for (variant, ty) in list {
                writeln!(o, "            match serde_json::from_value::<{ty}>(v.clone()) {{ Ok(x) => return Ok({name}::{variant}(x)), Err(e) => errs.push(e.to_string()) }}")?;
            }
            writeln!(o, "            return Err(D::Error::custom(format!(\"{name} {{}}: {{}}\", tag, errs.join(\"; \"))));\n        }}")?;
        }
        writeln!(o, "        Ok({name}::Unknown(v))\n    }}\n}}\n")?;

        // Serialize: the member's fields plus its tag.
        writeln!(o, "impl Serialize for {name} {{")?;
        writeln!(o, "    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {{")?;
        writeln!(o, "        use serde::ser::Error;\n        let (mut v, tag) = match self {{")?;
        for (variant, _, tag, _) in &members {
            writeln!(o, "            {name}::{variant}(x) => (serde_json::to_value(x).map_err(S::Error::custom)?, serde_json::json!({tag})),")?;
        }
        writeln!(o, "            {name}::Unknown(v) => return v.serialize(s),\n        }};")?;
        writeln!(o, "        if let Value::Object(m) = &mut v {{\n            m.insert({}.to_string(), tag);\n        }}\n        v.serialize(s)\n    }}\n}}\n", rust_str(&disc.prop))?;
        Ok(())
    }

    fn emit_struct(&mut self, name: &str, schema: &Value, docs: &str, skip_prop: Option<&str>) -> Result<()> {
        let props = schema["properties"].as_object().cloned().unwrap_or_default();
        let required: BTreeSet<String> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();

        let mut fields = String::new();
        let mut all_defaultable = true;
        for (json, ps) in &props {
            if skip_prop == Some(json.as_str()) {
                continue;
            }
            // A constant member of a tagged union is written by the union.
            if skip_prop.is_none() && ps.get("const").is_some() && self.is_union_tag(name, json) {
                continue;
            }
            let ident = field_ident(json);
            let ctx = format!("{name}{}", pascal(json));
            let (inner_schema, nullable) = strip_nullable(ps);
            let inner = self.type_of_inner(&inner_schema, &ctx)?;
            let is_required = required.contains(json);
            if let Some(d) = ps.get("description").and_then(Value::as_str) {
                fields.push_str(&doc(d, "    "));
            }
            let rename = if snake(json) != *json || ident.starts_with("r#") || field_ident(json) != *json {
                format!("rename = {}, ", rust_str(json))
            } else {
                String::new()
            };
            let (attr, ty) = match (is_required, nullable) {
                (true, false) => {
                    if !is_defaultable(&inner) {
                        all_defaultable = false;
                    }
                    // Required means required: structure is how same-tag members (the three
                    // `ACK` shapes) are told apart, so a missing array is not defaulted.
                    let a = if rename.is_empty() {
                        String::new()
                    } else {
                        format!("#[serde({})]", rename.trim_end_matches(", "))
                    };
                    (a, inner)
                }
                (true, true) => (format!("#[serde({rename}default)]"), format!("Option<{inner}>")),
                (false, false) => (
                    format!("#[serde({rename}default, skip_serializing_if = \"Option::is_none\")]"),
                    format!("Option<{inner}>"),
                ),
                (false, true) => (
                    format!(
                        "#[serde({rename}default, skip_serializing_if = \"Option::is_none\", with = \"crate::double_option\")]"
                    ),
                    format!("Option<Option<{inner}>>"),
                ),
            };
            if !attr.is_empty() {
                writeln!(fields, "    {attr}")?;
            }
            writeln!(fields, "    pub {ident}: {ty},")?;
        }
        let default = if all_defaultable { ", Default" } else { "" };
        write!(
            self.out,
            "{docs}#[derive(Debug, Clone, PartialEq, Serialize, Deserialize{default})]\npub struct {name} {{\n{fields}}}\n\n"
        )?;
        Ok(())
    }

    /// Whether `prop` is the tag of a union that names `struct_name` as a member.
    fn is_union_tag(&self, struct_name: &str, prop: &str) -> bool {
        for (uname, s) in self.schemas {
            let Some(members) = s.get("anyOf").and_then(Value::as_array) else { continue };
            if !members.iter().any(|m| m.get("$ref").and_then(Value::as_str) == Some(struct_name)) {
                continue;
            }
            let (members, _) = split_null(members);
            if self.string_consts(&members).is_some() {
                continue;
            }
            if let Some(d) = self.discriminator(uname, &members) {
                if d.prop == prop {
                    return true;
                }
            }
        }
        false
    }

    /// The Rust type of a property schema, including nullability.
    fn type_of(&mut self, s: &Value, ctx: &str) -> Result<String> {
        let (inner, nullable) = strip_nullable(s);
        let t = self.type_of_inner(&inner, ctx)?;
        Ok(if nullable { format!("Option<{t}>") } else { t })
    }

    fn type_of_inner(&mut self, s: &Value, ctx: &str) -> Result<String> {
        if let Some(r) = s.get("$ref").and_then(Value::as_str) {
            return Ok(r.to_string());
        }
        if let Some(members) = s.get("anyOf").and_then(Value::as_array) {
            let (members, nullable) = split_null(members);
            let t = if members.len() == 1 {
                self.type_of_inner(&members[0], ctx)?
            } else if let Some(values) = self.string_consts(&members) {
                let name = self.fresh_name(ctx);
                self.emit_string_enum(&name, &values);
                name
            } else if members.iter().all(|m| self.resolve(m).get("const").is_some_and(|c| c.is_i64())) {
                // `1 | 2 | 3 | 4`: a small closed set of integers.
                "i64".to_string()
            } else if members.iter().all(|m| self.resolve(m).get("type").and_then(Value::as_str) == Some("string")) {
                // e.g. `"" | 3..200 chars`: one string with two shapes.
                "String".to_string()
            } else if self.discriminator(ctx, &members).is_some() {
                let name = self.fresh_name(ctx);
                self.queue.push_back((name.clone(), Value::Object(
                    [("anyOf".to_string(), Value::Array(members.clone()))].into_iter().collect(),
                )));
                name
            } else {
                "Value".to_string()
            };
            return Ok(if nullable { format!("Option<{t}>") } else { t });
        }
        if let Some(c) = s.get("const") {
            return Ok(match c {
                Value::Bool(_) => "bool".into(),
                Value::Number(n) if n.is_i64() => "i64".into(),
                Value::Number(_) => "f64".into(),
                _ => "String".into(),
            });
        }
        let ty = s.get("type").and_then(Value::as_str).unwrap_or("");
        Ok(match ty {
            "string" => match s.get("format").and_then(Value::as_str) {
                Some("uuid") => "Uuid".into(),
                Some("date-time") => "Timestamp".into(),
                _ => "String".into(),
            },
            "integer" => "i64".into(),
            "number" => "f64".into(),
            "boolean" => "bool".into(),
            "null" => "()".into(),
            "array" => {
                let item = s.get("items").cloned().unwrap_or(Value::Null);
                let t = self.type_of(&item, &format!("{ctx}Item"))?;
                format!("Vec<{t}>")
            }
            "object" => {
                if is_object_with_props(s) {
                    let name = self.fresh_name(ctx);
                    self.queue.push_back((name.clone(), s.clone()));
                    name
                } else if let Some(pp) = s.get("patternProperties").and_then(Value::as_object) {
                    let v = pp.values().next().cloned().unwrap_or(Value::Null);
                    let t = self.type_of(&v, &format!("{ctx}Value"))?;
                    format!("BTreeMap<String, {t}>")
                } else if let Some(ap) = s.get("additionalProperties").filter(|v| v.is_object()) {
                    let t = self.type_of(ap, &format!("{ctx}Value"))?;
                    format!("BTreeMap<String, {t}>")
                } else if s.get("additionalProperties") == Some(&Value::Bool(false)) {
                    "EmptyObject".into()
                } else {
                    "JsonObject".into()
                }
            }
            _ => "Value".into(),
        })
    }
}

fn is_object_with_props(s: &Value) -> bool {
    s.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty())
}

fn split_null(members: &[Value]) -> (Vec<Value>, bool) {
    let mut nullable = false;
    let mut out = Vec::new();
    for m in members {
        if m.get("type").and_then(Value::as_str) == Some("null") {
            nullable = true;
        } else {
            out.push(m.clone());
        }
    }
    (out, nullable)
}

fn strip_nullable(s: &Value) -> (Value, bool) {
    if let Some(members) = s.get("anyOf").and_then(Value::as_array) {
        let (rest, nullable) = split_null(members);
        if nullable {
            let inner = if rest.len() == 1 {
                rest[0].clone()
            } else {
                Value::Object([("anyOf".to_string(), Value::Array(rest))].into_iter().collect())
            };
            return (inner, true);
        }
    }
    (s.clone(), false)
}

fn is_defaultable(ty: &str) -> bool {
    ty.starts_with("Option<")
        || ty.starts_with("Vec<")
        || ty.starts_with("BTreeMap<")
        || matches!(ty, "String" | "bool" | "i64" | "f64" | "Value" | "JsonObject" | "EmptyObject" | "Uuid" | "Timestamp")
}

/// `constants.rs`: the bundle's constants, error statuses and close codes.
pub fn constants(contract: &Value, lock: &crate::bundle::Lock) -> Result<String> {
    let mut o = String::new();
    writeln!(o, "#![allow(clippy::all)]\n")?;
    writeln!(o, "/// The server version this build's contract was taken from.\npub const PINNED_SERVER_VERSION: &str = {};", rust_str(&lock.server_version))?;
    writeln!(o, "/// SHA-256 of the pinned bundle's MANIFEST.json.\npub const PINNED_MANIFEST_SHA256: &str = {};\n", rust_str(&lock.manifest_sha256))?;
    let consts = contract["constants"].as_object().context("constants")?;
    for (k, v) in consts {
        emit_const(&mut o, k, v, "")?;
    }
    writeln!(o, "\n/// Every error `code` and the HTTP status it is answered with.\npub const ERROR_STATUS: &[(&str, u16)] = &[")?;
    for (k, v) in contract["errorStatus"].as_object().context("errorStatus")? {
        writeln!(o, "    ({}, {}),", rust_str(k), v)?;
    }
    writeln!(o, "];\n\npub fn error_status(code: &str) -> Option<u16> {{\n    ERROR_STATUS.iter().find(|(c, _)| *c == code).map(|(_, s)| *s)\n}}\n")?;
    writeln!(o, "/// The socket's application close codes.\npub mod close_codes {{")?;
    for (k, v) in contract["closeCodes"].as_object().context("closeCodes")? {
        writeln!(o, "    pub const {}: u16 = {};", shouty(k), v)?;
    }
    writeln!(o, "}}\n")?;
    writeln!(o, "/// Which module owns each server event (`core` or a module id).\npub const EVENT_OWNERS: &[(&str, &str)] = &[")?;
    for (k, v) in contract["eventOwners"].as_object().context("eventOwners")? {
        writeln!(o, "    ({}, {}),", rust_str(k), rust_str(v.as_str().unwrap_or("core")))?;
    }
    writeln!(o, "];\n")?;
    writeln!(o, "/// Which module owns each client operation.\npub const FRAME_OWNERS: &[(&str, &str)] = &[")?;
    for (k, v) in contract["frameOwners"].as_object().context("frameOwners")? {
        writeln!(o, "    ({}, {}),", rust_str(k), rust_str(v.as_str().unwrap_or("core")))?;
    }
    writeln!(o, "];")?;
    Ok(o)
}

fn emit_const(o: &mut String, k: &str, v: &Value, indent: &str) -> Result<()> {
    let name = if k.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()) { k.to_string() } else { shouty(k) };
    match v {
        Value::Number(n) if n.is_u64() || n.is_i64() => writeln!(o, "{indent}pub const {name}: i64 = {n};")?,
        Value::Number(n) => writeln!(o, "{indent}pub const {name}: f64 = {n};")?,
        Value::String(s) => writeln!(o, "{indent}pub const {name}: &str = {};", rust_str(s))?,
        Value::Bool(b) => writeln!(o, "{indent}pub const {name}: bool = {b};")?,
        Value::Array(a) => {
            if a.iter().all(|x| x.is_i64()) {
                writeln!(o, "{indent}pub const {name}: &[i64] = &[{}];", a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))?
            } else {
                writeln!(o, "{indent}pub const {name}: &[&str] = &[{}];", a.iter().map(|x| rust_str(x.as_str().unwrap_or_default())).collect::<Vec<_>>().join(", "))?
            }
        }
        Value::Object(m) => {
            writeln!(o, "{indent}pub mod {} {{", snake(k))?;
            for (k2, v2) in m {
                emit_const(o, k2, v2, &format!("{indent}    "))?;
            }
            writeln!(o, "{indent}}}")?;
        }
        Value::Null => {}
    }
    Ok(())
}
