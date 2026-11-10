//! The HTTP client every route goes through.
//!
//! - The session is a cookie value sent as an explicit `Cookie` header; reqwest's cookie
//!   jar is never used, so nothing else can add or keep a cookie.
//! - Every request says who it is: `X-Purlay-Client: windows/<semver> contract/<n>`.
//! - No `Origin` header: the server's CSRF guard allows a request with neither `Origin`
//!   nor `Sec-Fetch-Site`, which is what a non-browser client is.
//! - Bodies always carry `Content-Length` (the server answers 411 otherwise).
//! - GETs and the idempotent `sendMessage` retry on 502/503/504 and transport errors.

use parking_lot::RwLock;
use purlay_contract::endpoints::{Auth, Endpoint, Method};
use purlay_contract::models::{AppErrorBody, ErrorCode, ErrorResponse};
use purlay_contract::CONTRACT_VERSION;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

pub type Bytes = bytes::Bytes;
pub type ApiResult<T> = Result<T, ApiError>;

pub const SESSION_COOKIE: &str = "__Host-purlay_session";
pub const PLATFORM: &str = "windows";

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    /// The server answered with its error envelope.
    #[error("{status} {}", body.code)]
    Server { status: u16, body: AppErrorBody, retry_after: Option<Duration> },
    /// A non-2xx answer without the envelope (a proxy, an outage page).
    #[error("HTTP {status}")]
    Http { status: u16, body: String },
    /// The request did not reach the server or the answer did not arrive.
    #[error("network: {0}")]
    Transport(String),
    /// The answer did not have the contract's shape.
    #[error("unexpected answer: {0}")]
    Decode(String),
    /// A request that could not be built (a bad argument).
    #[error("bad request: {0}")]
    Request(String),
}

impl ApiError {
    pub fn code(&self) -> Option<&ErrorCode> {
        match self {
            ApiError::Server { body, .. } => Some(&body.code),
            _ => None,
        }
    }
    pub fn status(&self) -> Option<u16> {
        match self {
            ApiError::Server { status, .. } | ApiError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
    pub fn is(&self, code: ErrorCode) -> bool {
        self.code() == Some(&code)
    }
    pub fn is_unauthenticated(&self) -> bool {
        self.status() == Some(401)
    }
    /// A module that is switched off answers its routes with 404 ROUTE_NOT_FOUND.
    pub fn is_route_not_found(&self) -> bool {
        self.is(ErrorCode::RouteNotFound)
    }
    /// Worth trying again later without changing anything.
    pub fn is_transient(&self) -> bool {
        match self {
            ApiError::Transport(_) => true,
            ApiError::Server { status, .. } | ApiError::Http { status, .. } => matches!(status, 429 | 502 | 503 | 504),
            _ => false,
        }
    }
}

pub type SessionRejected = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    http: reqwest::Client,
    origin: url::Url,
    client_header: String,
    session: RwLock<Option<String>>,
    on_session_rejected: RwLock<Option<SessionRejected>>,
}

/// One server's HTTP client. Cheap to clone.
#[derive(Clone)]
pub struct ApiClient {
    inner: Arc<Inner>,
}

impl ApiClient {
    pub fn new(origin: url::Url, app_version: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("Purlay-Windows/{app_version}"))
            .build()
            .expect("reqwest client");
        ApiClient {
            inner: Arc::new(Inner {
                http,
                origin,
                client_header: client_header(app_version),
                session: RwLock::new(None),
                on_session_rejected: RwLock::new(None),
            }),
        }
    }

    pub fn origin(&self) -> &url::Url {
        &self.inner.origin
    }

    pub fn client_header(&self) -> &str {
        &self.inner.client_header
    }

    pub fn set_session(&self, cookie_value: Option<String>) {
        *self.inner.session.write() = cookie_value;
    }

    pub fn session(&self) -> Option<String> {
        self.inner.session.read().clone()
    }

    pub fn has_session(&self) -> bool {
        self.inner.session.read().is_some()
    }

