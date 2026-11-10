//! The HTTP plumbing: shared state, the session extractor, query and body readers that
//! answer in the contract's error envelope, the edge checks (411/413/415) and the router.

use crate::error::{AppError, AppResult};
use crate::world::{SESSION_COOKIE, SessionInfo, World};
use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Router, middleware};
use parking_lot::Mutex;
use purlay_contract::Uuid;
use purlay_contract::constants::ATTACHMENT_BYTES_MAX;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// JSON bodies are bounded like a socket frame (`JSON_BODY_MAX_BYTES`).
pub const JSON_BODY_MAX: u64 = 65_536;

pub struct App {
    pub world: Mutex<World>,
    /// `http://127.0.0.1:<port>`: `appUrl`, and the `Origin` the socket upgrade must carry.
    pub origin: String,
    /// Sign the native sign-in page straight in as this person (no choosing).
    pub auto_sign_in: Option<String>,
}

pub type Shared = Arc<App>;

// --- responses ---------------------------------------------------------------------------

pub fn json_status<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => (status, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(e) => AppError::new("INTERNAL", format!("could not serialize: {e}")).into_response(),
    }
}

pub fn ok<T: Serialize>(value: &T) -> AppResult<Response> {
    Ok(json_status(StatusCode::OK, value))
}

pub fn created<T: Serialize>(value: &T) -> AppResult<Response> {
    Ok(json_status(StatusCode::CREATED, value))
}

pub fn no_content() -> AppResult<Response> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- the session -------------------------------------------------------------------------

/// The value of `__Host-purlay_session` in the request's `Cookie` headers.
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

/// A signed-in request; `401 UNAUTHENTICATED` otherwise.
#[derive(Debug, Clone)]
pub struct Me {
    pub user: Uuid,
    pub session: SessionInfo,
}

impl FromRequestParts<Shared> for Me {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, AppError> {
        let token = session_cookie(&parts.headers).ok_or_else(AppError::unauthenticated)?;
        let info = app.world.lock().session_for_token(&token).ok_or_else(AppError::unauthenticated)?;
        Ok(Me { user: info.user.clone(), session: info })
    }
}

// --- query and body ------------------------------------------------------------------------

/// A query string read into a contract type. A parameter given twice, or one that does not
/// parse, is `400 VALIDATION_FAILED`.
pub struct Q<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for Q<T> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, AppError> {
        let raw = parts.uri.query().unwrap_or("");
        let mut seen: Vec<String> = Vec::new();
        for (k, _) in serde_urlencoded::from_str::<Vec<(String, String)>>(raw).unwrap_or_default() {
            if seen.contains(&k) {
                return Err(AppError::field(&k, "is given twice"));
            }
            seen.push(k);
        }
        serde_urlencoded::from_str::<T>(raw)
            .map(Q)
            .map_err(|e| AppError::validation(format!("The query is not valid: {e}")))
    }
}

/// A JSON body read into a contract type; `400 VALIDATION_FAILED` when it is not that.
pub struct JsonBody<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for JsonBody<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, AppError> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|_| AppError::validation("The body could not be read."))?;
        if bytes.is_empty() {
            return Err(AppError::validation("A JSON body is required."));
        }
        serde_json::from_slice::<T>(&bytes)
            .map(JsonBody)
            .map_err(|e| AppError::validation(format!("The body is not valid: {}", e.to_string().split(" at line").next().unwrap_or(""))))
    }
}

// --- the edge --------------------------------------------------------------------------------

fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .unwrap_or_default()
}

pub fn is_upload_path(path: &str) -> bool {
    path.starts_with("/api/workspaces/") && path.ends_with("/uploads")
}

