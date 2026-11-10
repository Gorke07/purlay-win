//! The endpoint table (purlay-contract) and one typed method per route (purlay-core).

use crate::naming::{doc, rust_str, shouty, snake};
use anyhow::{Context, Result};
use serde_json::Value;
use std::fmt::Write;

fn path_params(path: &str) -> Vec<String> {
    path.split('/').filter_map(|seg| seg.strip_prefix(':').map(String::from)).collect()
}

fn ref_name(v: &Value) -> Option<String> {
    v.get("$ref").and_then(Value::as_str).map(String::from)
}

enum Resp {
    Json(String),
    Empty,
    Binary,
}

fn response(e: &Value) -> Result<Resp> {
    let responses = e["responses"].as_object().context("responses")?;
    let mut json: Option<String> = None;
    let mut empty = false;
    let mut binary = false;
    for (_status, r) in responses {
        match r {
            Value::Null => empty = true,
            Value::String(s) if s == "binary" => binary = true,
            other => {
                let n = ref_name(other).context("response $ref")?;
                if let Some(prev) = &json {
                    anyhow::ensure!(*prev == n, "{}: two response types", e["id"]);
                }
                json = Some(n);
            }
        }
    }
    Ok(if binary {
        Resp::Binary
    } else if let Some(j) = json {
        Resp::Json(j)
    } else {
        let _ = empty;
        Resp::Empty
    })
}

pub fn endpoints(contract: &Value) -> Result<String> {
    let eps = contract["endpoints"].as_array().context("endpoints")?;
    let mut o = String::new();
    writeln!(o, "//! Every route of the pinned contract.\n")?;
    writeln!(o, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Method {{ Get, Post, Put, Patch, Delete }}\n")?;
    writeln!(o, "impl Method {{\n    pub fn as_str(self) -> &'static str {{\n        match self {{ Method::Get => \"GET\", Method::Post => \"POST\", Method::Put => \"PUT\", Method::Patch => \"PATCH\", Method::Delete => \"DELETE\" }}\n    }}\n}}\n")?;
    writeln!(o, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Auth {{ None, Session }}\n")?;
    writeln!(o, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct Endpoint {{\n    pub id: &'static str,\n    pub method: Method,\n    /// Template with `:name` parameters.\n    pub path: &'static str,\n    /// `core` or the module that owns the route (a module that is off answers 404 ROUTE_NOT_FOUND).\n    pub module: &'static str,\n    pub auth: Auth,\n    /// Safe to retry on a transient failure.\n    pub idempotent: bool,\n}}\n")?;
    for e in eps {
        let id = e["id"].as_str().context("id")?;
        let method = match e["method"].as_str().context("method")? {
            "GET" => "Get",
            "POST" => "Post",
            "PUT" => "Put",
            "PATCH" => "Patch",
            "DELETE" => "Delete",
            m => anyhow::bail!("method {m}"),
        };
        let auth = if e["auth"].as_str() == Some("none") { "None" } else { "Session" };
        // GETs are safe; `sendMessage` is idempotent on its client-made id (D-33).
        let idempotent = method == "Get" || id == "sendMessage";
        writeln!(
            o,
            "pub const {}: Endpoint = Endpoint {{ id: {}, method: Method::{method}, path: {}, module: {}, auth: Auth::{auth}, idempotent: {idempotent} }};",
            shouty(id),
            rust_str(id),
            rust_str(e["path"].as_str().unwrap_or_default()),
            rust_str(e["module"].as_str().unwrap_or("core")),
        )?;
    }
    writeln!(o, "\npub const ALL: &[Endpoint] = &[")?;
    for e in eps {
        writeln!(o, "    {},", shouty(e["id"].as_str().unwrap()))?;
    }
    writeln!(o, "];")?;
    Ok(o)
}

pub fn client_methods(contract: &Value) -> Result<String> {
    let eps = contract["endpoints"].as_array().context("endpoints")?;
    let mut o = String::new();
    writeln!(o, "//! One typed method per route of the pinned contract.\n")?;
    writeln!(o, "#![allow(clippy::all, unused_imports)]\n")?;
    writeln!(o, "use crate::api::{{ApiClient, ApiResult}};\nuse purlay_contract::endpoints as ep;\nuse purlay_contract::models::*;\nuse purlay_contract::*;\n")?;
    writeln!(o, "impl ApiClient {{")?;
    for e in eps {
        let id = e["id"].as_str().context("id")?;
        let path = e["path"].as_str().context("path")?;
        let params = path_params(path);
        let query = e.get("query").and_then(ref_name);
        let body = e.get("body");
        let fn_name = snake(id);
        let mut args = Vec::new();
        for p in &params {
            args.push(format!("{}: &str", snake(p)));
        }
        if let Some(q) = &query {
            args.push(format!("query: &{q}"));
        }
        let body_kind = match body {
            Some(Value::String(s)) if s == "octet-stream" => {
                args.push("bytes: Vec<u8>".into());
                "bytes"
            }
            Some(b) if ref_name(b).is_some() => {
                args.push(format!("body: &{}", ref_name(b).unwrap()));
                "json"
            }
            _ => "none",
        };
        let resp = response(e)?;
        let ret = match &resp {
            Resp::Json(t) => t.clone(),
            Resp::Empty => "()".into(),
            Resp::Binary => "crate::api::Bytes".into(),
        };
        let params_arr = format!("&[{}]", params.iter().map(|p| snake(p)).collect::<Vec<_>>().join(", "));
        let query_arg = if query.is_some() { "Some(crate::api::to_query(query)?)" } else { "None" };
        let summary = format!("`{} {}` ({})", e["method"].as_str().unwrap(), path, id);
        o.push_str(&doc(&summary, "    "));
        writeln!(o, "    pub async fn {fn_name}(&self, {}) -> ApiResult<{ret}> {{", args.join(", "))?;
        let call = match (&resp, body_kind) {
            (_, "bytes") => format!("self.call_upload(&ep::{}, {params_arr}, {query_arg}, bytes).await", shouty(id)),
            (Resp::Binary, _) => format!("self.call_bytes(&ep::{}, {params_arr}, {query_arg}).await", shouty(id)),
            (Resp::Empty, "json") => format!("self.call_empty(&ep::{}, {params_arr}, {query_arg}, Some(crate::api::to_body(body)?)).await", shouty(id)),
            (Resp::Empty, _) => format!("self.call_empty(&ep::{}, {params_arr}, {query_arg}, None).await", shouty(id)),
            (Resp::Json(_), "json") => format!("self.call_json(&ep::{}, {params_arr}, {query_arg}, Some(crate::api::to_body(body)?)).await", shouty(id)),
            (Resp::Json(_), _) => format!("self.call_json(&ep::{}, {params_arr}, {query_arg}, None).await", shouty(id)),
        };
        writeln!(o, "        {call}\n    }}\n")?;
    }
    writeln!(o, "}}")?;
    Ok(o)
}
