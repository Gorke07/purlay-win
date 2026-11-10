//! The core's routes: the handshake, sign-in, the person, the workspace, the inbox,
//! uploads, find and the issues module.

use crate::error::{AppError, AppResult};
use crate::http::{JsonBody, Me, Q, Shared, created, json_status, no_content, ok};
use crate::world::{SERVER_VERSION, SESSION_COOKIE, SESSION_MAX_AGE_S, ts};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use purlay_contract::models::*;
use purlay_contract::CONTRACT_VERSION;

pub fn auth_methods_value() -> AuthMethods {
    AuthMethods {
        github: false,
        email: true,
        oidc: None,
        google: false,
        sign_up: AuthMethodsSignUp::Open,
        password: true,
    }
}

pub async fn client_info(State(app): State<Shared>) -> AppResult<Response> {
    let info = ClientInfo {
        capabilities: Some(ClientInfoCapabilities {
            scheduled_messages: Some(true),
            message_reminders: Some(true),
            personal_message_deletion: Some(true),
            notification_type_filters: Some(true),
        }),
        server: ClientInfoServer { version: SERVER_VERSION.into() },
        contract_version: CONTRACT_VERSION,
        app_url: app.origin.clone(),
        min_client: ClientInfoMinClient {
            macos: ClientMinimum { contract_version: None, app_version: None, download_url: None },
        },
        auth: auth_methods_value(),
    };
    let mut res = json_status(StatusCode::OK, &info);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

pub async fn auth_methods() -> AppResult<Response> {
    ok(&auth_methods_value())
}

pub async fn native_authorize(State(app): State<Shared>, me: Me, JsonBody(req): JsonBody<NativeAuthorizeRequest>) -> AppResult<Response> {
    let redirect = app.world.lock().native_authorize(&me.user, &req.challenge, &req.state)?;
    ok(&NativeAuthorizeResponse { redirect })
}

pub fn session_set_cookie(value: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!("{SESSION_COOKIE}={value}; Max-Age={max_age}; Path=/; HttpOnly; Secure; SameSite=Lax"))
        .expect("cookie header")
}

pub async fn native_token(
    State(app): State<Shared>,
    headers: HeaderMap,
    JsonBody(req): JsonBody<NativeTokenRequest>,
) -> AppResult<Response> {
    let client = headers.get("x-purlay-client").and_then(|v| v.to_str().ok()).map(str::to_string);
    let cookie = app.world.lock().native_token(&req.code, &req.verifier, &req.device.name, client.as_deref())?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_set_cookie(&cookie, SESSION_MAX_AGE_S));
    Ok(res)
}

pub async fn logout(State(app): State<Shared>, me: Me) -> AppResult<Response> {
    app.world.lock().end_session(&me.session.session_id, SessionEndedFrameDataReason::SignedOut);
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_set_cookie("", 0));
    Ok(res)
}

pub async fn logout_all(State(app): State<Shared>, me: Me) -> AppResult<Response> {
    app.world.lock().end_all_sessions(&me.user);
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_set_cookie("", 0));
    Ok(res)
}

pub async fn list_sessions(State(app): State<Shared>, me: Me) -> AppResult<Response> {
    ok(&app.world.lock().list_sessions(&me.session))
}

pub async fn end_session(State(app): State<Shared>, me: Me, Path(id): Path<String>) -> AppResult<Response> {
    app.world.lock().end_my_session(&me.session, &id)?;
    no_content()
}

pub async fn latest_release(State(app): State<Shared>, _me: Me) -> AppResult<Response> {
    let published = app.world.lock().release_published_ms;
    ok(&LatestReleaseAnswer {
        current: SERVER_VERSION.into(),
        latest: Some(LatestRelease {
            version: SERVER_VERSION.into(),
            url: format!("https://github.com/doancan/purlay/releases/tag/v{SERVER_VERSION}"),
            published_at: ts(published),
        }),
    })
}

pub async fn me(State(app): State<Shared>, me: Me) -> AppResult<Response> {
    ok(&app.world.lock().me(&me.user)?)
}

pub async fn set_away(State(app): State<Shared>, me: Me, JsonBody(req): JsonBody<SetAwayRequest>) -> AppResult<Response> {
    ok(&app.world.lock().set_away(&me.user, req.away))
}

pub async fn list_workspaces(State(app): State<Shared>, me: Me) -> AppResult<Response> {
    let w = app.world.lock();
    let list: MembershipList = w.membership_view(&me.user).into_iter().collect();
    ok(&list)
}

pub async fn get_workspace(State(app): State<Shared>, me: Me, Path(ws): Path<String>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.workspace_view())
}