/// What the real server's edge refuses before a route runs: a body with no length (411),
/// a body that is not JSON (415) or too large (413); uploads are octet-stream.
async fn edge(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let writes = matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
    if writes && (path.starts_with("/api/") || path.starts_with("/auth/")) {
        let headers = req.headers();
        let length = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        let chunked = headers.contains_key(header::TRANSFER_ENCODING);
        if is_upload_path(&path) {
            let Some(length) = length else {
                return AppError::new("LENGTH_REQUIRED", "Send a Content-Length.").into_response();
            };
            if content_type(headers) != "application/octet-stream" {
                return AppError::new("UNSUPPORTED_MEDIA_TYPE", "Send the file as application/octet-stream.").into_response();
            }
            if length > ATTACHMENT_BYTES_MAX as u64 {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            }
        } else {
            if length.is_none() && chunked {
                return AppError::new("LENGTH_REQUIRED", "Send a Content-Length.").into_response();
            }
            if length.unwrap_or(0) > 0 {
                if content_type(headers) != "application/json" {
                    return AppError::new("UNSUPPORTED_MEDIA_TYPE", "Send JSON.").into_response();
                }
                if length.unwrap_or(0) > JSON_BODY_MAX {
                    return AppError::new("PAYLOAD_TOO_LARGE", "The body is too large.").into_response();
                }
            }
        }
    }
    let started = std::time::Instant::now();
    let res = next.run(req).await;
    tracing::debug!(target: "purlay_devserver::http", "{method} {path} → {} ({} ms)", res.status().as_u16(), started.elapsed().as_millis());
    res
}

async fn route_not_found() -> Response {
    AppError::route_not_found().into_response()
}

// --- the router ------------------------------------------------------------------------------

