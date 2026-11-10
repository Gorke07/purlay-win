//! The chat module's routes: thin readers of the request that call one `World` operation.

use crate::error::{AppError, AppResult};
use crate::http::{JsonBody, Me, Q, Shared, created, no_content, ok};
use axum::extract::{Path, State};
use axum::response::Response;
use purlay_contract::models::*;

type P1 = Path<String>;
type P2 = Path<(String, String)>;

// --- channels ------------------------------------------------------------------------------

pub async fn list_channels(State(app): State<Shared>, me: Me, Path(ws): P1) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_channels(&me.user))
}

pub async fn create_channel(
    State(app): State<Shared>,
    me: Me,
    Path(ws): P1,
    JsonBody(req): JsonBody<CreateChannelRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    created(&w.create_channel(&me.user, &req)?)
}

pub async fn open_conversation(
    State(app): State<Shared>,
    me: Me,
    Path(ws): P1,
    JsonBody(req): JsonBody<OpenConversationRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    if req.user_ids.is_empty() {
        return Err(AppError::field("userIds", "names nobody"));
    }
    let (channel, made) = w.open_conversation(&me.user, &req.user_ids)?;
    if made { created(&channel) } else { ok(&channel) }
}

pub async fn open_dm(
    State(app): State<Shared>,
    me: Me,
    Path(ws): P1,
    JsonBody(req): JsonBody<CreateDirectMessageRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    let others = if req.user_id == me.user { Vec::new() } else { vec![req.user_id.clone()] };
    if !w.people.contains_key(&req.user_id) {
        return Err(AppError::new("USER_NOT_FOUND", "No such person."));
    }
    let (channel, made) = w.open_conversation(&me.user, &others)?;
    if made { created(&channel) } else { ok(&channel) }
}

pub async fn mark_all_read(State(app): State<Shared>, me: Me, Path(ws): P1) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.mark_all_read(&me.user))
}

pub async fn list_groups(State(app): State<Shared>, me: Me, Path(ws): P1, Q(q): Q<ChannelGroupQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_groups(&me.user, &q)?)
}

pub async fn save_group(
    State(app): State<Shared>,
    me: Me,
    Path((ws, g)): P2,
    JsonBody(req): JsonBody<SaveChannelGroupRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.save_group(&me.user, &g, &req.name)?)
}

pub async fn fold_group(
    State(app): State<Shared>,
    me: Me,
    Path((ws, g)): P2,
    JsonBody(req): JsonBody<FoldChannelGroupRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.fold_group(&me.user, &g, req.collapsed)?)
}

pub async fn remove_group(State(app): State<Shared>, me: Me, Path((ws, g)): P2) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    w.remove_group(&me.user, &g);
    no_content()
}

pub async fn get_channel(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    ok(&app.world.lock().get_channel(&me.user, &c)?)
}

pub async fn join(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    ok(&app.world.lock().join_channel(&me.user, &c)?)
}

pub async fn leave(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    app.world.lock().leave_channel(&me.user, &c)?;
    no_content()
}

pub async fn channel_members(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    ok(&app.world.lock().channel_members(&me.user, &c)?)
}

pub async fn add_channel_member(
    State(app): State<Shared>,
    me: Me,
    Path(c): P1,
    JsonBody(req): JsonBody<AddChannelMemberRequest>,
) -> AppResult<Response> {
    app.world.lock().add_channel_member(&me.user, &c, &req.user_id)?;
    no_content()
}

pub async fn mark_read(State(app): State<Shared>, me: Me, Path(c): P1, JsonBody(req): JsonBody<ReadChannelRequest>) -> AppResult<Response> {
    ok(&app.world.lock().mark_read(&me.user, &c, &req.last_read_message_id)?)
}

pub async fn mark_unread(State(app): State<Shared>, me: Me, Path(c): P1, JsonBody(req): JsonBody<MarkUnreadRequest>) -> AppResult<Response> {
    ok(&app.world.lock().mark_unread(&me.user, &c, &req.from_message_id)?)
}