pub async fn list_members(State(app): State<Shared>, me: Me, Path(ws): Path<String>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.members())
}

pub async fn presence(State(app): State<Shared>, me: Me, Path(ws): Path<String>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.presence_snapshot())
}

pub async fn set_deep_work(
    State(app): State<Shared>,
    me: Me,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<SetDeepWorkRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.set_deep_work(&me.user, &req)?)
}

pub async fn list_inbox(State(app): State<Shared>, me: Me, Path(ws): Path<String>, Q(q): Q<InboxQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_inbox(&me.user, &q)?)
}

pub async fn mark_inbox_read(
    State(app): State<Shared>,
    me: Me,
    Path(ws): Path<String>,
    JsonBody(body): JsonBody<serde_json::Value>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    w.mark_inbox_read(&me.user, &body)?;
    no_content()
}

pub async fn find(State(app): State<Shared>, me: Me, Path(ws): Path<String>, Q(q): Q<FindQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.find(&me.user, &q)?)
}

// --- uploads -------------------------------------------------------------------------------

pub async fn upload(State(app): State<Shared>, me: Me, Path(ws): Path<String>, Q(q): Q<UploadQuery>, body: Bytes) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    let (attachment, made) = w.upload(&me.user, &q, body.to_vec())?;
    if made { created(&attachment) } else { ok(&attachment) }
}

fn serve_bytes(headers: &HeaderMap, attachment: &Attachment, bytes: &[u8]) -> Response {
    let etag = format!("\"{}\"", attachment.id);
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let image = attachment.kind == AttachmentKind::Image;
    let (ctype, disposition) = if image {
        (attachment.mime_type.clone(), "inline".to_string())
    } else {
        let safe: String = attachment.file_name.chars().filter(|c| c.is_ascii_graphic() && *c != '"' || *c == ' ').collect();
        ("application/octet-stream".to_string(), format!("attachment; filename=\"{safe}\""))
    };
    let mut res = (StatusCode::OK, bytes.to_vec()).into_response();
    let h = res.headers_mut();
    let set = |h: &mut HeaderMap, k: header::HeaderName, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(k, v);
        }
    };
    set(h, header::CONTENT_TYPE, &ctype);
    set(h, header::CONTENT_DISPOSITION, &disposition);
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::CONTENT_SECURITY_POLICY, "sandbox");
    set(h, header::HeaderName::from_static("cross-origin-resource-policy"), "same-origin");
    set(h, header::CACHE_CONTROL, "private, no-cache");
    set(h, header::ETAG, &etag);
    res
}

pub async fn download(State(app): State<Shared>, me: Me, headers: HeaderMap, Path(id): Path<String>) -> AppResult<Response> {
    let w = app.world.lock();
    let u = w.downloadable(&me.user, &id)?;
    Ok(serve_bytes(&headers, &u.attachment, &u.bytes))
}

pub async fn thumbnail(State(app): State<Shared>, me: Me, headers: HeaderMap, Path(id): Path<String>) -> AppResult<Response> {
    let w = app.world.lock();
    let u = w.downloadable(&me.user, &id)?;
    if u.attachment.kind != AttachmentKind::Image {
        return Err(AppError::new("ATTACHMENT_NOT_FOUND", "No such attachment."));
    }
    Ok(serve_bytes(&headers, &u.attachment, &u.bytes))
}

pub async fn discard(State(app): State<Shared>, me: Me, Path(id): Path<String>) -> AppResult<Response> {
    app.world.lock().discard_upload(&me.user, &id)?;
    no_content()
}

pub async fn external_image(_me: Me, Q(_q): Q<ExternalImageQuery>) -> AppResult<Response> {
    Err(AppError::new("IMAGE_NOT_AVAILABLE", "The image is not available."))
}

// --- issues --------------------------------------------------------------------------------

pub async fn list_issues(State(app): State<Shared>, me: Me, Path(ws): Path<String>, Q(q): Q<IssueQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_issues(&q)?)
}

pub async fn create_issue(
    State(app): State<Shared>,
    me: Me,
    Path(ws): Path<String>,
    JsonBody(req): JsonBody<CreateIssueRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    created(&w.create_issue(&me.user, &req)?)
}

pub async fn update_issue(
    State(app): State<Shared>,
    me: Me,
    Path((ws, id)): Path<(String, String)>,
    JsonBody(req): JsonBody<UpdateIssueRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.update_issue(&me.user, &id, &req)?)
}

pub async fn delete_issue(State(app): State<Shared>, me: Me, Path((ws, id)): Path<(String, String)>) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    w.delete_issue(&me.user, &id)?;
    no_content()
}