pub fn router(app: Shared) -> Router {
    use crate::routes::{base, chat};
    let api = Router::new()
        // core
        .route("/api/client", get(base::client_info))
        .route("/api/auth/methods", get(base::auth_methods))
        .route("/api/native/authorize", post(base::native_authorize))
        .route("/api/native/token", post(base::native_token))
        .route("/auth/logout", post(base::logout))
        .route("/auth/logout-all", post(base::logout_all))
        .route("/api/me", get(base::me))
        .route("/api/me/away", put(base::set_away))
        .route("/api/me/sessions", get(base::list_sessions))
        .route("/api/me/sessions/{session_id}", axum::routing::delete(base::end_session))
        .route("/api/releases/latest", get(base::latest_release))
        .route("/api/workspaces", get(base::list_workspaces))
        .route("/api/workspaces/{w}", get(base::get_workspace))
        .route("/api/workspaces/{w}/members", get(base::list_members))
        .route("/api/workspaces/{w}/presence", get(base::presence))
        .route("/api/workspaces/{w}/deep-work", put(base::set_deep_work))
        .route("/api/workspaces/{w}/inbox", get(base::list_inbox))
        .route("/api/workspaces/{w}/inbox/read", post(base::mark_inbox_read))
        .route("/api/workspaces/{w}/find", get(base::find))
        .route("/api/workspaces/{w}/uploads", post(base::upload))
        .route("/api/attachments/{a}", get(base::download).delete(base::discard))
        .route("/api/attachments/{a}/thumbnail", get(base::thumbnail))
        .route("/api/images", get(base::external_image))
        .route("/api/workspaces/{w}/issues", get(base::list_issues).post(base::create_issue))
        .route("/api/workspaces/{w}/issues/{i}", axum::routing::patch(base::update_issue).delete(base::delete_issue))
        // chat: channels
        .route("/api/workspaces/{w}/channels", get(chat::list_channels).post(chat::create_channel))
        .route("/api/workspaces/{w}/conversations", post(chat::open_conversation))
        .route("/api/workspaces/{w}/dms", post(chat::open_dm))
        .route("/api/workspaces/{w}/read", put(chat::mark_all_read))
        .route("/api/workspaces/{w}/channel-groups", get(chat::list_groups))
        .route(
            "/api/workspaces/{w}/channel-groups/{g}",
            put(chat::save_group).patch(chat::fold_group).delete(chat::remove_group),
        )
        .route("/api/channels/{c}", get(chat::get_channel))
        .route("/api/channels/{c}/join", post(chat::join))
        .route("/api/channels/{c}/leave", post(chat::leave))
        .route("/api/channels/{c}/members", get(chat::channel_members).post(chat::add_channel_member))
        .route("/api/channels/{c}/read", put(chat::mark_read))
        .route("/api/channels/{c}/unread", put(chat::mark_unread))
        .route("/api/channels/{c}/star", put(chat::star).delete(chat::unstar))
        .route("/api/channels/{c}/sidebar-group", put(chat::move_to_group))
        .route("/api/channels/{c}/notification-level", put(chat::set_level))
        .route("/api/channels/{c}/pins", get(chat::list_pins))
        .route("/api/channels/{c}/bookmarks", get(chat::list_bookmarks).post(chat::add_bookmark))
        .route("/api/channels/{c}/bookmarks/{b}", axum::routing::delete(chat::remove_bookmark))
        .route("/api/channels/{c}/files", get(chat::list_files))
        .route("/api/channels/{c}/unreads", get(chat::channel_unreads))
        .route("/api/channels/{c}/messages", get(chat::list_messages).post(chat::send_message))
        .route("/api/channels/{c}/changes", get(chat::list_changes))
        .route("/api/channels/{c}/scheduled/{s}", put(chat::schedule).delete(chat::cancel_scheduled))
        // chat: messages
        .route("/api/messages/{m}", get(chat::get_message).patch(chat::edit_message).delete(chat::delete_message))
        .route("/api/messages/{m}/around", get(chat::around))
        .route("/api/messages/{m}/versions", get(chat::versions))
        .route("/api/messages/{m}/knowledge", put(chat::set_knowledge))
        .route("/api/messages/{m}/thread", get(chat::get_thread))
        .route("/api/messages/{m}/thread/read", put(chat::mark_thread_read))
        .route("/api/messages/{m}/resolve", post(chat::resolve).delete(chat::reopen))
        .route("/api/messages/{m}/follow", get(chat::get_follow).put(chat::set_follow))
        .route("/api/messages/{m}/link-preview", post(chat::link_preview))
        .route("/api/messages/{m}/reactions/{emoji}", put(chat::put_reaction).delete(chat::delete_reaction))
        .route("/api/messages/{m}/pin", put(chat::pin).delete(chat::unpin))
        .route("/api/messages/{m}/saved", put(chat::save).delete(chat::unsave))
        .route("/api/messages/{m}/hidden", put(chat::hide))
        .route("/api/messages/{m}/reminder", put(chat::set_reminder).delete(chat::cancel_reminder))
        // chat: the person's lists
        .route("/api/workspaces/{w}/reminders", get(chat::list_reminders))
        .route("/api/workspaces/{w}/scheduled", get(chat::list_scheduled))
        .route("/api/workspaces/{w}/unresolved", get(chat::list_unresolved))
        .route("/api/workspaces/{w}/saved", get(chat::list_saved))
        .route("/api/workspaces/{w}/saved/ids", get(chat::saved_ids))
        .route("/api/workspaces/{w}/search", get(chat::search))
        .route("/api/workspaces/{w}/saved-searches", get(chat::list_saved_searches))
        .route("/api/workspaces/{w}/saved-searches/{s}", put(chat::save_search).delete(chat::delete_saved_search))
        .route("/api/notifications", get(chat::list_notifications))
        .route("/api/notifications/read", put(chat::mark_notifications_read))
        .route("/api/integrations/github/unfurl", post(chat::unfurl))
        // the socket, sign-in pages, health
        .route("/ws", get(crate::ws::upgrade))
        .route("/sign-in/native", get(crate::signin::page))
        .route("/sign-in/native/choose", post(crate::signin::choose))
        .route("/", get(crate::signin::index))
        .route("/health", get(|| async { "ok" }))
        .fallback(route_not_found)
        .method_not_allowed_fallback(route_not_found)
        .layer(axum::extract::DefaultBodyLimit::max(ATTACHMENT_BYTES_MAX as usize + 1))
        .layer(middleware::from_fn(edge));
    api.with_state(app)
}