pub async fn star(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    app.world.lock().set_star(&me.user, &c, true)?;
    no_content()
}

pub async fn unstar(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    app.world.lock().set_star(&me.user, &c, false)?;
    no_content()
}

pub async fn move_to_group(
    State(app): State<Shared>,
    me: Me,
    Path(c): P1,
    JsonBody(req): JsonBody<MoveChannelGroupRequest>,
) -> AppResult<Response> {
    ok(&app.world.lock().move_to_group(&me.user, &c, req.group_id.as_ref())?)
}

pub async fn set_level(
    State(app): State<Shared>,
    me: Me,
    Path(c): P1,
    JsonBody(req): JsonBody<SetNotificationLevelRequest>,
) -> AppResult<Response> {
    app.world.lock().set_level(&me.user, &c, req.level)?;
    no_content()
}

pub async fn list_pins(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    ok(&app.world.lock().list_pins(&me.user, &c)?)
}

pub async fn list_bookmarks(State(app): State<Shared>, me: Me, Path(c): P1) -> AppResult<Response> {
    ok(&app.world.lock().list_bookmarks(&me.user, &c)?)
}

pub async fn add_bookmark(State(app): State<Shared>, me: Me, Path(c): P1, JsonBody(req): JsonBody<AddBookmarkRequest>) -> AppResult<Response> {
    created(&app.world.lock().add_bookmark(&me.user, &c, &req)?)
}

pub async fn remove_bookmark(State(app): State<Shared>, me: Me, Path((c, b)): P2) -> AppResult<Response> {
    app.world.lock().remove_bookmark(&me.user, &c, &b)?;
    no_content()
}

pub async fn list_files(State(app): State<Shared>, me: Me, Path(c): P1, Q(q): Q<FileQuery>) -> AppResult<Response> {
    ok(&app.world.lock().list_files(&me.user, &c, &q)?)
}

pub async fn channel_unreads(State(app): State<Shared>, me: Me, Path(c): P1, Q(q): Q<CatchUpQuery>) -> AppResult<Response> {
    ok(&app.world.lock().channel_unreads(&me.user, &c, &q)?)
}

pub async fn list_messages(State(app): State<Shared>, me: Me, Path(c): P1, Q(q): Q<PageQuery>) -> AppResult<Response> {
    ok(&app.world.lock().list_messages(&me.user, &c, &q)?)
}

pub async fn list_changes(State(app): State<Shared>, me: Me, Path(c): P1, Q(q): Q<ChangesQuery>) -> AppResult<Response> {
    ok(&app.world.lock().list_changes(&me.user, &c, &q)?)
}

pub async fn send_message(State(app): State<Shared>, me: Me, Path(c): P1, JsonBody(req): JsonBody<SendMessageRequest>) -> AppResult<Response> {
    let attachments = req.attachment_ids.clone().unwrap_or_default();
    let m = app.world.lock().send_message(&me.user, &c, &req.id, &req.content, req.parent_id.as_ref(), &attachments)?;
    ok(&m)
}

pub async fn schedule(
    State(app): State<Shared>,
    me: Me,
    Path((c, s)): P2,
    JsonBody(req): JsonBody<ScheduleMessageRequest>,
) -> AppResult<Response> {
    ok(&app.world.lock().schedule_message(&me.user, &c, &s, &req)?)
}

pub async fn cancel_scheduled(State(app): State<Shared>, me: Me, Path((c, s)): P2) -> AppResult<Response> {
    app.world.lock().cancel_scheduled(&me.user, &c, &s)?;
    no_content()
}

// --- messages ------------------------------------------------------------------------------

pub async fn get_message(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    ok(&app.world.lock().get_message(&me.user, &m)?)
}

pub async fn edit_message(State(app): State<Shared>, me: Me, Path(m): P1, JsonBody(req): JsonBody<EditMessageRequest>) -> AppResult<Response> {
    ok(&app.world.lock().edit_message(&me.user, &m, &req.content)?)
}