    /// Called once when a session route answers 401.
    pub fn on_session_rejected(&self, f: SessionRejected) {
        *self.inner.on_session_rejected.write() = Some(f);
    }

    pub fn cookie_header(&self) -> Option<String> {
        self.session().map(|v| format!("{SESSION_COOKIE}={v}"))
    }

    /// An absolute URL on this server.
    pub fn url(&self, path: &str) -> url::Url {
        self.inner.origin.join(path).expect("joinable path")
    }

    pub fn build_path(ep: &Endpoint, params: &[&str]) -> ApiResult<String> {
        let mut out = String::new();
        let mut it = params.iter();
        for seg in ep.path.split('/').skip(1) {
            out.push('/');
            if seg.starts_with(':') {
                let v = it.next().ok_or_else(|| ApiError::Request(format!("{}: missing {seg}", ep.id)))?;
                if v.is_empty() {
                    return Err(ApiError::Request(format!("{}: empty {seg}", ep.id)));
                }
                out.push_str(&encode_segment(v));
            } else {
                out.push_str(seg);
            }
        }
        Ok(out)
    }

    async fn send_once(
        &self,
        ep: &Endpoint,
        path: &str,
        query: Option<&[(String, String)]>,
        body: Option<(Vec<u8>, &'static str)>,
    ) -> ApiResult<reqwest::Response> {
        let mut url = self.url(path);
        if let Some(q) = query {
            if !q.is_empty() {
                let mut pairs = url.query_pairs_mut();
                for (k, v) in q {
                    pairs.append_pair(k, v);
                }
            }
        }
        let method = match ep.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut req = self
            .inner
            .http
            .request(method.clone(), url)
            .header("X-Purlay-Client", &self.inner.client_header)
            .header("Accept", "application/json")
            .header("x-request-id", uuid::Uuid::new_v4().to_string());
        if ep.auth == Auth::Session || ep.path.starts_with("/auth/") {
            if let Some(c) = self.cookie_header() {
                req = req.header("Cookie", c);
            }
        }
        req = match body {
            Some((bytes, content_type)) => req
                .header("Content-Type", content_type)
                .header("Content-Length", bytes.len().to_string())
                .body(bytes),
            None if method != reqwest::Method::GET && method != reqwest::Method::DELETE => {
                req.header("Content-Length", "0")
            }
            None => req,
        };
        req.send().await.map_err(|e| ApiError::Transport(describe(&e)))
    }

    /// Sends a request, retrying idempotent ones, and turns a non-2xx answer into an error.
    pub async fn send(
        &self,
        ep: &Endpoint,
        params: &[&str],
        query: Option<Vec<(String, String)>>,
        body: Option<(Vec<u8>, &'static str)>,
    ) -> ApiResult<reqwest::Response> {
        let path = Self::build_path(ep, params)?;
        let attempts = if ep.idempotent { 3 } else { 1 };
        let mut last = None;
        for attempt in 0..attempts {
            if attempt > 0 {
                let base = [1000u64, 2000][(attempt - 1).min(1)];
                let jitter = rand::random::<f64>() * 0.25;
                tokio::time::sleep(Duration::from_millis((base as f64 * (1.0 + jitter)) as u64)).await;
            }
            let res = self.send_once(ep, &path, query.as_deref(), body.clone()).await;
            match res {
                Ok(r) if r.status().is_success() => return Ok(r),
                Ok(r) => {
                    let err = error_from(r).await;
                    let retry = matches!(err.status(), Some(502..=504));
                    if err.is_unauthenticated() && ep.auth == Auth::Session {
                        if let Some(f) = self.inner.on_session_rejected.read().clone() {
                            f();
                        }
                    }
                    if !retry {
                        return Err(err);
                    }
                    last = Some(err);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| ApiError::Transport("no attempt".into())))
    }

    pub async fn call_json<R: DeserializeOwned>(
        &self,
        ep: &Endpoint,
        params: &[&str],
        query: Option<Vec<(String, String)>>,
        body: Option<Vec<u8>>,
    ) -> ApiResult<R> {
        let r = self.send(ep, params, query, body.map(|b| (b, "application/json"))).await?;
        let bytes = r.bytes().await.map_err(|e| ApiError::Transport(describe(&e)))?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(format!("{}: {e}", ep.id)))
    }

    pub async fn call_empty(
        &self,
        ep: &Endpoint,
        params: &[&str],
        query: Option<Vec<(String, String)>>,
        body: Option<Vec<u8>>,
    ) -> ApiResult<()> {
        self.send(ep, params, query, body.map(|b| (b, "application/json"))).await?;
        Ok(())
    }

    pub async fn call_bytes(&self, ep: &Endpoint, params: &[&str], query: Option<Vec<(String, String)>>) -> ApiResult<Bytes> {
        let r = self.send(ep, params, query, None).await?;
        r.bytes().await.map_err(|e| ApiError::Transport(describe(&e)))
    }

    pub async fn call_upload<R: DeserializeOwned>(
        &self,
        ep: &Endpoint,
        params: &[&str],
        query: Option<Vec<(String, String)>>,
        bytes: Vec<u8>,
    ) -> ApiResult<R> {
        let r = self.send(ep, params, query, Some((bytes, "application/octet-stream"))).await?;
        let body = r.bytes().await.map_err(|e| ApiError::Transport(describe(&e)))?;
        serde_json::from_slice(&body).map_err(|e| ApiError::Decode(format!("{}: {e}", ep.id)))
    }
}

pub fn client_header(app_version: &str) -> String {
    format!("{PLATFORM}/{app_version} contract/{CONTRACT_VERSION}")
}

/// Percent-encodes a path segment so only unreserved characters stay as typed.
pub fn encode_segment(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

/// A query struct → sorted `key=value` pairs; absent and null values are left out.
pub fn to_query<T: Serialize>(q: &T) -> ApiResult<Vec<(String, String)>> {
    let v = serde_json::to_value(q).map_err(|e| ApiError::Request(e.to_string()))?;
    let mut out = Vec::new();
    if let serde_json::Value::Object(m) = v {
        for (k, v) in m {
            let s = match v {
                serde_json::Value::Null => continue,
                serde_json::Value::String(s) => s,
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            out.push((k, s));
        }
    }
    out.sort();
    Ok(out)
}

pub fn to_body<T: Serialize>(b: &T) -> ApiResult<Vec<u8>> {
    serde_json::to_vec(b).map_err(|e| ApiError::Request(e.to_string()))
}

async fn error_from(r: reqwest::Response) -> ApiError {
    let status = r.status().as_u16();
    let retry_after = r
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let text = r.text().await.unwrap_or_default();
    match serde_json::from_str::<ErrorResponse>(&text) {
        Ok(env) => ApiError::Server { status, body: env.error, retry_after },
        Err(_) => {
            // `/auth/*` answers in Better Auth's own `{ code, message }` shape.
            if let Ok(body) = serde_json::from_str::<AppErrorBody>(&text) {
                return ApiError::Server { status, body, retry_after };
            }
            ApiError::Http { status, body: text.chars().take(500).collect() }
        }
    }
}

fn describe(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        s.push_str(": ");
        s.push_str(&inner.to_string());
        src = inner.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use purlay_contract::endpoints;

    #[test]
    fn header_shape() {
        assert_eq!(client_header("0.1.0"), "windows/0.1.0 contract/1");
    }

    #[test]
    fn path_params_are_encoded() {
        let p = ApiClient::build_path(&endpoints::PUT_REACTION, &["0190abcd-0000-7000-8000-000000000001", "👍"]).unwrap();
        assert_eq!(p, "/api/messages/0190abcd-0000-7000-8000-000000000001/reactions/%F0%9F%91%8D");
        assert!(ApiClient::build_path(&endpoints::GET_MESSAGE, &[]).is_err());
    }

    #[test]
    fn query_is_sorted_and_skips_absent() {
        let q = purlay_contract::models::PageQuery { before: None, after: Some("b".into()), limit: Some(50) };
        assert_eq!(to_query(&q).unwrap(), vec![("after".into(), "b".into()), ("limit".into(), "50".into())]);
    }
}