pub async fn delete_message(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    app.world.lock().delete_message(&me.user, &m)?;
    no_content()
}

pub async fn around(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    ok(&app.world.lock().messages_around(&me.user, &m)?)
}

pub async fn versions(State(app): State<Shared>, me: Me, Path(m): P1, Q(q): Q<OlderPageQuery>) -> AppResult<Response> {
    ok(&app.world.lock().message_versions(&me.user, &m, &q)?)
}

pub async fn set_knowledge(State(app): State<Shared>, me: Me, Path(m): P1, JsonBody(req): JsonBody<SetKnowledgeRequest>) -> AppResult<Response> {
    ok(&app.world.lock().set_knowledge(&me.user, &m, &req)?)
}

pub async fn get_thread(State(app): State<Shared>, me: Me, Path(m): P1, Q(q): Q<PageQuery>) -> AppResult<Response> {
    ok(&app.world.lock().get_thread(&me.user, &m, &q)?)
}

pub async fn mark_thread_read(State(app): State<Shared>, me: Me, Path(m): P1, JsonBody(req): JsonBody<ReadThreadRequest>) -> AppResult<Response> {
    app.world.lock().mark_thread_read(&me.user, &m, &req.last_read_message_id)?;
    no_content()
}

pub async fn resolve(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    ok(&app.world.lock().resolve_thread(&me.user, &m, true)?)
}

pub async fn reopen(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    ok(&app.world.lock().resolve_thread(&me.user, &m, false)?)
}

pub async fn get_follow(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    ok(&app.world.lock().thread_follow(&me.user, &m)?)
}

pub async fn set_follow(State(app): State<Shared>, me: Me, Path(m): P1, JsonBody(req): JsonBody<SetThreadFollowRequest>) -> AppResult<Response> {
    ok(&app.world.lock().set_thread_follow(&me.user, &m, req.following)?)
}

pub async fn link_preview(State(app): State<Shared>, me: Me, Path(m): P1, JsonBody(req): JsonBody<WebLinkPreviewRequest>) -> AppResult<Response> {
    let preview: WebLinkPreviewResponse = app.world.lock().preview_link(&me.user, &m, &req.url)?;
    ok(&preview)
}

pub async fn put_reaction(State(app): State<Shared>, me: Me, Path((m, emoji)): P2) -> AppResult<Response> {
    ok(&app.world.lock().put_reaction(&me.user, &m, &emoji, true)?)
}

pub async fn delete_reaction(State(app): State<Shared>, me: Me, Path((m, emoji)): P2) -> AppResult<Response> {
    ok(&app.world.lock().put_reaction(&me.user, &m, &emoji, false)?)
}

pub async fn pin(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    app.world.lock().set_pin(&me.user, &m, true)?;
    no_content()
}

pub async fn unpin(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    app.world.lock().set_pin(&me.user, &m, false)?;
    no_content()
}

pub async fn save(State(app): State<Shared>, me: Me, Path(m): P1, body: axum::body::Bytes) -> AppResult<Response> {
    let state = if body.is_empty() {
        None
    } else {
        serde_json::from_slice::<SaveMessageRequest>(&body).map_err(|_| AppError::field("state", "is not a saved state"))?.state
    };
    app.world.lock().save_message(&me.user, &m, state)?;
    no_content()
}

pub async fn unsave(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    app.world.lock().unsave_message(&me.user, &m);
    no_content()
}

pub async fn hide(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    ok(&app.world.lock().hide_message(&me.user, &m)?)
}

pub async fn set_reminder(
    State(app): State<Shared>,
    me: Me,
    Path(m): P1,
    JsonBody(req): JsonBody<SetMessageReminderRequest>,
) -> AppResult<Response> {
    ok(&app.world.lock().set_reminder(&me.user, &m, &req.remind_at)?)
}

pub async fn cancel_reminder(State(app): State<Shared>, me: Me, Path(m): P1) -> AppResult<Response> {
    app.world.lock().cancel_reminder(&me.user, &m);
    no_content()
}

// --- the person's lists ----------------------------------------------------------------------

pub async fn list_reminders(State(app): State<Shared>, me: Me, Path(ws): P1) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_reminders(&me.user))
}

pub async fn list_scheduled(State(app): State<Shared>, me: Me, Path(ws): P1) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_scheduled(&me.user))
}

pub async fn list_unresolved(State(app): State<Shared>, me: Me, Path(ws): P1, Q(q): Q<UnresolvedQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_unresolved(&me.user, &q)?)
}

pub async fn list_saved(State(app): State<Shared>, me: Me, Path(ws): P1, Q(q): Q<SavedQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_saved(&me.user, &q)?)
}

pub async fn saved_ids(State(app): State<Shared>, me: Me, Path(ws): P1) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.saved_ids(&me.user))
}

pub async fn search(State(app): State<Shared>, me: Me, Path(ws): P1, Q(q): Q<SearchQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.search(&me.user, &q)?)
}

pub async fn list_saved_searches(State(app): State<Shared>, me: Me, Path(ws): P1, Q(q): Q<OlderPageQuery>) -> AppResult<Response> {
    let w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.list_saved_searches(&me.user, &q)?)
}

pub async fn save_search(
    State(app): State<Shared>,
    me: Me,
    Path((ws, s)): P2,
    JsonBody(req): JsonBody<SaveSearchRequest>,
) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    ok(&w.save_search(&me.user, &s, &req)?)
}

pub async fn delete_saved_search(State(app): State<Shared>, me: Me, Path((ws, s)): P2) -> AppResult<Response> {
    let mut w = app.world.lock();
    w.check_workspace(&ws, &me.user)?;
    w.delete_saved_search(&me.user, &s);
    no_content()
}

pub async fn list_notifications(State(app): State<Shared>, me: Me, Q(q): Q<NotificationQuery>) -> AppResult<Response> {
    ok(&app.world.lock().list_notifications(&me.user, &q)?)
}

pub async fn mark_notifications_read(State(app): State<Shared>, me: Me, JsonBody(body): JsonBody<serde_json::Value>) -> AppResult<Response> {
    app.world.lock().mark_notifications_read(&me.user, &body)?;
    no_content()
}

/// A made-up card for `github.com/<owner>/<repo>/(pull|issues)/<n>`; `null` for anything else.
pub async fn unfurl(State(app): State<Shared>, me: Me, JsonBody(req): JsonBody<UnfurlRequest>) -> AppResult<Response> {
    app.world.lock().check_workspace(&req.workspace_id, &me.user)?;
    let rest = req.url.trim_start_matches("https://").trim_start_matches("www.");
    let parts: Vec<&str> = rest.split(['/', '?', '#']).collect();
    let card: UnfurlResponse = match parts.as_slice() {
        ["github.com", owner, repo, kind @ ("pull" | "issues"), number, ..] => number.parse::<i64>().ok().map(|n| GitHubCardMetadata {
            r#type: if *kind == "pull" { GitHubCardMetadataType::Pr } else { GitHubCardMetadataType::Issue },
            repo: format!("{owner}/{repo}"),
            number: n,
            title: if *kind == "pull" { format!("Fix the thing (#{n})") } else { format!("Something is broken (#{n})") },
            state: GitHubCardMetadataState::Open,
            author: GitHubCardMetadataAuthor { login: "octocat".into(), avatar_url: "https://github.com/octocat.png".into() },
            ci_status: if *kind == "pull" { Some(GitHubCardMetadataCiStatus::Success) } else { None },
            review_decision: if *kind == "pull" { Some(GitHubCardMetadataReviewDecision::ReviewRequired) } else { None },
        }),
        _ => None,
    };
    ok(&card)
}
